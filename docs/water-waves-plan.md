# Water waves — the surface normal, driven by the existing wind history

**Status:** W1–W6 + W6a, and the E7 look pass (turbidity, caustics, bounce light) — all built and
green, 2026-08-06. Two app-run verdicts open: the wave gate and the E7 gate, both at the bottom.

**One sentence:** the water surface in `voxel-rt` is a perfectly flat axis-aligned voxel face, so
its Fresnel mirror is a *perfect* mirror and there is no sun glitter; this arc gives it a wave
normal derived from the wind history that already drives the cloud deck, and changes nothing else.

## Why this and not the Shadertoy version

The trigger was a Shadertoy voxel-water shader whose optics are strictly behind ours (invented
Fresnel `pow(1-dot, 1.333) * 0.5`, single-channel `exp(-depth)` extinction, hard-coded
`vec3(0.1, 0.7, 1.0)` water colour, no total internal reflection, no underwater case). One thing in
it we genuinely lack: it perturbs the surface normal from a noise field, and we do not perturb ours
at all.

Its method — finite-differencing 3-octave Perlin three times per pixel, time as the third axis — is
not the one to copy. Nine noise lookups per normal, no wind direction, no dispersion, and a
hand-tuned `0.1` strength. We already own a hierarchical wind model
(`voxel_core::wind::WindDriver`) whose frame is *already inside this crate* via
[`SkyWeather::wind()`](../crates/voxel-rt/src/sky_weather.rs) — and `sky_weather.rs` states the rule
this arc has to obey:

> It also owns the wind driver, because the deck's drift and the weather's severity must come from
> ONE wind history. Two drivers would produce a sky whose clouds move at a speed the weather does
> not agree with.

Waves are the third consumer of that history, not a fourth noise field. `WindFrame`'s own doc
already anticipates this split of duties:

> the components are exposed because different consumers want different scales (grass follows
> gusts, a cloud layer wants the slow weather, foam wants the eddies).

## The model: a short-crested sum of gravity waves, analytic normal

Height field, `WAVE_COMPONENTS` (4) directional waves:

```
h(p, t) = Σᵢ Aᵢ · sin(kᵢ · p − ωᵢ t + φᵢ)          p = position.xz in metres
```

The normal is the **analytic** gradient, not a finite difference — one evaluation of the field
instead of three, and exact:

```
∂h/∂x = Σᵢ Aᵢ kᵢ.x cos(…)      ∂h/∂z = Σᵢ Aᵢ kᵢ.z cos(…)
N      = normalize(vec3(−∂h/∂x, 1, −∂h/∂z))
```

### What is derived and what is chosen

**Derived (real physics, and free):** deep-water gravity-wave dispersion, `ω = sqrt(g·k)` with
`k = 2π/λ`. This is the term that stops a wave sum from reading as a scrolling texture: the long
components genuinely outrun the short ones, so the interference pattern never repeats on a beat.
A 6 m wave travels 3.06 m/s, a 0.6 m wave 0.97 m/s.

**Chosen, and documented as chosen — the wavelength band (0.6 m … 6 m, geometric).** The
fully-developed Pierson–Moskowitz peak gives `λ ≈ 0.88·U²` metres — 22 m at 5 m/s wind. That is
correct for open ocean and wrong for every body of water we have. What actually limits a pond is
**fetch**, not wind speed, and we do not model fetch: a pool has metres of it, so its waves are
short whatever the wind does. Rather than pretend a fetch model, the band is a hand-chosen constant
appropriate to the voxel scale, and this paragraph is the honest reason. Same for significant wave
height: PM gives `H_s = 0.0246·U²` = 0.6 m at 5 m/s, which would be surf in a pool. Amplitude is
therefore steepness-capped, not PM-scaled.

**Derived from the wind frame — and this is where the first build went wrong.**

The roughness is set by **Cox & Munk (1954)**: mean-square surface slope
`σ² = 0.003 + 5.12×10⁻³·U`, wind speed `U` in m/s. They obtained it by photographing **sun
glitter** from an aircraft and inverting the width of the glitter pattern, so it is
calibrated against exactly the phenomenon a wave normal exists to produce. At 5 m/s it
gives σ = 0.169, a 9.6° RMS slope.

| wind channel | drives | why that channel |
|---|---|---|
| `speed` (m/s) | the RMS slope, via Cox–Munk | the relation's argument is metres per second |
| `gust` | **redistributes** variance toward the short components | wave response time scales with period: a gust ruffles a surface in seconds, the swell needs minutes |
| `eddy` (−1..1) | phase jitter on the **shortest** component | `wind.rs`: "foam wants the eddies" — this is the chop |
| `wind_direction_degrees` | the mean bearing of `kᵢ` | one bearing for clouds and waves; they cannot disagree |

Per-component steepness follows: for waves with independent phases the slope variance is
`Σ sᵢ²/2`, so a component carrying variance share `wᵢ` has `sᵢ = σ·sqrt(2wᵢ)`. Shares are
equal (Phillips: a `k⁻³` equilibrium spectrum spreads slope variance evenly across
logarithmic bands, and the components are evenly spaced in `log k`), and the gust tilts
them toward the short end **with renormalisation**, so the total stays exactly Cox–Munk.
Adding gust energy instead would double-count, because `speed` already carries the gust.

> **The bug this replaced.** The first build mapped wind to steepness as
> `WAVE_MAX_STEEPNESS/N × activity` — an arbitrary fraction of the breaking limit, driven by
> `activity`, a *normalised 0..1 shape*, when the relation that matters takes *metres per
> second*. Measured, that produced **2.4° RMS at the session's mean wind — four times too
> flat** — and the longest wave stood 29 mm tall against a 125 mm voxel. Pascal's report was
> "I'm missing the shimmering", and porting the maths to the CPU turned that into two
> numbers. After the fix the same wind measures **9.39° RMS against Cox–Munk's 9.39°**, the
> mirror sweep goes ±12° → ±46°, and the longest wave stands 112 mm.

Directions are spread **±35° around the bearing**, not all parallel. A parallel sum is a
corrugated-iron surface; a spread sum is a short-crested sea, and the spread is what
produces glitter that breaks up rather than banding.

**Two caps, both derived, neither setting the look:**

- Per component, the **Stokes breaking limit**: `A·k ≤ 0.35`, just under the 0.443 at which
  a deep-water wave breaks. With the Cox–Munk calibration it would take ~47 m/s to bind.
- On the sum, `WAVE_MAX_TOTAL_STEEPNESS = 0.75`, which comes from the **refraction
  invariant**: refraction bends toward the normal, so the transmitted ray sits within 48.6°
  of `−optics`; for it to stay under the face without a guard, the tilt must satisfy
  `tilt + 48.6° < 90°`. `atan(0.75) = 36.9°` leaves 4.5° of margin. Cox–Munk at the wind
  model's maximum (12 m/s) asks for 0.72 — just inside, so the cap binds only in a gale.

## Cost

4 components × (one `sin`, one `cos`, a dot, two multiply-adds) ≈ 30 ALU per water-surface pixel,
zero memory traffic, no extra rays. E1 measured a marginal full-res secondary ray at
**2.25–3.55 ms**; this is noise next to the mirror ray the same pixel already pays for. It should
not move the bench, and if it does, that is the finding.

## Where the code goes (the existing seam, unchanged)

[`water.wgsl`](../crates/voxel-rt/shaders/water.wgsl) states its own contract: *physics only —
Fresnel, Snell, Beer-Lambert, the medium march — it knows nothing about pixels, sky colour, ambient
occlusion or the light volume*. A wave field is the surface's **shape**, so it belongs there, beside
`refract_at`. How the perturbed normal becomes a pixel stays in `dda.wgsl`.

```
water.wgsl        + WAVE_* consts, wave_height_gradient(), water_surface_normal(hit, point)
dda.wgsl           water_surface_radiance() calls water_surface_normal() instead of hit_normal()
src/water.rs      + the CPU mirror + tests (the file's stated purpose: "re-derived on the CPU so
                    … are checked against hand computations by test rather than by eye")
src/lighting.rs   + water_waves: vec4<f32> at offset 256
src/variants.rs   + LeverId::WaterWaves, subsystem Water
src/sky_weather.rs  unchanged — SkyWeather::wind() already exposes everything needed
```

### The uniform (as shipped)

`water_optics.z`/`.w` were the only free slots and two floats is not enough. What shipped needs
FIVE numbers, not four, so the bearing is passed as an **angle** rather than a vector — which is
what the shader wants anyway, since `component()` sin/cos's `bearing + fan` per component and never
uses a bare bearing vector. That fits exactly:

```wgsl
// water_waves, uniform offset 128 (the struct grew 256 -> 272 bytes)
//   x = mean wind bearing, radians — the SAME angle the cloud deck drifts along
//   y = wind activity 0..1   (WindFrame::activity)
//   z = gust pressure 0..1   (WindFrame::gust)
//   w = eddy -1..1           (WindFrame::eddy)
// plus water_optics.z = the amplitude lever
```

Note this is a correction to the original sketch, which folded `gust` and `eddy` into the amplitude
on the CPU. That would have been wrong: they do not scale amplitude uniformly — `gust` weights
toward the SHORT components and `eddy` moves phase, not amplitude — so neither survives being
pre-mixed into one number.

The five values ride in `WaterParams` rather than as a ninth argument to `lighting_uniform()`
(already at clippy's 7-argument limit), and the wind is attached by `WaterParams::with_wind`, a
builder for the same reason `LightingUniform::with_output_params` is one: of the callers that build
water knobs only the app has a wind history, and omitting it fails safe to flat water.

## The five things that will go wrong if not done deliberately

These are the whole reason this is a spec and not a two-line patch.

1. **Geometric normal for BIAS, perturbed normal for OPTICS.**
   `shadow_ray_origin()` and `water_interior_origin()` must keep `hit_normal(hit)`. Offsetting a ray
   origin along a *perturbed* normal moves it along a direction with no relation to the face it is
   escaping, and it self-intersects. Only `fresnel_schlick`, `reflect` and `refract_at` take the
   wave normal.

2. **The mirror ray must stay above the geometric face.**
   Tilt the normal far enough and `reflect(ray_direction, N)` points *into* the water body — the
   traced mirror then hits liquid at t≈0 and the pixel goes black. Classic sparkle-of-black-dots
   bug. Fix: after perturbation, if `dot(reflected, hit_normal(hit)) <= 0`, either clamp the normal
   toward geometric until it is not, or fall back to `sky_color(reflected)` (the stand-in that
   already exists for the ray-cutoff path). Decide in W2 and write the test.

3. **Top faces only.** Perturb when `hit.axis == 1u && hit.axis_sign < 0.0` (i.e. `hit_normal().y`
   is +1). A water voxel's *side* face — a pool wall seen from outside, a river bank in section —
   is not a heightfield surface and gets the flat normal. A height gradient applied to a vertical
   face is meaningless and reads as a wobbling wall.

4. **Distance LOD, or this looks WORSE than flat water.**
   A 0.6 m wave at 40 m is sub-pixel and turns into pure aliasing sparkle. `material_params.z`
   already carries *metres-per-pixel at one metre* and `pattern.wgsl` already fades octaves by
   footprint (`PATTERN_OCTAVE_LOD_SCALE`, line 276) — mirror that: fade component *i* out as
   `λᵢ` drops below ~2× the pixel footprint, converging to the flat normal at distance. Cheap and
   non-optional.

5. **Zero wind and lever-off must be bit-identical to today.**
   The isolation rule. `water_waves.w == 0.0` (or `WATER_WAVES == 0`) must return exactly
   `hit_normal(hit)` — not "approximately flat". naga folds the const-off path away entirely, and
   the runtime-zero path returns before any `sin`.

## Underwater, and why it is out of scope for now

The exit interface in `water_medium_march` sets `exit_normal` from the DDA face, and perturbing it
would shimmer the rim of Snell's window from below. But `WATER_UNDERWATER_INTERFACE` currently ships
as `WATER_INTERFACE_TRANSPARENT` (Pascal, E6 step 3: *"should be just transparent looking out and in
… only top should have the reflection"*), which has no Fresnel, no bend and therefore no window to
shimmer. The hook is noted here so that whoever flips that lever back knows the wave normal is the
other half of the job. **Not built in W1–W5.**

## Caustics: the follow-on this unlocks, and the reason to do waves first

The Shadertoy paints caustics as `1 − pow(abs(perlin), 0.25)` twice over. That is exactly the
painted knob the two-coefficient medium model exists to remove.

Once the wave field exists, the derived version is nearly free: caustic intensity is the
**divergence of the refracted sun direction** across the surface, which is the *second* derivative
of the same height field — a term `wave_height_gradient()` can return alongside the first with no
new noise and no new lookups. Bright where the surface focuses sunlight, dark where it defocuses,
peaked just under the surface. Their `exp(-abs(depth − 1.0))` depth shape is worth keeping as the
falloff. Tracked as its own arc; do not fold it into W1–W5.

**Built as E7 (2026-08-06)** — and the prediction held: it is the second derivative of the same
field, with no new noise and no new ray. See the E7 section below. The `exp(-|depth − 1|)` shape was
*not* kept, and that turned out to be the point: the Jacobian already peaks where the rays converge,
so a hand-authored depth falloff would have been fighting it.

## Stages

- [x] **W1 — the field, on the CPU first.** `WaveField` / `WaveComponent` in `src/water.rs`.
      14 tests: dispersion against hand computations (a 6 m wave is 3.2052 rad/s and
      3.0607 m/s; long outruns short by exactly √10), the analytic gradient against a
      finite difference of the height field it differentiates, the steepness cap over the
      whole input range including out-of-domain inputs, no net tilt, phase surviving six
      hours of uptime, and the flat case by **exact** equality.
- [x] **W2 — the WGSL mirror + the two safety rules.** `wave_height_gradient` /
      `water_surface_normal` in `water.wgsl`; `WATER_WAVES` as a bool const; the
      two-normal split in `water_surface_radiance`.
      - A test extracts every `WAVE_*` const out of the **built** shader source and
        compares it to its Rust twin — nothing else in the build was keeping the tested
        CPU physics and the rendered pixel in agreement.
      - Trap 1 is asserted on the composed source (bias uses `geometric`, optics uses
        `optics`, and a bias along `optics` is asserted absent).
      - Trap 2 became `water_lift_reflection`, with a CPU mirror swept over
        13 × 41 × 8 grazing-ray/tilt/azimuth combinations.
      - `every_lever_combination_compiles_headless` and `dda_pipeline_compiles_headless`
        compile the pass on a real device with waves both on and off.
- [x] **W3 — wiring.** `water_waves` at uniform offset 128 (fed via
      `WaterParams::with_wind`, a builder for the same reason
      `with_output_params` is one), `SkyWeather::wind_bearing_radians()`, and the app
      passing this frame's wind. The one-wind-history assertion goes the long way — through
      `deck.wind`, the vector the cloud shader advects with, and through
      `component(0).direction`, the one the water shader uses — because two equal angles
      prove nothing if one side then builds its vector with the axes swapped.
- [x] **W4 — LOD.** Per-component fade on cycles-per-pixel off `material_params.z`, and
      `total_steepness_at()` as the roughness bound.
- [x] **W5 — lever + panel.** `WaterWaves` (ShaderConst) and `WaterWaveAmplitude`
      (Runtime) in the registry, each with a verdict and the first with a bench point. The
      settings panel is generated from `levers_of(subsystem)`, so both controls appeared
      without a UI edit.
- [ ] **W5 gate — the app run.** Builds and runs clean on device (no validation error),
      but the visual verdict is not self-serviceable: there is no pixel-readback harness in
      the crate, and glitter is the kind of thing that has to be *looked at* with the sun
      low. Pascal's call.

### Two claims that were wrong, and what replaced them

Both were caught by tests written to check the value rather than the compile, which is the
project rule doing its job.

1. **"The reflection guard only ever fires on wave normals."** It fired on *flat* water
   too, past ~88.9° incidence, where `cos(incidence)` is already below the minimum cosine.
   That would have changed the no-waves image and broken the isolation rule. The guard now
   sits behind `optics == geometric`, justified rather than merely gated: a ray reflected
   off the geometric face always leaves at `|cos(incidence)|` above it, so a flat surface
   cannot reflect into itself and needs no guard at all.
2. **"Per-component fading can only reduce the slope."** False — the gradient is a vector
   sum, so dropping a component that was partly cancelling the others can make `|Σ vᵢ|`
   larger while every `|vᵢ|` shrinks. What is monotone is `Σ |vᵢ|`, which the triangle
   inequality makes a bound on the slope everywhere. That is now `total_steepness_at()`,
   and it is also why the steepness cap survives W4 with no second clamp.

### Seam moves W2 forced, and why each is the right home

- `hit_normal` moved `dda.wgsl` → `world.wgsl`. It is a pure accessor on `Hit`, which
  already lived in `world.wgsl`, and the wave field needs it while being concatenated
  *before* the shading pass. A type and the function that reads it belong together.
- The split-clock oscillator phase moved `graph_prelude.wgsl` → `world.wgsl` as
  `animation_oscillator_phase`, beside the `ANIMATION_EPOCH_SECONDS` it uses. It grew a
  second consumer, and water physics must not import the *material-graph* prelude to ask
  what time it is. The prelude compiles standalone for validation, so it takes a
  shape-only stub — exactly the treatment `world_event_sense` already gets for the same
  reason.

## W6 — splash ripples (built)

W1–W5 are **wind-driven only**: jumping in does nothing, because the field has no interaction term.
Adding one is a small, well-supported stage rather than a new system, because the two things it
needs already exist:

1. **The event field.** `world.wgsl`'s `WorldEvent` is described as *"Something that happened
   somewhere, at a time, with a reach: an entity's presence, an impact, a footstep"* — and it
   already carries `position_meters`, `radius_meters`, an **epoch-split** `started_*` stamp,
   `strength`, and an `open` flag, 16 slots, written once per frame by `src/world_event.rs`. A
   splash is exactly one of these. `AnimationClockSample::elapsed_since(epoch, remainder)` already
   returns "seconds from a stamped instant to now", which is the ripple's age.
2. **The moment.** [`character.rs`](../crates/voxel-rt/src/character.rs) already tracks the
   wade/swim/submerge boundaries and `head_submerged`, so the water-entry instant is known where
   the body is simulated. It needs to *emit* an event, not detect anything new.

The maths is the same field with a radial term added:

```
h_splash = Σ_events A(age) · spread(r) · sin(k·r − ω·age)      r = |p − event.position.xz|
```

**And the dispersion is already right, for free.** Because `ω = sqrt(g·k)` is in the model, a
splash ring disperses the way a real one does — the long wavelengths run ahead of the short ones, so
the ring spreads into a train with the widest waves at the leading edge. That is exactly what a
stone in a pond looks like, and it is the single strongest argument for adding ripples to *this*
field rather than as a separate animated texture. Amplitude decays with age, `spread(r) ≈ 1/sqrt(r)`
is the geometric spreading of a cylindrical wave, and the leading edge is gated to
`r < c_max · age` so the ring cannot appear before it has had time to travel.

Cost: it makes the wave loop's iteration count depend on nearby event count, which the existing
`event_params.z` live-count already bounds. The steepness cap has to absorb the splash term too, or
a jump could fold the surface past breaking.

- [x] **W6** — a `CHANNEL_SPLASH` world-event channel, emitted by the character when a
      speed-bearing dry-to-liquid crossing or wading movement occurs. The shader releases the
      event immediately and uses its start stamp for a four-second radial ring with the example's
      finite-difference animated noise normal.

### W6a — the two sources are summed, not chosen between (2026-08-06)

W6 shipped with `RIPPLE_USE_WIND_FIELD = false`, which made the splash ring an *alternative* to
W1–W5 rather than an addition: `water_surface_normal` returned the splash normal before the wave
field was ever evaluated. The consequences, both observed in the app:

- **Stationary water was exactly flat.** No glitter, no wobbling shoreline — the W1–W5 gate below
  could not be met, because the whole field was dead code.
- **The environment's wind speed moved nothing on the water**, which is the symptom that found it.

Fixed by combining them as **gradients** rather than normals, which is also what lets ONE steepness
cap cover both — this section's own requirement that "the steepness cap has to absorb the splash
term too". `water_clamp_surface_gradient` (and its CPU mirror `clamp_surface_gradient`) applies
`WAVE_MAX_TOTAL_STEEPNESS` to the sum, so a jump into a gale cannot fold the surface past the
refraction invariant: 0.75 of wind plus 0.35 of ring is 1.10 uncapped, 47.7° of tilt, past the 41.4°
the refracted ray's missing guard depends on.

Two consequences worth stating, because they were latent bugs of their own:

- **The `WATER_WAVES` lever was inert.** Its check sat *after* the `RIPPLE_USE_WIND_FIELD` early
  return, so dragging the settings panel's wave toggle changed nothing.
- **`WaveField::rms_slope` diverged from the shader.** The CPU mirror returned 0 below any wind at
  all while `wave_rms_slope` in the WGSL kept Cox & Munk's intercept, so the two disagreed at
  exactly the state a weather preset can ask for. The intercept now applies at every speed on both
  sides, and flatness is the amplitude lever's job alone.

**There is always a small ripple**, and it is the model's own number rather than a floor added on
top — Cox & Munk's 0.003 intercept. Measured on the surface by
`dead_calm_still_carries_a_small_ripple`: 3.135° RMS slope at dead calm, 5.150° at the wind model's
1 m/s floor, 14.000° in a 12 m/s gale.

## Gate

Looking across a body of water with the sun low, there is a **glitter path** — a broad, breaking-up
streak of specular highlights toward the sun that moves with the wind — and the reflected shoreline
wobbles. Distant water is smooth, not sparkly. Turning the lever off returns today's image exactly.

## E7 — the water LOOK pass: turbidity, caustics, bounce light (2026-08-06)

Pascal, looking at a shallow bed in the app: *"this should be probably not more then 3 blocks deep
and should fade deeper you go but we dont have this"*, and *"its like light reflection"*. Three
things were missing, and only one of them was a tuning problem.

### E7a — turbidity: why the bed did not fade

**Measured first.** Beer-Lambert with the material table's coefficients, per block
(1 block = 1 world voxel = 1.0 m):

| depth | ours (R,G,B) | reference `exp(-d)` | blue vs ref |
|---|---|---|---|
| 1 block | 0.638 0.887 **0.942** | 0.368 | 2.6× |
| 3 blocks | 0.259 0.698 **0.835** | 0.050 | **16.8×** |
| 6 blocks | 0.067 0.487 **0.698** | 0.002 | 281× |

At 3 blocks our water still passed 84% of blue, so the bed read as one flat cyan sheet to the
horizon with no fade at all. **And the coefficients were not wrong.** They are *pure water*, and
clear water at 3 m genuinely does not hide a sand bed — that is what a tropical lagoon looks like.
What hides a real lake's bed is **suspended sediment**, a term the model did not have.

Why it is its own grey term rather than a scale on the existing spectral pair: reaching a 3 m blue
horizon by scaling needs **16.6×**, which takes red to 0.001 within *one* block — the bed goes
blue-black instantly instead of fading through its own colour. Turbidity is broadband because the
particles are much larger than the wavelength.

#### The milkiness split, and the white-water build it came from

The first E7a build split turbidity 85% scattering, on the mineral-silt argument: particles much
larger than the wavelength scatter broadband and absorb little. Pascal's report was *"now it looks al
hazy and white :D but its closer"*, and measured, that is exactly right.

The albedo **is** the deep water's colour, and in-scattered radiance is
`albedo × downwelling × (1 − transmittance)`. So a high albedo does not merely tint the depths — it
makes **one block** of water return a large fraction of the sky:

| scattering share | deep-water albedo | in-scatter from ONE block (× sky) |
|---|---|---|
| 0.85 (mineral silt) | 0.539 0.769 0.843 | **0.380 0.452 0.474** |
| 0.15 (shipped) | 0.098 0.164 0.194 | 0.069 0.096 0.109 |
| 0.00 (pure absorption) | 0.003 0.034 0.054 | 0.002 0.020 0.031 |

Nearly *half the sky* coming back off a single block is the white sheet. The physical error was
choosing the wrong suspended agent: a silty river really is milky-bright, but what limits visibility
in most standing water is **dissolved organic matter and phytoplankton, which absorb** — a pond you
cannot see the bottom of is dark, not white.

Since that is a choice of what is suspended rather than a number to derive, the split ships as a
runtime lever (`LeverId::WaterTurbidityScattering`, `water_params.w`, default 0.15) — the dial to
drag against a real pool. 0 goes nearly black and keeps water's own steep blue tint; 1 is full milk.
It replaced `WaterParams::reserved_flow`, which B6 had reserved and never used.

The lever is a **visibility depth in blocks** (`LeverId::WaterVisibilityDepth`, default 3), because
that is the unit the look is specified in. `water::turbidity_per_meter` inverts
`exp(-turbidity · depth) = 0.10` on the CPU, once per frame, into `water_optics.w`. At the shipped
setting the total is (1.218, 0.888, 0.828)/m — a bed keeps 0.30/0.41/0.44 at one block and
0.026/0.070/0.083 at three. 0 restores the pure-water model exactly.

### E7b — caustics: the Jacobian, not two Perlin lobes

Caustics **are** the focusing of the refracted sun by the surface's curvature, so with W1's analytic
field in hand they are a determinant rather than a texture:

```
u(x) = x + d(1 − 1/n)·∇h(x)        the surface-to-bed map
gain = 1 / |det(I + d(1 − 1/n)·H)|  H = the Hessian of the height field
```

`det J < 0` is past the focus, where the map has folded; `|det|` is still the right density there.
Because `Aᵢkᵢ` is the steepness, each Hessian term is `steepness · wavenumber` — one more factor of
`k` than the gradient carries, which is why the SHORT components dominate the filaments. Physically
right: fine ripples make fine filaments.

It rides inside `water_sun_transmission`, which is exactly the correct home on both counts — caustics
are a redistribution of the sun's *own* path through the surface, and that function has already
marched from the bed up to the surface, so the entry point and the depth are in hand and **no second
ray is spent**. Ambient light is not focused by anything and is deliberately not scaled.

Measured one block down: mean gain **1.01** at 1 m/s (range 0.86–1.19) and **1.19** at 12 m/s (range
0.36–4.00). Light *moved*, not manufactured — which is what distinguishes a caustic from a
brightness offset, and why the test asserts both a bright and a dim extreme.

The reference's two Perlin lobes could not respond to wind speed, bearing or wavelength, because
they never look at the surface. This does all three for free.

### E7c — the bounce light, and the units trap in it

The wobbling bright band a pool throws onto the wall beside it. Nothing else in the renderer produces
it: CAGI's volume carries **diffuse** bounce, and a mirror-smooth specular bounce is not diffuse.

The trick that makes it one ray: for a flat plane the reflected sun is a **virtual sun below it**, so
the direction toward that reflection is the sun direction mirrored in Y. Look that way; if water is
there, that water is showing you the sun.

**The trap, caught before it shipped.** The obvious implementation — the reference's — samples the
sky in the reflected direction. Ours would have called `sky_color`, and that is wrong twice over:

- **Units.** `sky_color` carries the sun disc, whose radiance is enormous because the disc subtends
  6.8e-5 sr. A diffuse surface integrates radiance over solid angle, so disc radiance times a
  made-up strength factor is *thousands* of times too bright.
- **Cost.** `sky_color` runs a cloud march. One per shaded surface = a volumetric trace on every
  pixel in the frame.

So the bounce is built from `sun_color_intensity`, the same term the direct sun uses, which makes the
units right by construction. The reflected fraction is Fresnel — physical. The lobe width is
`2 · wave_rms_slope()`, because a normal tilted by `s` deflects a reflection by `2s`: **derived from
the same Cox-Munk roughness the glitter path uses**, so a choppy pool throws a broad soft band
(28.6° at 12 m/s) and a still one a tight bright spot (10.3° at the 1 m/s floor). The single
remaining constant is `WATER_BOUNCE_STRENGTH` (0.35), standing in for how much of the glitter path
the surface sees — the whole of the approximation, in one number.

Guards: skipped for liquids (the reflection path's job) and for submerged points (they already
receive the sun through the surface, with E7b's caustics on it — a bounce there double-counts).
Capped at 16 m. It **is** a ray per shaded surface and it also fires on secondary hits, so the
registry row carries a bench point; if the sweep objects, restricting it to primary hits is the first
thing to try.

- [x] **E7a** turbidity + the visibility-depth lever + the milkiness split lever
- [x] **E7b** analytic caustics on the sun path
- [x] **E7c** the water bounce light on terrain
- [x] **Gate** — app run passed, and it retuned the look. Five runtime numbers were dialled against a
      real pool and made the defaults (Pascal, 2026-08-06: *"this should be default"*):

| lever | was | is | why |
|---|---|---|---|
| absorption scale | 1.0 | **0.0** | water's own absorption off, so the medium is turbidity's — grey, so it darkens without colouring |
| scattering scale | 1.0 | **0.15** | in proportion with the above |
| ray cutoff | 0.04 | **0.0** | always trace; the analytic stand-ins are visible at this scrutiny |
| wave amplitude | 1.0 | **0.21** | Cox & Munk describes OPEN water; a courtyard pool is nearly still |
| visibility depth | 3 blocks | **10 blocks** | once the fade was VISIBLE it wanted to be much further out |

      The 3-block target that started E7a was a guess made while looking at water that had NO fade at
      all. With one, the answer moved: *water you can see into but not through*. Measured at the
      shipped setting — extinction (0.2309, 0.2348, 0.2370)/m, so a bed keeps 0.79 of its light at one
      block, 0.49 at three, 0.10 at ten, and the medium is **near-neutral** (under 3% spread across
      channels, against a factor of 30 for pure water). That last number is
      [the July instinct](#) carried to its conclusion rather than away from it: *"water shouldn't
      have a colour really"*.

- [ ] **Bench sweep** prices E7b/E7c — and now also the ray cutoff at 0, which the E6 sweep measured
      at -7.1% on the steep aerial view and which is now a cost paid deliberately.
