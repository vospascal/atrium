// dda.wgsl — the SHADING pass: primary rays, one sun shadow ray per hit,
// ambient occlusion (E1 ray-traced / E1b analytic), E4 CAGI indirect light,
// E6 water reflection/refraction/extinction, exposure + tonemap. Concatenated
// AFTER `world.wgsl` (the shared traversal core, which owns the brickmap
// bindings and the traversal/shadow levers), `cagi_volume.wgsl` (the shared
// light-volume bindings + sampler) and `water.wgsl` (E6's optics: Fresnel,
// Snell, Beer-Lambert and the medium march), so this file holds only what is
// specific to turning a camera ray into a pixel.
//
// Fullscreen compute pass (workgroup 8x8): one thread per output pixel builds
// a camera ray, traverses the two-level brickmap through the shared `trace`,
// and writes a shaded color to an rgba8unorm storage texture. Misses get a
// vertical sky gradient. Each primary hit fires ONE shadow ray toward the sun
// through `trace_shadow_visibility()` plus whatever the AO_MODE lever asks for
// (AO_RAY_COUNT short occlusion rays, or a ray-free analytic estimate from the
// local occupancy bits). Occlusion attenuates the INDIRECT term only — the sun
// term keeps its own shadow ray (see the AO and shadow lever blocks).
//
// Color pipeline: material albedos are sRGB-encoded (as authored in
// the former mesh renderer). This shader decodes them with the exact extended-sRGB
// transfer used by the tagged compositor surface, then does ALL lighting math in linear
// (sun term + indirect term), then applies EXPOSURE, one of six selectable
// tonemap curves, and the sRGB encode — in that order — before textureStore.
// Exposure and the curve are separate on purpose: without an exposure term the
// curve ends up doing exposure's job, which is what made switching curves read
// as a brightness change. The curves themselves are NOT in this file; they come
// from `voxel_color::tonemap::WGSL`. The storage-texture/blit contract is
// unchanged: the blit still receives sRGB-encoded bytes and undoes the
// swapchain's re-encode.
//
// Own bindings (group 0), on top of the shared ones documented in world.wgsl
// and cagi_volume.wgsl:
//   0  uniform  Camera        — camera.rs CameraUniform (80 bytes; position
//                               in world meters, ray basis vectors, resolution)
//   6  texture  output        — rgba8unorm storage texture, write-only

struct Camera {
    position: vec3<f32>,      // eye, world METERS
    _pad0: f32,
    forward: vec3<f32>,       // unit view direction
    _pad1: f32,
    right_scaled: vec3<f32>,  // right * tan(fov_y/2) * aspect
    _pad2: f32,
    up_scaled: vec3<f32>,     // up * tan(fov_y/2)
    _pad3: f32,
    resolution: vec2<f32>,    // output size, pixels
    _pad4: vec2<f32>,
}

@group(G_WORLD) @binding(B_CAMERA) var<uniform> camera: Camera;
@group(G_WORLD) @binding(B_OUTPUT) var output: texture_storage_2d<rgba8unorm, write>;

// ---- E1/E1b: ambient occlusion levers ----------------------------------------
// Ambient occlusion attenuates the hemisphere-ambient term only (never the
// direct sun term — the sun has its own shadow ray). The whole experiment
// folds away at AO_MODE = AO_MODE_OFF: with it off this shader is
// bit-identical to the pre-E1 renderer. The overlay's Quality panel switches
// the pipeline with these consts patched (src/ao.rs), and the benchmark measures
// every contender (bench doc, E1 / E1b sections). The RUNTIME knobs — strength
// (shading_params.x) and the fade ramp (.z/.w) — scale the result, never the
// work, so they need no pipeline rebuild.
//
// AO_MODE picks the technique (E1b shootout — E1c's presets tier by technique,
// not only by ray count: Potato/Quest/Balanced take 1, Beautiful takes 0):
//   0  ray-traced — AO_RAY_COUNT short occlusion rays through the shared
//      `trace` core (E1's winner: correct, +4.2-8.2 ms);
//   1  analytic corner — zero rays, classic voxel corner occlusion from the
//      8 occupancy bits around the hit face, bilinearly interpolated across
//      the face (technique bank T7);
//   2  off.
const AO_MODE_RAY_TRACED: u32 = 0u;
const AO_MODE_ANALYTIC_CORNER: u32 = 1u;
const AO_MODE_OFF: u32 = 2u;
const AO_MODE: u32 = 1u;
// Occlusion rays per primary hit (bench contenders: 1 / 2 / 4). 1 ray shows
// a stable but visible IGN crosshatch on flat ground; 2 is clean; 4 buys
// almost nothing more (E1 verdict).
const AO_RAY_COUNT: u32 = 2u;
// Max occlusion-ray length, voxel units (bench contenders: 8 / 16 / 32).
// 8 measured ~10-17% cheaper than 16 with near-identical grounding (the
// falloff already discounts far occluders); 32 just spreads a general
// dimming for +30% cost. See the E1 table in docs/voxel-rt-bench.md.
const AO_MAX_DISTANCE: f32 = 8.0;
// Ray direction strategy: 0 = cosine-weighted hemisphere, 1 = uniform
// hemisphere, 2 = fixed bent-up cone (normal tilted toward world up). All
// three are deterministic per pixel — the rotation comes from interleaved
// gradient noise over PIXEL COORDINATES only (no frame index, no temporal
// accumulation), so a still camera shows a stable, shimmer-free image.
const AO_DIRECTION_MODE: u32 = 0u;
// Occlusion falloff: true = distance-weighted (a hit at t contributes
// 1 - t / AO_MAX_DISTANCE, so close occluders darken more), false = binary.
const AO_DISTANCE_FALLOFF: bool = true;

// ---- E1b: AO cost-cutting levers (Pascal's addendum, 2026-07-30) -------------
// Three ways to spend fewer AO rays using data the pass already fetches. All
// default OFF; each is measured in isolation in the bench's E1b section.
//
// 1. Brick-neighbourhood early-out: if every brick of the 3x3x3 brick
//    neighbourhood around the hit voxel's own brick is empty, nothing outside
//    the own brick can occlude within AO_MAX_DISTANCE (8 voxels = 1 brick), so
//    the pixel falls back to the analytic corner estimate instead of tracing.
//    The test reads the 1-bit-per-brick occupancy grid (binding 9). NOTE the
//    known limitation measured in E1b: the chebyshev distance field cannot
//    drive this test (every neighbour of an occupied brick has distance <= 1),
//    and on terrain the bricks below/beside a surface brick are solid ground,
//    so it fires rarely — see the bench doc's verdict.
const AO_BRICK_EARLY_OUT: bool = false;
// 2. Distance level of detail: AO detail is sub-pixel far from the camera, so
//    fade the occlusion out over the ramp [shading_params.z, shading_params.w]
//    (voxel units; 8 voxels = 1 m) and skip the estimator entirely beyond the
//    end. Deterministic and view-dependent only through the primary hit
//    distance — no temporal component. The ramp bounds are RUNTIME uniform
//    fields (E1c measured the move out of shader consts as free), so the aerial
//    / Potato range is dialable without a pipeline rebuild; the flag itself
//    stays compile-time so the whole path folds away when it is off.
const AO_DISTANCE_FADE: bool = false;
// 3. Sun-aware ray budget: AO only modulates the ambient term, so it matters
//    least where the direct sun dominates. Halve the ray count (never below 1)
//    on pixels whose sun term exceeds AO_SUN_BUDGET_THRESHOLD.
const AO_SUN_AWARE_RAY_BUDGET: bool = false;
const AO_SUN_BUDGET_THRESHOLD: f32 = 0.5;

// Runtime material inspection views. The packed selector is decoded in
// pattern.wgsl so layer filtering and channel display share one override.
const MATERIAL_DEBUG_LIT: u32 = 0u;
const MATERIAL_DEBUG_BASE_COLOR: u32 = 1u;
const MATERIAL_DEBUG_SPECULAR: u32 = 2u;
const MATERIAL_DEBUG_AMBIENT_OCCLUSION: u32 = 3u;
const MATERIAL_DEBUG_DISPLACEMENT: u32 = 4u;
const MATERIAL_DEBUG_ROUGHNESS: u32 = 5u;
const MATERIAL_DEBUG_NORMAL: u32 = 6u;
const MATERIAL_DEBUG_EMISSION: u32 = 7u;
const MATERIAL_DEBUG_LOD: u32 = 8u;

// ---- Directional miss radiance (VGI, I3D'11 §5.1 / Fig. 7 point C) -----------
// Thiedemann et al.'s near-field gather reads an ENVIRONMENT MAP when an
// occlusion ray finds nothing within its search radius, instead of falling back
// to a scalar ambient. We already trace those rays (AO_MODE_RAY_TRACED) and we
// already have the sky as an analytic function, so the upgrade costs one
// gradient evaluation per ESCAPED ray and no new traversal.
//
// What it buys that `ambient_light(normal)` structurally cannot: that term is a
// function of normal.y alone, so it cannot tell a crevice open upward from an
// overhang open sideways. Sampling per RAY couples direction to VISIBILITY — the
// upward-open crevice receives the cool sky lobe, the sideways-open overhang the
// warm ground lobe, a sealed pocket nothing. That is the medium-scale directional
// band E1b's analytic corner AO gives up and CAGI only partly covers.
//
// The environment sampled is `ambient_light` itself, NOT `sky_color`. Measured
// 2026-07-30: sampling the raw sky function instead (luminance-normalized, so
// the level matched) turned shadowed grass teal and rock outcrops purple, because
// those constants are emitted radiance pushed through inverse Reinhard and their
// chromaticity — normalized zenith around (0.19, 0.73, 6.03) — is far outside
// anything usable as an ambient tint. Reusing the hemisphere lobes keeps every
// colour inside the range the look was already tuned for, and needs no new
// calibration constant.
//
// Two deliberate consequences, both measured before this ships as a default:
//  - The estimate is ALREADY visibility-weighted (occluded rays contribute
//    zero radiance), so it replaces the hemisphere term instead of being
//    multiplied by the AO factor — multiplying would double-count occlusion.
//    The artistic `strength` knob therefore stops applying to the hemisphere
//    term when this is on; it keeps scaling the CAGI volume term.
//  - Unbiased only under cosine-weighted directions (AO_DIRECTION_MODE = 0,
//    the shipped RT-AO default), since the ray density is the cosine factor of
//    the irradiance integral. The uniform and bent-up modes reweight it.
//
// The sun's specular lobe never enters this term, which is correct and worth
// stating: a short ray escaping toward the sun proves nothing about sun
// visibility (it is AO_MAX_DISTANCE long), so crediting it with sun radiance
// would warm exactly the surfaces the shadow ray already found to be shadowed.
const AO_MISS_RADIANCE: bool = false;

// ---- Color pipeline ----------------------------------------------------------
// `srgb_decode` / `srgb_encode` live in world.wgsl: the CAGI pass decodes
// table-derived albedo with the same curve.
//
// THE TONEMAP CURVES ARE NOT HERE. They live in `voxel_color::tonemap::WGSL`
// (`crates/voxel-color/shaders/tonemap.wgsl`) and are spliced into this module by
// `passes::dda::SHADER_SOURCE`, so `tonemap_reinhard`, `apply_tonemap` and the rest
// are in scope below exactly as if they were written here.
//
// They moved because the curve set is a TWO-SIDED CONTRACT that this file cannot hold
// on its own: `TonemapCurve::shader_index()` in voxel-color decides that GT7 is 5, and
// a `const TONEMAP_GT7: u32 = 5u` has to agree with it. While the two halves sat in
// different crates nothing could check that, and a reordered enum would have selected
// the wrong curve silently — a wiring bug wearing a rendering bug's clothes. One crate,
// one test.


// Robust secondary-ray origin (shadow AND AO rays). Reconstructing the hit
// point as
// origin + t * direction alone carries accumulated float error at large t;
// the hit voxel's INTEGER coordinate does not. So: clamp the reconstructed
// point strictly inside the hit voxel's footprint (a SHADOW_BIAS margin off
// every voxel edge, so the origin can never land in a neighboring solid
// column at shared edges/corners — no light leaks), snap the normal-axis
// component exactly onto the hit-face plane, then lift the point off the
// face by SHADOW_BIAS along the normal (never inside the solid — no acne).
fn shadow_ray_origin(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
                     normal: vec3<f32>) -> vec3<f32> {
    let voxel_min = vec3<f32>(hit.voxel);
    let voxel_max = voxel_min + vec3<f32>(1.0, 1.0, 1.0);
    var position = ray_origin + ray_direction * hit.distance;
    position = clamp(position,
                     voxel_min + vec3<f32>(SHADOW_BIAS, SHADOW_BIAS, SHADOW_BIAS),
                     voxel_max - vec3<f32>(SHADOW_BIAS, SHADOW_BIAS, SHADOW_BIAS));
    // A positive ray direction along the hit axis entered through the LOW
    // face of the voxel; a negative one through the HIGH face.
    if (hit.axis == 0u) {
        position.x = select(voxel_max.x, voxel_min.x, hit.axis_sign > 0.0);
    } else if (hit.axis == 1u) {
        position.y = select(voxel_max.y, voxel_min.y, hit.axis_sign > 0.0);
    } else {
        position.z = select(voxel_max.z, voxel_min.z, hit.axis_sign > 0.0);
    }
    return position + normal * SHADOW_BIAS;
}

// ---- E1/E1b: ambient occlusion --------------------------------------------------
//
// Isolated experiment unit (see the AO lever block): everything below folds
// away at AO_MODE = AO_MODE_OFF. The ray-traced estimator reuses the shared
// `trace` core with a short max distance — no forked DDA math; the analytic
// estimators reuse `voxel_occupied` — no forked index math.

// Interleaved gradient noise (Jimenez 2014): a fixed per-pixel dither in
// [0, 1) from pixel coordinates ONLY. Deterministic across frames, matching
// the engine's noiseless identity: no temporal accumulation, no per-frame
// randomness, and every value in a frame derived from that frame's inputs.
//
// S3 amended what "identical every frame" means, and the amendment is narrow.
// Material animation reads a clock and a world-event field from the frame
// uniform, so a still camera no longer implies a still image — but the pass
// remains a pure function OF those inputs. Freeze them (the deterministic
// animation lever pins the clock at zero and empties the event field) and
// frame-to-frame stability returns exactly. Nothing here accumulates history.
fn interleaved_gradient_noise(pixel: vec2<f32>) -> f32 {
    return fract(52.9829189 * fract(dot(pixel, vec2<f32>(0.06711056, 0.00583715))));
}

// Branchless orthonormal basis around `axis` (Duff et al. 2017). Columns:
// tangent, bitangent, axis.
fn orthonormal_basis(axis: vec3<f32>) -> mat3x3<f32> {
    let sign_z = select(-1.0, 1.0, axis.z >= 0.0);
    let a = -1.0 / (sign_z + axis.z);
    let b = axis.x * axis.y * a;
    let tangent = vec3<f32>(1.0 + sign_z * axis.x * axis.x * a, sign_z * b, -sign_z * axis.x);
    let bitangent = vec3<f32>(b, sign_z + axis.y * axis.y * a, -axis.y);
    return mat3x3<f32>(tangent, bitangent, axis);
}

// One AO ray direction. Stratified over ray_index (elevation strata +
// golden-ratio azimuth spacing), rotated per pixel by interleaved gradient
// noise so neighboring pixels probe different azimuths — a fixed dither, not
// frame-varying noise. Modes (AO_DIRECTION_MODE):
//   0  cosine-weighted hemisphere around the normal — matches the Lambert
//      weighting of the ambient term, so binary hits average to the correct
//      visibility integral;
//   1  uniform hemisphere around the normal — more grazing rays (finds
//      lateral occluders sooner, over-weights them physically);
//   2  fixed bent-up cone — the normal tilted toward world up (a cheap
//      sky-visibility proxy), fixed elevation ladder inside a ~37-degree
//      cone.
fn ao_ray_direction(normal: vec3<f32>, pixel: vec2<f32>, ray_index: u32,
                    ray_count: u32) -> vec3<f32> {
    let stratum = (f32(ray_index) + 0.5) / f32(ray_count);
    // Golden-ratio conjugate spaces the azimuths; the noise rotates the whole
    // fan per pixel.
    let azimuth = 6.28318530718
        * fract(f32(ray_index) * 0.61803398875 + interleaved_gradient_noise(pixel));
    let cos_azimuth = cos(azimuth);
    let sin_azimuth = sin(azimuth);

    var axis = normal;
    var cos_elevation = 0.0;
    if (AO_DIRECTION_MODE == 0u) {
        cos_elevation = sqrt(1.0 - stratum); // cosine-weighted: p ~ cos
    } else if (AO_DIRECTION_MODE == 1u) {
        cos_elevation = 1.0 - stratum; // uniform over the hemisphere
    } else {
        // Bent axis: normal + up degenerates on ceilings (normal = -Y);
        // fall back to the plain normal there.
        let bent = normal + vec3<f32>(0.0, 1.0, 0.0);
        if (dot(bent, bent) > 1e-4) {
            axis = normalize(bent);
        }
        // Fixed elevation ladder, 0.8..0.95 — stays inside the surface
        // hemisphere even with the axis bent 45 degrees off the normal.
        cos_elevation = mix(0.95, 0.8, stratum);
    }
    let sin_elevation = sqrt(max(1.0 - cos_elevation * cos_elevation, 0.0));
    return orthonormal_basis(axis)
        * vec3<f32>(cos_azimuth * sin_elevation, sin_azimuth * sin_elevation, cos_elevation);
}

// What one sweep of occlusion rays measured. `sky_radiance` stays zero unless
// AO_MISS_RADIANCE is compiled in, in which case the whole field folds away.
struct RayTracedAo {
    occlusion: f32,
    sky_radiance: vec3<f32>,
}

// Occlusion in [0, 1] from `ray_count` short occlusion rays out of the hit
// face, averaged (E1's estimator). Rays reuse `shadow_ray_origin` (same
// integer-reconstructed, acne-free origin as the sun ray) and the shared
// `trace` with AO_MAX_DISTANCE — the chebyshev distance field makes short rays
// through open space nearly free. `ray_count` is AO_RAY_COUNT except under the
// sun-aware budget lever.
fn ray_traced_occlusion(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
                        normal: vec3<f32>, pixel: vec2<f32>,
                        ray_count: u32) -> RayTracedAo {
    let surface_origin = shadow_ray_origin(hit, ray_origin, ray_direction, normal);
    var occlusion_sum = 0.0;
    var sky_sum = vec3<f32>(0.0, 0.0, 0.0);
    for (var ray_index = 0u; ray_index < ray_count; ray_index = ray_index + 1u) {
        let direction = ao_ray_direction(normal, pixel, ray_index, ray_count);
        let occluder = trace(surface_origin, direction, AO_MAX_DISTANCE, false);
        if (occluder.material != 0u) {
            if (AO_DISTANCE_FALLOFF) {
                occlusion_sum += 1.0 - clamp(occluder.distance / AO_MAX_DISTANCE, 0.0, 1.0);
            } else {
                occlusion_sum += 1.0;
            }
        } else if (AO_MISS_RADIANCE) {
            // VGI Fig. 7 point C: nothing within the search radius, so this ray
            // sees the environment — sampled in ITS OWN direction, not the
            // normal's. The "environment map" is the hemisphere term itself, so
            // the colours stay inside the range the look was tuned for.
            // The ray's own direction for the hemisphere lookup, the HIT's position for the
            // cloud lookup: this ray sees the sky from where the surface is, not from wherever
            // the ray happens to have reached.
            sky_sum += ambient_light(
                (ray_origin + ray_direction * hit.distance) * brickmap.voxel_size_meters,
                direction,
            );
        }
    }
    var estimate: RayTracedAo;
    estimate.occlusion = occlusion_sum / f32(ray_count);
    // Divided by the ray COUNT, not by the miss count: occluded rays contribute
    // zero, so the mean is the visibility-weighted sky integral (escaped
    // fraction times mean escaped radiance), which is the quantity that replaces
    // the flat hemisphere term.
    estimate.sky_radiance = sky_sum / f32(ray_count);
    return estimate;
}

// ---- E1b: analytic occlusion (technique bank T7) --------------------------------

// Integer face frame of a hit: the outward face normal plus the two positive
// axis directions spanning the face plane (axis 0 -> y/z, 1 -> x/z, 2 -> x/y).
// Integer so neighbour voxels can be addressed by adding these directly.
struct FaceBasis {
    normal: vec3<i32>,
    tangent: vec3<i32>,
    bitangent: vec3<i32>,
}

fn face_basis(hit: Hit) -> FaceBasis {
    var basis: FaceBasis;
    let outward = -i32(hit.axis_sign);
    if (hit.axis == 0u) {
        basis.normal = vec3<i32>(outward, 0, 0);
        basis.tangent = vec3<i32>(0, 1, 0);
        basis.bitangent = vec3<i32>(0, 0, 1);
    } else if (hit.axis == 1u) {
        basis.normal = vec3<i32>(0, outward, 0);
        basis.tangent = vec3<i32>(1, 0, 0);
        basis.bitangent = vec3<i32>(0, 0, 1);
    } else {
        basis.normal = vec3<i32>(0, 0, outward);
        basis.tangent = vec3<i32>(1, 0, 0);
        basis.bitangent = vec3<i32>(0, 1, 0);
    }
    return basis;
}

// Occlusion of ONE face corner in [0, 1] from the three neighbours touching
// it: two edge-adjacent and one diagonal. Two solid edge neighbours seal the
// corner completely regardless of the diagonal — the classic voxel corner-AO
// rule (this is the signal the former mesh path baked into vertex colors, so
// the look is known-good for this art style).
fn corner_occlusion(edge_a: bool, edge_b: bool, diagonal: bool) -> f32 {
    if (edge_a && edge_b) {
        return 1.0;
    }
    return (f32(edge_a) + f32(edge_b) + f32(diagonal)) / 3.0;
}

// Zero-ray occlusion from the 8 occupancy bits surrounding the hit face, in
// the voxel plane one step OUTSIDE it (that voxel is the one the ray came
// through, so the plane's center is empty by construction). The four face
// corners each take their three touching neighbours, and the result is
// interpolated bilinearly with the hit point's face-local UV — the same
// smooth-across-the-face gradient a meshed renderer gets from vertex colors,
// reconstructed per pixel from the exact DDA hit position.
fn analytic_corner_occlusion(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
                             normal: vec3<f32>) -> f32 {
    let basis = face_basis(hit);
    let plane_center = hit.voxel + basis.normal;

    let edge_tangent_low = voxel_occupied(plane_center - basis.tangent);
    let edge_tangent_high = voxel_occupied(plane_center + basis.tangent);
    let edge_bitangent_low = voxel_occupied(plane_center - basis.bitangent);
    let edge_bitangent_high = voxel_occupied(plane_center + basis.bitangent);
    let corner_low_low = voxel_occupied(plane_center - basis.tangent - basis.bitangent);
    let corner_high_low = voxel_occupied(plane_center + basis.tangent - basis.bitangent);
    let corner_low_high = voxel_occupied(plane_center - basis.tangent + basis.bitangent);
    let corner_high_high = voxel_occupied(plane_center + basis.tangent + basis.bitangent);

    let occlusion_low_low = corner_occlusion(edge_tangent_low, edge_bitangent_low,
                                             corner_low_low);
    let occlusion_high_low = corner_occlusion(edge_tangent_high, edge_bitangent_low,
                                              corner_high_low);
    let occlusion_low_high = corner_occlusion(edge_tangent_low, edge_bitangent_high,
                                              corner_low_high);
    let occlusion_high_high = corner_occlusion(edge_tangent_high, edge_bitangent_high,
                                               corner_high_high);

    // Face-local UV from the same clamped, integer-anchored hit reconstruction
    // the secondary rays use — inside the hit voxel's footprint, so both
    // components land in (0, 1).
    let surface_point = shadow_ray_origin(hit, ray_origin, ray_direction, normal);
    let local_point = surface_point - vec3<f32>(hit.voxel);
    let u = clamp(dot(local_point, vec3<f32>(basis.tangent)), 0.0, 1.0);
    let v = clamp(dot(local_point, vec3<f32>(basis.bitangent)), 0.0, 1.0);
    return mix(mix(occlusion_low_low, occlusion_high_low, u),
               mix(occlusion_low_high, occlusion_high_high, u), v);
}

// E1b cost-cutting lever 2, kept out of the estimator: the distance-fade
// weight in [0, 1] for a hit at `hit_distance` voxels, over the RUNTIME ramp
// (shading_params.z -> .w). 0 means "skip the estimator entirely".
fn ao_distance_fade(hit_distance: f32) -> f32 {
    return 1.0 - smoothstep(lighting.shading_params.z, lighting.shading_params.w,
                            hit_distance);
}

// Ambient-visibility factor in [1 - strength, 1]: the AO_MODE estimator's
// occlusion scaled by the runtime strength (lighting.shading_params.x), with
// the E1b cost-cutting levers applied around it. `sun_weight` is this pixel's
// direct sun term (only read by AO_SUN_AWARE_RAY_BUDGET).
// What the ambient term needs from the occlusion estimators.
//
// `factor` is E1's original scalar, semantics unchanged. `sky_radiance` and
// `sky_weight` serve AO_MISS_RADIANCE only, and `sky_weight` = 0 means "no rays
// measured a sky integral for this pixel" — which is how the lever composes
// with the two levers that legitimately skip the rays (AO_DISTANCE_FADE past
// its ramp, AO_BRICK_EARLY_OUT on an empty neighbourhood): those pixels fall
// back to the flat hemisphere term instead of going black.
struct AmbientEstimate {
    factor: f32,
    sky_radiance: vec3<f32>,
    sky_weight: f32,
}

fn ambient_estimate(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
                    normal: vec3<f32>, pixel: vec2<f32>,
                    sun_weight: f32) -> AmbientEstimate {
    var estimate: AmbientEstimate;
    estimate.factor = 1.0;
    estimate.sky_radiance = vec3<f32>(0.0, 0.0, 0.0);
    estimate.sky_weight = 0.0;

    var fade = 1.0;
    if (AO_DISTANCE_FADE) {
        fade = ao_distance_fade(hit.distance);
        if (fade <= 0.0) {
            return estimate; // sub-pixel detail at this range — skip the work entirely
        }
    }

    var occlusion = 0.0;
    if (AO_MODE == AO_MODE_RAY_TRACED) {
        var ray_count = AO_RAY_COUNT;
        if (AO_SUN_AWARE_RAY_BUDGET && sun_weight > AO_SUN_BUDGET_THRESHOLD) {
            ray_count = max(AO_RAY_COUNT / 2u, 1u);
        }
        if (AO_BRICK_EARLY_OUT
            && brick_neighborhood_empty(hit.voxel / vec3<i32>(8, 8, 8))) {
            // Nothing outside the own brick is within AO_MAX_DISTANCE, so the
            // rays could only find own-brick contact — the analytic estimate
            // already has that, for eight bit reads.
            occlusion = analytic_corner_occlusion(hit, ray_origin, ray_direction, normal);
        } else {
            let traced = ray_traced_occlusion(hit, ray_origin, ray_direction, normal,
                                              pixel, ray_count);
            occlusion = traced.occlusion;
            estimate.sky_radiance = traced.sky_radiance;
            estimate.sky_weight = fade;
        }
    } else if (AO_MODE == AO_MODE_ANALYTIC_CORNER) {
        occlusion = analytic_corner_occlusion(hit, ray_origin, ray_direction, normal);
    }
    estimate.factor = 1.0 - lighting.shading_params.x * fade * occlusion;
    return estimate;
}

// ---- E4: the indirect term ----------------------------------------------------

// This hit's INDIRECT radiance, before occlusion.
//
// With CAGI off it is E1c's hemisphere ambient, unchanged and bit-identical.
// With CAGI on it is the light volume sampled in the air cell in front of the
// hit face (`cagi_sample_surface`, which walks out of solid cells first), plus
// whatever share of hemisphere ambient the optional runtime override keeps.
// The authoritative default is zero: a sealed pocket with no emitter converges
// to black. Raising the override is an explicit non-physical readability choice.
//
// The surface point is reconstructed here rather than passed in so the whole
// function folds away when the lever is off: with CAGI_ENABLED = false naga
// deletes the `shadow_ray_origin` call too, which is what keeps the GI-off build
// byte-identical to E1c.
// Occlusion is applied HERE rather than to the sum in `shade_hit`, because
// AO_MISS_RADIANCE has to weight the two indirect terms differently: its sky
// integral is already visibility-weighted and must not be scaled by the AO
// factor a second time, while the CAGI volume still must be.
//
// The lever-off branch keeps the original arithmetic ORDER (occlusion times the
// sum), not just the original algebra, so the shipped default stays bit-identical
// — reassociating float multiplies would move the S2 pixel gate.
fn indirect_light(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
                  normal: vec3<f32>, ambient: AmbientEstimate) -> vec3<f32> {
    let hemisphere = ambient_light(
        (ray_origin + ray_direction * hit.distance) * brickmap.voxel_size_meters,
        normal,
    );
    if (!AO_MISS_RADIANCE) {
        if (!CAGI_ENABLED) {
            return hemisphere * ambient.factor;
        }
        let surface_point = shadow_ray_origin(hit, ray_origin, ray_direction, normal);
        let volume = cagi_sample_surface(surface_point, normal) * lighting.gi_params.x;
        return (hemisphere * lighting.gi_params.y + volume) * ambient.factor;
    }

    // The environment integral the AO rays measured. It carries the hemisphere
    // term's own colours and strength already (the rays sampled `ambient_light`),
    // so it needs no rescaling — only the fallback blend for pixels whose rays
    // another lever skipped (`sky_weight` = 0).
    let directional = mix(hemisphere * ambient.factor, ambient.sky_radiance,
                          ambient.sky_weight);
    if (!CAGI_ENABLED) {
        return directional;
    }
    let surface_point = shadow_ray_origin(hit, ray_origin, ray_direction, normal);
    let volume = cagi_sample_surface(surface_point, normal) * lighting.gi_params.x;
    return directional * lighting.gi_params.y + volume * ambient.factor;
}

// ---- E7: the water bounce light ----------------------------------------------
//
// The wobbling bright band a pool throws onto the wall beside it: the sun's SPECULAR
// reflection off the water surface, landing on nearby terrain. Nothing else in the
// renderer produces it — CAGI's volume carries diffuse bounce, and a mirror-smooth
// specular bounce is not diffuse.
//
// The trick that makes it one ray instead of an integral: for a flat water plane the
// reflected sun is a virtual sun BELOW the plane, so the direction from a surface toward
// it is just the sun direction MIRRORED IN Y. Look that way; if water is there, that
// water is showing you the sun.
//
// **It is a SECOND SUN, and it is built from the sun's own radiance** rather than from
// `sky_color`. Two reasons, and both are load-bearing:
//
//   * UNITS. `sky_color` carries the sun disc, whose radiance is enormous because the disc
//     subtends 6.8e-5 sr. A diffuse surface integrates radiance over solid angle, so
//     multiplying disc radiance by a made-up factor would be thousands of times too
//     bright. Reusing `sun_color_intensity` — the same term the direct sun uses — makes the
//     units right by construction: the mirrored sun's irradiance is the sun's, times what
//     the water reflects (Fresnel), which is physical rather than chosen.
//   * COST. `sky_color` runs a cloud march. Calling it once per shaded surface would put a
//     volumetric trace on every pixel in the frame.
//
// The lobe is where the waves come in. A flat surface reflects the sun into exactly one
// direction — `toward_mirror` — so the band would be a hard-edged spot. A normal tilted by
// `s` deflects the reflection by `2s`, so the lobe's width is `2 * wave_rms_slope()`:
// DERIVED from the same Cox-Munk roughness the glitter path uses, not a gloss parameter. It
// is why a choppy pool throws a broad soft band and a still one throws a tight bright one.
const WATER_BOUNCE_LIGHT: bool = true;

// How far a surface may be from the water and still catch its reflection, in METRES.
//
// A cap, because this is a ray per shaded pixel and an uncapped one would march the whole
// world from every surface that happens to face down-sun. 16 m also has a physical reading:
// the single sample stands in for a patch of water whose apparent size falls off with
// distance, so beyond a point the estimate is wrong anyway.
const WATER_BOUNCE_MAX_DISTANCE_METERS: f32 = 16.0;

// The share of the mirrored sun a facing surface receives.
//
// The one place a constant is unavoidable: a single sample cannot know how much of the
// glitter path the surface actually sees, which is what the correct factor would integrate.
// Everything else in this function is physical — the magnitude is the sun's, the reflected
// fraction is Fresnel's, the lobe width is the wave field's — so this is the whole of the
// approximation, in one number. 0.35 keeps a sunlit bank clearly brighter without letting a
// second-hand light out-compete the direct sun.
const WATER_BOUNCE_STRENGTH: f32 = 0.35;

// Floor on the lobe's half-width, radians. Dead-flat water would otherwise reflect the sun
// into a mathematically exact direction and the band would alias into a one-pixel line.
// 0.02 rad is about a degree — a touch wider than the sun's own 0.53 deg disc, which is the
// physical floor on how sharp any solar reflection can be.
const WATER_BOUNCE_MIN_LOBE_RADIANS: f32 = 0.02;

fn water_bounce_light(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
                      normal: vec3<f32>) -> vec3<f32> {
    if (!WATER_BOUNCE_LIGHT || WATER_MODE == WATER_MODE_OPAQUE) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    // Water lighting water is the reflection path's job, and a SUBMERGED surface already
    // receives the sun through the surface (with E7's caustics on it) — adding a bounce
    // there would count the same light twice.
    if (material_is_liquid(hit.material)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    let surface_point = shadow_ray_origin(hit, ray_origin, ray_direction, normal);
    if (point_is_submerged(surface_point)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }

    // Toward the virtual sun below the water plane.
    let sun = lighting.sun_direction;
    let toward_mirror = normalize(vec3<f32>(sun.x, -sun.y, sun.z));
    let facing = dot(normal, toward_mirror);
    if (facing <= 0.0 || toward_mirror.y >= 0.0) {
        return vec3<f32>(0.0, 0.0, 0.0); // faces away, or the sun is already below us
    }

    let reach = WATER_BOUNCE_MAX_DISTANCE_METERS / brickmap.voxel_size_meters;
    let water = trace(surface_point, toward_mirror, reach, false);
    if (water.material == 0u || !material_is_liquid(water.material)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }

    // The wave normal at the point of the water we are looking at — the reason the band
    // moves. `water_surface_normal` returns the flat face when the field is calm, so a still
    // pool still throws a band, it just does not shimmer.
    let water_point = shadow_ray_origin(water, surface_point, toward_mirror,
                                        hit_normal(water));
    let optics = water_surface_normal(water, water_point);

    // How close this patch of water actually throws the sun at us. Exactly 1 for flat water
    // (the mirror direction IS the sun), and it wanders as the waves tilt — which is the
    // wobble. `1 - cos(theta) ~ theta^2 / 2`, so this is a Gaussian in the angle.
    let reflected = reflect(toward_mirror, optics);
    let alignment = clamp(dot(reflected, sun), -1.0, 1.0);
    let lobe_radians = max(2.0 * wave_rms_slope(), WATER_BOUNCE_MIN_LOBE_RADIANS);
    let lobe = exp(-2.0 * (1.0 - alignment) / (lobe_radians * lobe_radians));
    if (lobe <= 0.0) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }

    // Fresnel at the WATER — the fraction it reflects rather than swallows — and the
    // receiving surface's own cosine. The magnitude is the SUN's, so this term is in the
    // same units as the direct sun above and cannot be off by a scale factor.
    let fresnel = fresnel_schlick(max(-dot(toward_mirror, optics), 0.0),
                                  material_index_of_refraction(water.material));
    let sun_radiance = lighting.sun_color_intensity.rgb * lighting.sun_color_intensity.w;
    return sun_radiance * (fresnel * facing * lobe * WATER_BOUNCE_STRENGTH);
}

// Linear-space shading of an ORDINARY surface: albedo * (sun lambert *
// visibility + indirect * AO). One shadow ray per hit through
// `trace_shadow_visibility` (binary in hard mode, a penumbra factor in soft
// mode); faces pointing away from the sun skip the trace outright (their lambert
// term is zero anyway).
//
// E4 composition contract, as documented since E1: occlusion multiplies the
// INDIRECT term only — never the direct sun term or its shadow ray. The
// multiply itself now lives inside `indirect_light` (see AO_MISS_RADIANCE),
// which is why this function passes the estimate down instead of scaling the
// result.
//
// E6 renamed this from `shade_hit`, which is now the dispatch that sends liquid
// hits to the water model and everything else here. A water hit shaded through
// THIS function is the "water is an opaque diffuse surface" fallback — what
// WATER_MODE_OPAQUE ships and what the two half-modes use for the half they do
// not trace.
const PBR_PI: f32 = 3.14159265359;

fn pbr_fresnel_schlick(cos_theta: f32, f0: vec3<f32>) -> vec3<f32> {
    let one_minus_cos = 1.0 - clamp(cos_theta, 0.0, 1.0);
    let factor = one_minus_cos * one_minus_cos * one_minus_cos * one_minus_cos * one_minus_cos;
    return f0 + (vec3<f32>(1.0) - f0) * factor;
}

fn pbr_distribution_ggx(n_dot_h: f32, alpha: f32) -> f32 {
    let alpha_squared = alpha * alpha;
    let denominator_term = n_dot_h * n_dot_h * (alpha_squared - 1.0) + 1.0;
    return alpha_squared / max(PBR_PI * denominator_term * denominator_term, 1e-6);
}

fn pbr_visibility_schlick_ggx(n_dot_x: f32, roughness: f32) -> f32 {
    let k = (roughness + 1.0) * (roughness + 1.0) * 0.125;
    return n_dot_x / max(n_dot_x * (1.0 - k) + k, 1e-5);
}

fn pbr_visibility_smith(n_dot_v: f32, n_dot_l: f32, roughness: f32) -> f32 {
    return pbr_visibility_schlick_ggx(n_dot_v, roughness)
        * pbr_visibility_schlick_ggx(n_dot_l, roughness);
}

fn shade_surface(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
                 pixel: vec2<f32>, sun_transmission: vec3<f32>) -> vec3<f32> {
    let geometric_normal = hit_normal(hit);
    let graph_material = material_graph_surface(
        hit.material,
        ray_origin + ray_direction * hit.distance,
        geometric_normal,
    );
    // S1 + S2: the per-face albedo with the row's albedo pattern layers applied.
    // Identical to the row's base albedo unless the row authored roles or layers AND
    // the matching lever is on, so this is the pre-S1 value bit-for-bit in the
    // shipped configuration.
    //
    // The sample is built once and shared by albedo and emission: the frames need a
    // world position, a voxel and a face, and none of that changes between targets.
    let flat_pattern = pattern_sample(hit, ray_origin, ray_direction);
    // Pick the bases FIRST, then run the layer stack once over them. The graph
    // branch used to sit after an unconditional `material_pattern_albedo` call and
    // throw its result away, so every graph-active material walked the whole layer
    // stack twice — once for a row base it did not want. Choosing the base is a
    // handful of selects; evaluating the stack is the expensive part, and it now
    // happens exactly once whether or not a graph is driving the surface.
    var pbr_base: PbrSurface;
    pbr_base.base_color = material_face_albedo(
        hit.material, flat_pattern.axis, flat_pattern.axis_sign);
    pbr_base.roughness = material_face_roughness(
        hit.material, flat_pattern.axis, flat_pattern.axis_sign);
    pbr_base.specular = materials[hit.material].specular;
    pbr_base.ambient_occlusion = 1.0;
    pbr_base.normal = vec3<f32>(0.0);
    pbr_base.emission = materials[hit.material].emission;
    var animation = pattern_animation_identity();
    if (graph_material.graph_active) {
        pbr_base.base_color = graph_material.base_color.rgb;
        // Graphs authored before face-role nodes existed used a flat base color.
        // Let those graphs retain the material table's directional appearance;
        // an explicit face_color node opts into graph-owned face semantics.
        if (!graph_material.face_color_active && MATERIAL_FACE_ROLES
            && (materials[hit.material].flags & MATERIAL_FLAG_FACE_ROLES) != 0u) {
            pbr_base.base_color = material_face_albedo(hit.material, hit.axis, hit.axis_sign);
        }
        pbr_base.roughness = graph_material.roughness;
        pbr_base.emission = graph_material.emission.rgb;
        pbr_base.specular = clamp(graph_material.specular, 0.0, 1.0);
        pbr_base.ambient_occlusion = clamp(graph_material.ambient_occlusion, 0.0, 1.0);
        pbr_base.normal = graph_material.normal;
        animation = graph_material.animation;
    }
    // P1 — the parallax march slides the shading point to where the ray really
    // lands on the relief; every pattern read below (color, roughness, emission,
    // height, normal) then happens at the DISPLACED point, which is what glues
    // plate tones to their plates under camera motion.
    let march = pattern_parallax_march(
        hit.material, flat_pattern, geometric_normal, ray_direction,
        brickmap.voxel_size_meters, animation);
    let pattern = march.sample;
    let relief_height_meters = pattern_relief_height(
        hit.material, pattern, brickmap.voxel_size_meters, animation);
    let displacement_meters = clamp(relief_height_meters, -0.25, 0.25);
    let displacement_offset_voxels = displacement_meters
        / max(brickmap.voxel_size_meters, 1e-5);
    var normal = pattern_relief_normal(
        hit.material, pattern, geometric_normal, brickmap.voxel_size_meters, animation);
    if (march.hit_wall) {
        // The march landed on a plate SIDE: the wall's own axis normal, not a
        // gradient of the top surface.
        normal = march.wall_normal;
    }
    if (length(pbr_base.normal) > 0.0001) {
        // The graph normal is a full world-space normal. Add its deviation from
        // the geometric face to the relief normal so connecting the built-in
        // Normal node does not accidentally erase displacement detail.
        normal = normalize(normal + normalize(pbr_base.normal) - geometric_normal);
    }
    var surface = material_pattern_surface_from_base(
        hit.material, pattern, pbr_base, animation);
    surface.normal = normal;

    // Keep inspection after the complete material stack and relief-normal
    // derivation. In particular, Normal is the normal that lighting would use,
    // not the graph's optional raw normal output painted over the base color.
    if (material_debug_enabled()) {
        let debug_view = material_debug_view();
        if (debug_view == MATERIAL_DEBUG_BASE_COLOR) {
            return srgb_decode(surface.base_color);
        } else if (debug_view == MATERIAL_DEBUG_SPECULAR) {
            return vec3<f32>(surface.specular);
        } else if (debug_view == MATERIAL_DEBUG_AMBIENT_OCCLUSION) {
            return vec3<f32>(surface.ambient_occlusion);
        } else if (debug_view == MATERIAL_DEBUG_DISPLACEMENT) {
            return vec3<f32>(clamp(relief_height_meters / 0.05, 0.0, 1.0));
        } else if (debug_view == MATERIAL_DEBUG_ROUGHNESS) {
            return vec3<f32>(surface.roughness);
        } else if (debug_view == MATERIAL_DEBUG_NORMAL) {
            return surface.normal * 0.5 + vec3<f32>(0.5);
        } else if (debug_view == MATERIAL_DEBUG_EMISSION) {
            return max(surface.emission, vec3<f32>(0.0));
        } else if (debug_view == MATERIAL_DEBUG_LOD) {
            let lod = pattern_debug_lod(hit.material, pattern);
            return vec3<f32>(lod, 1.0 - lod, 0.05);
        }
    }
    let albedo = srgb_decode(surface.base_color);

    var sun_visibility = 0.0;
    let sun_facing = dot(normal, lighting.sun_direction);
    if (sun_facing > 0.0) {
        let shadow_origin = shadow_ray_origin(hit, ray_origin, ray_direction, normal)
            + geometric_normal * displacement_offset_voxels;
        sun_visibility = trace_shadow_visibility(shadow_origin, lighting.sun_direction);
        // P2 — relief self-shadowing: plates shade their own joints and the
        // ground behind them. Direct sun only; ambient and GI are untouched.
        sun_visibility = sun_visibility * pattern_parallax_sun_shadow(
            hit.material, pattern, march.height_meters, geometric_normal,
            lighting.sun_direction, brickmap.voxel_size_meters, animation);
    }
    // `sun_transmission` is the transmittance of the sun's own path through any
    // medium in front of it (E6): exactly vec3(1.0) for every ray in air, so the
    // multiply is the float identity and this stays bit-identical off the water
    // path, and the per-channel attenuation of the water above a submerged surface
    // when there is one.
    // The cloud deck's transmittance along the sun axis, from the shadow map. Folded into the
    // existing multiply rather than added as a second lighting term: a cloud shadow IS reduced
    // sun visibility, and treating it as anything else would double-count the ambient a
    // shadowed surface still receives.
    //
    // Costs one texture fetch, and returns exactly 1.0 when the deck is off — so this stays
    // bit-identical with clouds disabled, the same property `sun_transmission` has off water.
    // Voxel units -> world metres, the same conversion CAGI applies before this lookup. The
    // shadow map is indexed in world space because it is world-anchored; handing it voxel
    // coordinates would scale the whole cloud shadow by `voxel_size_meters`.
    let surface_voxels = ray_origin + ray_direction * hit.distance
        + geometric_normal * displacement_offset_voxels;
    let cloud_sun_visibility = cloud_shadow_at(surface_voxels * brickmap.voxel_size_meters);
    let n_dot_l = max(dot(normal, lighting.sun_direction), 0.0);
    let sun_irradiance = lighting.sun_color_intensity.rgb * lighting.sun_color_intensity.w
        * sun_visibility * sun_transmission * cloud_sun_visibility;
    let sun = sun_irradiance * n_dot_l;
    var ambient: AmbientEstimate;
    ambient.factor = 1.0;
    ambient.sky_radiance = vec3<f32>(0.0, 0.0, 0.0);
    ambient.sky_weight = 0.0;
    if (AO_MODE != AO_MODE_OFF) {
        ambient = ambient_estimate(hit, ray_origin, ray_direction, normal, pixel,
                                   max(sun_facing, 0.0) * sun_visibility);
    }
    ambient.factor = ambient.factor * surface.ambient_occlusion;
    let indirect = indirect_light(hit, ray_origin, ray_direction, normal, ambient);
    let view_direction = normalize(-ray_direction);
    let n_dot_v = max(dot(normal, view_direction), 0.0);
    let half_direction = normalize(lighting.sun_direction + view_direction);
    let n_dot_h = max(dot(normal, half_direction), 0.0);
    let v_dot_h = max(dot(view_direction, half_direction), 0.0);
    let roughness = clamp(surface.roughness, 0.04, 1.0);
    let alpha = max(roughness * roughness, 0.0025);
    let fresnel_view = pbr_fresnel_schlick(n_dot_v, vec3<f32>(surface.specular));
    let fresnel_light = pbr_fresnel_schlick(v_dot_h, vec3<f32>(surface.specular));
    let distribution = pbr_distribution_ggx(n_dot_h, alpha);
    let visibility = pbr_visibility_smith(n_dot_v, n_dot_l, roughness);
    let specular_brdf = distribution * visibility * fresnel_light
        / max(4.0 * n_dot_v * n_dot_l, 1e-5);
    let direct_specular = sun_irradiance * n_dot_l * specular_brdf;
    let diffuse_weight = vec3<f32>(1.0) - fresnel_view;
    // E5: an emitter's own radiance is ADDED, not modulated by albedo or by any
    // occlusion term — the surface is a source, so it looks the same lit or
    // shadowed. gi_params.w scales it in step with what the CA injects, so the
    // block and the light it casts stay consistent when the scale moves.
    // S2: patterned emission, for a surface whose glow is not uniform — embers in
    // rock, a rune in a wall. Identical to the row's flat emission with the lever
    // off, and identical on every row that authors no emission layer.
    let emission = surface.emission * lighting.gi_params.w;
    let bounce = water_bounce_light(hit, ray_origin, ray_direction, normal);
    return albedo * (diffuse_weight * (sun + indirect + bounce))
        + direct_specular + emission;
}

// ---- E6: the water model, composed ------------------------------------------
//
// The optics themselves (Fresnel, Snell, Beer-Lambert, the medium march) live in
// `water.wgsl`; this section is how they become a pixel. It folds away entirely
// at WATER_MODE = WATER_MODE_OPAQUE, which is the isolation rule's requirement:
// with water off the shading pass is the E4 renderer.
//
// Structure, and why there is no recursion (WGSL has none): three levels, each
// strictly calling the one below it.
//
//   shade_hit              dispatch: liquid -> the water model, else shade_surface
//   water_surface_radiance the first interface, from above: one mirror ray + the
//                          refracted march, mixed by Fresnel
//   water_medium_radiance  inside the liquid: a LOOP of marches (never recursion)
//                          — extinction, in-scatter, the bed, Snell's window
//   shade_secondary        the terminal for every secondary hit: full shading,
//                          except that water hit AGAIN gets the zero-ray
//                          Fresnel-tint approximation instead of splitting
//
// So the ray budget is bounded by construction: one mirror ray per water surface
// seen directly, WATER_BOUNCES marches through the body, and at most one escape
// ray per interface crossed. Nothing a secondary ray finds can start a new split.

// The DOWNWELLING irradiance on a horizontal surface: the sun's radiance times its
// elevation cosine, plus the sky hemisphere. This is the light that enters a body
// of water from above, and it is the only light source the medium model has.
//
// Two documented simplifications: it is uniform inside the body (no attenuation
// with depth below the surface, which would need the local surface height) and
// unshadowed (one evaluation per pixel, not per point along the ray).
// Evaluated at the CAMERA's column for the cloud terms, deliberately.
//
// The ambient contract now needs a position, and threading a real per-point one through the water
// chain would mean rethreading `water_in_scattered_radiance` and `water_cheap_surface_radiance`
// and their five call sites inside the reflection and refraction paths — measured E6 code, for a
// quantity this function *already* documents as one evaluation per pixel rather than per point
// along the ray. Using the camera column keeps the approximation at exactly the level the rest of
// this function is written to, instead of making one term precise and leaving the others coarse.
//
// The visible consequence: a distant pool takes the cloud cover above the viewer rather than above
// itself. Correct per-point water shadowing is tracked as its own stage.
fn water_downwelling_radiance() -> vec3<f32> {
    let cloud_sun = cloud_shadow_at(atmosphere.camera_position);
    return lighting.sun_color_intensity.rgb * lighting.sun_color_intensity.w
        * max(lighting.sun_direction.y, 0.0) * cloud_sun
        + ambient_light(atmosphere.camera_position, vec3<f32>(0.0, 1.0, 0.0));
}

// The radiance a ray picks UP over a path through the medium — the in-scattered
// term, and the reason deep water is blue where no bottom is visible at all.
//
// **Derived, not painted** (Pascal, 2026-07-31: *"water shouldn't have a colour
// really .. water blocks light coming in"*). The closed form of single scattering
// through a uniform medium with constant source radiance J is
//
//     L_in = (sigma_s / sigma_t) * J * (1 - exp(-sigma_t * d))
//
// i.e. the single-scattering ALBEDO times the source times the same
// `1 - transmittance` the absorption already gives us. The colour is therefore
// `scattering / extinction` — for water ~(0.009, 0.25, 0.75), deeply blue with
// almost no red — which comes out of the material's two coefficients rather than
// out of anything anyone chose. The previous implementation used the water row's
// diffuse ALBEDO here, which is a surface-reflectance quantity standing in for a
// volume colour: paint, and the reason the medium read teal no matter what the
// light was doing.
fn water_in_scattered_radiance(water_material: u32, transmittance: vec3<f32>) -> vec3<f32> {
    return water_single_scattering_albedo(water_material) * water_downwelling_radiance()
        * (vec3<f32>(1.0, 1.0, 1.0) - transmittance);
}

// Transmittance of the SUN's OWN path through the liquid down to a submerged
// point — so the bed darkens, and reddens away, with depth for the right reason.
//
// Pascal asked for exactly this (*"the distance it travels the less light comes
// down so ... the block at the bottom become darker"*). `WATER_SUN_THROUGH_LIQUID`
// lets the sun REACH a submerged surface at all; this is what makes it arrive
// dimmer and bluer the deeper the surface is. One bounded march per submerged
// shaded point, and the march runs from the surface point toward the sun, so its
// length is the depth divided by the sun's elevation sine.
fn water_sun_transmission(surface_point: vec3<f32>, water_material: u32) -> vec3<f32> {
    if (!WATER_SUN_THROUGH_LIQUID) {
        return vec3<f32>(1.0, 1.0, 1.0);
    }
    let medium = water_medium_march(surface_point, lighting.sun_direction);
    // E7: caustics ride HERE rather than in a pass of their own, because they are a
    // redistribution of the sun's own path through the surface — the same path this
    // transmittance integrates — and this march has already reached that surface, so the
    // entry point and the depth come free. See `water_caustic_gain`.
    return water_transmittance(water_material, medium.distance_voxels)
        * water_caustic_gain(medium, water_material);
}

// Terminal radiance of a SECONDARY ray's hit. Ordinary surfaces get the full
// shading path (sun, shadow ray, AO, CAGI) — the E6 requirement that reflections
// and refractions see GI-lit terrain. A liquid gets the zero-ray Fresnel-tint
// approximation: this is the recursion budget's hard stop, and it is why a
// reflection that lands on another pool costs nothing extra.
fn shade_secondary(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
                   pixel: vec2<f32>, sun_transmission: vec3<f32>) -> vec3<f32> {
    let surface = shade_surface(hit, ray_origin, ray_direction, pixel, sun_transmission);
    if (WATER_MODE == WATER_MODE_OPAQUE || !material_is_liquid(hit.material)) {
        return surface;
    }
    let normal = hit_normal(hit);
    let fresnel = fresnel_schlick(max(-dot(ray_direction, normal), 0.0),
                                  material_index_of_refraction(hit.material));
    return mix(surface, sky_color(reflect(ray_direction, normal)), fresnel);
}

// A point just INSIDE the liquid behind a water hit's face — the refracted ray's
// origin. Built from `shadow_ray_origin` (whose integer-anchored reconstruction is
// what keeps secondary rays acne-free at large t) by stepping back through the
// face instead of off it, so the two origins are the same point plus or minus one
// SHADOW_BIAS and the existing function stays untouched.
fn water_interior_origin(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
                         normal: vec3<f32>) -> vec3<f32> {
    return shadow_ray_origin(hit, ray_origin, ray_direction, normal)
        - normal * (2.0 * SHADOW_BIAS);
}

// The CHEAP shading of a submerged surface: albedo x downwelling x the face's own
// share of it, with NO shadow ray, NO ambient occlusion and NO light-volume sample.
//
// This is the stand-in half of `WATER_TIR_STANDIN`. It exists because the
// expensive part of a second water bounce is not the march, it is shading what the
// march finds through the full path — and the full path's dominant cost underwater
// is precisely the sun shadow ray, which has to walk metres of water
// (`WATER_SUN_THROUGH_LIQUID`, +77% measured). Dropping it keeps the GEOMETRY,
// which is the whole point: structure instead of a constant.
//
// S2 deliberately does NOT run the pattern stack here, and it is the one shading
// site that skips it. This path has already dropped the shadow ray, the ambient
// occlusion and the light-volume sample; a per-hit generator evaluation on top of
// that would be the most expensive thing left in the cheapest path in the renderer,
// to add detail to a surface being viewed through metres of water. It also has no
// ray to build a sample from — the caller hands it a hit from its own march — so
// keeping it on S1's per-face value is honest rather than incidental.
fn water_cheap_surface_radiance(hit: Hit, water_material: u32) -> vec3<f32> {
    let normal = hit_normal(hit);
    let albedo = srgb_decode(material_face_albedo(hit.material, hit.axis, hit.axis_sign));
    // Downwelling light arrives from above, so a face's share of it is its own
    // up-facing cosine — floored at a quarter so a vertical pool wall is dim rather
    // than black, which is what the real multiply-scattered field does.
    let up_facing = max(normal.y, 0.0) * 0.75 + 0.25;
    return albedo * water_downwelling_radiance() * up_facing;
}

// The stand-in for the mirrored view OUTSIDE Snell's window: one more medium march,
// shaded cheaply. Never recurses — a TIR inside the stand-in is where it stops.
fn water_mirror_standin(origin: vec3<f32>, direction: vec3<f32>,
                        water_material: u32) -> vec3<f32> {
    let medium = water_medium_march(origin, direction);
    let transmittance = water_transmittance(water_material, medium.distance_voxels);
    var radiance = water_in_scattered_radiance(water_material, transmittance);
    if (medium.kind == WATER_MEDIUM_SOLID) {
        radiance = radiance
            + transmittance * water_cheap_surface_radiance(medium.hit, water_material);
    }
    return radiance;
}

// What a ray leaving the medium into AIR sees, looking along `escape_direction`
// from the exit point. Shared by both underwater interface modes, which differ only
// in the DIRECTION they hand it (bent by Snell, or the ray's own).
//
// Only the modes that own transmission trace it; the others take the analytic sky,
// which is what keeps WATER_MODE_FRESNEL_TINT at zero secondary rays even
// underwater, where the primary ray has no choice but to march.
fn water_escaped_radiance(medium: WaterMedium, escape_direction: vec3<f32>,
                          pixel: vec2<f32>) -> vec3<f32> {
    if (!WATER_TRACES_REFRACTION) {
        return sky_color(escape_direction);
    }
    let escape_origin = medium.exit_point - medium.exit_normal * SHADOW_BIAS;
    let above = trace(escape_origin, escape_direction, MAX_TRACE_DISTANCE, false);
    if (above.material == 0u) {
        return sky_color(escape_direction);
    }
    return shade_secondary(above, escape_origin, escape_direction, pixel, WATER_NO_MEDIUM);
}

// Radiance arriving from inside a body of liquid, entered at `entry_origin`
// travelling along `entry_direction` (both voxel units). Serves BOTH callers: the
// refracted ray of a surface seen from above, and the primary ray of a camera
// that is itself underwater.
//
// A loop over up to WATER_BOUNCES marches, carrying a running `throughput`:
//
//   - every march contributes the in-scattered radiance over that segment at the
//     current throughput and then multiplies the throughput by the transmittance;
//   - a march that ends on terrain shades it — with the sun attenuated by ITS own
//     path through the water, so the bed darkens and reddens away with depth — and
//     stops;
//   - a march that ends at the surface from below is the interface, and what
//     happens there is `WATER_UNDERWATER_INTERFACE`. Under the shipped
//     `transparent` mode the ray passes straight out and the function RETURNS; under
//     `fresnel` it is Snell's window — inside the critical angle the ray refracts
//     out and sees the sky or the shore, outside it `refract_at` reports total
//     internal reflection and the whole throughput mirrors back down;
//   - when the bounce budget runs out while still wet, `WATER_TIR_FALLBACK` decides
//     what the remaining throughput buys. This was the E6 gate failure: with a flat
//     constant there, tilting the head underwater filled most of the screen with ONE
//     COLOUR, because the window is only a ~97-degree cone.
//
// **Inert under the shipped `transparent` interface:** every branch above returns
// inside the FIRST iteration (solid, murk limit, or straight out through the
// surface), so `WATER_BOUNCES` and `WATER_TIR_FALLBACK` have no effect from below —
// there is no mirror to bounce and no region outside a window. Both stay levered
// because they are exactly what the `fresnel` interface needs, and the overlay greys
// them out rather than offering dead dials.
fn water_medium_radiance(entry_origin: vec3<f32>, entry_direction: vec3<f32>,
                         water_material: u32, pixel: vec2<f32>) -> vec3<f32> {
    // The medium's OWN index (material.rs's authored column) pulled toward 1.0 by
    // the runtime window-width dial, so oil or honey would bend differently through
    // the same code.
    let bending_index = water_bending_index(water_material);
    var radiance = vec3<f32>(0.0, 0.0, 0.0);
    var throughput = vec3<f32>(1.0, 1.0, 1.0);
    var origin = entry_origin;
    var direction = entry_direction;

    for (var bounce = 0u; bounce < WATER_BOUNCES; bounce = bounce + 1u) {
        let medium = water_medium_march(origin, direction);
        let transmittance = water_transmittance(water_material, medium.distance_voxels);
        radiance = radiance
            + throughput * water_in_scattered_radiance(water_material, transmittance);
        throughput = throughput * transmittance;

        if (medium.kind == WATER_MEDIUM_SOLID) {
            let normal = hit_normal(medium.hit);
            let surface_point = shadow_ray_origin(medium.hit, origin, direction, normal);
            let sun_transmission = water_sun_transmission(surface_point, water_material);
            return radiance
                + throughput * shade_secondary(medium.hit, origin, direction, pixel,
                                               sun_transmission);
        }
        if (medium.kind == WATER_MEDIUM_LIMIT) {
            return radiance; // murk horizon or the world's edge: nothing beyond
        }

        // The surface, from below.
        if (WATER_UNDERWATER_INTERFACE == WATER_INTERFACE_TRANSPARENT) {
            // Fully transmissive and UNBENT (E6 step 3): the ray keeps its own
            // direction, so there is no critical angle, no mirror and no window —
            // just the world above, dimmed and tinted by the water it already
            // travelled. Returns here, which is why the bounce loop cannot iterate
            // a second time from below under this mode.
            return radiance + throughput * water_escaped_radiance(medium, direction, pixel);
        }
        let refraction = refract_at(direction, medium.exit_normal,
                                    bending_index / WATER_AIR_INDEX);
        var fresnel = 1.0; // total internal reflection mirrors everything
        if (!refraction.total_internal_reflection) {
            fresnel = fresnel_schlick(refraction.cos_incidence,
                                      material_index_of_refraction(water_material));
            // Inside Snell's window.
            let escaped = water_escaped_radiance(medium, refraction.direction, pixel);
            radiance = radiance + throughput * (1.0 - fresnel) * escaped;
        }
        throughput = throughput * fresnel;
        if (fresnel <= 0.0) {
            return radiance; // nothing mirrored back down — no budget to spend
        }
        // Mirror back down and keep marching (the exit normal points into the
        // liquid, so `reflect` turns the ray around and the bias re-enters it).
        direction = reflect(direction, medium.exit_normal);
        origin = medium.exit_point + medium.exit_normal * (2.0 * SHADOW_BIAS);
    }
    // The budget is spent and the ray is still inside the medium — which for a view
    // from below is almost the whole screen outside Snell's window.
    if (WATER_TIR_FALLBACK == WATER_TIR_STANDIN) {
        return radiance + throughput * water_mirror_standin(origin, direction, water_material);
    }
    // WATER_TIR_FLAT: the documented negative. Kept selectable so the bench can
    // measure what the fix is worth, and so the failure it caused stays visible.
    return radiance
        + throughput * water_in_scattered_radiance(water_material, vec3<f32>(0.0, 0.0, 0.0));
}

// A water surface hit by a ray travelling through AIR: split into a mirror ray
// and a refracted march, weighted by Fresnel. Grazing angles land near F = 1 and
// read as a mirror; steep angles land near F = 0.02 and read as glass.
//
// The un-traced halves are not left black — each has a cheap stand-in, which is
// what makes the four modes a clean cost ladder rather than four different looks:
// the mirror term falls back to the analytic sky function (which already carries
// the sun glint, so a grazing water surface still glares), and the transmitted
// term falls back to the surface's own diffuse shading.
// W2 — the two normals, and why there are two.
//
// `geometric` is the voxel face. `optics` is the face plus the wave field's gradient
// (`water_surface_normal`), and it is what makes a water pixel look like water: a flat
// normal is a PERFECT mirror, which is why this surface had no sun glitter and a
// reflected shoreline that never moved.
//
// The split is not stylistic. `geometric` still does every job that is about GEOMETRY —
// offsetting a secondary ray off the face it is escaping (`shadow_ray_origin`,
// `water_interior_origin`) and bounding where those rays may point. `optics` does every
// job that is about LIGHT — Fresnel's weight, the mirror direction, Snell's bend. Mixing
// them up self-intersects rays against the face they just left.
fn water_surface_radiance(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
                          pixel: vec2<f32>) -> vec3<f32> {
    let geometric = hit_normal(hit);

    // ONE robust reconstruction of the point on the face, used for both the wave phase
    // and the mirror ray's origin. `ray_origin + ray_direction * hit.distance` alone
    // carries accumulated float error at large t, which for a wave field would read as
    // phase jitter on distant water; `shadow_ray_origin` clamps into the voxel and snaps
    // the face coordinate, and its bias is along the normal — i.e. it moves only Y on a
    // top face, and the wave field reads XZ.
    let face_point = shadow_ray_origin(hit, ray_origin, ray_direction, geometric);
    let optics = water_surface_normal(hit, face_point);

    let medium_index = material_index_of_refraction(hit.material);
    let fresnel = fresnel_schlick(max(-dot(ray_direction, optics), 0.0), medium_index);

    // The reflected ray must stay ABOVE the face it leaves, or it starts inside the water
    // body, `trace` hits liquid at t ~ 0 and the pixel goes black — the classic
    // wave-normal sparkle-of-black-dots. The steepness cap bounds how bad this gets (the
    // normal tilts at most atan(0.35) = 19.3 degrees, so a grazing ray is thrown at most
    // 38.6 degrees below the plane) and `water_lift_reflection` closes the remainder. It
    // is inert on a flat surface, which is what keeps the no-waves image untouched.
    let reflected_direction =
        water_lift_reflection(reflect(ray_direction, optics), geometric, optics);

    var mirrored = sky_color(reflected_direction);
    if (WATER_TRACES_REFLECTION && water_ray_is_worth_tracing(fresnel)) {
        // Biased along the GEOMETRIC normal, because that offset is about escaping the
        // face rather than about where the light goes.
        let mirror = trace(face_point, reflected_direction, MAX_TRACE_DISTANCE, false);
        if (mirror.material != 0u) {
            mirrored = shade_secondary(mirror, face_point, reflected_direction, pixel,
                                       WATER_NO_MEDIUM);
        }
    }

    var transmitted = shade_surface(hit, ray_origin, ray_direction, pixel, WATER_NO_MEDIUM);
    if (WATER_TRACES_REFRACTION && water_ray_is_worth_tracing(1.0 - fresnel)) {
        // Entering the denser medium can never totally reflect, so the
        // `total_internal_reflection` branch of `refract_at` is unreachable here.
        //
        // The refracted ray needs no counterpart to the mirror's guard, and the
        // steepness cap is why: refraction bends TOWARD the normal, so with eta = 0.75
        // the transmitted ray sits within 48.6 degrees of -optics, and optics is itself
        // within 19.3 degrees of straight down. 48.6 + 19.3 = 67.9 degrees from
        // vertical, so it is always comfortably below the face.
        let refraction = refract_at(ray_direction, optics,
                                    WATER_AIR_INDEX / medium_index);
        let interior_origin = water_interior_origin(hit, ray_origin, ray_direction, geometric);
        transmitted = water_medium_radiance(interior_origin, refraction.direction,
                                            hit.material, pixel);
    }
    return mix(transmitted, mirrored, fresnel);
}

// The shading dispatch: a liquid is a medium boundary, everything else is a
// surface. One compare, folded away when water is off.
fn shade_hit(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
             pixel: vec2<f32>) -> vec3<f32> {
    if (WATER_MODE != WATER_MODE_OPAQUE && material_is_liquid(hit.material)) {
        return water_surface_radiance(hit, ray_origin, ray_direction, pixel);
    }
    return shade_surface(hit, ray_origin, ray_direction, pixel, WATER_NO_MEDIUM);
}

// ---- Entry point ------------------------------------------------------------

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) invocation: vec3<u32>) {
    let texture_size = textureDimensions(output);
    if (invocation.x >= texture_size.x || invocation.y >= texture_size.y) {
        return;
    }

    // Pixel center → NDC (x right, y up) → ray through the camera basis.
    let pixel = vec2<f32>(f32(invocation.x) + 0.5, f32(invocation.y) + 0.5);
    let ndc = vec2<f32>(
        pixel.x / camera.resolution.x * 2.0 - 1.0,
        1.0 - pixel.y / camera.resolution.y * 2.0,
    );
    let direction = normalize(
        camera.forward + ndc.x * camera.right_scaled + ndc.y * camera.up_scaled);
    // Camera lives in world meters; traversal runs in voxel units.
    let origin = camera.position / brickmap.voxel_size_meters;

    var color = vec3<f32>(0.0, 0.0, 0.0);
    // E6 — the underwater camera. Tested on the primary ray's OWN origin, which
    // is why it is true both for the walking body's submerged head (E2b's
    // `head_submerged`) and for a fly camera that flew into a pool: there is one
    // condition, not two, and it cannot disagree with the geometry the ray then
    // marches. `trace` would answer "the water voxel you are standing in", so the
    // submerged case takes the medium march instead of the two-level traversal.
    if (WATER_MODE != WATER_MODE_OPAQUE && point_is_submerged(origin)) {
        let eye_material = voxel_material_at(vec3<i32>(floor(origin)));
        color = water_medium_radiance(origin, direction, eye_material, pixel);
    } else {
        let hit = trace(origin, direction, MAX_TRACE_DISTANCE, false);
        if (hit.material == 0u) {
            color = sky_color_at_distance(
                direction,
                MAX_TRACE_DISTANCE * brickmap.voxel_size_meters,
            );
        } else {
            color = shade_hit(hit, origin, direction, pixel);
        }
    }
    // Linear radiance -> tonemap -> sRGB encode. The ENCODE IS UNCONDITIONAL and the
    // tonemap is the only thing the output mode changes, so every mode hands the blit
    // and egui the same kind of value and only the highlight rolloff differs. One
    // variable between SDR and HDR is also what makes the difference reviewable.
    // EXPOSURE FIRST, then the tonemap. The order is the whole point: exposure decides
    // where the scene sits, the tonemap only shapes what happens near and above white.
    // Folding them together is what made a curve change read as a brightness change.
    color = color * lighting.output_params.w;
    color = srgb_encode(apply_tonemap(color, lighting.output_params.x,
                                     u32(lighting.output_params.y),
                                     lighting.output_params.z));
    textureStore(output, vec2<i32>(i32(invocation.x), i32(invocation.y)),
                 vec4<f32>(color, 1.0));
}
