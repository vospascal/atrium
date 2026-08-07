# Cloud system plan

Volumetric clouds for the voxel renderer, built on `voxel-environment`.

Status: **C1–C6, C10 and C11 implemented 2026-08-05, not yet seen running.** The affected workspace
tests and formatting checks are green. C7 (audio coupling) deliberately skipped. The perf gate at
C2 has **not** been taken — nothing here has been measured on a GPU, and that remains the arc's real
risk.

Where the code landed:

| stage | files |
| --- | --- |
| C1 | `voxel-environment/src/clouds.rs`, `src/api.rs` |
| C2 | `shaders/lut/cloud_noise.wgsl`, `shaders/clouds/density.wgsl`, `shaders/environment/clouds.wgsl` |
| C3 | `shaders/lut/cloud_shadow.wgsl`, `voxel-rt/shaders/{dda,cagi}.wgsl` |
| C4 | `shaders/environment/clouds.wgsl` |
| C4b | `shaders/environment/dispatch.wgsl` |
| C5 | `voxel-rt/src/ground_bounce.rs` |
| C6 | `voxel-core/src/weather.rs`, `voxel-rt/src/sky_weather.rs` |
| C10 | `voxel-environment/src/{api,state,clouds}.rs`, `voxel-rt/src/{render,lighting,sky_weather}.rs` |
| C11 | `shaders/clouds/density.wgsl` — Nubis modeling → up-res → density cascade |

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

### Reference URLs

Kept here so they can be re-opened without digging through chat. Fetch notes included, because
several of these defeat automated readers and that wastes time on every revisit.

**Primary — Horizon Zero Dawn / Nubis** (Schneider & Vos, SIGGRAPH 2015). Three copies of the same
talk; the ARTR one is the version read for this doc.
- <https://advances.realtimerendering.com/s2015/The%20Real-time%20Volumetric%20Cloudscapes%20of%20Horizon%20-%20Zero%20Dawn%20-%20ARTR.pdf>
- <https://d3d3g8mu99pzk9.cloudfront.net/AndrewSchneider/The-Real-time-Volumetric-Cloudscapes-of-Horizon-Zero-Dawn.pdf>
- <https://www.guerrilla-games.com/read/the-real-time-volumetric-cloudscapes-of-horizon-zero-dawn>

*Fetch note: WebFetch cannot extract the text (image-based slides), but it saves the PDF to disk
and the `Read` tool reads PDFs directly via its `pages` parameter, 20 pages per call. The modelling
chapter is pages 22–41; that is where the noise layout, height gradients, coverage chain, erosion
and weather map all are.*

**Frostbite — physically based sky, atmosphere and clouds** (Hillaire, 2016). Same author as our
atmosphere provider, so doubly relevant for how cloud lighting should meet the sky LUT.
- <https://media.contentapi.ea.com/content/dam/eacom/frostbite/files/s2016-pbs-frostbite-sky-clouds-new.pdf>

*Fetch note: exceeds WebFetch's 10 MB limit. Download with `curl -o` first, then `Read` it.*
**Not yet read.**

**PBR Book — volume scattering processes.** The reference for σₛ / σₜ / albedo correctness.
- <https://pbr-book.org/4ed/Volume_Scattering/Volume_Scattering_Processes>

*HTML, should fetch normally.* **Not yet read.**

**Patapom — Revision 2013 real-time volumetric rendering course notes.** Detailed treatment of the
scattering integral and energy-conserving multiple scattering.
- <https://patapom.com/topics/Revision2013/Revision%202013%20-%20Real-time%20Volumetric%20Rendering%20Course%20Notes.pdf>

*Fetch note: same as HZD — binary to WebFetch, readable via `Read` with `pages`.* **Not yet read.**

**DiVA thesis on volumetric clouds.**
- <https://www.diva-portal.org/smash/get/diva2:1223894/FULLTEXT01.pdf>

**Not yet read.**

**Secondary, already mined:**
- <https://github.com/Spiri0/volumetric-clouds> — MIT, log-Z sampling, single tileable 128³
- <https://blog.maximeheckel.com/posts/real-time-cloudscapes-with-volumetric-raymarching/> — directional derivatives, blue noise, bicubic
- <https://shaderbits.com/blog/creating-volumetric-ray-marcher> — depth compositing, inner-loop tricks. *Serves only tag metadata to fetchers; ask for the body.*
- Shadertoy: `4tdSWr`, `4ttSWf`, `WslGWl`, `4dsXWn`, `XtBXDw` — **403 to fetchers**, and Shadertoy defaults to CC BY-NC-SA 3.0, so read-for-technique only, never port.

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

## The mist bug (found and fixed 2026-08-05)

First app run: *"the clouds themselves don't look that good, it's always like mist mostly."*
Diagnosed by porting the WGSL density chain to CPU and measuring it
(`scratchpad/noise_check.rs`), not by inspection. Two compounding causes.

**1. The shape field had almost no contrast.** `cloud_perlin_fbm` summed *unsigned* value noise
(each octave mean 0.5), so the sum piled up around its mean — measured **sd 0.105**, p05..p95 of
0.26..0.60. The Perlin–Worley remap then *compressed* it further, to **sd 0.077** over
0.49..0.74. A shape field spanning a quarter of its range is uniformly half-cloudy everywhere:
no cores, no gaps, no silhouette. Heckel's noise returns `* 2.0 - 1.0` — signed — which is
independent confirmation.

Fixed by centring each octave and normalizing by total amplitude, expanding the Worley sum from
its measured 0.31..0.67 band, narrowing the PW remap window from `worley - 1.0` (span up to 2.0)
to `worley * 0.55 - 0.15`, and adding a smoothstep contrast curve. Result: **sd 0.211** over
0.00..0.95.

**2. The height gradient gated the deck instead of shaping it.** It was multiplied into the base
*before* the coverage remap, so it decided whether anything survived:

| gradient | before: nonzero | before: mean density | after (scale 1.8): nonzero | after: mean |
| --- | --- | --- | --- | --- |
| 1.0 | 81% | 0.211 | 16.8% | 0.406 |
| 0.8 | 18.7% | **0.073** | 16.8% | 0.332 |
| 0.5 | **0%** | — | 16.8% | 0.209 |

Density 0.073 against extinction 0.08 is an optical depth of **0.006 per world unit**. That is a
transparent veil — the mist was arithmetic, not aesthetics. Fixed by remapping coverage against
the raw base and multiplying the gradient afterwards, plus a new `density_scale` (default 1.8)
so cores saturate: the noise supplies the *shape*, the scale supplies the *substance*.

Pinned by `coverage_remaps_the_raw_base_before_the_height_gradient`.

**Octave frequencies must stay integers.** Heckel drifts his lacunarity (`factor += 0.21`) to
decorrelate octaves, which is right for a 2D texture lookup that never wraps a lattice. Ours
wraps on the frequency itself, so a fractional ratio would seam across the whole sky. Uses
4/9/19/41 instead — ratios of ~2.1–2.25, decorrelated and each an exact period. Pinned by
`perlin_octave_frequencies_are_integers_so_the_field_tiles`.

**Sunset ambient was sampling the zenith.** `cloud_ambient_light` read
`environment_hillaire_sky(vec3(0,1,0))` — straight up, which stays blue at sunset while the warm
light sits at the horizon. So a deck was filled with cool ambient that cancelled the warm direct
term: grey cloud over orange terrain. Now mixes zenith with a sunward horizon sample, weighted
0.65 toward the horizon.

### Heckel's dreamy look is partly non-physical

Worth recording, because it is a fork we did **not** take. His final composite is:

```glsl
color = color + sunColor * res;   // res is a scalar 0..1
```

The cloud has no colour of its own — it is `vec3(1.0, 0.5, 0.3)` times an energy scalar, **added**
to the sky gradient. Clouds never darken anything. That additive glow is a large part of why it
reads as dreamy, and it is unavailable here: the same deck feeds the shadow map and CAGI, so a
cloud that adds light without blocking it would break the crate's invariant. The legitimate route
to the same feel is warm ambient (fixed above), the forward HG lobe, powder, and the
multiple-scattering octaves. An `additive_glow` blend weight could be offered as an explicit
lever if the physical composite still reads too flat.

## C9 — one definition of "light from above" (2026-08-05)

Pascal: *"the ambient fix — does this also propagate down to the LUT / CAGI?"* It did not, and
tracing it found the same bug in **three** places plus a dead code path of my own.

`cloud_ambient_light` is called from exactly one site, inside `cloud_march_view` — camera-only.
Nothing else read it. What actually crossed into CAGI was only the cloud *shadow*, via
`environment_sun_transmittance_with_clouds`; the deck's *radiance* crossed nowhere.

The three instances of "sample the sky straight up and ignore the deck":

1. `cloud_ambient_light` — fixed earlier.
2. `environment_diffuse_radiance` — the DDA hemisphere ambient. Its `up` is
   `normalize(normal * 0.35 + …)`, which for flat ground is *exactly* the zenith. Its ground
   bounce was a hardcoded `vec3(0.45, 0.36, 0.28)` that could not warm at sunset whatever the
   terrain did.
3. `cagi_sky_radiance` — the light volume's sky injection. Zenith-only **and** entirely
   cloud-unaware, so an overcast sky injected full clear-sky radiance into CAGI.

**And a real error of mine from C1:** `CloudSettings::sun_dimming` and `::ambient_dimming` were
computed, documented and tested but **never applied to anything** — referenced only by their own
tests. The `dispatch.wgsl` comment claiming `ambient_dimming` "arrives pre-multiplied into
`ambient_scale`" was false. The sun *was* dimmed, via the C3 shadow map; the ambient was dimmed by
nothing. Both methods are deleted rather than wired: the shader now derives the same behaviour
from the deck's own transmittance, where it cannot be forgotten or double-counted.

**The fix is one function**, `environment_sky_ambient_at(position, up)`, read by both the surface
path and the volume path so they cannot disagree about the weather:

- zenith blended 0.4 toward the **sunward horizon** — sunset warmth;
- the sky attenuated by the deck's shadow-map transmittance, weighted by coverage
  (`CLOUD_SKY_OCCLUSION = 0.8`, not 1.0: a surface still sees light around the deck's edges);
- plus the deck's **underside radiance**, `sun_illuminance * (1 - transmittance) * coverage *
  CLOUD_UNDERSIDE_SHARE`. Cloud albedo is ~0.999, so a deck is a diffuser rather than an
  absorber — what it removes from the beam is redirected, not lost. Without this term, adding
  cloud attenuation would make an overcast sky *darker* than a clear one, which is not what
  weather does.

The non-monotonic ambient response therefore now **emerges from the physics** instead of being an
authored curve. That is why the CPU curves could be deleted rather than replaced.

### Contract change

`ambient_light` / `environment_diffuse_radiance` now take a **world position** as well as a
normal. Forced, and worth stating plainly: once a deck exists, "how much sky reaches this surface"
is a question about where the surface *is*. A position-free version can only apply cloud cover
globally — the difference between an overcast slider and a shadow crossing a field. Updated at
both DDA sites and both CAGI sites; `cagi_sky_radiance`/`cagi_sky_light` now take a cell.

### Scoped out, deliberately

**Water keeps a per-pixel approximation.** `water_downwelling_radiance` is already documented as
"one evaluation per pixel, not per point along the ray", and threading a real position through it
would mean rethreading `water_in_scattered_radiance` and `water_cheap_surface_radiance` and their
five call sites inside the reflection and refraction paths — measured E6 code. It now takes the
cloud terms at the **camera's column**, which keeps the approximation at the level the rest of that
function is written to. Visible consequence: a distant pool takes the cloud cover above the viewer
rather than above itself. Correct per-point water shadowing is its own stage.

**CAGI still transports cloud radiance only through its sky injection**, which is the right seam —
but the deck's downward radiance is derived analytically in the shader rather than reduced from the
deck itself. A real per-cell deck radiance would want the C5-style aggregate treatment.

### This changes shipped appearance

Unlike everything else in this arc, C9 alters the image **with clouds disabled too**: the sunward
horizon blend and the SH ground bounce replace a zenith sample and a hardcoded constant on every
surface and in every sky-visible CAGI cell. The cloud half is inert when the deck is off, but the
sky half is not. Magnitude was checked rather than assumed — the SH aggregate at noon evaluates to
about `[0.162, 0.145, 0.107]` against the old constant's `[0.167, 0.133, 0.104]`, so exposure is
close to neutral and the difference is chiefly *hue at low sun*.

## C10 — one authoritative environment frame (2026-08-05)

The CPU evaluates `SunSettings::environment_frame()` once per frame. The renderer stores that
value and forwards it as `EnvironmentRequest`; it no longer reconstructs atmosphere inputs from
the display lighting uniform.

```text
SunSettings
    └─ EnvironmentFrame
       ├─ physical sun direction + illuminance ──> atmosphere LUTs
       ├─ active sun-or-moon direction + illuminance ──> cloud direct scattering / world direct light
       ├─ ambient_scale ──> environment diffuse radiance / CAGI sky injection
       └─ zenith, horizon, stars, phase ──> camera-only sky appearance

WeatherFrame ──> CloudRequest ──> visible cloud march + solar cloud shadow + CAGI sky input
```

The atmosphere remains solar even at night, while the active light becomes the moon. The cloud
shadow map follows that active light, so both daylight cloud shadows and moonlit cloud attenuation
respond to the same day cycle. Precipitation now reaches the shared density shader and raises
extinction for rain-bearing cells.

### CAGI and ambient source of truth

Pascal supplied the actual Nubis slides. Read pages 22–41. Three things confirmed, three real
problems found — one of them backwards.

**Confirmed matching:** the 128³ four-channel layout (1 Perlin-Worley + 3 layered Worley, slide 31);
three mathematical height-gradient presets blended by cloud type (slide 34); the weather-map channel
assignment R = coverage, G = precipitation, B = cloud type (slide 40).

**Bug — erosion was inverted at the wrong end.** Slide 37: *"if you invert the Worley noise at the
BASE of the clouds you get some nice whispy shapes"*. The code inverted at the top, giving wispy
tops over billowed bottoms — upside-down clouds. The comment beside it described the correct
behaviour while the code did the opposite, which is why review never caught it. Fixed.

**Bug — coverage was a scalar, not a map.** The single largest look problem, reported as *"just one
big one"*. Slide 40 is explicit that coverage and cloud type are *"a FUNCTION of our weather
system"* — a 2D map over the world. A single number for the whole sky gives no large-scale
organisation, so the 3D noise only modulates an unbroken slab and every direction is equally
cloudy; slide 25 makes the same criticism of naive fBM. `cloud_coverage_at(position)` now reads a
separate 256² NDF covering 16 km × 16 km, with the uniform weather controls layered over that
field. The generated NDF is a procedural fallback; an authored NDF can replace the same texture
without changing the density, lighting, shadow or CAGI seams.

**Nubis Evolved modeling/up-res cascade (C11).** The Evolved presentation confirms that authored
modeling data must be resolved before the procedural noise is up-resolved. The shader now follows
that shape:

```
modeling_profile = base * height_gradient;
modeling_profile = Remap(modeling_profile, 1.0 - coverage, 1.0, 0.0, 1.0);
noise_composite = mix(wispy_noise, billowy_noise, modeling_type);
density = ValueErosion(modeling_profile, noise_composite * detail_strength);
density *= pow(modeling_density_scale, 4);
density = sharpen(density, modeling_density_scale);
density *= distance_remap * close_detail_fade;
```

The NDF channels and the procedural low-frequency field are the fallback for the pack's modeling
channels: dimensional profile, local type, and density scale. The folded high-frequency noise,
powered density response, sharpening, and 50–150 m transition now match the Evolved sampler
contract. Both the view sampler and shadow pass consume the same stage. The actual NVDF VDB/TGA
asset upload seam is intentionally still open; the renderer does not hardcode a path into the
Downloads folder or copy the 180 MB reference pack into the repository.

**Missing feature:** a 2D curl-noise texture (128², 3 channels) distorting the detail noise, which
Nubis uses to *"fake the swirly distortions from atmospheric turbulence"* (slide 33/37).

**Temporal status:** wind advection is implemented and now moves the NDF and up-res field through
one shared offset. Full temporal reprojection is not implemented yet: the current renderer has no
persistent cloud history target or reprojection matrix in the environment contract. The Evolved
sample code's previous-position reconstruction therefore remains a separate, explicit stage rather
than being implied by the spatial dither.

## Aerial perspective on the clouds (2026-08-05)

From the Burning Shores reference Pascal supplied: distant towers are visibly hazed toward the sky
colour while near ones stay crisp, and that depth cue was entirely absent. `sky_color` composited
`backdrop * transmittance + scattering` with the cloud's own radiance never attenuated by range, so
a cloud 30 km out was as saturated as one overhead and a deck reaching the horizon read as a wall.

`CloudMarch` now carries an **opacity-weighted mean distance** — weighted by each sample's actual
contribution, so it reports where the cloud visibly *is* rather than where the ray entered the deck
— and `cloud_aerial_fade` pushes the radiance through the same aerial-perspective LUT and the same
blend curve `sky_color_at_distance` uses, so a cloud and the terrain behind it fade at one rate
rather than two.

## The "no clouds at all" bug — coverage did not mean coverage (2026-08-05)

After the weather map landed, the app showed **no clouds forming**. Pascal asked whether the scale
was wrong. It was a scale problem, but of the noise *distribution*, not the altitudes.

The density chain does `remap(base, 1.0 - coverage, 1.0, 0.0, 1.0)`. That only yields "a `coverage`
fraction of the sky has cloud" **if the base field is uniformly distributed on 0..1**. It is not:
measured, the shaped Perlin–Worley came out **p05 0.001 / p50 0.330 / p95 0.688** — concentrated low.

Consequences, both arithmetic rather than aesthetic:

- Requested coverage 0.45 set a floor of 0.55 that most of the field could never clear.
- The weather map then centred its variation on **0.5** while the signal's median is **0.330**, so the
  variation term was *systematically negative* — dropping local coverage to ~0.31, hence a floor of
  **0.693 against a p95 of 0.688**. Under 5% of the field survived in the *median* case, and none at
  p05. Adding the weather map is what turned a bad calibration into an empty sky.

**Fix: flatten the field at generation time.** A linear stretch of the measured p05..p95 band onto
0.05..0.95 fits the three percentile anchors to within 0.02 and costs one `remap`. Measured after:
**p05 0.050 / p50 0.481 / p95 0.950, sd 0.271** — essentially uniform. End-to-end with the weather
map active, requested coverage now produces the sky coverage it names:

| requested | sky covered |
| --- | --- |
| 0.08 | 6.5% |
| 0.45 | 40.6% |
| 0.92 | 91.9% |

The two constants are stated as numbers rather than buried in a gamma, because they are calibrated
against this exact chain: **re-measure and re-fit them if the octave weights or the contrast curve
change.** Pinned by `the_base_field_is_flattened_so_coverage_means_coverage`.

## Scale audit (2026-08-05)

Prompted by Pascal asking whether the scaling was right. Units were correct; one constant was not.

**Units check out.** `camera_position` is world **metres** (`camera.rs`: "Eye position, world
meters"), matching `FROM_KILOMETERS_SCALE = 1000`. So the Stormscapes-derived altitudes are genuine
metres. The world itself is **125 × 32 × 125 m** (`WORLD_VOXELS 125×32×125` at 1 m per world voxel).

**`CLOUD_SHADOW_EXTENT_WORLD` was 8x too coarse.** 4096 m across 512 texels is 8 m per texel, so the
entire 125 m world spanned about **fifteen texels** — the ground received a smooth wash rather than
cloud shadows. C3 was structurally correct and numerically useless. Now **512 m = exactly 1 m per
texel**, one texel per world voxel, with 4x the world's width of margin.

Nothing caught it because `voxel-environment` deliberately does not depend on the world, so no test
could see both numbers. `cloud_shadow_extent_resolves_a_world_voxel_per_texel` lives in `voxel-rt` —
the only crate that sees both — and asserts metres-per-texel ≤ one world voxel *and* that the extent
still covers the world.

## The blocker: the sky ray never reached the deck (2026-08-05)

Pascal, after three density fixes: *"is the skybox interfering cause i still see no clouds"*. The
density field was fine by then. The ray was the problem, and this one outranks every look bug above —
it removed the deck entirely, so none of those fixes could have shown.

`dda.wgsl`'s miss path called `sky_color_at_distance(direction, MAX_TRACE_DISTANCE * voxel_size)`.
`MAX_TRACE_DISTANCE` is **2048** voxels = 2048 m, and `cloud_march_view` clamps `exit = min(span.y,
max_distance)`. Cloud base is 900–2400 m depending on preset, and a slab's entry distance is
`(base - eye) / direction.y`, which **diverges toward the horizon**. At the shipped Scattered base of
1500 m the bound was already exceeded past ~44° off vertical; at a thoroughly ordinary 20° elevation
even the lowest 900 m deck enters at 2573 m. So the deck was clipped everywhere except a narrow cone
straight up, and truncated even there.

The category error: **`MAX_TRACE_DISTANCE` on a miss is a give-up sentinel, not a depth.** It was
passed as though it were a measured distance.

Worse, the comment on that call claimed the bound was the *feature* — "cloud in front of a mountain
occludes it and cloud behind it does not… passing the true distance is the whole fix" (C4b). That case
cannot occur: `sky_color_at_distance` has exactly one caller, the miss, and when there *is* a mountain
`shade_hit` runs and never calls it. The parameter is only ever the sentinel. A comment asserting a
capability the code could not have is how this survived several readings.

Fixed by marching the deck unbounded in the miss path. `distance_world` still bounds the atmosphere
blend, which is a genuine use of it.

Pinned by `the_deck_is_out_of_reach_of_the_trace_radius_so_the_sky_march_must_be_unbounded` in
`voxel-rt` — again the only crate that sees both numbers, the trace radius living in its WGSL and the
altitudes in `voxel-core`. It asserts the geometric argument (lowest deck at 20° elevation is out of
trace reach) *and* that the march still passes no bound. The second half is asserted **positively**,
on the presence of the unbounded literal, because a negative match on `distance_world` would pass
after a mere reformat — a guard that silently stops guarding.

**Resolved:** the default sky is now Hillaire radiance rather than the authored two-colour backdrop.
The resolved sun and moon discs remain a presentation layer, but use the physical celestial
directions and illuminances and are attenuated by the atmosphere transmittance LUT. Stars use the
same view transmittance and are then attenuated by the cloud march, so the sky, stars, moon, clouds,
and aerial perspective share one cascade. The old zenith/horizon colours remain compatibility
metadata only; they no longer replace the physical sky.

## Deviations from the plan as written

Flagged rather than folded in silently, per the project's rule.

**Coverage does not modulate the sky-view LUT.** The plan had it invalidating view-independent
state. Implementation found a strictly better shape: the deck attenuates the sky *when it is
sampled*, so a coverage change touches no atmosphere table at all and is free. `weather` still
exists as an invalidation bit, but it reaches only the cloud shadow map.

**No new dispatch entry point for C3.** `environment_sun_transmittance_at(position, direction)`
already existed and was already called by CAGI, so cloud shadows fold into
`environment_sun_transmittance_with_clouds` — DDA and CAGI multiply a function they were
already using. One fewer contract than planned.

**A fifth invalidation bit, `viewer_moved`.** The shadow map is world-anchored but
camera-*centred*, so walking invalidates it and turning the head must not. `camera` conflates
both, and without the split the choice was a per-mouse-movement 512² pass or a stale map
centred where the viewer used to stand. Matters most in VR.

**C5 is analytic, not a CAGI reduction.** The coefficients come from the sun and sky state times
a representative ground albedo, so sunset and daylight response work. Local emissive sources —
lava, a lit window — do *not* tint the deck above them. The shader reads only the four
coefficients, so a real GPU reduction over CAGI's top layer is a drop-in upgrade with no shader
change.

**The environment multiplier has one source.** `EnvironmentFrame::ambient_scale` is carried by
`EnvironmentRequest` into the environment uniform. Both the DDA surface path and
`cagi_sky_radiance` multiply the shared `environment_sky_ambient_at` result by that value; CAGI no
longer reaches into the renderer's `lighting.sky_ambient` uniform.

## Checked against the primary source (2026-08-05)

## Open

**Not yet done, and known:**

- **The C2 perf gate.** Nothing has been measured. Defaults are 35 primary × 7 light taps at
  full resolution with no half-res target and no temporal reprojection, which is the
  *expensive* end of every estimate in this doc. If it is too slow, the levers to reach for
  first are `primary_steps`, then a half-res cloud target.
- **No `RenderQuality` levers yet.** Cloud knobs live on `CloudSettings` and are reachable in
  code but are not in the lever registry, so the bench does not sweep them and the overlay
  cannot edit them. That is the next piece of work regardless of the perf verdict.
- **No imported Nubis modeling volume yet.** The C11 cascade currently uses the procedural
  Perlin–Worley field and generated NDF as its modeling-field fallback. Loading the pack's NVDF
  field/modeling volume pair needs an explicit project asset contract and GPU upload path; that is
  separate from the lighting cascade and should not be hidden behind a machine-specific Downloads
  path.
- **No temporal cloud history yet.** Wind advection is coherent and deterministic, but the Evolved
  previous-position reprojection path still needs a persistent cloud radiance target, previous
  camera state, rejection tests and a neighborhood clamp. Spatial dither remains the safe default
  until that seam exists.
- ~~No overlay panel.~~ **Done** — a **Weather** section sits next to Sun in the settings
  window (**O**): four condition buttons, transition speed, and the deck's dials split into
  weather-driven / look / cost groups. Needed a `SkyWeather::manual` flag, because the weather
  frame rewrites the shape fields *every* frame (the wind's slow channel breathes coverage
  continuously), so without it the shape sliders were dead controls.
- **`voxel-rt` grass does not read this wind.** `SkyWeather` owns the only `WindDriver` in the
  renderer; the grass animation still runs off the animation-params path. The "wind you see and
  wind you hear are one phenomenon" claim is true of the *model*, not yet of the grass.
