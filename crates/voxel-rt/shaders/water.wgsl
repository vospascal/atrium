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
fn water_absorption_per_meter(material: u32) -> vec3<f32> {
    return materials[material].absorption_per_meter * max(lighting.water_params.x, 0.0);
}

fn water_scattering_per_meter(material: u32) -> vec3<f32> {
    return materials[material].scattering_per_meter * max(lighting.water_params.y, 0.0);
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
