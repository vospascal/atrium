# Output depth — 8-bit vs 10-bit, and the road to HDR

A runtime toggle for how many bits per channel reach the display. **Shipped and
working (2026-08-03), default 8-bit.**

**10-bit is SDR, not HDR.** Same brightness range, more steps within it. Reaching an
XDR panel's actual headroom is the third mode, `HdrFloat`, which **now works on macOS**
— see [Tier 2](#tier-2--actual-hdr-and-the-mechanism-is-simpler-than-expected) and, for
the thing that was missing, [the colour-space tag](#the-colour-space-tag-the-bug-that-made-hdr-look-worse-than-sdr).

> **This document is the record: what was built, what broke, and what each thing cost.**
> For where the colour path should eventually GO — the two-stage pipeline it sits in, why
> BT.2390 is doing the wrong stage's job, what blocks auto-exposure, wide gamut and PQ, and
> which counter-intuitive decisions must not be "fixed" — read
> [`crates/voxel-color/README.md`](../crates/voxel-color/README.md). Start there if you are
> picking this arc back up rather than debugging it.

## Where it lives

`crates/voxel-rt/src/output_format.rs` owns the whole decision. It is deliberately
**not** part of `RenderQuality`: output depth is a property of the DISPLAY, not a
quality tier — the cheapest tier on an OLED still wants ten bits, and the most
expensive tier on an old panel cannot have them. It sits beside the vsync toggle,
which is an `App` field for the same reason (`grep -c Vsync variants.rs` → 0).

It is also the **first control in the engine that can be unavailable**. Every quality
lever is always valid; this one depends on the adapter, the surface's advertised
formats and the display, so the overlay disables it and names which half is missing.

## The formats

| | 8-bit (default) | 10-bit |
|---|---|---|
| surface | `Bgra8UnormSrgb` (whatever sRGB the surface offers) | `Rgb10a2Unorm` |
| frame storage texture | `Rgba8Unorm` | `Rgba16Unorm` |
| blit | **decodes** sRGB | **passes through** |
| device feature | — | `TEXTURE_FORMAT_16BIT_NORM` |

`Rgba16Unorm` is unsigned *normalized* — **integer storage, no floats** — and it is
filterable, so the blit keeps `textureSample` and needs no hand-rolled bilinear. The
4-byte alternative, packing 10:10:10 into `R32Uint`, is not filterable and would have
forced manual filtering into the blit for every render scale below 1.0.

**Ten bits is useless without the wider storage texture.** A 10-bit swapchain fed from
`Rgba8Unorm` gains nothing — the value was already quantised before the blit saw it.
`OutputFormat::resolve` therefore returns both formats together and never one alone.

## The sRGB asymmetry

The least obvious part, and the one most likely to be "fixed" wrongly later.

The 8-bit surface is an `*Srgb` format, so the **hardware** applies the transfer
function when the blit stores its fragment. That is why the blit *decodes*: it hands
back linear light for the hardware to re-encode, making the round trip exact.
`Rgb10a2Unorm` is plain unorm and applies **no** transfer at all, so the
already-encoded value must pass straight through.

- decode into a non-sRGB surface → washed-out, milky midtones
- pass through into an sRGB surface → dark, crushed

Neither failure names itself, so both directions are pinned by
`the_blit_decodes_only_when_the_surface_carries_the_transfer_function`.

## The six consumers

One format, six places that must agree. wgpu wants a storage texture's format in
**three** of them and validates all three against each other, and every RENDER
pipeline bakes its colour attachment format at construction.

| # | consumer | asks |
|---|---|---|
| 1 | `gpu.rs` swapchain | `OutputFormat::surface()` |
| 2 | `render.rs` frame texture | `OutputFormat::storage()` |
| 3 | `dda.wgsl` binding TYPE | `OutputFormat::patch_shader_source()` |
| 4 | `passes/dda.rs` bind group **layout** entry | `OutputFormat::storage()` |
| 5 | `blit.wgsl` transfer function | `OutputFormat::patch_blit_source()` |
| 6 | **egui's own render pipeline** (`overlay.rs`) | `Overlay::set_surface_format()` |

`OutputFormat::resolve` holds the **only `match` on depth in the codebase**. No
consumer branches; each asks a question.

Consumer 6 is the one worth remembering, because it is not ours. **Anything that owns
a render pipeline targeting the swapchain is a consumer** — the engine has two such
pipelines and only one of them is in `passes/`.

## The seven bugs this took, and their error messages

Recorded because each surfaced several layers from its cause, and recognising the
message is most of the fix.

| symptom | cause |
|---|---|
| `Texture format Rgba16Unorm can't be used due to missing features` | `TEXTURE_FORMAT_16BIT_NORM` was never requested at device creation. Features cannot be added later, so `REQUIRED_DEVICE_FEATURES` is requested UP FRONT and unconditionally |
| toggle enabled itself on a device that could not do it | `OutputSupport::probe` read `adapter.features()`. An adapter that *offers* a feature proves nothing — it must read `device.features()` |
| `Storage texture binding 6 expects format = Rgba8Unorm, but given a view with format = Rgba16Unorm` | the bind group LAYOUT entry (consumer 4) still declared the old format |
| the same message again, after fixing the layout | ORDER. `recreate_storage` rebinds the EXISTING passes, so it ran the old layout against the new view. The texture is now recreated inline and both passes are rebuilt from scratch |
| `Texture class Storage { format: Rgba16Unorm } doesn't match the shader Storage { format: Rgba8Unorm }` | `DdaPass::new` compiled the raw `SHADER_SOURCE`; the WGSL type needs patching too. Same omission in the preset prewarm path |
| `Render pipeline targets are incompatible with render pass … 'egui_pipeline' uses attachments with formats [Bgra8UnormSrgb]` | consumer 6. egui bakes the attachment format into its pipeline at construction, so its whole `Renderer` is replaced |
| `Tried to update a texture that has not been allocated yet` — then `panic in a function that cannot unwind`, aborting | replacing the `Renderer` alone. `egui::Context` remembers which textures it gave a renderer, so a fresh renderer gets a PARTIAL font-atlas update for a texture it never allocated. The `Context` and `egui_winit::State` are rebuilt with it; `set_fonts` is no escape, since it compares definitions and no-ops when unchanged. Costs UI state (collapsibles, scroll), keeps the frame-time history |

A depth change **rebuilds** both passes rather than rebinding them, because each holds
format state a rebind cannot reach — the blit's render target format, and the DDA's
bind group layout. That drops the DDA pipeline cache, which is correct: every entry
was compiled for the old format.

## How to test it

**Headless, no display needed** — builds a validating pipeline for both depths:

```sh
cargo test -p voxel-rt --lib passes::dda::tests::both_output_depths -- --nocapture
```

It skips a depth the local adapter cannot do, printing the reason.

**Note what it does NOT cover.** It validates the compute pipeline against its layout
— consumers 2, 3 and 4 — because that is where three of the six bugs were. It cannot
catch consumers 1 and 6, which need a real surface: a swapchain format and a render
pipeline targeting it. **Those two remain app-run-only**, and the egui failure was
found that way after this test was already green.

**The seam's own contracts**, including the sRGB asymmetry and that the 8-bit path
leaves the shader source byte-identical:

```sh
cargo test -p voxel-rt --lib output_format
```

**In the app** — startup prints the surface's formats and a line like
`output: 8-bit | 10-bit: available`. The toggle is in the overlay beside VSync.

## The three test materials

Added 2026-08-03 as slots **28-30**, placeable voxels like any other. Build a wall of
one and count the bands; flip the toggle and count again. Four times as many means it
is working.

| material | what it isolates | how |
|---|---|---|
| `hdr_ramp` | the readable baseline | black -> white over 2 m |
| `hdr_dark` | **the sensitive one** | black -> 1% white over 2 m — ~31 code values at 8 bits, ~126 at 10 |
| `hdr_glow` | the bright end, per channel | three emissive ramps at 3.5 / 3.75 / 4 m toward R, G, B |

**Why they work**, and every field matters:

- **`wave` with distortion 0 is a TRIANGLE ramp** — `1 - |2·fract(x) - 1|` — the only
  generator in the library that is smooth by construction, so there is no noise to
  hide quantisation behind. **No new generator or node type was needed**; `wave`
  already had both.
- **`texels_per_voxel: 0`** is the critical field. Any other value snaps the coordinate
  to a 1.56 cm lattice and you would count *texel* steps instead — a test that fails
  the same way whatever the output format is.
- **`PatternFrame::World`** keeps the ramp continuous across voxels; the face and voxel
  frames restart it per element, turning one long gradient into a row of short ones.
- **Shallowness comes from `amount`, not the period.** `PATTERN_PERIOD_FIELD` declares
  a 0.005–4 m range and that bound is deliberate, so `hdr_dark` compresses the range
  instead of stretching the distance.

### Reason in DISPLAY space, not linear space

The first version of these rows used `amount: 0.08` for `hdr_dark` on the reasoning
that 8% of the range is 20 of 255 codes. That is wrong by 4x, and the mistake is worth
recording because it is the natural one to make.

The displayed value is `pow(reinhard(L), 1/2.2)`. Both steps matter: Reinhard
compresses 1.0 to 0.5, and **the gamma encode EXPANDS shadows** — linear 0.08 lands at
0.31 encoded. The curve exists precisely to give darks more code values, so making a
ramp darker buys far less than linear intuition suggests.

| `amount` | encoded top | 8-bit codes | 10-bit codes | px/band over 700 px |
|---|---|---|---|---|
| 1.00 (`hdr_ramp`) | 0.730 | 186 | 747 | 3.8 |
| 0.08 (first try) | 0.306 | 78 | 313 | 9.0 |
| **0.01 (`hdr_dark`)** | 0.123 | **31** | 126 | **22.4** |

At irradiance ~1. Real lighting shifts them, so treat them as order-of-magnitude —
but the ordering and the ratios hold.

`hdr_glow` carries a faint non-zero base emission rather than `Some([0.0; 3])`: a row
claiming to be emissive while emitting nothing is the contradiction
`only_the_emissive_rows_emit` exists to catch. It is therefore a real emitter to CAGI,
so a placed wall lights its surroundings.

**A neutral ramp is deliberate.** A hue sweep looks better and reveals LESS — the eye
tracks the colour change instead of the steps. `hdr_glow` is coloured only because
per-channel banding is worth seeing separately.

## The three colour-speckle blocks — slots 31-33

Blue, green and magenta emissive blocks, each carrying a **contrasting colour speckle**
at an amplitude 8-bit output cannot represent and 10-bit can. The question is presence
rather than smoothness — "is the speckle there" is much easier to judge than "is this
gradient banded".

| material | base emission | speckle | amount | 8-bit steps | 10-bit steps |
|---|---|---|---|---|---|
| `hdr_speckle_blue` | `[0.05, 0.10, 0.90]` | yellow | 0.0012 | **0.66** | **2.64** |
| `hdr_speckle_green` | `[0.05, 0.90, 0.15]` | magenta | 0.0012 | 0.66 | 2.64 |
| `hdr_speckle_magenta` | `[0.85, 0.05, 0.85]` | green | 0.0012 | 0.66 | 2.64 |

0.66 of an 8-bit code step means quantisation **erases** it; 2.64 steps at 10 bits means
it survives. A flat block that grows speckle when the toggle flips.

Three hues rather than three amplitudes because the controls already exist above —
`hdr_ramp` is the positive control, `hdr_dark` the sensitive one. What these add is
whether the effect holds ACROSS channels, which a grey block cannot show, since
per-channel quantisation is independent.

### Why the bases are not pure primaries

This is the non-obvious constraint, and it decides the design.

A colour speckle adds into a channel the base leaves dim, and **gamma expands
near-black enormously**. Measured against `pow(reinhard(base + 0.0012), 1/2.2)`:

| base channel | 8-bit steps | |
|---|---|---|
| **0.00** | **11.99** | blatantly visible — impossible to hide |
| 0.05 | 0.66 | hidden at 8 bits, visible at 10 ✓ |
| 0.90 | 0.06 | hidden at BOTH depths — too subtle to be a test |

So the target channel must be **dim but non-zero**, which is what the `0.05` floor in
each base is for. A pure primary — `[0, 0, 1]`, the sRGB analogue of
`color(rec2100-pq 0 0 1)` — has zero channels, and any colour speckle on one is a
12-code-step jump. **Pure primaries are structurally incompatible with a hidden colour
speckle**, whatever the output depth.

### What would make these genuinely HDR-only

Neither is a material edit. Both are output-path work.

**Brightness — and PQ is absolute.** `color(rec2100-pq 1 0 0)` is not "saturated red";
a PQ signal of 1.0 is the encoding's **10,000-nit** ceiling, with SDR white sitting near
0.5. So the demo's punch is absolute luminance far above white, not just gamut. A
speckle above 1.0 is HDR-only only if SDR **clips** it — and `tonemap_reinhard` is
`L/(1+L)`, which *compresses*, so 3.0 stays visible in SDR merely dimmer. Clipping or a
nits-based tonemap is the prerequisite.

**Gamut.** Rec.2020 magenta is outside sRGB entirely and no precision reaches it. That
needs wide primaries on the surface.

> **⚠ Every amplitude in this document is specific to `pow(reinhard(L), 1/2.2)`.** PQ
> allocates codes perceptually across 0–10,000 nits, so it spends far FEWER of them on
> the 0–100 nit range than gamma 2.2 does — which is exactly why HDR standards mandate
> 10-bit, and why 8-bit PQ bands catastrophically in shadows. If a PQ path ever lands,
> all six diagnostic rows need retuning against the PQ curve, not merely re-checking.

### What adding them cost

Three rows is not a free edit, and the compiler and tests named every coupling:

- `MATERIAL_COUNT` 28 -> 31, and three `Voxel` variants in `voxel-core` — a material is
  reachable only through a voxel type, so a row with no variant is unplaceable
- `UPLOAD_PIN`'s length was tied to `MATERIAL_COUNT`; it is now `PINNED_ROW_COUNT`,
  because that pin is a recorded baseline and padding it with hand-computed rows would
  have been fabricated evidence rather than a regression check
- `only_lava_and_slate_tile_author_pattern_layers` and `only_the_emissive_rows_emit`
  widened as explicit decisions — the first spells its list out on purpose
- the project needed slots: `cargo run -p voxel-rt --example sync_project`

## What to look for visually

Only banding in smooth gradients should change:

- sky at a low sun angle — the widest gradient in the scene
- fog
- dark cave interiors, where 8-bit sRGB steps are widest in absolute terms

**Be prepared for "identical".** 8-bit *sRGB* is perceptually distributed — that is
what the transfer curve is for — so the headroom gained is real but small. If banding
persists it is the **tonemap**, not the container: a fixed Reinhard with no exposure
control will band at 16 bits too.

Worth flipping the toggle back and forth a few times: it is the heaviest toggle in the
engine (surface reconfigure + texture reallocation + two pipeline rebuilds), so
repeated switching is the interesting test.

## Tier 2 — actual HDR, and the mechanism is simpler than expected

**The WebGPU spec already defines it**, and it is not a custom nits tonemap.
`GPUCanvasToneMappingMode` (spec §GPUCanvasToneMappingMode):

| mode | values outside `[0, 1]` |
|---|---|
| `"standard"` (default) | *"projected to the standard dynamic range of the screen"* — in practice **clamped** to `[0, 1]` |
| `"extended"` | *"Color values in the extended dynamic range of the screen are unchanged"* — **may include values greater than 1** |

So the platform's model is: **write extended-range values and let the compositor clamp
or preserve them.** One render path, two presentations. The spec's own example: `2.5`
becomes white under `standard` and 2.5x white under `extended`.

That is exactly "invisible in SDR, visible in HDR", with no per-mode authoring — which
is what the six `hdr_*` rows were built for.

### The one change that unblocks it, and it is a subtraction

**`tonemap_reinhard` has to go for the HDR path.** `L/(1+L)` maps every value into
`[0, 1]` by construction, so it destroys the extended range *before* the compositor
ever sees it — a speckle at linear 100 arrives as 0.996 instead of 100. The spec's model
wants the shader to emit radiance and the compositor to decide; Reinhard pre-empts that
decision.

That is a look-affecting change, so it belongs behind a lever rather than replacing
Reinhard outright:

| mode | shader output | presentation |
|---|---|---|
| Reinhard (today) | compressed into `[0,1]` | SDR, nothing can exceed white |
| exposure-only + `standard` | extended range | compositor clamps — same as Reinhard visually at the top end, but hard-clipped |
| exposure-only + `extended` | extended range | HDR |

Note the middle row is the honest SDR fallback and it is **not** free: clamping and
compressing look different in bright regions, so switching the tonemap is a visual
decision on its own, independent of HDR.

### Where the API is, and where it is not

Verified against wgpu 29.0.4:

- **`GpuCanvasToneMapping` bindings exist**, but only under
  `wgpu-29.0.4/src/backend/webgpu/webgpu_sys/` — the auto-generated web-sys layer.
- **They are absent from `wgpu-types`.** `SurfaceConfiguration` carries only `usage`,
  `format`, `width`, `height`, `present_mode`, `desired_maximum_frame_latency`,
  `alpha_mode`. No colour space, no tone mapping.

So the mechanism is specified, wgpu vendors the bindings, and the portable API does not
surface it. Two escape hatches, both bypassing that API:

1. **Web (wasm):** reach `GPUCanvasContext.configure({ toneMapping: { mode: "extended" } })`
   through the web-sys bindings wgpu already ships. Not done.
2. **Native macOS:** `wgpu::Surface::as_hal` down to the `CAMetalLayer`. **Done** —
   `voxel_color::color_space`, and it needed only *one* of the two things listed here
   originally. See below.

Neither is portable, and that is the honest cost of tier 2 — not the shader work.

## The colour-space tag: the bug that made HDR look worse than SDR

The symptom, reported from an XDR MacBook: switching to `HdrFloat` made the speckle
blocks light up correctly **and greyed out everything else**. Not dimmer — *shifted*,
as though the whole spectrum had moved rather than the colours having gained range.

That is a missing transfer function, and it was.

### What Apple actually documents

`CAMetalLayer.colorspace`:

> The default value is `nil`, indicating that the rendered content isn't
> color-matched. If you set this to a different color space, Core Animation performs
> any necessary color transformations when compositing the view's contents.

An Apple engineer, on the same property:

> The fastest approach is to opt out of color matching entirely, by setting the
> `colorspace` property to `nil`.

So **untagged is not neutral — it is pass-through.** The panel receives whatever numbers
are in the drawable and displays them under its own native transfer function.

### Why that greys the image

`HdrFloat` originally wrote scene-linear radiance and skipped the sRGB encode.
Handed to an untagged layer, linear 0.5 lands where *encoded* 0.5 lands — 0.21 linear.
Mid-tones drop to about a fifth of intended luminance, shadows further, and only
near-white survives intact. Perceived saturation falls with luminance, so it reads as
washed and shifted rather than simply dark. Exactly the report.

And the speckle still worked, which is the detail that made it confusing: extended range
survived untouched, because range and encoding are independent. The bright thing stayed
bright while everything around it went wrong.

### Both directions were wrong, not just HDR

The integer depths write sRGB-*encoded* values into an untagged layer too. Pass-through
means a P3 panel reads sRGB primaries as its own — everything slightly oversaturated. So
SDR was subtly too vivid while HDR was badly too dull, and no toggle between them could
have matched. **Tagging both is what makes the depth toggle change only the depth.**

### The three settings, and who sets each

Apple's EDR recipe, against what we were doing:

| setting | who does it | was it done? |
|---|---|---|
| `pixelFormat = .rgba16Float` | `OutputFormat::surface` | yes |
| `colorspace = extendedSRGB` | `voxel_color::color_space` | **no — this was the bug** |
| `wantsExtendedDynamicRangeContent = true` | **`wgpu-hal`, automatically** | yes |

That third row corrects a claim this document made and the crate repeated: that the EDR
flag was the missing piece and needed reaching through `as_hal`. It never did.
`wgpu-hal-29.0.4/src/metal/surface.rs:93` sets it whenever the surface format is
`Rgba16Float`:

```rust
// opt-in to Metal EDR
let wants_edr = config.format == wgt::TextureFormat::Rgba16Float;
if wants_edr != render_layer.wantsExtendedDynamicRangeContent() {
    render_layer.setWantsExtendedDynamicRangeContent(wants_edr);
}
```

Two lessons worth keeping: the flag we chased was already handled, and the one thing
nobody mentioned was the one that mattered. `wgpu-hal` deliberately does not set
`colorspace` — it cannot know what the application's values mean.

### Then the overlay went pale, because the surface has TWO writers

Tagging `extendedLinearSRGB` fixed the scene and broke the UI: egui rendered light grey
and washed out. That is the same class of bug pointing the other way, and it exposed the
assumption underneath the whole design.

**egui chooses its own transfer function.** `egui-wgpu-0.35.0/src/renderer.rs:406` picks
its fragment entry point from the target format:

```rust
entry_point: Some(if output_color_format.is_srgb() {
    "fs_main_linear_framebuffer"     // hardware will encode
} else {
    "fs_main_gamma_framebuffer"      // egui encodes itself
}),
```

So egui writes **gamma-encoded** into any non-sRGB target, and `Rgba16Float` is non-sRGB.
Lining that up against what the shading pass wrote:

| depth | blit writes | egui writes | agree? |
|---|---|---|---|
| 8-bit (`Bgra8UnormSrgb`) | linear → HW encodes | linear → HW encodes | ✓ encoded |
| 10-bit (`Rgb10a2Unorm`) | encoded, passed through | gamma-encoded | ✓ encoded |
| HDR float, first attempt | **scene-linear** | gamma-encoded | ✗ |

One tag cannot serve two conventions. Whichever it named, the other writer was wrong —
and only one of the two is ours to change.

### The resolution: HDR is not a different KIND of value

**The encode stays on in every mode. `HdrFloat` changes the tonemap and nothing else.**

The original reasoning was that an sRGB encode would flatten the extended range the way
Reinhard does. That is false, and the difference is the crux:

| | ceiling | preserves values above 1.0? |
|---|---|---|
| `tonemap_reinhard` — `L/(1+L)` | 1.0 by construction | **no** |
| exact extended-sRGB encode | none; monotonic and finite | **yes** — linear 4.0 → about 1.82 |

**Range survives an encode; it does not survive Reinhard.** Two separate problems, and
only the second needed solving. So the surface is tagged `extendedSRGB` — the *encoded*
extended space, `kCGColorSpaceExtendedSRGB`, also Vulkan's
`VK_COLOR_SPACE_EXTENDED_SRGB_NONLINEAR_EXT` — and both writers are correct with no extra
pass.

The alternative was to keep linear and render egui into its own `Bgra8UnormSrgb` texture,
then composite it down with an sRGB decode. That is the more conventional EDR shape, and
it costs a texture, a pipeline, a pass, and a premultiplied-alpha unpremultiply/decode/
re-premultiply dance that would land squarely on text antialiasing. Rejected on cost, not
on principle.

**The general rule:** when a resource has a writer you do not control, adopt the contract
that writer already satisfies.

The encode and decode are the exact piecewise IEC 61966-2-1 functions, extended by signed
reflection outside the ordinary range. This is not optional once the surface carries the
`extendedSRGB` tag: the compositor applies that exact inverse. The former gamma-2.2
approximation made linear 0.01 display as roughly 0.014 (+40%) and linear 4.0 as roughly
4.28 (+7%). The same exact helpers now decode authored material colours and encode the
finished frame.

`tonemap_hdr` also gained a `max(color, 0)`. `pow` of a negative base is NaN; a unorm
surface clamps NaN to zero and hides it, a float surface stores it and the compositor
renders whatever it makes of it. That guards a bug, it does not shape a look.

## Headroom is measured, not assumed

`tonemap_hdr` needs one number: the display's peak luminance over SDR reference white. It
used to be `const OUTPUT_HDR_HEADROOM: f32 = 4.0` — a claim about hardware nothing ever
checked. It is now `lighting.output_params.x`, probed **every frame**, because real EDR
headroom moves: the brightness slider changes it, thermal state changes it, dragging the
window to the other display changes it.

A uniform rather than a const, and no new binding was needed — `lighting` was already bound
and already uploaded every frame, which quietly retires the reason it was a const.

### The provider abstraction

`voxel_color::headroom::HeadroomProvider` — one method, one **named type per platform**, so
an unimplemented backend is a documented type rather than an invisible `else`:

| platform | provider | API | verified how |
|---|---|---|---|
| macOS | `MetalScreenHeadroom` | `NSScreen.maximumExtendedDynamicRangeColorComponentValue` | **runs on hardware** |
| Android / Quest | `AndroidDisplayHeadroom` | `Display.getHdrSdrRatio()` (API 34) via JNI | compiles for `aarch64-linux-android` |
| Windows | `DxgiOutputHeadroom` | `GetContainingOutput` → `IDXGIOutput6::GetDesc1` | compiles for `x86_64-pc-windows-msvc` |
| Linux, web | `UnsupportedHeadroom` | none stable | n/a |
| any | `ManualHeadroom(f32)` | — | the override |

**"Compiles" is not "works."** Only macOS has been seen running. Type-checking against the
real SDKs did earn its keep though — it caught `GetDesc1` returning by value rather than
filling an out-param, which reading the docs had not.

Each platform solves the same non-obvious problem — *which* display is the window on:
macOS walks `CAMetalLayer` → delegate/NSView → window → **that window's** screen (never
`mainScreen`); Windows uses `GetContainingOutput()`; Android uses `Activity.getDisplay()`
rather than the deprecated `getDefaultDisplay()`. Picking the primary display is wrong about
half the time on a laptop with an external monitor, which is exactly the setup that exposes
it.

Windows has a known weakness, stated rather than buried: `DXGI_OUTPUT_DESC1` gives peak
nits but **not** the SDR white level, which Windows lets the user set independently
(typically ~200 nits, not 100). Dividing by 100 therefore over-reports headroom — the
dangerous direction. The real fix is a paper-white slider, which is what shipping games
expose for precisely this reason. It also prefers `MaxFullFrameLuminance` over
`MaxLuminance`: the latter is a small-highlight peak a panel cannot sustain across a whole
frame, and we render whole frames.

**Windows cannot be compiled from this workspace**, for a reason outside this crate:
`gpu-allocator 0.28` accepts `windows = ">=0.53, <=0.62"` and Cargo unifies it onto `0.61`
because `sysinfo` (via `bevy_egui` → `atrium-bevy`) requires `^0.61`, while `wgpu-hal 29`
needs `0.62`. wgpu-hal's DX12 backend then fails with ten type errors before reaching our
code. In a workspace containing only `voxel-color` it builds clean. Fix is a separate
workspace for the renderer, or waiting for `sysinfo`.

### The unmeasured fallback is 1.0, deliberately

Not a guess in the middle. The failure directions are asymmetric: claiming headroom that
does not exist gives the blown picture; claiming none gives a hard clip at white, which is
just SDR. Android's own `getHdrSdrRatio()` returns 1.0 when it cannot answer, which is some
confirmation the conservative choice is conventional.

**That fallback exposed a real shipping bug.** At headroom exactly 1.0, `tonemap_hdr`
computes `highs * room / (room + highs)` with both terms zero — `0/0`, NaN, for every pixel
at or below white. A unorm surface clamps NaN away; a float surface stores it. It was
unreachable while headroom was hard-coded to 4.0 and became reachable the moment the
fallback became 1.0. A unit test caught it before a display did; the denominator now carries
a `max`.

## Two selectors, so the look is comparable instead of asserted

Beside the Output row:

- **Headroom** — `Auto | 1x | 2x | 4x | 8x | 16x`. `Auto` is the only correct shipping
  setting; the fixed values exist because "the display says 1.6x" is unfalsifiable from
  outside. `4x` reproduces the old hard-coded behaviour exactly. `1x` shows what every
  platform without a provider does.
- **Tonemap** — `Reinhard | Reinhard+HDR | Knee | Hable | BT.2390 | GT7`, with provenance shown.

Both report their source, so a measurement is never mistaken for a guess.

| curve | provenance | mid-tones | above white |
|---|---|---|---|
| `Reinhard` | Reinhard et al. 2002 eq. 3 | compressed | impossible |
| `Reinhard+HDR` | ours: Reinhard plus a bounded C¹ continuation | **exact SDR through scene white** | approaches headroom |
| `Knee` | ours, not a standard | untouched | rolls into headroom |

**`Reinhard+HDR` is the HDR default.** It is plain Reinhard through scene white, adds only a
bounded highlight continuation, and becomes plain Reinhard for the entire curve at 1x
headroom. The knee's brightening is opt-in rather than something the depth toggle does
behind your back.

The replaced `Reinhard+W` implementation was Reinhard et al. equation 4 with display
headroom incorrectly supplied as its input white point. Algebra exposes both failures: at
`W = 1` it becomes identity, not equation 3, and as input grows it is unbounded. It could
therefore neither reproduce the stated SDR fallback nor guarantee the display-headroom
ceiling.

### Why BT.2390's computed knee is still not here

Not laziness — a missing input. The EETF maps *content peak* to *display peak* and needs
both. Display peak is now measured; **content peak is unbounded**, since an emitter can be
authored to 64x white. Normalising needs either a per-frame luminance reduction or an
authored scene maximum, and adding the curve before deciding that would mean inventing the
number. That decision is the next step, not the shader work — the Hermite shoulder itself is
a dozen lines once there is something to normalise against.

### Why the Knee curve looks BRIGHTER

The default depth switch no longer brightens the mid-tones: Reinhard+HDR deliberately
preserves the SDR mapping there. Selecting the Knee is a separate look decision, and it is
brighter because Reinhard was compressing every mid-tone.

| linear in | Reinhard / Reinhard+HDR below white | Knee out |
|---|---|---|
| 0.25 | 0.20 | 0.25 |
| 0.5 | 0.33 | 0.50 |
| 1.0 | 0.50 | 1.00 |
| 4.0 | 0.80 (Reinhard+HDR continues above this) | 4.00 |

`tonemap_hdr` is identity up to white and rolls off only above it. Reinhard compresses
everywhere — it maps linear 1.0 to display 0.5. So the room genuinely is brighter in HDR,
by design, and it is the tonemap doing it, not the colour space.

Whether the Knee's brighter reading is wanted is a look decision this document does not
settle. Reinhard+HDR is the comparison-safe default; Knee remains available for the
identity-through-white look.

### Why sRGB primaries and not P3 or Rec.2020

`extendedSRGB`, not `extendedDisplayP3` or an `ITUR_2020` variant, because that is what
the content *is*: albedos are authored sRGB and decoded to linear sRGB. Tagging wider
primaries would claim saturation the scene never had and skew every hue — it would make
colours *wrong*, not *richer*.

**This is the answer to "the colours should get enriched".** They do not, and cannot,
from a swapchain tag. Extended sRGB buys **headroom above white**, not gamut. Wide-gamut
colour is a separate axis that starts at the material — see
[Gamut is a separate axis](#gamut-is-a-separate-axis).

The `extendedGray` / `extendedLinearGray` spaces are the single-channel variants of the
same pair and have no use here; the axis that matters is `extended*` versus
`extendedLinear*`, which is transfer function, not channel count.

### Where it lives, and the one ordering rule

`crates/voxel-color/src/color_space.rs`. `SurfaceColorSpace` has two variants and
deliberately **no `Untagged`** — leaving the surface unlabelled is the bug, not a third
option. `ColorSpaceOutcome` is returned rather than swallowed, and named in the startup
log, because a silent no-op is what made this hard to find.

`Gpu::configure_surface` is now the only place that calls `Surface::configure`, and it
tags in the same function. The two are not independent: a configure that forgets the tag
renders a wrong picture that nothing validates and no error mentions. One rule —
**update `output_format` before configuring**, or the new surface gets the old depth's
colour space. `set_output_depth` carries that as a comment.

Objective-C versions in `voxel-color/Cargo.toml` are **pinned to whatever `wgpu-hal` 29
uses** and must move with it. We reach the layer through `as_hal`, so wgpu-hal's
`CAMetalLayer` and ours have to be the same type; a skew is a type error at
`setColorspace`, not a warning.

### The nits convention

Adopted so authored values mean something absolute. SDR reference white is 100 cd/m²
(Rec.709/sRGB):

```
linear 1.0   =    100 nits   <- SDR reference white
linear 100.0 = 10,000 nits   <- the PQ ceiling, color(rec2100-pq 1.0)
nits = linear * 100
```

Both numbers are now constants — `voxel_color::SDR_REFERENCE_WHITE_NITS` and
`PQ_CEILING_NITS`, with `nits()` / `linear_from_nits()` either way — so ranges can be
checked against the convention instead of against a magic number, and the graph's own
range test is written in nits rather than multipliers.

The six `hdr_*` rows were authored on this convention already: base at 1.0, speckle at
100. **They needed no re-authoring when the tag landed** — in 8-bit the speckle clamps to
white and the block looks flat; in `HdrFloat` on an XDR panel it is visibly brighter than
its own base. Confirmed on hardware: speckle visible on the MacBook XDR display,
invisible on an SDR external — which is the whole behaviour the rows were built to prove.

### Authoring above white — the colour picker question

A colour picker cannot author HDR, and that is not a limitation to fix: **a swatch
authors chromaticity, magnitude comes from a scale.** Every HDR tool splits it this way,
`color_edit_button_rgba_unmultiplied` clamps to `[0, 1]` like all of them, and a picker
that let you drag to 40x white would be a worse picker.

So the HDR authoring surface is exactly one control: `material.emission_strength`.
Emission is the only authored quantity that can legitimately exceed white — albedo above
1.0 creates energy, so `base_color` stays in `[0, 1]` on purpose.

What changed:

- **Range 16x → 64x.** 64x = 6400 cd/m², past the 1600 cd/m² peak of the brightest panel
  we target, so nothing physically displayable is unreachable. Deliberately **not** the
  100x PQ ceiling: that is a signalling limit no display realises, and spending most of
  the slider on it would make the 0–2 band everything else lives in undraggable.
- **The descriptions now state the convention** — that 1.0 is a 100 cd/m² white, that
  above 1.0 only reaches the display in `HdrFloat`, and that the swatch is chromaticity
  only. These are declaration data, so the generic inspector tooltip picks them up with
  no per-node UI branch.
- **A test ties the range to the nits constants**, so the slider cannot silently stop
  reaching what the panel can show.

One caveat, stated because it is a real asymmetry and not a bug: the GI volume quantises
radiance into `[0, 1]` (`cagi.rs` `quantize_radiance`), so past 1.0 the *bounced* light
saturates while the lit surface itself keeps brightening. That is pre-existing behaviour
at any strength above 1.0, not something the wider range introduced — but a 40x emitter
will light the room like a 1x one.

### PQ versus extended range

Extended range (an `Rgba16Float` surface plus `extended` tone mapping) is the simpler
route and is effectively what `testufo.com/ufos/__hdr.js` does — it writes
`color(rec2020 0 3 0)`, i.e. wide primaries with values above 1.0, and lets the browser
map them.

PQ is the alternative and carries a trap worth stating: **a PQ signal of 1.0 is 10,000
nits, not white.** PQ is absolute, with SDR white near signal 0.5, and it spreads codes
perceptually across the whole 0–10,000 range — so it spends far FEWER codes on the
0–100 nit band than gamma 2.2 does. That is why HDR standards mandate 10-bit and why
8-bit PQ bands catastrophically in shadows. **If a PQ path lands, every amplitude in
this document needs recomputing against the PQ curve**, not merely re-checking.

### Gamut is a separate axis

Brightness and gamut are independent, and only the first is discussed above. Rec.2020
magenta is outside sRGB entirely and no amount of precision or headroom reaches it —
that needs wide primaries on the surface plus a decision about whether material albedos
are authored in sRGB and converted, or authored in the wider space. The six pure
primaries and secondaries are the rows that will show it first.

---

## What the curves cost (M3 Max, 2560x1440, 2026-08-03)

Everything above argues about which curve is *correct*. This section prices them,
because five selectable curves is a lever set and an unpriced lever is a liability.
Harness: **bench section 14**, `cargo run -p voxel-rt --example bench_dda --release -- 14`.

The section is unusually exact, and the reason is structural: **the curve is a runtime
uniform** (`lighting.output_params.y`), so every column runs the same shader, the same
pipeline object and the same converged light volume. The only thing that differs between
two timings is four bytes in a buffer. Nothing about pipeline caching, shader residency
or brickmap state can leak into a delta. Columns are interleaved round-robin, so the
thermal ramp over a 26-second run lands on all twelve equally.

### Per-curve cost

Per-dispatch median ms at 2560x1440 (3.7 Mpx), exposure 1.0, `RenderQuality::default()`.
Deltas are against Reinhard at headroom 1x — the shipped SDR curve and the cheapest thing
the branch can do. Two independent runs; the range covers both.

| curve | A aerial | C ground | delta vs Reinhard | per megapixel |
|---|---|---|---|---|
| **Reinhard** | 3.438–3.440 | 3.079–3.081 | baseline | — |
| **Reinhard+W (superseded implementation)** | 3.437–3.454 | 3.043–3.083 | **free** | — |
| **Knee** (ours) | 3.434–3.440 | 3.077–3.079 | **free** | — |
| **Hable** | 3.439–3.444 | 3.078–3.090 | **free** | — |
| **BT.2390** | 3.591–3.598 | 3.237–3.242 | **+0.16 ms** | ~43 µs |
| **GT7** | 3.675–3.700 | 3.330–3.339 | **+0.25 ms** | ~68 µs |

`Reinhard+HDR`, which replaced the measured `Reinhard+W` implementation for correctness,
has not been re-benchmarked yet. It remains in the same small-ALU cost class, but that is
an expectation until section 14 is rerun rather than a measured claim.

**The delta is scene-independent, and that is the measurement's own consistency check.**
A tonemap is the one per-pixel term here that does not scale with scene complexity — it
runs once per pixel after shading — so its absolute cost must be identical on the aerial
and ground shots while the frame around it is not. It is: BT.2390's four measurements
(two scenarios x two headrooms) land at +0.153 / +0.153 / +0.156 / +0.157 within a single
run, a spread of 4 µs. If those had disagreed, the table would have been measuring
something other than the curve.

Headroom 1x versus 4x changes nothing measurable, including for GT7, which takes a
different code path below the SDR boundary (`peakTarget = 2.5` plus a correction
multiply). The branch is uniform across the dispatch, so it costs the taken path only.

**The ratio checks against the arithmetic.** BT.2390 does 3 PQ encodes + 3 PQ decodes
(~12 `pow`); GT7 does two ICtCp round trips plus three curve evaluations (~24 `pow`).
Predicted 2x, measured 1.6x — the gap being fixed overhead the ratio does not include.
The cost is transcendental throughput, not bandwidth or branching.

### The five curves you did not select are free

The table above cannot answer this, and that is worth being explicit about: every column
runs the *same shader*, which is what made the comparison exact and is exactly why none
of them prices the arc itself. The concern is not the dispatch branch — that is a few
instructions — it is that GT7's two ICtCp matrices and BT.2390's PQ constants are live
values in one function, and **register allocation is decided by a kernel's worst path,
not its taken one**. Had that pushed occupancy down a step, every column would have been
slow together and the table would show a flat, innocent zero.

So section 14 also runs a compile-time A/B, in the shape of E1c's
`fade_range_as_shader_consts_variant`: the shipped source against one with
`apply_tonemap` collapsed to its Reinhard return, which makes the other five unreachable
and lets the Metal compiler drop them before register allocation. That second variant is
what the output path looked like before this arc, so the delta is the arc's resident cost.

| scenario | six-curve (shipped) | one-curve (pre-arc) | resident cost |
|---|---|---|---|
| A aerial | 3.431 | 3.443 | **-0.012 ms (-0.3%)** |
| C ground | 3.059 | 3.068 | **-0.009 ms (-0.3%)** |

Negative in both, i.e. zero within noise. **Adding five curves to the shader costs
nothing until one of them is selected.** The selector is free; only the choice has a price.

### The verdict, and the one place it matters

Choose freely on desktop. 0.25 ms is 8% of *this* frame because this frame is 3 ms — the
right denominator is the frame budget, and 0.25 ms is 1.5% of a 60 Hz one.

**Quest is where the number bites, and it is worse than proportional.** Per megapixel is
the honest unit: ~68 µs/Mpx for GT7 on an M3 Max. The Quest preset renders 2048x1152
(2.36 Mpx), so a naive scaling gives ~0.16 ms — but that scaling is optimistic twice
over. Mobile GPUs are disproportionately weak at transcendentals relative to general
ALU, and `pow` is the whole cost here; and a stereo pass pays it per eye. **This has not
been measured on-device and the extrapolation should not be quoted as though it had.**
The Quest verdict is therefore the standard lever-hygiene one: GT7 and BT.2390 stay
selectable and stay measured, and the Quest tier defaults to a free curve until someone
runs section 14 on the hardware.

### The two optimisations this measurement makes available, neither taken

Both are recorded rather than done, because the shipped default is a free curve and
neither is worth spending before an on-device number exists.

- **GT7 computes one ICtCp round trip more than it uses.** `rgb_to_ictcp(skewed_rgb)`
  is called for `.x` alone, and the I channel is a function of L' and M' only — the S
  encode is dead. Roughly one sixth of the ICtCp work.
- **BT.2390 is per-channel as we apply it, so a 1D LUT would collapse it to a texture
  fetch.** GT7 cannot use one: it is a colour-volume operator by construction, which is
  the entire reason it preserves highlight hue, and a per-channel LUT would throw away
  the property it was added for.

---

## Where the curves live: voxel-color owns both halves

Moved 2026-08-03, after the cost measurement above. The curves were implemented **twice**
— Rust in `voxel-color/src/tonemap.rs`, WGSL in `voxel-rt/shaders/dda.wgsl` — in two
crates, with nothing holding them together.

**The duplicated maths was not the strongest argument; the duplicated CONTRACT was.**
`TonemapCurve::shader_index()` decides that GT7 is 5. `dda.wgsl` independently declared
`const TONEMAP_GT7: u32 = 5u` and dispatched on it. Insert or reorder a variant and the
selector silently renders the wrong curve — a wiring bug that presents as a rendering bug,
on the one control whose entire purpose is comparing curves by eye. Across a crate
boundary nothing could check it.

So `crates/voxel-color/shaders/tonemap.wgsl` now holds the 389-line GPU implementation,
exposed as `voxel_color::tonemap::WGSL`, and four tests hold the halves together:

| test | what it prevents |
|---|---|
| `wgsl_curve_indices_match_the_rust_enum` | the enum and the WGSL consts disagreeing |
| `every_curve_has_a_wgsl_implementation_and_a_dispatch_arm` | a menu entry with no curve behind it |
| `the_two_halves_implement_the_same_curves` | a curve added to one side only |
| `the_wgsl_is_self_contained` | the file growing a binding or a uniform read |

### The CPU half is real code now, not test scaffolding

A follow-on the same day, and the honest correction to the paragraph above: when the WGSL
first moved, "owns both halves" was only three quarters true. The Rust curves —
`reinhard`, `hable`, the PQ pair, `bt2390`, the ICtCp transforms, `gt7`, about 153 lines —
lived inside `#[cfg(test)]`, so nothing outside a test binary could evaluate a curve.

They are now `voxel_color::tonemap::reference`, which puts the crate in line with the
house pattern rather than inventing one: `pattern.rs` and `material_graph.rs` in `voxel-rt`
both carry production CPU reference evaluators beside their WGSL, for the same reason.

Three things the promotion bought that the test-only version could not:

- **`reference::apply` mirrors `apply_tonemap`, and its `match` is exhaustive.** A new
  curve cannot reach `main` without a CPU implementation — the compiler enforces on this
  side what a string test has to enforce on the WGSL side.
- **The signatures carry the distinction the prose keeps making.** Five curves are
  `fn(f32) -> f32` because they are per-channel; `gt7` is `fn([f32; 3]) -> [f32; 3]`
  because it is a colour-volume operator and cannot be expressed one channel at a time.
  `only_gt7_lets_one_channel_affect_another` asserts exactly that, across all six.
- **The dispatcher is testable.** `apply_routes_every_curve_to_its_own_implementation`
  catches a copy-paste sending Hable to `reinhard` — which compiles fine and would present
  as "Hable looks wrong" rather than as a wiring bug. And
  `content_peak_moves_only_the_curve_that_declares_it` checks `uses_content_peak` against
  behaviour, so the overlay cannot hide a control that still bites.

**What did NOT get split, and why.** The file was 979 lines but only **295 of production
code** — the smallest production body in the crate, against `headroom.rs`'s 633. The rest
was tests, and inline `#[cfg(test)]` is universal here (59 files, no exceptions), so
moving them out would have made this the one file breaking the convention in exchange for
a smaller number. Nor is the enum worth splitting by curve: six methods each `match` the
same six variants, so answering *"what is GT7"* would mean opening six files to read one
arm from each.

Plus, on the renderer side, `both_pass_shaders_share_the_traversal_core` now also asserts
`dda.wgsl` **calls** `apply_tonemap` and never **defines** it — the same shape as the
existing "neither pass may carry its own copy of `dda_step`" rule, one crate out.

**The move was cheap because the seam was already clean.** The block called nothing
outside itself: every non-builtin (`gt7_curve`, `rgb_to_ictcp`, `ictcp_to_rgb`,
`pq_from_relative`, `relative_from_pq`, `bt2390_channel`, `hable_partial`) was defined
within it, and it touched no binding, no uniform and not `srgb_encode`. Exposure and the
encode were always the caller's.

Two mechanical consequences, both deliberate:

- **`passes::dda::SHADER_SOURCE` is now a `LazyLock<String>`, not a `&'static str`.**
  `concat!` takes literals, and the piece from another crate is a const. `voxel-color`'s
  manifest states a deliberate policy — *"only wgpu and the platform colour-management
  API"* — so pulling in `const_format` to keep the const was the wrong trade for a string
  join. Ten call sites took `&` or `.as_str()`; no shim.
- **The colour path is concatenated LAST.** WGSL module-scope declarations may appear in
  any order, so position is free — and last keeps `world.wgsl` at the front, which
  `both_pass_shaders_share_the_traversal_core` reads as a `starts_with`.

Section 14 re-run after the move reproduces the table above, and
`every_lever_combination_compiles_headless` plus `both_output_depths_build_a_valid_shading_pipeline`
still pass, so the splice is validated on every lever combination rather than just the
shipped one.
