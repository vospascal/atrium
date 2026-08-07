// water.wgsl — E6 water OPTICS: the levers, the three physical laws, and the
// traversal of the water body itself. Concatenated after `world.wgsl` +
// `cagi_volume.wgsl` and before `dda.wgsl`:
//
//   dda source  = world.wgsl + cagi_volume.wgsl + water.wgsl + dda.wgsl
//   cagi source = world.wgsl + cagi_volume.wgsl + cagi.wgsl
//
// so the CA pass does not compile any of this (it has no camera and no water to
// look through), while the shading pass gets it in front of the composition that
// uses it.
//
// **The split is deliberate and it is the seam E7 and B6 build on:** this file
// holds only physics — Fresnel, Snell, Beer-Lambert, and the march that measures
// how far a ray travelled inside a liquid. It knows nothing about pixels, sky
// colour, ambient occlusion or the light volume. HOW those terms are composed
// into a pixel lives in `dda.wgsl` (`water_surface_radiance`,
// `water_medium_radiance`), because that is where the shading path is. A look
// pass (E7) that wants a different surface treatment, or a fluid CA (B6) that
// wants the same optics over cells with a mass instead of a bit, replaces the
// composition and keeps this file.
//
// Rust mirror: `src/water.rs` — the same constants and the same three functions,
// re-derived on the CPU so `fresnel_schlick`, `refract_direction`,
// `critical_angle_degrees` and `transmittance` are checked against hand
// computations by test rather than by eye.

// ---- E6: water levers ---------------------------------------------------------
// WATER_MODE picks the optics, ordered by cost. Registry rows with the measured
// verdicts: `src/variants.rs::REGISTRY`, subsystem `Water`.
//
//   0  opaque       — water is an ordinary diffuse surface. Folds the WHOLE
//                     experiment away: with this the shading pass is the E4
//                     renderer, which is the isolation rule's requirement and
//                     the bench's no-regression anchor.
//   1  fresnel tint — ZERO secondary rays: the mirror term is the analytic sky
//                     function evaluated in the reflected direction (which
//                     already carries the sun glint), the transmitted term is
//                     the surface's own diffuse shading, mixed by Fresnel. The
//                     Potato/Quest tier.
//   2  reflection   — the mirror ray is traced; transmission stays diffuse.
//                     Prices the reflection ray alone.
//   3  refraction   — the refracted ray marches the medium with extinction;
//                     the mirror term stays analytic. Prices refraction alone.
//   4  full         — both. The shipped model.
const WATER_MODE_OPAQUE: u32 = 0u;
const WATER_MODE_FRESNEL_TINT: u32 = 1u;
const WATER_MODE_REFLECTION: u32 = 2u;
const WATER_MODE_REFRACTION: u32 = 3u;
const WATER_MODE_FULL: u32 = 4u;
const WATER_MODE: u32 = 4u;

// How many water INTERFACES one camera ray may cross. E1 measured a marginal
// full-res secondary ray at 2.25-3.55 ms, so this is a budget, not a physical
// constant. 1 = the surface split plus one march through the body (a
// total-internal-reflection mirror then reads as the flat body colour); 2 lets
// the march bounce once more, which is what draws the bed mirrored outside
// Snell's window and what a ray leaving through the far wall of a pool needs.
const WATER_BOUNCES: u32 = 1u;

// What the region OUTSIDE Snell's window gets once the full-shading bounce budget
// is spent — the E6 gate failure and its fix.
//
//   0  flat        — the in-scatter constant. DOCUMENTED NEGATIVE: past the
//                    critical angle `refract_at` reports total internal reflection,
//                    Fresnel is 1, and with WATER_BOUNCES = 1 the loop had nothing
//                    left to add, so the whole mirrored region was ONE FLAT COLOUR.
//                    Since the window is only a ~97-degree cone, tilting the head
//                    underwater fills most of the screen with it — Pascal's
//                    "completely broken", and the reason the view read as "all
//                    teal" and the cone's rim as harsh.
//   1  standin     — one more medium march, shaded CHEAPLY (albedo x downwelling,
//                    no shadow ray, no AO, no CAGI). Real geometry — the bed and
//                    the pool walls, mirrored — for a fraction of a full bounce.
//                    Same principle as the above-water half-modes: substitute a
//                    cheap stand-in for the term you cannot afford to trace
//                    properly, never a constant.
const WATER_TIR_FLAT: u32 = 0u;
const WATER_TIR_STANDIN: u32 = 1u;
const WATER_TIR_FALLBACK: u32 = 1u;

// What the surface interface does when a ray reaches it from BELOW (E6 step 3,
// Pascal: *"lets disable the fresnel like camera looking up out of water for now
// should be just transparent looking out and in .. only top should have the
// reflection"*).
//
//   0  fresnel     — the physical interface: Snell's bend, a Fresnel-weighted split,
//                    and total internal reflection past the critical angle, whose
//                    mirrored region `WATER_TIR_FALLBACK` then fills. This is what
//                    E6 shipped through step 1.
//   1  transparent — the interface is FULLY TRANSMISSIVE and the ray continues
//                    STRAIGHT through it: no Fresnel weighting, no mirror, no total
//                    internal reflection, and no bend. Only the absorption and
//                    scattering along the path still apply.
//
// **Why "just transparent" has to mean UNBENT, and is therefore the only coherent
// version of this request:** total internal reflection is not a separable effect
// that can be switched off on its own — it *is* what Snell's law yields when
// `sin(theta_transmitted) > 1`. Past the 48.607-degree critical angle there is no
// transmitted direction to bend toward, so a build that kept the bend and dropped
// the mirror would have nothing to draw beyond the window. Dropping the bend
// removes the critical angle along with it, and the interface becomes a plain
// window. The instinct picked the consistent option.
//
// Consequences, accepted deliberately and not defects: **Snell's window disappears
// from below**, and with it every cue that the surface is there at all — looking up
// simply shows the world above, dimmed and tinted by the water it travelled
// through. The ABOVE-water side is untouched: it keeps its Fresnel-weighted mirror
// ("only top should have the reflection") and its refracted march inward.
const WATER_INTERFACE_FRESNEL: u32 = 0u;
const WATER_INTERFACE_TRANSPARENT: u32 = 1u;
const WATER_UNDERWATER_INTERFACE: u32 = 1u;

// Derived, not levers (the same pattern as USE_COLUMN_HEIGHTS): which secondary
// rays this mode is allowed to trace. naga folds both branches away.
const WATER_TRACES_REFLECTION: bool =
    WATER_MODE == WATER_MODE_REFLECTION || WATER_MODE == WATER_MODE_FULL;
const WATER_TRACES_REFRACTION: bool =
    WATER_MODE == WATER_MODE_REFRACTION || WATER_MODE == WATER_MODE_FULL;

// ---- Physical constants -------------------------------------------------------

const WATER_AIR_INDEX: f32 = 1.0;

// "No medium in front of the sun": the `sun_transmission` every ray travelling
// through AIR passes to the shading path. Exactly 1.0 per channel, so the multiply
// is the float identity and the non-water path stays bit-identical.
const WATER_NO_MEDIUM: vec3<f32> = vec3<f32>(1.0, 1.0, 1.0);

// The medium's coefficients now live in the MATERIAL TABLE (src/material.rs), per
// channel and per metre, as an absorption/scattering PAIR — see
// `water_extinction_per_meter` and `water_single_scattering_albedo` below. There is
// no extinction constant here any more, and no volume colour anywhere: water's
// blue-green is derived from red being absorbed ~30x faster than blue while blue
// scatters ~11x more than red.

// The murk horizon: how far a ray may march INSIDE a liquid, in voxel units.
// 256 voxels = 32 m, where transmittance is (2e-5, 0.056, 0.24) — so the light
// dropped at the cap is at most a quarter of the blue channel, and it reads as
// distance going murky rather than as a wall. It is also the cost bound: this
// march is per-voxel (a random-access material read per step), unlike `trace`.
const WATER_MEDIUM_MAX_DISTANCE: f32 = 256.0;
const WATER_MEDIUM_MAX_STEPS: u32 = 256u;

// How a march through a liquid ended.
const WATER_MEDIUM_SOLID: u32 = 0u; // hit terrain: the bed, a rock, a bank
const WATER_MEDIUM_AIR: u32 = 1u;   // reached the surface from below
const WATER_MEDIUM_LIMIT: u32 = 2u; // murk horizon or the edge of the world

// ---- Fresnel, Snell, Beer-Lambert --------------------------------------------

// Normal-incidence reflectance of an air/medium boundary, DERIVED from the
// medium's own index rather than tuned: ((n1 - n2) / (n1 + n2))^2. Water's 1.333
// gives 0.0204 — straight down it reflects 2% and transmits 98%.
fn fresnel_f0_of(index_of_refraction: f32) -> f32 {
    let ratio = (index_of_refraction - WATER_AIR_INDEX)
        / (index_of_refraction + WATER_AIR_INDEX);
    return ratio * ratio;
}

// Schlick's approximation of the Fresnel reflectance at an air/medium boundary.
// cos_incidence = 1 (straight on) gives the medium's F0; 0 (grazing) gives 1 —
// the mirror-at-grazing / see-through-when-steep behaviour the E6 gate asks for.
fn fresnel_schlick(cos_incidence: f32, index_of_refraction: f32) -> f32 {
    let cosine = clamp(cos_incidence, 0.0, 1.0);
    let one_minus = 1.0 - cosine;
    let fifth = one_minus * one_minus * one_minus * one_minus * one_minus;
    let f0 = fresnel_f0_of(index_of_refraction);
    return f0 + (1.0 - f0) * fifth;
}

// ---- The look dial (runtime, `water_optics`) ----------------------------------
// ONE dial, and it is a physical parameter: the index of refraction, which sets
// how wide Snell's window is. Its default is the physical 1.333, so no taste is
// baked into the shipped look.
//
// There is deliberately NO tint or volume-colour dial. Pascal's "the water should
// be less teal" is answered by the absorption and scattering SCALES
// (`water_params.x` / `.y`), which are the physical quantities the colour is
// derived from — a tint multiplier would be exactly the painted knob the
// two-coefficient model exists to remove.

// The index of refraction to BEND by: the material's authored index pulled toward
// 1.0 (no refraction) by `refraction_strength`. This is the window-width dial,
// because the half-angle is asin(1 / n): 1.333 -> 48.6 deg (physical, strength 1),
// 1.25 -> 53.1, 1.167 -> 59.0, 1.083 -> 67.4, 1.0 -> the whole hemisphere.
//
// Deliberately NOT used for Fresnel — `fresnel_f0_of` keeps the AUTHORED index —
// because widening the window and softening the surface's mirror are different
// visual properties, and ganging them would have this dial quietly destroy the
// grazing reflection above water (F0 at n = 1.1 is 0.002, i.e. no mirror at all).
fn water_bending_index(material: u32) -> f32 {
    let authored = material_index_of_refraction(material);
    return mix(WATER_AIR_INDEX, authored, clamp(lighting.water_optics.x, 0.0, 1.0));
}

// Whether a term weighted `weight` is worth a SECONDARY RAY, or whether the cheap
// analytic stand-in will do (E6's ray cutoff, `water_params.z`).
//
// The observation is that Fresnel already tells us how much each half of a water
// pixel is worth: head-on, the mirror term carries 2% of the pixel and we were
// paying a full traced reflection plus a full shading for it; at grazing angles it
// is the transmitted term that carries almost nothing. Below the threshold the
// stand-in (the analytic sky for the mirror, the diffuse surface for the
// transmission) is substituted, so the ray budget follows the physics instead of
// the geometry. A threshold of 0 restores "always trace".
fn water_ray_is_worth_tracing(weight: f32) -> bool {
    return weight > lighting.water_params.z;
}

// The index of refraction a material bends rays by (material.rs's authored
// column). Air and every opaque row read 1.0, i.e. "does not refract".
fn material_index_of_refraction(material: u32) -> f32 {
    return max(materials[material].index_of_refraction, WATER_AIR_INDEX);
}

struct Refraction {
    direction: vec3<f32>,
    // Set when sin(theta_transmitted) > 1, which can only happen leaving the
    // denser medium: the edge of Snell's window. `direction` then holds the
    // mirrored direction instead.
    total_internal_reflection: bool,
    cos_incidence: f32,
}

// Snell's law in vector form. `normal` must point toward the side the incident
// ray comes FROM — the same convention `hit_normal` produces (it opposes the
// ray). `eta` is index_from / index_to: `WATER_AIR_INDEX / n` entering the
// medium, `n / WATER_AIR_INDEX` leaving it.
fn refract_at(incident: vec3<f32>, normal: vec3<f32>, eta: f32) -> Refraction {
    var result: Refraction;
    result.cos_incidence = clamp(-dot(incident, normal), 0.0, 1.0);
    let sin_squared_transmitted =
        eta * eta * (1.0 - result.cos_incidence * result.cos_incidence);
    if (sin_squared_transmitted > 1.0) {
        result.total_internal_reflection = true;
        result.direction = reflect(incident, normal);
        return result;
    }
    result.total_internal_reflection = false;
    let cos_transmitted = sqrt(max(1.0 - sin_squared_transmitted, 0.0));
    result.direction =
        incident * eta + normal * (eta * result.cos_incidence - cos_transmitted);
    return result;
}

// Beer-Lambert transmittance of a path of `distance_voxels` inside the liquid,
// per channel. The absorption coefficients are per METER, so the conversion goes
// through the brickmap's own voxel size — the physics cannot drift if the world
// resolution ever changes. `water_params.x` is the runtime clarity knob.
// This medium's absorption and scattering coefficients per metre, each scaled by
// its own runtime dial. Both dials are PHYSICAL: absorption is how much light the
// water destroys (the clarity/darkening axis) and scattering is how much it
// redirects (the brightness/colour axis). 1.0 = the authored coefficients.
// E7 — TURBIDITY: per-metre extinction from suspended matter, `water_optics.w`.
//
// The term the pure-water model was missing, and the reason the bed did not fade. It is
// GREY (equal in all three channels) and SCATTERING-DOMINANT, because suspended mineral
// sediment is both: particles much larger than the wavelength scatter broadband and absorb
// little, so their single-scattering albedo is high. Water's own coefficients are the
// opposite — steeply blue and absorption-dominant — which is exactly why scaling THEM
// could not produce this look: reaching a 3 m horizon in blue that way needs 16.6x, and
// that kills red inside one block.
//
// The SPLIT between the two is `water_params.w`, a runtime dial rather than a constant,
// because it is a choice of what is suspended rather than a number to be derived. Mineral
// silt is much larger than the wavelength, so it scatters broadband and absorbs little — a
// silty river genuinely is milky-bright, and a fraction near 0.85 renders exactly that. What
// limits visibility in most standing water is instead dissolved organic matter and
// phytoplankton, which ABSORB: a pond you cannot see the bottom of is dark, not white.
//
// Measured, and the reason the first build looked like milk: at 0.85 ONE block of water
// in-scatters 0.38-0.47 of the sky's radiance, so even shallow water reads as a white sheet.
// At the shipped 0.15 that is 0.07-0.11 and it reads as water again.
fn water_turbidity_per_meter() -> f32 {
    return max(lighting.water_optics.w, 0.0);
}

fn water_turbidity_scattering_fraction() -> f32 {
    return clamp(lighting.water_params.w, 0.0, 1.0);
}

// Both dials scale the MATERIAL's coefficients only. Turbidity is added after, and
// deliberately outside them: it is a property of the body of water, not of the substance,
// so scaling "how clear this water is" must not also scale how much silt is in it.
fn water_absorption_per_meter(material: u32) -> vec3<f32> {
    let turbidity = water_turbidity_per_meter()
        * (1.0 - water_turbidity_scattering_fraction());
    return materials[material].absorption_per_meter * max(lighting.water_params.x, 0.0)
        + vec3<f32>(turbidity, turbidity, turbidity);
}

fn water_scattering_per_meter(material: u32) -> vec3<f32> {
    let turbidity = water_turbidity_per_meter() * water_turbidity_scattering_fraction();
    return materials[material].scattering_per_meter * max(lighting.water_params.y, 0.0)
        + vec3<f32>(turbidity, turbidity, turbidity);
}

// Extinction = absorption + scattering: the total rate light leaves a ray, and the
// exponent of the Beer-Lambert term.
fn water_extinction_per_meter(material: u32) -> vec3<f32> {
    return water_absorption_per_meter(material) + water_scattering_per_meter(material);
}

// The medium's apparent COLOUR, derived rather than authored: the share of what
// leaves the ray that is redirected rather than destroyed. For water this comes out
// ~(0.009, 0.25, 0.75) — deeply blue, almost no red — purely from the coefficients.
// Channels with no extinction read 0 instead of dividing by zero.
fn water_single_scattering_albedo(material: u32) -> vec3<f32> {
    let extinction = water_extinction_per_meter(material);
    let scattering = water_scattering_per_meter(material);
    return select(vec3<f32>(0.0, 0.0, 0.0), scattering / max(extinction, vec3<f32>(1e-6)),
                  extinction > vec3<f32>(0.0, 0.0, 0.0));
}

// Beer-Lambert transmittance of a path of `distance_voxels` inside the medium, per
// channel. The coefficients are per METRE, so the conversion goes through the
// brickmap's own voxel size — the physics cannot drift if the world resolution
// changes.
fn water_transmittance(material: u32, distance_voxels: f32) -> vec3<f32> {
    let meters = max(distance_voxels, 0.0) * brickmap.voxel_size_meters;
    return exp(-water_extinction_per_meter(material) * meters);
}

// ---- Surface motion -----------------------------------------------------------
//
// TWO sources, and they are SUMMED rather than chosen between:
//
//   * the wind field (W1-W5) is the ALWAYS-ON base. Still water is never glass, and that
//     is not a stylistic claim: Cox & Munk's slope relation has a 0.003 INTERCEPT, so
//     even at the wind model's floor of 1 m/s the surface carries sigma = 0.090, a
//     5.2 degree RMS slope. There is always a small ripple, and the weather's wind speed
//     moves it.
//   * splash rings (W6) are a short-lived ADDITION on top, from CHANNEL_SPLASH world
//     events raised when the character falls in or wades.
//
// They combine as GRADIENTS, not as normals, and that is what makes one steepness cap
// cover both — the plan's requirement that "the steepness cap has to absorb the splash
// term too, or a jump could fold the surface past breaking". Blending normals would need
// a second, differently-shaped bound and could not be checked against the refraction
// invariant `WAVE_MAX_TOTAL_STEEPNESS` encodes.
//
// The previous build made these two EXCLUSIVE (`RIPPLE_USE_WIND_FIELD = false` returned
// the splash normal before the wave field was ever evaluated), which left W1-W5 as dead
// code: stationary water was exactly flat and the weather's wind speed had no effect on
// it at all.
//
// The surface of a water voxel used to be a perfectly flat axis-aligned face, which
// made its Fresnel mirror a PERFECT mirror: no sun glitter, and a reflected shoreline
// that does not move. This block is the height field whose gradient replaces that flat
// normal. `src/water.rs` is the CPU mirror — the SAME constants and the same
// expressions, tested there against hand computations (dispersion), against a finite
// difference (the analytic gradient), and against exact equality (the flat case).
// `docs/water-waves-plan.md` carries the argument.
//
// Wind comes from the ONE wind history that already drives the cloud deck and the
// weather, via `lighting.water_waves`. Waves are its third consumer, not a fourth
// noise field.
const WATER_WAVES: bool = true;
const RIPPLE_ENABLED: bool = true;
const RIPPLE_CHANNEL: u32 = 1u;
const RIPPLE_MAX_AGE: f32 = 4.0;
const RIPPLE_WAVE_SPEED: f32 = 3.2;
const RIPPLE_NOISE_SCALE: f32 = 0.8;
const RIPPLE_TIME_SCALE: f32 = 0.35;

// Peak steepness (dh/dx) one splash ring contributes at the crest of its envelope.
//
// Equal to WAVE_MAX_STEEPNESS on purpose: a splash is allowed exactly ONE breaking-limit
// wave component's worth of slope, which is the same bound the wind field puts on any
// single component of its own sum. It is a slope, so it is directly comparable to the
// wind field's numbers — the old normal-space blend used the same 0.35 as an opaque mix
// weight, whose slope depended on the noise field's arbitrary magnitude.
const RIPPLE_STEEPNESS: f32 = 0.35;

const WAVE_TAU: f32 = 6.28318530718;

// Standard gravity, for the deep-water dispersion relation. Deliberately NOT the
// character controller's 22.0 — that one is tuned game feel, and sharing it because
// both are spelled "gravity" would make every wave travel 1.5x too fast.
const WAVE_GRAVITY: f32 = 9.80665;

const WAVE_COMPONENTS: u32 = 4u;

// The wavelength band, geometrically spaced. CHOSEN, not derived: the fully-developed
// Pierson-Moskowitz peak is lambda ~ 0.88 U^2 metres (22 m at 5 m/s of wind), which is
// right for open ocean and absurd for a pool. What limits a pond is FETCH, not wind
// speed, and we do not model fetch — see the plan doc.
const WAVE_LONGEST_METERS: f32 = 6.0;
const WAVE_SHORTEST_METERS: f32 = 0.6;

// Cox & Munk's mean-square surface slope: sigma^2 = 0.003 + 5.12e-3 * U, wind speed U in
// m/s. THIS is what sets how rough the water is, and it is a MEASUREMENT rather than a
// choice: Cox & Munk (1954) obtained it by photographing SUN GLITTER from an aircraft and
// inverting the width of the glitter pattern, so it is calibrated against exactly the
// phenomenon a wave normal exists to produce. At 5 m/s it gives sigma = 0.169, a 9.6 deg
// RMS slope.
//
// It replaced an arbitrary mapping (a fixed fraction of the breaking limit times the wind's
// `activity`) that measured out at 2.4 deg RMS — FOUR TIMES TOO FLAT — which is why the
// first build had no visible shimmer.
const WAVE_SLOPE_VARIANCE_INTERCEPT: f32 = 0.003;
const WAVE_SLOPE_VARIANCE_PER_METER_PER_SECOND: f32 = 0.00512;

// Per-component ceiling on steepness A*k — the STOKES BREAKING LIMIT with margin (a
// deep-water wave breaks at A*k ~ 0.443). Bounds ONE component, which is what the physical
// limit is about. With the Cox-Munk calibration it never binds at any weather the wind
// model can produce; it would take about 47 m/s.
const WAVE_MAX_STEEPNESS: f32 = 0.35;

// Ceiling on the SUM, and therefore on the normal's tilt: atan(0.75) = 36.9 deg.
//
// DERIVED FROM THE REFRACTION INVARIANT. Refraction bends toward the normal, so with
// eta = 0.75 the transmitted ray sits within 48.6 deg of -optics; for it to stay below the
// face — which is what lets `water_surface_radiance` skip a guard on the refracted ray
// entirely — the tilt must satisfy tilt + 48.6 < 90, i.e. tilt < 41.4 deg. This leaves
// 4.5 deg of margin. Cox-Munk at the wind model's maximum (12 m/s) asks for 0.72, just
// inside, so the cap binds only in a full gale.
const WAVE_MAX_TOTAL_STEEPNESS: f32 = 0.75;

// Half-angle of the directional fan (35 degrees). A sum of parallel waves is corrugated
// iron; spreading the components makes the sea short-crested, which is what breaks
// glitter into moving highlights instead of bands.
const WAVE_SPREAD_RADIANS: f32 = 0.61086524;

// How far a full gust shifts slope variance toward the SHORT components, as a fraction of
// each component's equal share. Short only, because wave response time scales with period:
// a gust ruffles a surface in seconds while the long components would need minutes.
//
// It REDISTRIBUTES variance and never adds any — the shares are renormalised, so the total
// stays exactly Cox-Munk. Adding energy here would double-count, because WindFrame::speed
// already carries the gust.
const WAVE_GUST_SHORT_BIAS: f32 = 0.6;

// Phase jitter the wind's eddy channel applies to the shortest component — the chop.
// Phase rather than amplitude, so it cannot disturb the steepness cap.
const WAVE_EDDY_PHASE_RADIANS: f32 = 0.8;

// Golden ratio, for the per-component phase offset `fract(i * phi) * TAU`. The sequence
// that stays maximally unaligned at every prefix length, so the components do not all
// cross zero together at t = 0.
const WAVE_GOLDEN_RATIO: f32 = 0.618034;

// W4 — where the per-component distance fade starts and ends, in wave CYCLES PER PIXEL.
// Not optional: a 0.6 m component at 40 m is sub-pixel, and a sub-pixel sinusoid does not
// read as a small wave, it reads as aliasing sparkle — worse than the flat water it
// replaced.
//
// The criterion is NYQUIST, not taste. A footprint of f metres samples a wavelength L at
// f / L cycles per pixel, and past 0.5 (two pixels per wave) the sinusoid is beyond the
// sampling limit and cannot be represented at all, so the fade reaches zero exactly
// there.
//
// It STARTS well before that, at 0.125 (eight pixels per wave), because the shading is not
// linear in the normal: the mirror term is a near-perfect reflection carrying a sun disc,
// so a normal wobbling by a fraction of a pixel swings the pixel between sky, sun and
// shoreline. Visible sparkle precedes the sampling limit. Doing it properly means
// PRE-FILTERING the normal distribution (roughening with distance, LEAN/Toksvig style)
// rather than flattening; that is a separate arc, and this is the correct-in-the-limit
// cheap version. `src/water.rs` carries the same argument.
const WAVE_LOD_FADE_START_CYCLES_PER_PIXEL: f32 = 0.125;
const WAVE_LOD_FADE_END_CYCLES_PER_PIXEL: f32 = 0.5;

// The lowest cosine to the GEOMETRIC face a mirror ray may leave at (about 1.1 degrees
// above the plane). Below it the ray is lifted back — see `water_surface_radiance`,
// which is the only caller and carries the argument. Not zero, because a ray exactly in
// the plane of the face is the degenerate case the DDA is least happy with.
const WATER_REFLECTION_MIN_COSINE: f32 = 0.02;

// Lift a mirror direction back above the face it leaves, if the wave normal threw it
// below. `src/water.rs::lift_reflection_above_face` is the CPU mirror and carries the
// argument, including why ONE step suffices with no iteration.
//
// The `optics == geometric` precondition is load-bearing: a ray reflected off the
// geometric face always leaves at |cos(incidence)| above it, so a FLAT surface cannot
// reflect into itself and must not be touched — otherwise extremely grazing rays (past
// ~88.9 degrees) would be nudged on flat water too, and the no-waves image would change.
fn water_lift_reflection(reflected: vec3<f32>, geometric: vec3<f32>,
                         optics: vec3<f32>) -> vec3<f32> {
    if (all(optics == geometric)) {
        return reflected;
    }
    let above_face = dot(reflected, geometric);
    if (above_face < WATER_REFLECTION_MIN_COSINE) {
        return normalize(reflected + geometric * (WATER_REFLECTION_MIN_COSINE - above_face));
    }
    return reflected;
}

// RMS surface slope this frame's wind produces — Cox & Munk, evaluated, scaled by the look
// lever. The ONE number that says how rough the water is; everything else only decides
// which wavelengths carry it.
fn wave_rms_slope() -> f32 {
    let speed = max(lighting.water_waves.y, 0.0);
    let variance = WAVE_SLOPE_VARIANCE_INTERCEPT
        + WAVE_SLOPE_VARIANCE_PER_METER_PER_SECOND * speed;
    return sqrt(variance) * clamp(lighting.water_optics.z, 0.0, 1.0);
}

// Component `index`'s share of the slope variance, normalised so the shares sum to 1.
//
// Equal shares are the PHILLIPS result: a k^-3 equilibrium spectrum spreads slope variance
// evenly across logarithmic bands, and the components are spaced evenly in log k. The gust
// then tilts the shares toward the short end, and the renormalisation is what keeps the
// total untouched.
fn wave_component_variance_share(index: u32) -> f32 {
    let gust = clamp(lighting.water_waves.z, 0.0, 1.0);
    var total = 0.0;
    var wanted = 0.0;
    for (var slot = 0u; slot < WAVE_COMPONENTS; slot = slot + 1u) {
        let short_weight = f32(slot) / f32(WAVE_COMPONENTS - 1u);
        // (short_weight - 0.5) * 2 spans -1 (longest) to +1 (shortest).
        let share = max(1.0 + WAVE_GUST_SHORT_BIAS * gust * (short_weight - 0.5) * 2.0, 0.0);
        total = total + share;
        if (slot == index) {
            wanted = share;
        }
    }
    if (total <= 0.0) {
        return 1.0 / f32(WAVE_COMPONENTS);
    }
    return wanted / total;
}

// A component's steepness before the total cap: sigma * sqrt(2 * share), then the Stokes
// limit.
//
// DERIVED FROM THE RMS SLOPE rather than from a fraction of a cap. For a sum of waves with
// independent phases the slope variance is sum(si^2) / 2 (the cross terms vanish), so a
// component carrying variance share wi of a total sigma^2 has si = sigma * sqrt(2 wi).
fn wave_component_steepness_uncapped(index: u32) -> f32 {
    let uncapped = wave_rms_slope() * sqrt(2.0 * wave_component_variance_share(index));
    return min(uncapped, WAVE_MAX_STEEPNESS);
}

// How much every component must shrink for the sum to respect WAVE_MAX_TOTAL_STEEPNESS.
// 1.0 whenever the cap is not binding, which is everything short of a gale. Applied
// equally, so capping cannot change the SHAPE of the sea — only its overall roughness.
fn wave_total_cap_scale() -> f32 {
    var uncapped = 0.0;
    for (var index = 0u; index < WAVE_COMPONENTS; index = index + 1u) {
        uncapped = uncapped + wave_component_steepness_uncapped(index);
    }
    if (uncapped <= WAVE_MAX_TOTAL_STEEPNESS) {
        return 1.0;
    }
    return WAVE_MAX_TOTAL_STEEPNESS / uncapped;
}

fn wave_component_steepness(index: u32) -> f32 {
    return wave_component_steepness_uncapped(index) * wave_total_cap_scale();
}

// How much of a component survives at a pixel footprint of `footprint_meters` (W4).
// 0 = infinitely sharp, i.e. no fade.
//
// Kept OUT of the steepness chain above on purpose: that chain is the WIND's roughness, a
// property of the weather, while this is a property of where the pixel is. Because the
// fade can only reduce it, WAVE_MAX_TOTAL_STEEPNESS keeps bounding the sum at every
// distance for free.
fn wave_component_lod_fade(wavelength: f32, footprint_meters: f32) -> f32 {
    if (footprint_meters <= 0.0) {
        return 1.0;
    }
    let cycles_per_pixel = footprint_meters / wavelength;
    return 1.0 - smoothstep(WAVE_LOD_FADE_START_CYCLES_PER_PIXEL,
                            WAVE_LOD_FADE_END_CYCLES_PER_PIXEL, cycles_per_pixel);
}

// Whether this frame has no waves at all — no wind, or the lever dialled to zero. The
// early-out that makes the off path bit-identical to the pre-wave renderer.
fn wave_field_is_flat() -> bool {
    var total = 0.0;
    for (var index = 0u; index < WAVE_COMPONENTS; index = index + 1u) {
        total = total + wave_component_steepness(index);
    }
    return total <= 0.0;
}

// (d h / d x, d h / d z) of the height field
// `h = sum Ai sin(ki (di . p) - wi t + phi_i)`, evaluated ANALYTICALLY:
// `sum Ai ki di cos(...)`. One evaluation of the field rather than the three a finite
// difference would need, and exact.
//
// Note what the amplitude does here: `Ai ki` IS the component's steepness, so the
// gradient is linear in the quantity WAVE_MAX_STEEPNESS bounds and the slope ceiling
// needs no clamp in this loop.
fn wave_height_gradient(position_meters: vec2<f32>, footprint_meters: f32) -> vec2<f32> {
    var gradient = vec2<f32>(0.0, 0.0);
    let last = f32(WAVE_COMPONENTS - 1u);
    for (var index = 0u; index < WAVE_COMPONENTS; index = index + 1u) {
        let position_in_band = f32(index) / last;

        // Geometric spacing: equal steps in log wavelength, so equal steps in log k.
        let wavelength = WAVE_LONGEST_METERS
            * pow(WAVE_SHORTEST_METERS / WAVE_LONGEST_METERS, position_in_band);
        let wavenumber = WAVE_TAU / wavelength;

        // Deep-water gravity-wave dispersion. THE term that stops a wave sum reading as
        // a scrolling texture: a 6 m wave runs at 3.06 m/s and a 0.6 m wave at 0.97, so
        // the interference never repeats on a beat.
        let angular_frequency = sqrt(WAVE_GRAVITY * wavenumber);

        // The fan widens toward the short components and alternates side. Component 0
        // carries no fan, which is what makes it the wind bearing itself.
        let fan_side = select(1.0, -1.0, (index & 1u) == 0u);
        let angle = lighting.water_waves.x
            + WAVE_SPREAD_RADIANS * position_in_band * fan_side;
        let direction = vec2<f32>(cos(angle), sin(angle));

        var phase_offset = fract(f32(index) * WAVE_GOLDEN_RATIO) * WAVE_TAU;
        if (index == WAVE_COMPONENTS - 1u) {
            phase_offset = phase_offset
                + WAVE_EDDY_PHASE_RADIANS * clamp(lighting.water_waves.w, -1.0, 1.0);
        }

        // The temporal term goes through the SHARED split-clock recombination in
        // `world.wgsl` rather than a second one invented here: `omega * t` in a plain
        // f32 loses the fraction an oscillator needs within hours of uptime, which is
        // the whole reason the clock ships as epochs plus a remainder. The spatial term
        // needs no such care — world positions are bounded.
        let temporal = WAVE_TAU * animation_oscillator_phase(angular_frequency / WAVE_TAU);
        let phase = wavenumber * dot(direction, position_meters) - temporal + phase_offset;

        let steepness = wave_component_steepness(index)
            * wave_component_lod_fade(wavelength, footprint_meters);
        gradient = gradient + direction * (steepness * cos(phase));
    }
    return gradient;
}

// ---- E7: caustics -------------------------------------------------------------
//
// The bright shifting web on a sunlit pool bed. **Derived from the wave field, not
// painted from noise** — which is the whole reason to have W1's analytic field: caustics
// ARE the focusing of the refracted sun by the surface's CURVATURE, so with the height
// field in hand the effect is a Jacobian rather than a texture.
//
// The construction. A near-vertical sun ray meeting a surface of slope s refracts to
// s/n, so it leaves the vertical by s(1 - 1/n) and lands, at depth d, displaced
// horizontally by `d(1 - 1/n) grad h`. That makes the surface-to-bed map
//
//     u(x) = x + d (1 - 1/n) grad h(x)
//
// whose Jacobian is `J = I + d(1 - 1/n) H`, H the HESSIAN of the height field. Light is
// conserved along a tube, so the irradiance gain is `1 / |det J|`: where neighbouring
// rays converge (det J -> 0) the bed is bright, where they spread it is dim. det J < 0 is
// past the focus, where the map has folded — |det| is still the right density there.
//
// The reference Shadertoy used two Perlin lobes times `exp(-|depth - 1|)`, which cannot
// respond to wind, direction or wavelength because it never looks at the surface. This
// does all three for free, since sin/cos of the same phase is already being evaluated.
//
// Applied to the SUN TERM ONLY, and inside `water_sun_transmission`, which is exactly
// right on both counts: caustics are a redistribution of the sun's own path through the
// surface, and that function has already marched from the bed up to the surface — so the
// entry point (`exit_point`) and the depth (`distance_voxels`) are in hand and no second
// ray is spent. Ambient light is not focused by anything and must not be scaled.
const WATER_CAUSTICS: bool = true;

// Ceiling on the focus gain, i.e. how bright the brightest filament may get.
//
// A cap is not optional: at a true focus det J -> 0 and the gain diverges. Real caustics
// are bounded by the sun's angular size (0.53 deg smears the focus) and by wavelength;
// modelling either properly is a caustic-map arc of its own, so this is the honest cheap
// stand-in. 4.0 is a stated look bound, and `caustic_gain_stays_within_its_stated_bounds`
// pins the whole distribution against it.
const WATER_CAUSTIC_MAX_GAIN: f32 = 4.0;

// The HESSIAN of the same height field `wave_height_gradient` differentiates once:
// returns `(d2h/dx2, d2h/dz2, d2h/dxdz)`.
//
// `h = sum Ai sin(phi_i)` differentiates twice to `-sum Ai ki^2 (di (x) di) sin(phi_i)`,
// and `Ai ki` IS the component's steepness — so the term is `steepness * wavenumber`,
// bounded by the same cap the gradient is, times one more factor of k. That extra k is
// why the short components dominate curvature and therefore dominate caustics, which is
// the physically right answer: fine ripples make fine filaments.
//
// `src/water.rs::WaveField::height_hessian` is the CPU mirror, checked against a second
// finite difference of the analytic gradient.
fn wave_height_hessian(position_meters: vec2<f32>, footprint_meters: f32) -> vec3<f32> {
    var hessian = vec3<f32>(0.0, 0.0, 0.0);
    let last = f32(WAVE_COMPONENTS - 1u);
    for (var index = 0u; index < WAVE_COMPONENTS; index = index + 1u) {
        let position_in_band = f32(index) / last;
        let wavelength = WAVE_LONGEST_METERS
            * pow(WAVE_SHORTEST_METERS / WAVE_LONGEST_METERS, position_in_band);
        let wavenumber = WAVE_TAU / wavelength;
        let angular_frequency = sqrt(WAVE_GRAVITY * wavenumber);
        let fan_side = select(1.0, -1.0, (index & 1u) == 0u);
        let angle = lighting.water_waves.x
            + WAVE_SPREAD_RADIANS * position_in_band * fan_side;
        let direction = vec2<f32>(cos(angle), sin(angle));
        var phase_offset = fract(f32(index) * WAVE_GOLDEN_RATIO) * WAVE_TAU;
        if (index == WAVE_COMPONENTS - 1u) {
            phase_offset = phase_offset
                + WAVE_EDDY_PHASE_RADIANS * clamp(lighting.water_waves.w, -1.0, 1.0);
        }
        let temporal = WAVE_TAU * animation_oscillator_phase(angular_frequency / WAVE_TAU);
        let phase = wavenumber * dot(direction, position_meters) - temporal + phase_offset;

        let steepness = wave_component_steepness(index)
            * wave_component_lod_fade(wavelength, footprint_meters);
        let magnitude = -steepness * wavenumber * sin(phase);
        hessian = hessian + vec3<f32>(direction.x * direction.x,
                                      direction.y * direction.y,
                                      direction.x * direction.y) * magnitude;
    }
    return hessian;
}

// The sun's focus gain at a submerged point, from the medium march that already reached
// the surface above it. 1.0 means "no focusing" and is returned, with no trigonometry
// evaluated, whenever the geometry cannot focus anything.
fn water_caustic_gain(medium: WaterMedium, water_material: u32) -> f32 {
    if (!WATER_CAUSTICS || !WATER_WAVES) {
        return 1.0;
    }
    // Only a ray that actually reached the SURFACE carries focused sunlight. SOLID means
    // the sun is blocked by terrain (so there is no sun term to focus) and LIMIT means the
    // march gave up in the murk.
    if (medium.kind != WATER_MEDIUM_AIR || wave_field_is_flat()) {
        return 1.0;
    }
    let bending_index = water_bending_index(water_material);
    if (bending_index <= WATER_AIR_INDEX) {
        return 1.0; // the refraction dial is fully open: nothing bends, nothing focuses
    }

    // No LOD fade on the Hessian, and the reason is turbidity rather than laziness: a
    // distant bed is a DEEP bed, and E7's fade has already removed it (0.083 of blue at
    // three blocks), so there is no far-field caustic left to alias. What remains bounded
    // is the near field, and WATER_CAUSTIC_MAX_GAIN bounds that.
    let entry_meters = medium.exit_point.xz * brickmap.voxel_size_meters;
    let depth_meters = medium.distance_voxels * brickmap.voxel_size_meters;
    let hessian = wave_height_hessian(entry_meters, 0.0);

    let bend = depth_meters * (1.0 - WATER_AIR_INDEX / bending_index);
    let determinant = (1.0 + bend * hessian.x) * (1.0 + bend * hessian.y)
        - (bend * hessian.z) * (bend * hessian.z);
    return min(1.0 / max(abs(determinant), 1.0 / WATER_CAUSTIC_MAX_GAIN),
               WATER_CAUSTIC_MAX_GAIN);
}

// The source example's hash and gradient-noise implementation. Keep this local
// rather than routing through pattern.wgsl: the ripple should match the pasted
// reference byte-for-byte, including its gradient directions and smoothstep
// interpolation.
fn hash33(p3_in: vec3<f32>) -> vec3<f32> {
    var p3 = fract(p3_in * vec3<f32>(0.1031, 0.1030, 0.0973));
    p3 = p3 + dot(p3, p3.yxz + 33.33);
    return fract((p3.xxy + p3.yxx) * p3.zyx);
}

fn hash22(p_in: vec2<f32>) -> vec2<f32> {
    var p3 = fract(vec3<f32>(p_in.xyx) * vec3<f32>(0.1031, 0.1030, 0.0973));
    p3 = p3 + dot(p3, p3.yzx + 33.33);
    return fract((p3.xx + p3.yz) * p3.zy);
}

fn hash12(p_in: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p_in.xyx) * 0.1031);
    p3 = p3 + dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

fn getGradient3D(pos: vec3<f32>) -> vec3<f32> {
    return normalize(hash33(pos) - 0.5);
}

fn getPerlinNoise3D(pos: vec3<f32>) -> f32 {
    let pos1 = floor(pos);
    let pos2 = pos1 + vec3<f32>(1.0, 0.0, 0.0);
    let pos3 = pos1 + vec3<f32>(0.0, 1.0, 0.0);
    let pos4 = pos1 + vec3<f32>(1.0, 1.0, 0.0);
    let pos5 = pos1 + vec3<f32>(0.0, 0.0, 1.0);
    let pos6 = pos1 + vec3<f32>(1.0, 0.0, 1.0);
    let pos7 = pos1 + vec3<f32>(0.0, 1.0, 1.0);
    let pos8 = pos1 + vec3<f32>(1.0, 1.0, 1.0);

    let v1 = getGradient3D(pos1);
    let v2 = getGradient3D(pos2);
    let v3 = getGradient3D(pos3);
    let v4 = getGradient3D(pos4);
    let v5 = getGradient3D(pos5);
    let v6 = getGradient3D(pos6);
    let v7 = getGradient3D(pos7);
    let v8 = getGradient3D(pos8);

    var delta = pos - pos1;
    var r1 = dot(v1, delta);
    var r2 = dot(v2, pos - pos2);
    var r3 = dot(v3, pos - pos3);
    var r4 = dot(v4, pos - pos4);
    var r5 = dot(v5, pos - pos5);
    var r6 = dot(v6, pos - pos6);
    var r7 = dot(v7, pos - pos7);
    var r8 = dot(v8, pos - pos8);

    delta.x = smoothstep(0.0, 1.0, delta.x);
    delta.y = smoothstep(0.0, 1.0, delta.y);
    delta.z = smoothstep(0.0, 1.0, delta.z);

    r1 = mix(r1, r2, delta.x);
    r2 = mix(r3, r4, delta.x);
    r3 = mix(r5, r6, delta.x);
    r4 = mix(r7, r8, delta.x);
    r1 = mix(r1, r2, delta.y);
    r2 = mix(r3, r4, delta.y);
    return mix(r1, r2, delta.z);
}

fn getGradient2D(pos: vec2<f32>) -> vec2<f32> {
    return normalize(hash22(pos) - 0.5);
}

fn getPerlinNoise2D(pos: vec2<f32>) -> f32 {
    let pos1 = floor(pos);
    let pos2 = pos1 + vec2<f32>(1.0, 0.0);
    let pos3 = pos1 + vec2<f32>(0.0, 1.0);
    let pos4 = pos1 + vec2<f32>(1.0, 1.0);
    let v1 = getGradient2D(pos1);
    let v2 = getGradient2D(pos2);
    let v3 = getGradient2D(pos3);
    let v4 = getGradient2D(pos4);
    var delta = pos - pos1;
    var r1 = dot(v1, delta);
    var r2 = dot(v2, pos - pos2);
    var r3 = dot(v3, pos - pos3);
    var r4 = dot(v4, pos - pos4);
    delta.x = smoothstep(0.0, 1.0, delta.x);
    delta.y = smoothstep(0.0, 1.0, delta.y);
    r1 = mix(r1, r2, delta.x);
    r2 = mix(r3, r4, delta.x);
    return mix(r1, r2, delta.y);
}

fn example_water_noise(point: vec3<f32>) -> f32 {
    return getPerlinNoise3D(point)
        + getPerlinNoise3D(point * 2.0 + vec3<f32>(3.0)) * 0.3
        + getPerlinNoise3D(point * 4.0 + vec3<f32>(5.0)) * 0.1;
}

fn splash_age(event: WorldEvent) -> f32 {
    return (lighting.event_params.y - event.started_epoch) * ANIMATION_EPOCH_SECONDS
        + lighting.event_params.x - event.started_remainder_seconds;
}

// The height GRADIENT (dh/dx, dh/dz) every live splash ring contributes at this point.
//
// A finite-difference gradient of the example's animated noise, multiplied by a radial
// impact envelope. The event's start stamp supplies iTime, so the ripple dies without
// leaving any persistent animation behind — what remains once it has is the wind field's
// small always-on ripple, not flat water.
//
// A gradient rather than a normal so it adds to `wave_height_gradient` and shares its cap;
// see the section header. Zero when no event is live, which is the common case and costs
// one comparison per water pixel.
fn water_splash_gradient(point_voxels: vec3<f32>) -> vec2<f32> {
    var gradient = vec2<f32>(0.0, 0.0);
    let point_meters = point_voxels * brickmap.voxel_size_meters;
    let epsilon = 0.001;
    let count = min(world_event_count(), MAX_WORLD_EVENTS);
    for (var index = 0u; index < MAX_WORLD_EVENTS; index = index + 1u) {
        if (index >= count) { break; }
        let event = world_events[index];
        if (event.channel != RIPPLE_CHANNEL || event.strength <= 0.0) {
            continue;
        }
        let age = splash_age(event);
        if (age < 0.0 || age > RIPPLE_MAX_AGE) {
            continue;
        }

        let offset = point_meters.xz - event.position_meters.xz;
        let radius = length(offset);
        let front = RIPPLE_WAVE_SPEED * age;
        let width = max(0.28, age * 0.16);
        let ring = exp(-pow((radius - front) / width, 2.0))
            * exp(-age * 0.65)
            * (1.0 - smoothstep(0.0, event.radius_meters, radius));
        if (ring <= 0.0001) { continue; }

        // Same finite-difference construction as the Shadertoy example: sample the
        // animated field at p, p+dx and p+dz. Divided by epsilon so what comes out is an
        // actual height gradient rather than an epsilon-sized difference (the example's
        // tangent cross-product collapsed to ~epsilon and made the ripple invisible at
        // our voxel scale).
        let noise_point = vec3<f32>(
            point_meters.xz * RIPPLE_NOISE_SCALE,
            age * RIPPLE_TIME_SCALE,
        );
        let n1 = example_water_noise(noise_point);
        let n2 = example_water_noise(noise_point + vec3<f32>(epsilon, 0.0, 0.0));
        let n3 = example_water_noise(noise_point + vec3<f32>(0.0, 0.0, epsilon));
        let slope = vec2<f32>((n2 - n1) / epsilon, (n3 - n1) / epsilon);

        // DIRECTION from the noise, MAGNITUDE from the envelope. The noise field is
        // dimensionless, so its raw slope is in no particular units and cannot be summed
        // with a wave steepness; normalising it and scaling by RIPPLE_STEEPNESS makes the
        // ring's contribution a slope with a stated bound.
        let slope_length_squared = dot(slope, slope);
        if (slope_length_squared <= 0.0) { continue; }
        gradient = gradient + slope * (RIPPLE_STEEPNESS * ring * event.strength
            / sqrt(slope_length_squared));
    }
    return gradient;
}

// The ONE cap on the summed surface gradient, wind plus splashes.
//
// `wave_total_cap_scale` bounds the WIND field's own sum, so this only ever binds when a
// splash rides on top of an already-rough sea — but it has to exist, because
// WAVE_MAX_TOTAL_STEEPNESS is not a taste knob: it is the REFRACTION INVARIANT. A tilt
// past atan(0.75) = 36.9 degrees can throw the refracted ray back above the face, and
// `water_surface_radiance` skips a guard on that ray precisely because this bound holds.
//
// Scaled rather than clipped per-axis, so capping cannot rotate the normal — only flatten
// it. `src/water.rs::clamp_surface_gradient` is the CPU mirror.
fn water_clamp_surface_gradient(gradient: vec2<f32>) -> vec2<f32> {
    let steepness = length(gradient);
    if (steepness <= WAVE_MAX_TOTAL_STEEPNESS) {
        return gradient;
    }
    return gradient * (WAVE_MAX_TOTAL_STEEPNESS / steepness);
}

// The wave normal at a point on a water surface, `normalize(-dh/dx, 1, -dh/dz)`.
//
// **This is the OPTICS normal and not the BIAS normal.** Secondary-ray origins must
// keep being offset along the GEOMETRIC face normal (`hit_normal`): offsetting along a
// perturbed normal moves a ray in a direction unrelated to the face it is escaping, and
// it self-intersects. `dda.wgsl` keeps the two apart; see the plan doc's trap 1.
//
// Returns the geometric normal exactly, with no trigonometry evaluated, when:
//   * both levers are off (naga folds this whole file's contribution away),
//   * the wave field is flat AND no splash ring is live, or
//   * the face is not the TOP of the voxel. A pool wall seen in section is not a
//     heightfield surface; a height gradient applied to a vertical face is meaningless
//     and reads as a wobbling wall.
fn water_surface_normal(hit: Hit, point_voxels: vec3<f32>) -> vec3<f32> {
    let geometric = hit_normal(hit);
    if (!WATER_WAVES && !RIPPLE_ENABLED) {
        return geometric;
    }
    // The +Y face: the DDA reports `axis_sign` as the sign of the ray's own component,
    // so a downward ray crossing the top face gives axis 1 and a negative sign. Checked
    // ONCE here, for both sources — a splash ring is as meaningless on a pool wall as a
    // wind wave is.
    if (hit.axis != 1u || hit.axis_sign >= 0.0) {
        return geometric;
    }

    // Source 1: the wind field, always on. `wave_field_is_flat` is true only when the
    // amplitude lever is at zero — Cox & Munk's intercept keeps the field non-flat at
    // every wind speed the weather can produce, including its 1 m/s floor.
    var gradient = vec2<f32>(0.0, 0.0);
    if (WATER_WAVES && !wave_field_is_flat()) {
        let position_meters = point_voxels.xz * brickmap.voxel_size_meters;
        // W4: the pixel's footprint at this surface, metres. `material_params.z` is the
        // metres-per-pixel at ONE metre — a resolution-dependent uniform rather than a
        // const, because the render scale moves with the quality preset — so scaling it by
        // the hit distance gives the footprint here. Same term `pattern.wgsl` budgets
        // octaves with.
        let distance_meters = hit.distance * brickmap.voxel_size_meters;
        let footprint_meters = distance_meters * lighting.material_params.z;
        gradient = wave_height_gradient(position_meters, footprint_meters);
    }

    // Source 2: splash rings, added on top. Deliberately NOT LOD-faded: a ring is a
    // metres-wide transient the player just caused, close to the camera by construction,
    // and its noise is not a sinusoid the Nyquist argument applies to.
    if (RIPPLE_ENABLED) {
        gradient = gradient + water_splash_gradient(point_voxels);
    }

    if (dot(gradient, gradient) <= 0.0) {
        return geometric;
    }
    let capped = water_clamp_surface_gradient(gradient);
    return normalize(vec3<f32>(-capped.x, 1.0, -capped.y));
}

// ---- The medium march ---------------------------------------------------------

struct WaterMedium {
    // One of WATER_MEDIUM_SOLID / _AIR / _LIMIT.
    kind: u32,
    // Path length travelled INSIDE the liquid, voxel units — what the extinction
    // integrates over, and the reason this is a march and not a `trace` call.
    distance_voxels: f32,
    // WATER_MEDIUM_SOLID only: the terrain voxel the ray stopped on, in the same
    // shape `trace` returns, so it goes straight into the shading path.
    hit: Hit,
    // WATER_MEDIUM_AIR only: where the ray left the liquid (voxel units) and the
    // interface normal OPPOSING the ray (i.e. pointing back into the liquid),
    // matching `hit_normal`'s convention so `refract_at` takes it directly.
    exit_point: vec3<f32>,
    exit_normal: vec3<f32>,
}

// March a ray that STARTS inside a liquid until the liquid ends, and report how
// it ended and how far it travelled.
//
// Why this is not a `trace` call: a liquid voxel is *occupied*, so `trace` would
// return a hit at t = 0 on the voxel the ray is already inside. The question here
// is the complement — "where does this medium stop" — which needs a per-voxel
// walk with a material read, and it must also detect the medium/AIR boundary,
// which is not an occupied voxel at all and therefore not a `trace` result.
//
// It is NOT a forked DDA: it steps the shared `DdaState` core (`dda_setup` /
// `dda_step`) at the fine cell size, exactly as `trace_shadow_visibility` is a
// second consumer of the same core. What differs is only the predicate at a cell.
fn water_medium_march(origin: vec3<f32>, direction: vec3<f32>) -> WaterMedium {
    var result: WaterMedium;
    result.kind = WATER_MEDIUM_LIMIT;
    result.distance_voxels = 0.0;
    result.hit.material = 0u;
    result.hit.axis = 0u;
    result.hit.axis_sign = 1.0;
    result.hit.distance = 0.0;
    result.hit.voxel = vec3<i32>(0, 0, 0);
    result.exit_point = origin;
    // Degenerate fallback (the caller's origin was not in a liquid after all):
    // an interface facing straight back along the ray refracts without bending.
    result.exit_normal = -direction;

    let inverse_direction = vec3<f32>(
        safe_inverse(direction.x),
        safe_inverse(direction.y),
        safe_inverse(direction.z),
    );
    let bounds = intersect_world_bounds(origin, inverse_direction);
    if (bounds.x > bounds.y) {
        return result;
    }
    let world_max = vec3<i32>(brickmap.world_size_voxels) - vec3<i32>(1, 1, 1);
    var state = dda_setup(origin, direction, inverse_direction, bounds.x, 1.0,
                          vec3<i32>(0, 0, 0), world_max);
    let t_limit = min(bounds.y, WATER_MEDIUM_MAX_DISTANCE);

    for (var step_index = 0u; step_index < WATER_MEDIUM_MAX_STEPS; step_index = step_index + 1u) {
        if (any(state.cell < vec3<i32>(0, 0, 0)) || any(state.cell > world_max)) {
            break; // left the world while still wet — nothing more to shade
        }
        if (state.t > t_limit) {
            result.distance_voxels = t_limit;
            return result; // murk horizon
        }
        let material = voxel_material_at(state.cell);
        if (material == 0u) {
            result.kind = WATER_MEDIUM_AIR;
            result.distance_voxels = max(state.t, 0.0);
            result.exit_point = origin + direction * result.distance_voxels;
            if (step_index > 0u) {
                var normal = vec3<f32>(0.0, 0.0, 0.0);
                let axis_sign = sign(component_of(direction, state.face_axis));
                if (state.face_axis == 0u) {
                    normal.x = -axis_sign;
                } else if (state.face_axis == 1u) {
                    normal.y = -axis_sign;
                } else {
                    normal.z = -axis_sign;
                }
                result.exit_normal = normal;
            }
            return result;
        }
        if (!material_is_liquid(material)) {
            result.kind = WATER_MEDIUM_SOLID;
            result.distance_voxels = max(state.t, 0.0);
            result.hit.material = material;
            result.hit.axis = state.face_axis;
            result.hit.axis_sign = sign(component_of(direction, state.face_axis));
            result.hit.distance = result.distance_voxels;
            result.hit.voxel = state.cell;
            return result;
        }
        dda_step(&state);
    }
    result.distance_voxels = min(max(state.t, 0.0), t_limit);
    return result;
}

// Whether a point given in VOXEL units sits inside a liquid — the underwater
// test, applied to the primary ray's own origin so it is true for the walking
// body's submerged head (E2b's `head_submerged`) AND for a fly camera that
// happens to be under the surface. `src/water.rs::eye_is_submerged` is the CPU
// mirror, pinned against the character controller by test.
fn point_is_submerged(position_voxels: vec3<f32>) -> bool {
    return material_is_liquid(voxel_material_at(vec3<i32>(floor(position_voxels))));
}
