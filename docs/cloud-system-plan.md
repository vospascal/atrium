# Cloud system plan

Volumetric clouds for the voxel renderer, built on `voxel-environment`.

Status: **planning**. No code written. Two decisions still open (see [Open decisions](#open-decisions)).

## Why this belongs in `voxel-environment`

The crate exists to hold one invariant: *the sky a camera sees and the sky that illuminates a
surface are the same thing*. Clouds are the strongest test of that invariant — Pascal's
framing was "the clouds that is but also the ground under it".

`shaders/environment/dispatch.wgsl` already splits along exactly that line:

| concern | entry point |
| --- | --- |
| what the camera sees | `sky_color`, `sky_color_at_distance` |
| what lights the ground | `environment_diffuse_radiance`, `ambient_light` |

Both read only this crate's own uniform. So clouds are an edit *inside* the crate; the twelve
consuming files in `voxel-rt` do not change. Cloud shadows add exactly **one** new entry
point, `environment_sun_transmittance(world_position)`.

## Constraints (Pascal, 2026-08-05)

> "as long as it works with dda and cagi it looks good and its fast and properly scatter lights"

| constraint | how it is met |
| --- | --- |
| works with DDA | `dda.wgsl:645-648` already computes a lambert × `trace_shadow_visibility` sun term; clouds multiply it |
| works with CAGI | `cagi.wgsl:335` computes the *same* term — one entry point serves both passes |
| properly scatters | `dda.wgsl:335` samples `ambient_light` per hemisphere ray and CAGI transports it, so cloud-modulated ambient propagates through existing GI |
| fast | half-res + spatial denoise + log-Z sampling + early-out; every knob a `RenderQuality` lever the bench sweeps |

## What CAGI can and cannot do

Design contributed by Pascal (2026-08-05): procedural density owns the *visibility
integration*; CAGI contributes only low-frequency indirect. That split is correct. Three
findings from verifying it against the code:

**CAGI has no directional information.** A cell is 2 × `u32`: word 0 packed RGB
(10:10:10 + shared exponent), word 1 attributes. So CAGI can only ever supply
indirect/multiple scattering — direct sun scattering must be computed separately. Not a
preference; forced.

**The participating-media channel already exists and is switched off.**
`cagi_volume.wgsl:210-214` holds a 4-bit (15-level) per-cell transmittance behind
`CAGI_TRANSMISSION`, with propagation that attenuates rather than treating cells as binary
solid — i.e. exactly the proposed `incoming_radiance *= medium_t`. Default-off pending an
app-run verdict. **Settle this before designing around it** (stage C0).

**Sampling CAGI at cloud altitude returns a constant.** The grid's Y is *deliberately*
clamped to occupied terrain height + `SKY_MARGIN_CELLS` (`cagi.rs:399`), and
`cagi_cell_radiance` returns `cagi_sky_radiance()` — one constant — for any cell above it.
A cloud sample a thousand metres up therefore gets no terrain bounce, no sunset warmth, no
lava, no lamps. The comment is right that allocating that space would be "paying for a
constant"; growing Y to cloud altitude is not the fix.

**The fix — a top-slice aggregate.** One compute pass reduces the topmost CAGI layer to an
8×8 or 16×16 map of upward radiance stored as **SH-L1** (4 coefficients per channel). The
cloud marcher takes one fetch. Real ground-bounce colour, no grid growth, and it restores
the directionality the packed-RGB cells discard. SH-L1 in every cell would be 4× the volume;
SH-L1 in an 8×8 aggregate is free. Stage C5.

Injecting *averaged cloud shadowing back* into the volume works unchanged, because it acts at
the grid's top layer — the one place the volume does exist. The cloud shadow map seeds that
layer and CAGI propagates it down. Stage C3.

## Technique

Three references, none sufficient alone. They agree on the skeleton and differ by scene
scale.

| source | contributes | withhold |
| --- | --- | --- |
| Schneider & Vos, *Nubis* (SIGGRAPH 2015) | the lighting model | its two-texture density stack |
| [Spiri0/volumetric-clouds](https://github.com/Spiri0/volumetric-clouds) (MIT) | the sampling strategy | it has no phase function at all |
| [Heckel, *Real-time cloudscapes*](https://blog.maximeheckel.com/posts/real-time-cloudscapes-with-volumetric-raymarching/) | directional derivatives, blue-noise + bicubic | constant step size, SDF shapes |
| [Brucks, *Creating a Volumetric Ray Marcher*](https://shaderbits.com/blog/creating-volumetric-ray-marcher) | volume/geometry compositing, inner-loop optimisation | pseudo-volume flipbook (UE4 workaround) |

Pascal rates **Heckel and Brucks the best two** of the set (2026-08-05), so they carry the
most weight in C4.

### Brucks — read in full 2026-08-05

An earlier revision of this doc claimed Brucks used a *baked lighting volume built by slice
propagation*. **That was wrong** — recorded here because it briefly flipped C4's default. The
article uses the **brute-force nested light march**, stated outright: *"the cost of the shader
will be DensitySteps × ShadowSteps, or N×M."* It also **refutes** the baked approach directly:
precomputed lighting *"will not match the new details that arise from the combination of
volume textures"* when a base volume is modulated by a tiling detail volume — i.e. exactly the
HZD-style stack C2 uses. Prebaked ambient is listed as an option and rejected for the same
reason, plus it cannot be rotated or instanced.

What it does contribute:

**Compositing clouds against opaque geometry — the only reference that covers this.** Every
other source is sky-only. `t1 = min(t1, localscenedepth)` stops the march at the depth buffer;
then, because terminating on whole steps leaves stair-steps where geometry cuts the volume,
**one final partial step sized to the remainder** is taken outside the loop. Unavoidable the
moment a cloud deck touches a mountain.

**View-aligned plane snapping.** Beginning the march at the bounding surface makes the sample
pattern inherit the box shape — moiré that *"betrays the box geometry"*. Snap ray starts to
view-aligned planes, stabilised against *both* screen depth and object position so slices do
not crawl as the camera moves.

**Analytic slab intersection to precompute the step count** (`t0`/`t1` via `invraydir`): no
per-step bounds check, no steps wasted outside the volume, and the full corner-to-corner
diagonal covered rather than a fixed 0–1 range.

**The log threshold trick.** The inner loop accumulates *linear density* (one add) rather than
transmittance (two multiplies + `1-x`). To still early-out on a transmittance threshold,
invert Beer's law: `shadow_threshold_distance = -log(threshold) / shadow_density`. Cheapest
possible inner loop, identical exit condition.

**A measured counter-intuitive result — for the variant registry.** The fastest shadow-loop
exit is the branchless `floor(0.5 + abs(0.5 - lpos))` summed ≥ 1. Per-component `if`
comparisons were slow, and **precomputing the shadow step count was the slowest method of
all** — the opposite of what works for the primary ray. Do not assume the primary-ray
optimisation transfers to the inner loop.

**Phase function on the directional light only, never on ambient** — deliberate in the
article, and the same split the CAGI analysis reached independently: direct sun explicit and
phase-weighted, indirect low-frequency and isotropic.

**Three upward offset samples for sky occlusion.** Complementary to C5's SH-L1 aggregate, not
a rival: the 3-tap gives *how much* sky reaches a voxel, the aggregate gives *what colour and
from which direction*. They multiply.

**Coloured extinction** by dividing shadow density by an RGB vector. Unnecessary for cloud
(Mie scattering is wavelength-neutral, as the article notes) but right for water and the other
`Media` classes.

Do not port: the **pseudo-volume flipbook** (2D atlas of slices) is a UE4 workaround for
missing 3D texture support — wgpu has native 3D textures, so the whole encode/decode layer is
dead weight. Also prefer `exp(-σₜ·Δ)` over the article's linear `transmittance *= 1 - density`,
which is step-count dependent at low step counts.

### Density

- One **tileable 128³ Perlin–Worley, R8 (2 MB)**, sampled at several frequencies for FBM
  detail. Spiri0's leaner alternative to Nubis's 128³ base *plus* separate 32³ erosion —
  one texture, one fetch chain.
- 2D **weather map** (R coverage, G precipitation, B cloud type). This is the surface
  `WeatherState` drives, and the reason weather is one texture rather than a hundred
  uniforms.
- **Height-density gradient** per cloud type — stratus flat, cumulus billowing,
  cumulonimbus tall. Type is one lerp, not three code paths.
- LOD: fewer octaves with distance.

### Marching

**Logarithmic Z distribution**, ~10× fewer samples far than near (Spiri0) — *not* Heckel's
constant step. The two disagree because of scene scale: constant step suits a cloud blob
near the camera; a deck spanning to the horizon needs log-Z. Constant step stays the right
choice for the *local* `Media` fog banks of `transparent-voxels-plan.md`.

Log-Z is not a new parameterization here. The aerial-perspective froxel already uses
"logarithmic distance between the configured near/far bounds", so **the cloud march shares
the froxel's Z space** and clouds composite with aerial perspective in one depth
parameterization instead of two that drift.

Plus: early-out at accumulated transmittance < 0.01; blue-noise ray-start offset.

### Lighting

- **Beer–Lambert** extinction, with σₛ (scattering) and σₜ (extinction) as **two named
  fields**. For cloud they nearly coincide (single-scatter albedo ≈ 0.999), which is why
  every reference conflates them — but `Media` also covers smoke at albedo ~0.2–0.5, which
  would come out glowing. Split them from the start.
- **Dual-lobe Henyey–Greenstein** (forward g ≈ 0.8 + back g ≈ −0.2) — the silver lining.
- **Beer–Powder** for the bright rim Beer's law alone makes dark. Known-fiddly: Heckel
  reports failing to implement it. The usual error is applying powder to *transmittance*
  instead of only to sun-facing in-scatter.
- **Multiple-scattering octaves** (Wrenninge): 2–3 octaves with decaying attenuation,
  contribution and eccentricity. This is what makes thick interiors glow instead of going
  black — the difference between "volumetric fog" and "cloud".
- **Cone-sampled light march**, 5–6 taps toward the sun, **distance-bounded** (~1000 m) and
  **density-adaptive** (fewer taps where primary density is low).
- **Directional derivatives** as a cheap lever: `(density(p) − density(p + k·sunDir)) / k`,
  2 samples instead of a light march (Heckel). A *local gradient*, so it self-shadows at
  small scale but cannot let one cloud shadow another 200 m away, and it handles a single
  light only. Right for the low quality tier; not a replacement.

### Ambient / sky lighting — Pascal's pick (2026-08-05)

Without an ambient term a fully shadowed cloud reads flat and dead, and this is the cheapest
credible fix in any of the references. Brucks takes **three upward offset samples** at
geometrically spaced distances (0.05, 0.1, 0.2 in texture space — near/mid/far occlusion),
accumulates their density, and applies:

```
light_energy += exp(-shadow_distance * ambient_density) * current_density * sky_color * transmittance
```

Three fetches for what would otherwise be a hemisphere of rays.

**This engine can do strictly better than the article at the same cost, because `sky_color` is
a constant `vec3` there and a real quantity here.** Two substitutions, no extra taps:

1. Replace the constant with a **sky-view LUT read** — physical, direction-dependent, and
   already correct at sunset because Hillaire's LUT handles it. The clouds then agree with the
   sky behind them by construction, which is the crate's whole invariant.
2. Add the **C5 SH-L1 ground aggregate**. The 3-tap gives *how much* sky reaches a voxel; the
   aggregate gives *what colour, from which direction*. They multiply — occlusion × incoming
   radiance — so terrain, sunset and lava tint the cloud undersides.

Refinements over the article: jitter or cone-spread the three offsets instead of pure +Y so
overhangs are not systematically under-occluded, and keep the geometric spacing — it is doing
real work, sampling three occlusion scales for the price of three taps.

Phase function stays off this term (see above): ambient is isotropic, direct sun is not.

### Denoise

**Spatial is the default, temporal is a lever.** Half- or quarter-res target, blue-noise
offset, then either Kawase (Spiri0) or 16-tap bicubic upscale (Heckel).

Reprojection is demoted deliberately: Spiri0 *tried temporal accumulation and abandoned it*
for fuzzy semi-transparent surfaces. It should still work here — a WebGL post-process has no
motion vectors or depth and we do, and clouds are effectively at infinity so a
stationary-but-turning camera reprojects by rotation alone, which is exact and
disocclusion-free (also the commonest VR case). But the perf gate must not depend on it, so
if it smears it stays default-off with a working fallback rather than a hole in the plan.

Blue-noise temporal offset should use the **golden-ratio increment** (Heitz & Belcour),
`fract(blue_noise + frame * 0.618…)`, not Heckel's `/sqrt(0.5)` — it decorrelates better
across frames.

## The cost problem

Back-of-envelope, from Heckel's stated 50 primary × 6 light steps at 6-octave FBM:

| | density evals / frame | noise fetches / frame |
| --- | --- | --- |
| 1080p full-res | ~230 M | ~1.4 G |
| quarter-res (960×540) | ~57 M | ~340 M |

At 60 fps the full-res figure is ~82 G fetches/s, sharing an M3 Max with DDA traversal,
CAGI, water and shadows. It is not close to feasible. Quarter-res with early-out and octave
LOD is in the plausible range, and that is *why* the C2 gate is a real go/no-go rather than
a formality — not because the technique is doubtful, but because the sample budget is the
whole problem.

Naming note: naga_oil rejects digit-terminated identifiers, so `worley_3d` /
`perlin_worley_fbm`, never `worley3`.

## Stages

Each stage gates on a real app run, per the project's stage-gate rule.

- [ ] **C0 — Settle `CAGI_TRANSMISSION`.** App run, verdict recorded. No new code. It is the
      participating-media channel this design needs and it is already written.
- [ ] **C1 — Coverage as a weather input, no geometry.** `CloudSettings` on
      `EnvironmentRequest`; `weather` bit on `EnvironmentInvalidation`; coverage modulates
      the sky-view LUT and sun illuminance. *Gate:* one slider darkens sky and ground
      together, and a test proves coverage invalidates view-independent state only. No new
      pass.
- [ ] **C2 — Density field + raymarch.** Tileable 128³ Perlin–Worley, weather map, height
      gradients, log-Z sampling sharing the aerial froxel's Z space, blue noise, early-out,
      half-res + spatial denoise. Camera-visible only; no lighting contribution yet.
      *Gate: perf go/no-go, cost recorded in the optimization ledger.*
- [ ] **C3 — Cloud shadow map.** Top-down sun-axis transmittance texture in the existing
      group-1 bind group; new `environment_sun_transmittance(pos)` entry point consumed by
      DDA and CAGI; seeds CAGI's top layer. *Gate:* a cloud's shadow visibly crosses the
      ground and moves with the cloud. **This is the "ground under it" stage.**
- [ ] **C4 — Scattering.** Bounded, density-adaptive **cone light march** with the log
      threshold + branchless box exit; Beer–Powder; dual-lobe HG on the direct light only;
      Wrenninge octaves; σₛ/σₜ split. Plus the **3-tap sky-occlusion ambient**, with the
      sky-view LUT substituted for a constant sky colour. *Gate:* a backlit sunset cloud edge
      glows and a fully shadowed cloud still reads as volume, not a flat grey blob.
      **Likely where "looks awesome" actually lives.**
- [ ] **C4b — Depth compositing.** `t1 = min(t1, scene_depth)` plus the final partial step,
      and view-aligned plane snapping. *Gate:* a cloud deck cutting a mountain ridge shows no
      stair-stepping and no box-shaped moiré under camera motion.
- [ ] **C5 — Ground-bounce aggregate.** Top-slice SH-L1 reduction sampled by the marcher.
      *Gate:* a sunset or lava field visibly tints the cloud undersides.
- [ ] **C6 — `WeatherState`** in `voxel-core` beside `wind`, driving coverage / thickness /
      wind coherently (clear → scattered → overcast → storm). *Gate:* a time-lapse reads as
      weather, not as a slider.
- [ ] **C7 — Audio coupling.** The same weather state drives the atrium wind/rain synth.
      *Gate:* you hear the storm you see.

### Wind is already there

`voxel-core/src/wind.rs:161` says it outright: *"a cloud layer wants the slow weather"*. The
hierarchical model was built for this, and clouds currently keep the steady mean —
deliberate for grass, wrong for a deck. It is also the **same model as the audio field-wind
synth**, which is what makes C7 a wiring stage rather than a rewrite, and why this arc
serves the voxel+audio+VR north star.

## Levers

For `RenderQuality` as a `CloudSettings` block, swept by the bench, measured losers kept
default-off per the variant-hygiene rule:

`cloud_primary_steps`, `cloud_light_steps`, log-Z bias, render scale, denoise mode
(Kawase / bicubic), Kawase passes, **temporal reprojection on/off**, octave count + LOD
falloff, shadow-map resolution, deck count, **light model (cone march / directional
derivatives)**, ambient tap count (0 / 1 / 3), shadow transmittance threshold, plane
snapping on/off, jitter amount.

## Licensing

- **Spiri0/volumetric-clouds is MIT** — portable with a notice in
  `crates/voxel-environment/THIRD_PARTY.md`, same as the Jolifanto sky. It is three.js
  GLSL, so a WGSL port, not a copy.
- **Shadertoy defaults to CC BY-NC-SA 3.0** unless the author overrode it in a header. NC
  and SA are both incompatible with this workspace's MIT footing, and **SA is the dangerous
  one** — porting such code arguably makes the crate share-alike. Check per shader before
  any port. Reading for technique and reimplementing is fine; ideas are not copyrightable.
- Nubis and Wrenninge are published papers — implement freely, cite.

## Decisions

**The look: realistic** (Pascal, 2026-08-05). Physical Earth cloudscapes, not the stylised
flat-bottomed cumulus of the `voxel-sandbox` diorama. So: cauliflower silhouettes, strong
erosion on the high-frequency octaves, Earth-like height-density gradients per cloud type, and
the dual-lobe HG kept physical rather than tuned for graphic contrast. The individual numbers
stay levers, to be judged on screen at C4 rather than guessed now.

**`CAGI_TRANSMISSION`: wanted on** (Pascal, 2026-08-05 — "dimming version for sure"). Pascal
is running the app-run verdict himself. The default in `CagiSettings` stays `false` until that
measurement lands, because the file's own rule is that *a lever's default follows a
measurement* — the decision is which outcome we expect, not a licence to skip measuring.

Testing note for that run: the lever is **no-op on opaque geometry by design** (stone, dirt,
sand and trunk all transmit 0, enforced by
`opaque_materials_transmit_nothing_and_foliage_transmits_something`). The only visible delta
is **foliage** — with it off a leaf canopy absorbs like stone and the ground under a tree goes
black; with it on the canopy passes light while keeping its shadow. Cost is one extra
propagate on solid cells; bench point `gi-transmission` in `BenchSection::Cagi`.

## Open

Nothing blocking. Next action is C1.
