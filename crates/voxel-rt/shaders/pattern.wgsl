// S2 — the pattern layer model: the generators, the sampling frames and the
// blends, plus the three functions that apply a row's stack.
//
// Concatenated AFTER `world.wgsl` and into the shading pass only. `world.wgsl` owns
// the row LAYOUT — `struct PatternLayer` and the flag bits — because the CAGI pass
// shares that file and `struct Material` embeds the slots; the CAGI pass bakes its
// own cell attributes and never reads the material table, so it needs none of the
// behaviour here.
//
// ## This file is a hand-mirror of `src/pattern.rs`
//
// Every function here has a namesake in that module, and `src/pattern.rs`'s tests
// pin the Rust side against hand-computed values so this side has something to be
// checked against. The mirroring is by hand and therefore the thing most likely to
// drift: if a pattern looks right in a CPU test and wrong on screen, the diff is
// between these two files and nowhere else.
//
// The integer hash is the load-bearing part of that. `lowbias32` is three
// multiplies and three shifts over `u32`, and both languages wrap `u32` multiplies
// mod 2^32 and shift `u32` logically, so the two implementations agree bit for bit
// rather than approximately.

// ---- A/B benchmark levers ---------------------------------------------------
//
// Patched by `MaterialSettings::patch_shader_source` (src/variants.rs). The
// shipped values are the ones written here, so the unpatched file IS the default
// configuration.

// The shipped path evaluates authored layers; Potato patches this const off for
// the deliberately flat fallback tier.
const MATERIAL_PATTERNS: bool = true;

// Global scale on every layer's amount, 0..1. The tier knob: it turns detail down
// everywhere without editing 26 rows, which is what a Quest preset needs.
const MATERIAL_PATTERN_STRENGTH: f32 = 1.0;

// Layers evaluated per hit, whatever the row authored. Also a tier knob — a row
// with four layers costs four generator evaluations, and this is what buys them
// back. Clamps against MAX_PATTERN_LAYERS.
const MATERIAL_PATTERN_MAX_LAYERS: u32 = 4u;

// Absolute fade-start distance in metres from the runtime registry.

// Generators. Mirrors `PatternGenerator::code`.
const PATTERN_GENERATOR_FLAT: u32 = 0u;
const PATTERN_GENERATOR_NOISE: u32 = 1u;
const PATTERN_GENERATOR_SPECKLE: u32 = 2u;

// Frames. Mirrors `PatternFrame::code`.
const PATTERN_FRAME_WORLD: u32 = 0u;
const PATTERN_FRAME_VOXEL: u32 = 1u;
const PATTERN_FRAME_FACE: u32 = 2u;

// Targets. Mirrors `PatternTarget::code`.
const PATTERN_TARGET_ALBEDO: u32 = 0u;
const PATTERN_TARGET_ROUGHNESS: u32 = 1u;
const PATTERN_TARGET_EMISSION: u32 = 2u;

// Blends. Mirrors `PatternBlend::code`.
const PATTERN_BLEND_MULTIPLY: u32 = 0u;
const PATTERN_BLEND_MIX_TO_COLOR: u32 = 1u;
const PATTERN_BLEND_ADD: u32 = 2u;

fn pattern_generator(layer: PatternLayer) -> u32 { return layer.packed & 0x7u; }
fn pattern_frame(layer: PatternLayer) -> u32 { return (layer.packed >> 3u) & 0x3u; }
fn pattern_target(layer: PatternLayer) -> u32 { return (layer.packed >> 5u) & 0x3u; }
fn pattern_blend(layer: PatternLayer) -> u32 { return (layer.packed >> 7u) & 0x3u; }
fn pattern_face_mask(layer: PatternLayer) -> u32 { return (layer.packed >> 9u) & 0x7u; }
fn pattern_octaves(layer: PatternLayer) -> u32 { return (layer.packed >> 12u) & 0x7u; }
// Texels per voxel edge, 0 = continuous. Bits 15-22, so up to 255 fits even though
// TEXEL_RUNGS stops at 32.
fn pattern_texels(layer: PatternLayer) -> u32 { return (layer.packed >> 15u) & 0xffu; }
// Bit 23: give every face its own draw. Face frame only — see pattern_variation_salt.
fn pattern_varies_per_face(layer: PatternLayer) -> bool { return (layer.packed & (1u << 23u)) != 0u; }

// Where a hit is, in every form the frames need. Built once per hit by
// `pattern_sample` below, then shared by every layer in the stack — the
// coordinate mapping is per layer (it depends on the period) but the position it
// maps is not.
struct PatternSample {
    // The hit point in world METRES, not voxel units. The traced space is voxel
    // units (dda.wgsl divides the camera position by voxel_size before tracing),
    // so this is that position scaled back up — because a period authored in
    // metres has to mean metres regardless of what VOXEL_SIZE happens to be.
    world_meters: vec3<f32>,
    voxel: vec3<i32>,
    // Face axis: 0 = x, 1 = y, 2 = z.
    axis: u32,
    // Sign of the ray along that axis. The TOP face is axis 1 with a NEGATIVE
    // sign — see `material_face_albedo` in world.wgsl for why that reads backwards.
    axis_sign: f32,
    distance_meters: f32,
}

// ---- The hash ---------------------------------------------------------------

// Chris Wellons' `lowbias32`. See the file header on why this exact function.
fn pattern_hash_u32(value: u32) -> u32 {
    var hashed = value;
    hashed = hashed ^ (hashed >> 16u);
    hashed = hashed * 0x7feb352du;
    hashed = hashed ^ (hashed >> 15u);
    hashed = hashed * 0x846ca68bu;
    hashed = hashed ^ (hashed >> 16u);
    return hashed;
}

// A 3D lattice cell hashed to 0..1.
//
// `bitcast<u32>` rather than `u32()`: the conversion has to be a two's-complement
// REINTERPRETATION so a negative coordinate hashes the same as Rust's `as u32`.
// `u32(-1)` in WGSL is a saturating value conversion and would not agree.
fn pattern_hash_cell(cell: vec3<i32>, salt: u32) -> f32 {
    let mixed = (bitcast<u32>(cell.x) * 0x27d4eb2du)
        ^ (bitcast<u32>(cell.y) * 0x9e3779b9u)
        ^ (bitcast<u32>(cell.z) * 0x85ebca6bu)
        ^ (salt * 0xc2b2ae35u);
    return f32(pattern_hash_u32(mixed)) / 4294967296.0;
}

// `3t^2 - 2t^3` on an already-clamped 0..1. Written out rather than calling
// `smoothstep(0.0, 1.0, t)` so the polynomial is visibly the same one the Rust
// reference evaluates.
fn pattern_ease(t: f32) -> f32 {
    return t * t * (3.0 - 2.0 * t);
}

// ---- Generators -------------------------------------------------------------

// Value noise: hash the eight lattice corners, ease-interpolate between them.
fn pattern_value_noise(point: vec3<f32>, salt: u32) -> f32 {
    let base = floor(point);
    let cell = vec3<i32>(base);
    let fraction = vec3<f32>(
        pattern_ease(point.x - base.x),
        pattern_ease(point.y - base.y),
        pattern_ease(point.z - base.z),
    );
    var accumulated = 0.0;
    for (var corner = 0u; corner < 8u; corner = corner + 1u) {
        let offset = vec3<u32>(corner & 1u, (corner >> 1u) & 1u, (corner >> 2u) & 1u);
        let weight_x = select(1.0 - fraction.x, fraction.x, offset.x == 1u);
        let weight_y = select(1.0 - fraction.y, fraction.y, offset.y == 1u);
        let weight_z = select(1.0 - fraction.z, fraction.z, offset.z == 1u);
        let corner_cell = cell + vec3<i32>(offset);
        accumulated = accumulated
            + weight_x * weight_y * weight_z * pattern_hash_cell(corner_cell, salt);
    }
    return accumulated;
}

// Fractal value noise, normalised back into 0..1 — so the octave count changes the
// texture without changing the contrast, and the period always names the LARGEST
// feature.
fn pattern_fractal_noise(point: vec3<f32>, octaves: u32, salt_base: u32) -> f32 {
    let count = clamp(octaves, 1u, 4u);
    var frequency = 1.0;
    var amplitude = 1.0;
    var total = 0.0;
    var normalisation = 0.0;
    for (var octave = 0u; octave < count; octave = octave + 1u) {
        total = total + amplitude * pattern_value_noise(point * frequency, salt_base ^ octave);
        normalisation = normalisation + amplitude;
        frequency = frequency * 2.0;
        amplitude = amplitude * 0.5;
    }
    return total / normalisation;
}

const PATTERN_SPECKLE_PRESENCE_SALT: u32 = 11u;
const PATTERN_SPECKLE_JITTER_X_SALT: u32 = 12u;
const PATTERN_SPECKLE_JITTER_Y_SALT: u32 = 13u;
const PATTERN_SPECKLE_JITTER_Z_SALT: u32 = 14u;
const PATTERN_SPECKLE_RADIUS_CELLS: f32 = 0.32;
const PATTERN_FLAT_SALT: u32 = 31u;

// Scattered round specks. `density` is the fraction of CELLS that carry one, not
// the fraction of area covered.
fn pattern_speckle(point: vec3<f32>, density: f32, salt_base: u32) -> f32 {
    let base = floor(point);
    let cell = vec3<i32>(base);
    if (pattern_hash_cell(cell, salt_base ^ PATTERN_SPECKLE_PRESENCE_SALT) >= density) {
        return 0.0;
    }
    // Jittered inside its cell, or the specks line up on the lattice and read as
    // a grid rather than as scatter.
    let centre = vec3<f32>(
        0.25 + 0.5 * pattern_hash_cell(cell, salt_base ^ PATTERN_SPECKLE_JITTER_X_SALT),
        0.25 + 0.5 * pattern_hash_cell(cell, salt_base ^ PATTERN_SPECKLE_JITTER_Y_SALT),
        0.25 + 0.5 * pattern_hash_cell(cell, salt_base ^ PATTERN_SPECKLE_JITTER_Z_SALT),
    );
    let offset = point - base - centre;
    let edge = clamp(1.0 - length(offset) / PATTERN_SPECKLE_RADIUS_CELLS, 0.0, 1.0);
    // Smooth rather than a hard disc, so a speck does not alias into a flickering
    // dot the moment it approaches a pixel in size.
    return pattern_ease(edge);
}

// ---- Frames -----------------------------------------------------------------

// Quantise a position in metres to the CENTRE of its texel — one sample per texel,
// which is what makes every generator blocky at once rather than needing a blocky
// variant of each. The grid is anchored at world zero and its size divides the voxel
// exactly, so a texel never straddles a voxel edge and the blocky look survives
// cross-voxel continuity.
fn pattern_snap_to_texels(meters: vec3<f32>, texels: u32,
                          voxel_size_meters: f32) -> vec3<f32> {
    if (texels == 0u) {
        return meters;
    }
    // The brick is the authoritative one-metre world voxel. The smaller value in
    // BrickmapMeta is the ray-traversal/detail-cell size.
    let world_voxel_size_meters = voxel_size_meters * BRICK_SIZE;
    let texel = world_voxel_size_meters / f32(texels);
    return floor(meters / texel) * texel + vec3<f32>(texel * 0.5);
}

// A layer's sample coordinate, in period units. The whole frame mechanism: put the
// position in the right space, snap it to the texel grid, divide by the period.
// S3 — how far this layer's pattern has drifted, in metres.
//
// The offset is QUANTISED TO THE TEXEL GRID, with no opt-out, and the reason is
// that an opt-out would not do anything. `pattern_coordinate` snaps AFTER
// subtracting this offset, so the sampled value steps a whole texel at a time
// either way; leaving the offset un-quantised produces byte-identical output and
// only costs the texel grid its alignment to world voxel boundaries. Genuinely
// continuous motion is what `texels_per_voxel = 0` already means — no grid, no
// snap, and the early return below hands the raw offset straight back.
fn pattern_drift_meters(layer: PatternLayer, velocity: vec3<f32>,
                        voxel_size_meters: f32) -> vec3<f32> {
    if (all(velocity == vec3<f32>(0.0))) {
        return vec3<f32>(0.0);
    }
    let offset = velocity * graph_animation_seconds();
    let texels = pattern_texels(layer);
    if (texels == 0u) {
        return offset;
    }
    let texel = (voxel_size_meters * BRICK_SIZE) / f32(texels);
    // WGSL `trunc` mirrors the CPU path: quantise symmetrically toward zero
    // so negative motion does not take an immediate one-texel step.
    return trunc(offset / texel) * texel;
}

fn pattern_coordinate(layer: PatternLayer, sample: PatternSample,
                      voxel_size_meters: f32, drift_meters: vec3<f32>) -> vec3<f32> {
    let period = max(layer.period_meters, 1e-4);
    let frame = pattern_frame(layer);
    let world_voxel_size_meters = voxel_size_meters * BRICK_SIZE;
    let world_voxel = sample.voxel / vec3<i32>(i32(BRICK_SIZE));
    var meters = sample.world_meters - drift_meters;
    if (frame == PATTERN_FRAME_VOXEL) {
        // Quantised to the voxel's own centre, so the generator returns ONE value
        // for the whole voxel without any generator knowing about voxels — and why
        // the texel snap is a no-op here, a centre already being one point.
        //
        // S3: drift is therefore IGNORED in this frame, and deliberately. The
        // coordinate is one point per voxel, so translating it would step the
        // whole voxel's value between neighbours rather than move a pattern
        // across it. A voxel-frame layer that wants motion wants the world or
        // face frame instead.
        meters = (vec3<f32>(world_voxel) + vec3<f32>(0.5)) * world_voxel_size_meters;
    } else if (frame == PATTERN_FRAME_FACE) {
        // Voxel-local, so the pattern repeats identically on every face — which is
        // what "about the face" means. The face's own axis keeps its local value
        // rather than being zeroed, so a 3D generator still sees three varying
        // inputs on a face that happens to be flat in one of them.
        meters = meters - vec3<f32>(world_voxel) * world_voxel_size_meters;
    }
    // Otherwise WORLD: a field the world sits in, so it flows across neighbouring
    // voxels and CANNOT tile per voxel. The default, and the continuity argument.
    let snapped = pattern_snap_to_texels(meters, pattern_texels(layer), voxel_size_meters);
    return snapped / period;
}

// A per-face hash salt, or 0 for no variation.
//
// The face frame is voxel-local, so without this it draws the IDENTICAL pattern on
// every face in the world — a visible repeat rather than detail. A salt re-rolls the
// random draw and moves NOTHING, which is why it is safe to leave on: an offset would
// slide the pattern within the face and break the texel grid's alignment to it.
//
// Only the face frame gets one. The world frame must not (a per-face salt would destroy
// the continuity that is the point of it) and the voxel frame does not need one. Zero is
// exactly the unvaried behaviour, since every generator mixes this with `^`.
fn pattern_variation_salt(layer: PatternLayer, sample: PatternSample) -> u32 {
    if (!pattern_varies_per_face(layer) || pattern_frame(layer) != PATTERN_FRAME_FACE) {
        return 0u;
    }
    let world_voxel = sample.voxel / vec3<i32>(i32(BRICK_SIZE));
    // The face index 0..5 over (axis, sign), so a world voxel's top and bottom differ too.
    var face = sample.axis * 2u;
    if (sample.axis_sign >= 0.0) {
        face = face + 1u;
    }
    return pattern_hash_u32(
        (bitcast<u32>(world_voxel.x) * 0x9e3779b9u)
        ^ (bitcast<u32>(world_voxel.y) * 0x85ebca6bu)
        ^ (bitcast<u32>(world_voxel.z) * 0xc2b2ae35u)
        ^ (face * 0x27d4eb2du)
    );
}

// The generator's raw value, 0..1, before fade, amount, face mask or blend.
fn pattern_generator_value(layer: PatternLayer, sample: PatternSample,
                           voxel_size_meters: f32, drift_meters: vec3<f32>) -> f32 {
    let point = pattern_coordinate(layer, sample, voxel_size_meters, drift_meters);
    let salt = pattern_variation_salt(layer, sample);
    let generator = pattern_generator(layer);
    if (generator == PATTERN_GENERATOR_NOISE) {
        return pattern_fractal_noise(point, pattern_octaves(layer), salt);
    }
    if (generator == PATTERN_GENERATOR_SPECKLE) {
        return pattern_speckle(point, clamp(layer.param_a, 0.0, 1.0), salt);
    }
    return pattern_hash_cell(vec3<i32>(floor(point)), salt ^ PATTERN_FLAT_SALT);
}

// ---- Fade, mask and strength ------------------------------------------------

// How much of this layer survives at this distance, 0..1. Applied to the AMOUNT, so
// a faded layer converges on the material's unpatterned base rather than on grey.
fn pattern_fade(layer: PatternLayer, distance_meters: f32) -> f32 {
    let fade_start_meters = lighting.material_params.x;
    let fade_end_meters = lighting.material_params.y;
    if (fade_end_meters <= 0.0) {
        return 1.0;
    }
    let start = fade_start_meters;
    let end = max(fade_end_meters, start);
    if (distance_meters <= start) {
        return 1.0;
    }
    if (distance_meters >= end) {
        return 0.0;
    }
    return 1.0 - pattern_ease((distance_meters - start) / (end - start));
}

// Whether this layer's face mask includes the face that was hit. Bit 0 top, 1 side,
// 2 bottom, and the top is the NEGATIVE sign on axis 1.
fn pattern_covers_face(layer: PatternLayer, axis: u32, axis_sign: f32) -> bool {
    let mask = pattern_face_mask(layer);
    if (axis != 1u) {
        return (mask & 2u) != 0u;
    }
    if (axis_sign < 0.0) {
        return (mask & 1u) != 0u;
    }
    return (mask & 4u) != 0u;
}

// The layer's effective strength at this sample: its amount, globally scaled,
// faded, and zero on a face the mask excludes.
fn pattern_strength(layer: PatternLayer, sample: PatternSample, gain: f32) -> f32 {
    if (!pattern_covers_face(layer, sample.axis, sample.axis_sign)) {
        return 0.0;
    }
    // The graph's gain multiplies the AUTHORED amount rather than replacing it,
    // and is a separate value from it: an unconnected socket is 1.0, so the
    // authored number keeps its one meaning and nothing is applied twice.
    return clamp(layer.amount, 0.0, 1.0)
        * max(gain, 0.0)
        * MATERIAL_PATTERN_STRENGTH
        * pattern_fade(layer, sample.distance_meters);
}

// ---- Blends -----------------------------------------------------------------

fn pattern_apply_color(layer: PatternLayer, base: vec3<f32>, sample: PatternSample,
                       voxel_size_meters: f32, gain: f32,
                       drift_velocity: vec3<f32>) -> vec3<f32> {
    let strength = pattern_strength(layer, sample, gain);
    if (strength <= 0.0) {
        return base;
    }
    let drift_meters = pattern_drift_meters(layer, drift_velocity, voxel_size_meters);
    let value = pattern_generator_value(layer, sample, voxel_size_meters, drift_meters);
    let blend = pattern_blend(layer);
    if (blend == PATTERN_BLEND_MIX_TO_COLOR) {
        return base + (layer.target_color - base) * strength * value;
    }
    if (blend == PATTERN_BLEND_ADD) {
        return base + layer.target_color * strength * value;
    }
    // Multiply: `1 - strength` where the value is 0, `1` where it is 1. Only ever
    // darkens, so it cannot push an albedo out of range, and turning the amount
    // down converges on the base.
    return base * (1.0 - strength * (1.0 - value));
}

fn pattern_apply_scalar(layer: PatternLayer, base: f32, sample: PatternSample,
                        voxel_size_meters: f32, gain: f32,
                        drift_velocity: vec3<f32>) -> f32 {
    let strength = pattern_strength(layer, sample, gain);
    if (strength <= 0.0) {
        return base;
    }
    let drift_meters = pattern_drift_meters(layer, drift_velocity, voxel_size_meters);
    let value = pattern_generator_value(layer, sample, voxel_size_meters, drift_meters);
    let blend = pattern_blend(layer);
    if (blend == PATTERN_BLEND_MIX_TO_COLOR) {
        return base + (layer.target_color.x - base) * strength * value;
    }
    if (blend == PATTERN_BLEND_ADD) {
        return base + layer.target_color.x * strength * value;
    }
    return base * (1.0 - strength * (1.0 - value));
}

// S2 — everything the pattern frames need about a hit, built once per hit.
//
// Two conversions happen here and nowhere else, which is the reason this exists as
// a function rather than four lines at each shading site:
//
//  * The traced space is VOXEL UNITS (dda.wgsl divides the camera position by
//    voxel_size before tracing), so `hit.distance` and the reconstructed position
//    are both in voxel units and both get scaled to metres — because a period
//    authored in metres has to mean metres.
//  * The position is CLAMPED into the hit voxel, the same way `shadow_ray_origin`
//    clamps its own reconstruction. Without it a grazing hit at large t can land a
//    hair outside the cube, and the face frame's voxel-local coordinate then falls
//    outside 0..1 — which shows up as a one-pixel seam along every silhouette.
fn pattern_sample(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>) -> PatternSample {
    let voxel_min = vec3<f32>(hit.voxel);
    let voxel_max = voxel_min + vec3<f32>(1.0, 1.0, 1.0);
    let position = clamp(ray_origin + ray_direction * hit.distance, voxel_min, voxel_max);
    var sample: PatternSample;
    sample.world_meters = position * brickmap.voxel_size_meters;
    sample.voxel = hit.voxel;
    sample.axis = hit.axis;
    sample.axis_sign = hit.axis_sign;
    sample.distance_meters = max(hit.distance, 0.0) * brickmap.voxel_size_meters;
    return sample;
}

// ---- S2: the pattern stack, applied ----------------------------------------
//
// These three functions are the seam: they are the only place a pattern layer meets
// the material table, and the only thing in this file that reads a binding.
//
// Layers apply in SLOT ORDER, each on the previous one's output, and a layer aimed
// at another target is skipped. Order matters — a mortar mask followed by grain
// grains the mortar too, and the reverse does not — so the panel lets you reorder
// rather than hiding it.
//
// Cost, and why the flag test comes first: a row with no patterns pays one bit test
// per hit. A row with layers pays one generator evaluation per layer, per hit —
// once per HIT and never once per traversal step, which is what keeps the whole
// stage off the hot loop.

// The authored layer stack, applied over bases the caller supplies.
//
// ONE loop over the slots, producing ALL THREE targets. There used to be three
// functions with a loop each, and the shape was costing far more than it looked:
// a four-layer row walked twelve slot iterations to do four slots' work, and each
// function re-read the row and re-tested the flag. Fusing them is what makes the
// per-hit cost proportional to the layers a material actually authored.
//
// The row is read THROUGH the storage binding — `materials[material].patterns[slot]`
// and not `let row = materials[material]` — for the reason `pattern_animation_drift`
// spells out one file over. Binding `row` copies the whole 256-byte Material into
// private memory, `patterns` included, and the loop then dynamically indexes that
// copy; reading through the binding is a plain buffer access the hardware offsets
// for free. That single distinction measured -1.9 ms on the saturated table.
//
// `animation` carries the per-slot gain and drift a material graph supplied;
// `pattern_animation_identity()` is the un-animated case and folds away.
struct PatternSurface {
    albedo: vec3<f32>,
    roughness: f32,
    emission: vec3<f32>,
}

fn material_pattern_surface_from_base(material: u32, sample: PatternSample,
                                      albedo_base: vec3<f32>, roughness_base: f32,
                                      emission_base: vec3<f32>,
                                      animation: PatternAnimation) -> PatternSurface {
    var surface: PatternSurface;
    surface.albedo = albedo_base;
    surface.roughness = roughness_base;
    surface.emission = emission_base;
    let flags = materials[material].flags;
    if (!MATERIAL_PATTERNS || (flags & MATERIAL_FLAG_PATTERNS) == 0u) {
        return surface;
    }
    let count = min(
        min(material_pattern_count(flags), MATERIAL_PATTERN_MAX_LAYERS),
        MAX_PATTERN_LAYERS,
    );
    let voxel_size_meters = brickmap.voxel_size_meters;
    for (var slot = 0u; slot < count; slot = slot + 1u) {
        let layer = materials[material].patterns[slot];
        let layer_target = pattern_target(layer);
        let gain = pattern_animation_gain(animation, slot);
        let drift = pattern_animation_drift(animation, slot);
        if (layer_target == PATTERN_TARGET_ALBEDO) {
            surface.albedo = pattern_apply_color(
                layer, surface.albedo, sample, voxel_size_meters, gain, drift);
        } else if (layer_target == PATTERN_TARGET_EMISSION) {
            surface.emission = pattern_apply_color(
                layer, surface.emission, sample, voxel_size_meters, gain, drift);
        } else if (layer_target == PATTERN_TARGET_ROUGHNESS) {
            surface.roughness = clamp(pattern_apply_scalar(
                layer, surface.roughness, sample, voxel_size_meters, gain, drift), 0.0, 1.0);
        }
    }
    return surface;
}

// The row's own bases, for a material with no graph. One call, so the layer loop
// above stays the single implementation.
fn material_pattern_surface(material: u32, sample: PatternSample) -> PatternSurface {
    return material_pattern_surface_from_base(
        material,
        sample,
        material_face_albedo(material, sample.axis, sample.axis_sign),
        material_face_roughness(material, sample.axis, sample.axis_sign),
        materials[material].emission,
        pattern_animation_identity(),
    );
}
