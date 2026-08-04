# voxel-color

The output colour path: how finished radiance becomes pixels on a display. Bit depth,
transfer functions, texture formats, display headroom, tone mapping curves, the nits
convention.

Nothing here knows about voxels, materials, brickmaps or render passes. **The dependency
runs one way** — the renderer asks this crate what formats and curves to use, never the
reverse. If this crate ever needs to know what a voxel is, the boundary is in the wrong
place.

- **How it works today** → module docs, and `docs/output-depth.md` for the full history
  including every bug and its error message.
- **What it costs** → `docs/voxel-rt-bench.md` section 14.
- **What it should eventually be** → this file.

---

## Why this file exists

The output path shipped in a state that is *correct but not finished*. Several of the
remaining gaps are not oversights — they are blocked on machinery that does not exist yet,
and the reasons are non-obvious enough that a future reader would otherwise rediscover
them the slow way, or "fix" something that is deliberate.

So: what the right answer looks like, what stands between here and there, and which
decisions must survive the journey.

---

## The shape of the correct answer

The standards world assumes **two stages**, and conflating them is the single most
expensive mistake available in this domain:

```
scene-referred radiance ─[ 1. tone map ]─▶ display-referred image ─[ 2. display map ]─▶ panel
     unbounded, ungraded                    graded to a known peak        this display's peak
```

**Stage 1 is a rendering transform.** It decides what the image *looks like*: where
mid-grey sits, how highlights roll off, whether saturated highlights keep their hue. GT7,
Hable and ACES live here. It is parameterised by artistic intent plus, at most, the
display's peak.

**Stage 2 is display mapping.** It takes content *already graded to a known peak* and fits
it onto a panel with a different one. BT.2390's EETF lives here. It is what a TV or a media
player runs. Its input contract assumes a mastered signal.

### Where we sit

**We are the last step.** We write extended-range float into a compositor whose headroom we
have measured, so there is no separate stage 2 after us. That makes stage 1 our whole job,
and it is why `TonemapCurve::Gt7` — scene-referred, parameterised by the display alone — is
the curve that structurally fits.

**BT.2390 is offered anyway, and it is the one curve here doing the wrong stage's job.**
That is not a bug in the implementation; it is the reason it needs a content peak nothing
can measure. See [Gap 2](#gap-2--bt2390-has-no-content-peak).

---

## Status

| concern | today | ideal |
|---|---|---|
| bit depth | 8 / 10 / HDR float, runtime switchable | ✅ |
| surface colour space | tagged on Apple; HDR vetoed where no compatible hook exists | tagged everywhere HDR is exposed |
| display headroom | **measured** per frame; macOS runs, Android + Windows compile only | measured everywhere, verified on device |
| exposure | its own uniform term, manual slider | driven by measured scene statistics |
| tone curves | six, selectable at runtime, cost measured | ✅ (plus maybe ACES) |
| BT.2390 content peak | **assumed** (10× = 1000 nits) | measured, or a fixed mastering peak |
| scene radiance scale | arbitrary (`SUN_INTENSITY = 2.2`, tuned to fit Reinhard) | physical |
| gamut | sRGB primaries only | wide primaries end to end |
| PQ / HDR10 output | none | optional path |
| CPU ↔ GPU curve parity | properties + indices checked, **values never compared** | one test evaluating both |

---

## The gaps

Ordered as they should be tackled, cheap-and-unblocking first.

### Cheap, no dependencies

**A real CPU↔GPU parity test.** The crate carries both halves of every curve —
`tonemap::reference` in Rust, `shaders/tonemap.wgsl` on the GPU — and tests that pin the
curve *indices*, the dispatch arms and mathematical *properties*. **Nothing evaluates both
over the same inputs and compares the numbers.** That is the strongest check available and
it is absent. The crate already depends on `wgpu`, so a headless device in a test is
reachable; `voxel-rt`'s `pattern.rs` and `report_cagi_cpu_cross_check` are the precedents
for what it should look like. Do this first — it is the thing that makes every other change
here safe.

**Plot the selected curve in the overlay.** The selector exists because *"comparison is the
entire purpose of the control"*, and right now you compare two curves by flipping between
them and remembering. A small plot beside the image would show at a glance why Reinhard
reads flat (linear 0.5 → 0.33) and where GT7's shoulder starts on *your* measured headroom.
`tonemap::reference::apply` was promoted out of `#[cfg(test)]` specifically so this is
possible; it is the consumer that justifies the promotion.

**A paper-white control.** `ASSUMED_WINDOWS_SDR_WHITE_NITS = 200.0` exists because
`DXGI_OUTPUT_DESC1` carries no SDR white level — it is user-set on Windows, commonly around
200 rather than our 100-nit convention, so `/100` over-reports headroom there. A slider is
the honest fix and it is small.

**Measure the curves on Quest.** GT7 costs ~68 µs/Mpx and BT.2390 ~43 µs/Mpx on an M3 Max
(bench section 14). Mobile GPUs are disproportionately weak at transcendentals, which is the
entire cost here, and a stereo pass pays per eye — so the desktop number extrapolates
*badly* and the docs deliberately do not quote it as though it were measured. Run `-- 14`
on device before either curve goes near a Quest default.

**Two recorded micro-optimisations, deliberately not taken.** Neither is worth spending
before an on-device number exists:
- GT7 calls `rgb_to_ictcp(skewed_rgb)` for `.x` alone, and I is a function of L′ and M′
  only — the S encode is dead. Roughly one sixth of the ICtCp work.
- BT.2390 is per-channel as applied, so a 1D LUT would collapse it to a texture fetch.
  **GT7 cannot use a LUT** — being a colour-volume operator is precisely why it holds
  highlight hue, and a per-channel LUT would discard the property it was added for.

### Gap 1 — exposure is manual because nothing measures the scene

**Perfect:** a per-frame luminance reduction over the rendered image (histogram, or a max
with percentile rejection), temporally smoothed, driving exposure automatically.

**Why not now:** no reduction pass exists. It needs a compute pass plus either a readback or
a GPU-resident histogram, and it needs smoothing — without it the curve pumps as you turn
your head and a bright emitter enters frame.

**Unblocks:** auto-exposure *and* [Gap 2](#gap-2--bt2390-has-no-content-peak). **Build them
together** — a histogram for BT.2390 alone, followed later by a second one for exposure,
is doing the work twice.

### Gap 2 — BT.2390 has no content peak

**Perfect:** the EETF maps *content peak* → *display peak*. We measure the display half per
frame from the OS. The content half is `DEFAULT_CONTENT_PEAK = 10.0`, an assumption — 10× =
1000 cd/m², chosen because it is HDR10's mastering baseline rather than a number that looked
reasonable.

**Why not now, and this is the important part:** **the content peak is not a number you
measure, it is an *output* of stage 1.** No probe was ever going to appear for it. Our
content peak is also genuinely unbounded — `emission_strength` authors to 64× white — so
there is no static scene maximum either.

Getting it wrong costs both directions: too high and the curve compresses range that was
never used; too low and highlights clip before the shoulder reaches them.

> **Sharp edge:** `bt2390` returns **exact identity** when `content_peak <= display_peak`.
> Set scene peak to 2× on a 4×-headroom display and BT.2390 does nothing at all. It looks
> like a broken curve and is a correct no-op. The overlay should probably say so.

**Two routes out, and the cheap one is underrated:**

1. **Adopt a fixed mastering peak** (tone map to a constant 1000 nits with GT7, then
   BT.2390 to fit the panel). Content peak stops being an assumption and becomes a constant
   we chose. This makes *both* curves correct, in the order the standard intends, and needs
   no histogram. **Probably the right next move.**
2. **Measure it** — Gap 1.

### Gap 3 — the scene's radiance scale is not physical

**Perfect:** radiance in real units, the sun at a physical value, exposure as a genuine
EV / aperture-shutter-ISO analogue.

**Why not now:** `SUN_INTENSITY = 2.2` was chosen so a sunlit surface *"lands near the top
of the Reinhard curve's usable range"* — the scene's absolute scale was tuned to fit a
curve. This is the root cause the whole HDR arc kept circling back to: with no exposure
term, the tonemap was doing exposure's job, which is why switching curves read as a
brightness change. Exposure now exists, but the scale it operates on is still arbitrary.

Everything downstream is calibrated against that arbitrary scale — `emission_strength`
ranges, the material table, the look of every authored graph. Moving it is a re-authoring
pass, not a constant change.

> **A real physical inconsistency to fix with it:** `cagi.rs::quantize_radiance` clamps to
> `[0, 1]`, so past 1.0 **bounced** light saturates while the lit surface keeps brightening.
> A physical scale cannot be built on top of a GI volume that cannot represent it.

### Gap 4 — gamut is a separate axis, and it is untouched

**Perfect:** wide primaries end to end — materials authored in a defined space, the surface
tagged with wide primaries, the curve applied in a known working space. The reference shape
is the three.js path: tone map → Rec.709 → Rec.2020 → P3 → transfer function.

**Why not now:** it starts at the *material*, not the surface. Albedos are authored as sRGB
0–1 swatches with no colour-space tag, and there is a decision to make first — author in
sRGB and convert, or author wide. Note the correction already in the docs: **extended-linear
sRGB buys headroom above white, not gamut.** No swapchain tag enriches colour.

The six pure-primary/secondary diagnostic materials are the rows that will show it first.

### Gap 5 — no PQ output path

**Perfect:** an optional HDR10/PQ surface for paths that want absolute encoding — a TV, or a
platform where the compositor expects it.

**Why not now:** low value here (macOS extended-range already works and we are the last step)
against real cost. **PQ signal 1.0 is 10,000 nits, not white**; PQ is absolute, spends far
fewer codes on the 0–100 nit band than gamma 2.2 does, and 8-bit PQ bands catastrophically
in shadows — so it forces 10-bit minimum. Every amplitude in the diagnostics needs
recomputing against the PQ curve, not merely re-checking.

This becomes interesting alongside route 1 of Gap 2: master to a fixed peak, emit PQ, let
the display run its own EETF.

### Gap 6 — infrastructure

**Windows cannot be built from this workspace.** `gpu-allocator 0.28` accepts
`windows >= 0.53, <= 0.62`, but Cargo unifies it onto `0.61` because `sysinfo`
(via `bevy_egui` → `atrium-bevy`) requires `^0.61`, while `wgpu-hal 29` needs `0.62` — ten
type errors inside wgpu-hal's DX12 backend. **Not caused by this crate**; it compiles clean
in a workspace containing only `voxel-color`. Fix is a separate workspace for the renderer,
or waiting for `sysinfo`.

**Android and Windows headroom providers compile but have never run.** `aarch64-linux-android`
and `x86_64-pc-windows-msvc` both build. Compiling still earned its keep — it caught
`GetDesc1` returning by value rather than filling an out-param — but neither is verified on
hardware. Their HDR output choices remain gated off until those platforms also have a
presentation path matching the renderer's encoded extended-sRGB contract; an FP16 format
by itself is not sufficient. Quest is Android, so `AndroidDisplayHeadroom` is the one that
matters most once that presentation contract lands.

---

## Decisions that look wrong and are right

**Do not "fix" these.** Each is counter-intuitive, each was arrived at the expensive way,
and each has a test holding it.

**Tag the surface `extendedSRGB`, not `extendedLinearSRGB`** — even though every Apple EDR
sample uses the linear one. This surface has **two writers**: egui picks its transfer
function from `format.is_srgb()` and writes gamma-encoded into any non-sRGB target. A linear
tag fixes the scene and turns the overlay pale grey. So every mode keeps the sRGB encode and
`HdrFloat` swaps only the tonemap.

> General rule worth carrying elsewhere: *when a resource has a writer you do not control,
> adopt the contract that writer already satisfies.*

**Range survives an sRGB encode. It does not survive Reinhard.** The exact extended-sRGB
transfer is monotonic and finite above 1.0 — linear 4.0 encodes to about 1.82. Conflating
"encoded" with "clamped" cost a full round trip of wrong design. The exact function matters:
tagging the surface extended sRGB while encoding with a gamma-2.2 approximation made linear
0.01 display as roughly 0.014.

**Reinhard's equation 4 is not an HDR output bound.** Its `W` is an input white point. At
`W = 1` it simplifies to identity rather than equation 3, and at high input it is unbounded.
The old `Reinhard+W` path passed display headroom as `W`, so the conservative 1x fallback
brightened and clipped instead of matching SDR. `Reinhard+HDR` now keeps equation 3 exactly
through scene white, adds a C¹ bounded continuation above it, approaches measured headroom,
and becomes exactly plain Reinhard for every nonnegative input at 1x.

**The unmeasured headroom fallback is 1.0, not something optimistic.** Over-claiming headroom
is the blown-highlight bug; under-claiming is merely SDR. This also exposed a real NaN in
`tonemap_hdr` — at headroom exactly 1.0 the shoulder term is `0/0` for every pixel at or
below white. Unreachable while headroom was hardcoded 4.0. A float surface *stores* the NaN;
unorm would have hidden it. **Caught by a unit test, not a display.**

**The tonemap curve is a runtime uniform, not a shader const.** A const would fold the branch
away, and the fear was that GT7's ICtCp matrices and BT.2390's PQ constants would raise
register pressure for the whole kernel. **Measured: ±0.3% across three runs, i.e. zero.** The
unselected curves are free, and the uniform buys the one thing a const cannot — flipping
between curves while looking at the same frame, which is the entire point of the control.

**Hable's final clamp is load-bearing and not in the original.** The normalised ratio climbs
to ~1.17 rather than stopping at white, because `hable_partial` is bounded by `1 − E/F` =
0.933 while `f(W)` is smaller still. The original relied on an 8-bit framebuffer clipping it;
a float surface keeps it. It is a fixed artifact that does not scale with headroom, so it is
not usable range.

**`SHADER_SOURCE` in `voxel-rt` is a `LazyLock<String>`, not a `&'static str`.** `concat!`
takes literals and `voxel_color::tonemap::WGSL` is a const. Adding `const_format` to keep the
const would violate this crate's stated dependency policy — *only wgpu and the platform
colour-management API* — for a string join.

**The colour path concatenates LAST into the DDA source.** WGSL module-scope declarations may
appear in any order, so position is free; last keeps `world.wgsl` at the front, which
`both_pass_shaders_share_the_traversal_core` reads as a `starts_with`.

---

## Measured facts, so nobody re-derives them

Apple M3 Max, 2560×1440 (3.7 Mpx), `RenderQuality::default()`. Full tables in
`docs/voxel-rt-bench.md` §14.

| | cost |
|---|---|
| Reinhard, Knee, Hable | **free** (inside noise) |
| Reinhard+HDR | same small-ALU class, but **re-benchmark after the correctness replacement** |
| BT.2390 | **+0.16 ms** (~43 µs/Mpx) |
| GT7 | **+0.25 ms** (~68 µs/Mpx) |
| the five unselected curves, resident | **zero** (−0.3%, straddles zero over three runs) |

The delta is **scene-independent**, which is the measurement's own consistency check — a
tonemap runs once per pixel after shading, so its absolute cost must match on the aerial and
ground shots while the frame around it does not. BT.2390's four measurements within one run
span 4 µs.

The BT.2390 : GT7 ratio matches the `pow` count (~12 vs ~24), so the cost is transcendental
throughput, not bandwidth or branching.

**Not folklore, corrected twice:** `wantsExtendedDynamicRangeContent` needs nothing from us —
`wgpu-hal` sets it for `Rgba16Float` itself. And gain maps (ISO 21496-1 / Apple Adaptive HDR
/ Android Ultra HDR) are a **delivery format**, not applicable to a per-frame renderer; what
transfers is only the principle, *adapt to the measured display*.

---

## Working on it

```sh
cargo test -p voxel-color                                   # the crate, ~55 tests
cargo test -p voxel-rt --lib output_format                  # the six-consumer contract
cargo test -p voxel-rt --lib passes::dda::tests::both_output_depths -- --nocapture
cargo run -p voxel-rt --example bench_dda --release -- 14   # curve cost + residency A/B
cargo run -p voxel-rt --release                             # the only way to check consumers 1 and 6

# The platform providers, neither testable on a Mac:
cargo check -p voxel-color --target aarch64-linux-android
cargo check -p voxel-color --target x86_64-pc-windows-msvc  # needs its own workspace, see Gap 6
```

**Adding a curve** touches five places, and three tests fail if you miss one: the
`TonemapCurve` variant, its `shader_index`, a `TONEMAP_*` const and a `fn tonemap_*` in
`shaders/tonemap.wgsl`, an arm in the WGSL `apply_tonemap`, and an arm in
`reference::apply` — the last enforced by the compiler, since that `match` is exhaustive.

**Status of the visual gate:** seen in the app on 2026-08-04. Per-curve look verdicts are not
yet recorded here; they belong in `docs/output-depth.md` when they are.
