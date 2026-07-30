// dda.wgsl — the SHADING pass: primary rays, one sun shadow ray per hit,
// ambient occlusion (E1 ray-traced / E1b analytic), E4 CAGI indirect light,
// Reinhard tonemap. Concatenated AFTER `world.wgsl` (the shared traversal
// core, which owns the brickmap bindings and the traversal/shadow levers) and
// `cagi_volume.wgsl` (the shared light-volume bindings + sampler), so this file
// holds only what is specific to turning a camera ray into a pixel.
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
// voxel-sandbox's mesh.rs). This shader decodes them to linear (pow-2.2
// approximation — cheap and self-consistent with the encode below; the exact
// piecewise curve buys nothing at 8 bits), does ALL lighting math in linear
// (sun term + indirect term), applies a one-line Reinhard tonemap
// (Stage 4 refines the curve), then re-encodes to sRGB before textureStore.
// The storage-texture/blit contract is unchanged: the blit still receives
// sRGB-encoded bytes and undoes the swapchain's re-encode.
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

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(6) var output: texture_storage_2d<rgba8unorm, write>;

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
//   2  analytic 3x3x3 — zero rays, hemisphere-weighted occupancy of the 26
//      voxels around the face-front voxel (wider than corner AO, flat per
//      voxel);
//   3  off.
const AO_MODE_RAY_TRACED: u32 = 0u;
const AO_MODE_ANALYTIC_CORNER: u32 = 1u;
const AO_MODE_ANALYTIC_NEIGHBORHOOD: u32 = 2u;
const AO_MODE_OFF: u32 = 3u;
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

// Simple Reinhard: maps [0, inf) radiance into [0, 1). Stage 4 refines this.
fn tonemap_reinhard(color: vec3<f32>) -> vec3<f32> {
    return color / (vec3<f32>(1.0, 1.0, 1.0) + color);
}

// ---- Shading ----------------------------------------------------------------

// Warm horizon fading into a blue zenith, with a sun glow. Linear radiance:
// the constants are the Stage 1 sRGB sky pushed through decode + inverse
// Reinhard (x^2.2 / (1 - x^2.2)) so the sky looks unchanged after the new
// tonemap + encode.
fn sky_color(direction: vec3<f32>) -> vec3<f32> {
    let horizon = vec3<f32>(2.55, 1.37, 0.63);
    let zenith = vec3<f32>(0.08, 0.31, 2.55);
    let elevation = clamp(direction.y * 0.5 + 0.5, 0.0, 1.0);
    var sky = mix(horizon, zenith, smoothstep(0.42, 0.78, elevation));
    let sun_amount = pow(max(dot(direction, lighting.sun_direction), 0.0), 64.0);
    sky = sky + lighting.sun_color_intensity.rgb * lighting.sun_color_intensity.w * sun_amount;
    return sky;
}

// Face normal from the DDA hit record (axis-aligned, opposing the ray).
fn hit_normal(hit: Hit) -> vec3<f32> {
    var normal = vec3<f32>(0.0, 0.0, 0.0);
    if (hit.axis == 0u) {
        normal.x = -hit.axis_sign;
    } else if (hit.axis == 1u) {
        normal.y = -hit.axis_sign;
    } else {
        normal.z = -hit.axis_sign;
    }
    return normal;
}

// Hemisphere ambient: sky color from above, warm ground bounce from below,
// mixed by normal.y. This is what lights shadowed pixels (they must stay
// readable, never black) and it replaces Stage 1's per-face tint — the
// vertical gradient comes from the hemisphere, the horizontal differentiation
// from the sun angle.
fn ambient_light(normal: vec3<f32>) -> vec3<f32> {
    let sky_weight = normal.y * 0.5 + 0.5;
    return mix(lighting.ground_ambient.rgb, lighting.sky_ambient.rgb, sky_weight)
        * lighting.sky_ambient.w;
}

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
// [0, 1) from pixel coordinates ONLY. Deterministic across frames — a still
// camera shows an identical image every frame, matching the engine's
// noiseless identity (no temporal accumulation, no per-frame randomness).
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
        let occluder = trace(surface_origin, direction, AO_MAX_DISTANCE);
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
            sky_sum += ambient_light(direction);
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
// rule (this is the signal voxel-sandbox bakes into its mesh vertex colors, so
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

// Zero-ray occlusion from the 26 voxels around the FACE-FRONT voxel
// (hit.voxel + normal), each solid neighbour weighted by how much of the
// surface hemisphere it blocks: (0.5 + 0.5 * cos) / distance, normalized by
// the same weight sum over all 26 offsets so a fully enclosed face reads 1.0.
//
// Centering on the face-front voxel rather than the hit voxel is deliberate:
// centered on the hit voxel, the surface's OWN in-plane neighbours (always
// solid on any flat ground) carry cos = 0 weight and darken open terrain by
// ~45% — the classic analytic over-darkening failure. One voxel out, that
// layer sits at cos < 0 and the same flat ground reads ~9%.
fn analytic_neighborhood_occlusion(hit: Hit, normal: vec3<f32>) -> f32 {
    let center = hit.voxel + vec3<i32>(normal);
    var occlusion_sum = 0.0;
    var weight_sum = 0.0;
    for (var offset_z = -1; offset_z <= 1; offset_z = offset_z + 1) {
        for (var offset_y = -1; offset_y <= 1; offset_y = offset_y + 1) {
            for (var offset_x = -1; offset_x <= 1; offset_x = offset_x + 1) {
                if (offset_x == 0 && offset_y == 0 && offset_z == 0) {
                    continue;
                }
                let offset = vec3<f32>(f32(offset_x), f32(offset_y), f32(offset_z));
                let inverse_length = 1.0 / length(offset);
                let weight = (0.5 + 0.5 * dot(offset * inverse_length, normal))
                    * inverse_length;
                weight_sum += weight;
                if (voxel_occupied(center + vec3<i32>(offset_x, offset_y, offset_z))) {
                    occlusion_sum += weight;
                }
            }
        }
    }
    return occlusion_sum / weight_sum;
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
    } else if (AO_MODE == AO_MODE_ANALYTIC_NEIGHBORHOOD) {
        occlusion = analytic_neighborhood_occlusion(hit, normal);
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
// whatever share of the hemisphere ambient the runtime floor keeps — the floor
// exists because a coarse volume in a sealed pocket legitimately converges to
// black, and a voxel engine with pitch-black interiors is unreadable.
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
    let hemisphere = ambient_light(normal);
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

// Linear-space shading: albedo * (sun lambert * visibility + indirect * AO).
// One shadow ray per hit through `trace_shadow_visibility` (binary in hard mode,
// a penumbra factor in soft mode); faces pointing away from the sun skip the
// trace outright (their lambert term is zero anyway).
//
// E4 composition contract, as documented since E1: occlusion multiplies the
// INDIRECT term only — never the direct sun term or its shadow ray. The
// multiply itself now lives inside `indirect_light` (see AO_MISS_RADIANCE),
// which is why this function passes the estimate down instead of scaling the
// result.
fn shade_hit(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
             pixel: vec2<f32>) -> vec3<f32> {
    let normal = hit_normal(hit);
    let albedo = srgb_decode(materials[hit.material].albedo);

    var sun_visibility = 0.0;
    let sun_facing = dot(normal, lighting.sun_direction);
    if (sun_facing > 0.0) {
        let shadow_origin = shadow_ray_origin(hit, ray_origin, ray_direction, normal);
        sun_visibility = trace_shadow_visibility(shadow_origin, lighting.sun_direction);
    }
    let sun = lighting.sun_color_intensity.rgb * lighting.sun_color_intensity.w
        * max(sun_facing, 0.0) * sun_visibility;
    var ambient: AmbientEstimate;
    ambient.factor = 1.0;
    ambient.sky_radiance = vec3<f32>(0.0, 0.0, 0.0);
    ambient.sky_weight = 0.0;
    if (AO_MODE != AO_MODE_OFF) {
        ambient = ambient_estimate(hit, ray_origin, ray_direction, normal, pixel,
                                   max(sun_facing, 0.0) * sun_visibility);
    }
    let indirect = indirect_light(hit, ray_origin, ray_direction, normal, ambient);
    return albedo * (sun + indirect);
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

    let hit = trace(origin, direction, MAX_TRACE_DISTANCE);
    var color = vec3<f32>(0.0, 0.0, 0.0);
    if (hit.material == 0u) {
        color = sky_color(direction);
    } else {
        color = shade_hit(hit, origin, direction, pixel);
    }
    // Linear radiance -> tonemap -> sRGB encode: the blit contract still
    // receives sRGB-encoded bytes.
    color = srgb_encode(tonemap_reinhard(color));
    textureStore(output, vec2<i32>(i32(invocation.x), i32(invocation.y)),
                 vec4<f32>(color, 1.0));
}
