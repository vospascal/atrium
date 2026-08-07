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
// Lazily-filled texel cache for the generators.
const MATERIAL_PATTERN_CACHE: bool = true;
// Coarsen the texel grid with distance so aerial pixels share cache entries.
const MATERIAL_PATTERN_TEXEL_LOD: bool = true;

// ---- The entry-cost bisection ------------------------------------------------
//
// A MEASUREMENT INSTRUMENT, not a quality knob. Every value above 0 renders
// deliberately wrong output; the shipped path is 0 and the pixel gates only pass
// there.
//
// It exists because the cached ground residual is dominated by a layer whose
// generator computes nothing: a one-layer stack costs ~1.47 ms with the generator
// stubbed out, and the second layer only adds ~0.15 ms. So the cost is not the
// noise — it is the per-layer scaffolding around it, and no single reading tells
// you which part.
//
// The ladder is CUMULATIVE: each value stubs everything the value below it stubs,
// plus one more stage. That is what makes it a bisection rather than a set of
// opinions — the deltas between neighbours sum to the total, and the top rung
// lands on the layers-off floor, so the decomposition has a closure check the
// hardware itself enforces.
//
// Stages are removed INNERMOST FIRST, because the generator has to go before
// anything upstream of it can be priced: with the generator live it dominates,
// and with it stubbed to a constant naga would fold the entire entry path away as
// dead code. `pattern_entry_sink` is what prevents that — see its comment.
//
// The cache is deliberately absent from the ladder. It lives inside
// `pattern_generator_value`, so rung 1 removes it along with the generator, and
// every rung above measures the entry path cache-free. That is the right split:
// the cache is its own lever with its own verdict, and mixing it in would make
// each rung's delta depend on a hit rate rather than on work.
// Rungs 5-8 are the SECOND cut. The first pass through this ladder put 49-59% of
// the whole pattern path inside `pattern_coordinate` alone — an order of magnitude
// more than the fade, the salt or the snap, and more than the generator itself —
// so that one rung was split into the five things it actually does.
const PATTERN_ENTRY_ALL: u32 = 0u;
const PATTERN_ENTRY_NO_GENERATOR: u32 = 1u;
const PATTERN_ENTRY_NO_FADE: u32 = 2u;
const PATTERN_ENTRY_NO_SALT: u32 = 3u;
const PATTERN_ENTRY_NO_SNAP: u32 = 4u;
const PATTERN_ENTRY_NO_PERIOD: u32 = 5u;
const PATTERN_ENTRY_NO_TILE_FRAME: u32 = 6u;
const PATTERN_ENTRY_NO_FRAMES: u32 = 7u;
const PATTERN_ENTRY_NO_DRIFT: u32 = 8u;
const PATTERN_ENTRY_NO_COORDINATE: u32 = 9u;
const PATTERN_ENTRY_NO_STRENGTH: u32 = 10u;
const PATTERN_ENTRY_NO_LAYERS: u32 = 11u;

const MATERIAL_PATTERN_ENTRY_PROBE: u32 = 0u;

// One bit per generator code, all set. See `pattern_generator_enabled`.
const MATERIAL_PATTERN_GENERATOR_MASK: u32 = 32767u;

// P1 — parallax occlusion march over the relief height field. The derived
// normal alone reads painted-on at oblique angles: nothing slides, nothing
// occludes. The march moves the SHADING POINT to where the ray actually lands
// on the raised plates, which is what buys parallax, plates hiding what is
// behind them, and visible plate sides. Voxel silhouettes stay straight —
// this is a shading effect, not traversal geometry.
//
// OFF in the shipped default: even budgeted it multiplies the pattern-entry
// cost on every relief pixel, and that is a per-project trade the panel's
// materials group opts into, not a tax every world pays.
const MATERIAL_PARALLAX: bool = false;
// Linear search steps from the relief ceiling down to the face. The height
// field is piecewise constant, so a fixed budget plus the binary refine below
// finds plateau tops exactly; more samples only tighten thin walls.
const MATERIAL_PARALLAX_SAMPLES: u32 = 24u;
// Height-field shadow steps from the displaced point toward the sun; 0
// disables relief self-shadowing.
const MATERIAL_PARALLAX_SHADOW_SAMPLES: u32 = 16u;
// Camera distance past which the march is skipped outright. A 5 cm relief
// offset at 48 m is around a pixel: past it the march is all cost and no
// picture, and terrain is MOSTLY far pixels — this cap, not the sample
// count, is the difference between "parallax on the block in front of you"
// and "parallax on the whole landscape".
const MATERIAL_PARALLAX_END_METERS: f32 = 48.0;
// Binary refinement iterations between the last two linear samples. Fixed:
// five bisections resolve 1/32 of a step, already sub-texel everywhere.
const MATERIAL_PARALLAX_REFINE: u32 = 5u;

// Global scale on every layer's amount, 0..1. The tier knob: it turns detail down
// everywhere without editing 26 rows, which is what a Quest preset needs.
const MATERIAL_PATTERN_STRENGTH: f32 = 1.0;

// Layers evaluated per hit, whatever the row authored. Also a tier knob — a row
// with four layers costs four generator evaluations, and this is what buys them
// back. Clamps against MAX_PATTERN_LAYERS.
const MATERIAL_PATTERN_MAX_LAYERS: u32 = 4u;

// Absolute fade-start distance in metres from the runtime registry.

// Generators. Mirrors `PatternGenerator::code`. FOUR BITS — 16 codes, 15 spent.
// It was three bits and 8 until the generator library grew past it; widening it
// shifted every accessor below, which is why they are all written out from the
// same table rather than derived.
//
// Three lattice noises rather than one, deliberately, because they differ in BOTH
// axes a bench cares about:
//
//   noise    value noise    8 hashes/octave, no dots     blobby, slight axis bias
//   perlin   gradient noise 8 hashes/octave, 8 dots      classic, zero at lattice
//   simplex  gradient noise 4 hashes/octave, 4 dots, branchy  isotropic
//
// Picking between them is a measurement, not a preference, and none of them is
// obviously the winner on a GPU: simplex halves the hashes and pays for it with a
// divergent tetrahedron choice, perlin doubles the ALU per corner but stays
// branchless.
const PATTERN_GENERATOR_FLAT: u32 = 0u;
const PATTERN_GENERATOR_NOISE: u32 = 1u;
const PATTERN_GENERATOR_SPECKLE: u32 = 2u;
const PATTERN_GENERATOR_PERLIN: u32 = 3u;
const PATTERN_GENERATOR_SIMPLEX: u32 = 4u;
const PATTERN_GENERATOR_RIDGED: u32 = 5u;
const PATTERN_GENERATOR_TURBULENCE: u32 = 6u;
const PATTERN_GENERATOR_WORLEY: u32 = 7u;
const PATTERN_GENERATOR_WORLEY_EDGE: u32 = 8u;
const PATTERN_GENERATOR_WORLEY_SMOOTH: u32 = 9u;
const PATTERN_GENERATOR_WAVE: u32 = 10u;
const PATTERN_GENERATOR_CHECKER: u32 = 11u;
const PATTERN_GENERATOR_TILE_TONE: u32 = 12u;
const PATTERN_GENERATOR_TILE_EDGE: u32 = 13u;
const PATTERN_GENERATOR_EDGE_BAND: u32 = 14u;

// Tier 1b — drop octaves whose feature size has fallen below a pixel.
//
// An octave contributes detail at `period / 2^octave` metres. Once that projects
// to under a pixel it cannot be resolved, so summing it adds nothing but aliasing
// and cost — the same argument mip-mapping makes, applied to a procedural sum.
// The cutoff is therefore quality-POSITIVE, not a quality trade: it removes the
// octaves that were only ever contributing shimmer.
//
// `lighting.material_params.z` carries metres-per-pixel at one metre (the camera's
// vertical FOV over the render height), so `distance * that` is the footprint. An
// octave survives while its feature size exceeds the footprint.
const PATTERN_OCTAVE_LOD: bool = false;
// Octaves finer than this many footprints are dropped. Below 1.0 it keeps octaves
// that are already sub-pixel; above ~4 it starts visibly softening mid-distance.
const PATTERN_OCTAVE_LOD_SCALE: f32 = 2.0;

// Frames. Mirrors `PatternFrame::code`.
const PATTERN_FRAME_WORLD: u32 = 0u;
const PATTERN_FRAME_VOXEL: u32 = 1u;
const PATTERN_FRAME_FACE: u32 = 2u;
const PATTERN_FRAME_TILE: u32 = 3u;

// Targets. Mirrors `PatternTarget::code`.
const PATTERN_TARGET_ALBEDO: u32 = 0u;
const PATTERN_TARGET_ROUGHNESS: u32 = 1u;
const PATTERN_TARGET_EMISSION: u32 = 2u;

// Runtime inspection bits packed into Lighting.material_params.w by the
// windowed app. They are deliberately a renderer override: graph assets and
// material rows remain unchanged while a layer or output channel is inspected.
fn material_debug_word() -> u32 {
    return u32(max(lighting.material_params.w, 0.0));
}

fn material_debug_enabled() -> bool {
    return (material_debug_word() & 1u) != 0u;
}

fn material_debug_layer_enabled(slot: u32) -> bool {
    if (!material_debug_enabled()) {
        return true;
    }
    return (material_debug_word() & (1u << (slot + 1u))) != 0u;
}

fn material_debug_view() -> u32 {
    return (material_debug_word() >> 5u) & 0x0fu;
}

// Blends. Mirrors `PatternBlend::code`.
const PATTERN_BLEND_MULTIPLY: u32 = 0u;
const PATTERN_BLEND_MIX_TO_COLOR: u32 = 1u;
const PATTERN_BLEND_ADD: u32 = 2u;

// The packed word, field by field. Mirrors `PatternLayer::packed` in pattern.rs;
// the two are pinned against each other by a round-trip test there.
//
//   0-3   generator (4 bits, 12 of 16 used)
//   4-5   frame          6-7   target        8-9   blend
//   10-12 face mask      13-15 octaves       16-23 texels per voxel
//   24    vary per face  25    domain warp   26    edge band grows from bottom
//   27-29 displacement face mask          30 displacement normal  31 relief invert
fn pattern_generator(layer: PatternLayer) -> u32 { return layer.packed & 0xfu; }
fn pattern_frame(layer: PatternLayer) -> u32 { return (layer.packed >> 4u) & 0x3u; }
fn pattern_target(layer: PatternLayer) -> u32 { return (layer.packed >> 6u) & 0x3u; }
fn pattern_blend(layer: PatternLayer) -> u32 { return (layer.packed >> 8u) & 0x3u; }
fn pattern_face_mask(layer: PatternLayer) -> u32 { return (layer.packed >> 10u) & 0x7u; }
fn pattern_relief_face_mask(layer: PatternLayer) -> u32 {
    return (layer.packed >> 27u) & 0x7u;
}
fn pattern_relief_normal_enabled(layer: PatternLayer) -> bool {
    return (layer.packed & (1u << 30u)) != 0u;
}
// Bit 31: the relief mask is inverted — the LOW end of the generator is the
// raised one. The colour blend keeps the un-inverted value, so a layer that
// darkens where the mask is high can still raise its LIGHT texels.
fn pattern_relief_inverted(layer: PatternLayer) -> bool {
    return (layer.packed & (1u << 31u)) != 0u;
}
fn pattern_octaves(layer: PatternLayer) -> u32 { return (layer.packed >> 13u) & 0x7u; }
// Texels per voxel edge, 0 = continuous. Bits 16-23, so up to 255 fits even though
// TEXEL_RUNGS stops at 32.
fn pattern_texels(layer: PatternLayer) -> u32 { return (layer.packed >> 16u) & 0xffu; }
// Bit 24: give every face its own draw. Face frame only — see pattern_variation_salt.
fn pattern_varies_per_face(layer: PatternLayer) -> bool { return (layer.packed & (1u << 24u)) != 0u; }
// Bit 25: push the sample point through a noise field before the generator reads
// it (iq, "domain warping"). Strength rides in `param_b`.
fn pattern_warps(layer: PatternLayer) -> bool { return (layer.packed & (1u << 25u)) != 0u; }
// Bit 26: EdgeBand grows from the bottom instead of the top.
fn pattern_edge_band_from_bottom(layer: PatternLayer) -> bool {
    return (layer.packed & (1u << 26u)) != 0u;
}

// Where a hit is, in every form the frames need. Built once per hit by
// `pattern_sample` below, then shared by every layer in the stack — the
// coordinate mapping is per layer (it depends on the period) but the position it
// maps is not.
// Mirrors EXPOSURE_TOP / EXPOSURE_BOTTOM / EXPOSURE_ALL in pattern.rs.
const PATTERN_EXPOSURE_TOP: u32 = 1u;
const PATTERN_EXPOSURE_BOTTOM: u32 = 2u;
const PATTERN_EXPOSURE_ALL: u32 = 3u;

struct PatternSample {
    // The hit point in world METRES, not voxel units. The traced space is voxel
    // units (dda.wgsl divides the camera position by voxel_size before tracing),
    // so this is that position scaled back up — because a period authored in
    // metres has to mean metres regardless of what VOXEL_SIZE happens to be.
    world_meters: vec3<f32>,
    voxel: vec3<i32>,
    // Face axis: 0 = x, 1 = y, 2 = z.
    axis: u32,
    // Column exposure at the hit's one-metre block: bit 0 set when the block
    // ABOVE is empty (an exposed column top), bit 1 when the one BELOW is.
    // Filled from the occupancy grid by `pattern_sample`; the CPU mirror has
    // no world and defaults to fully exposed. The edge band reads it so a lip
    // only draws where the column actually ends.
    exposure: u32,
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

// How many octaves are worth summing at this distance — Tier 1b.
//
// `authored` is what the layer asked for; the return is what survives. Always at
// least one, so a distant layer softens toward its base frequency rather than
// vanishing (vanishing would pop, and the layer already has `pattern_fade` for
// disappearing gracefully).
//
// The comparison is done in FEATURE SIZE, not frequency, because that is the thing
// with a pixel to compare against: octave k has features of `period / 2^k` metres,
// and the pixel footprint at the hit is `distance * metres_per_pixel_at_one_metre`.
fn pattern_octave_budget(authored: u32, period_meters: f32, distance_meters: f32) -> u32 {
    if (!PATTERN_OCTAVE_LOD) {
        return authored;
    }
    // The clamp goes AFTER the scale, matching `octave_budget` in pattern.rs — the
    // two must agree at the degenerate end as well as the useful one.
    let footprint = max(
        distance_meters * lighting.material_params.z * PATTERN_OCTAVE_LOD_SCALE, 1e-6);
    var budget = 1u;
    for (var octave = 1u; octave < authored; octave = octave + 1u) {
        if (period_meters / exp2(f32(octave)) < footprint) {
            break;
        }
        budget = octave + 1u;
    }
    return budget;
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

// ---- Perlin: gradient noise on the CUBIC lattice ------------------------------
//
// Same eight corners value noise reads, but each corner contributes the dot of a
// pseudo-random gradient with the offset to it, rather than a scalar. Costs eight
// dot products on top of the eight hashes and stays completely branchless — the
// opposite trade from simplex below.
//
// Perlin's quintic fade `6t^5 - 15t^4 + 10t^3` rather than the cubic ease the value
// path uses: the cubic has a discontinuous second derivative at the lattice, which
// value noise gets away with and a gradient field does not (it shows as faint
// creases along the cell planes).
fn pattern_quintic(t: f32) -> f32 {
    return t * t * t * (t * (t * 6.0 - 15.0) + 10.0);
}

// One of 12 edge-midpoint gradients — the classic Perlin set. All the same length
// and evenly spread, so no direction is favoured. Shared with simplex.
fn pattern_gradient(cell: vec3<i32>, salt: u32) -> vec3<f32> {
    let mixed = (bitcast<u32>(cell.x) * 0x27d4eb2du)
        ^ (bitcast<u32>(cell.y) * 0x9e3779b9u)
        ^ (bitcast<u32>(cell.z) * 0x85ebca6bu)
        ^ (salt * 0xc2b2ae35u);
    let index = pattern_hash_u32(mixed) % 12u;
    let axis = index / 4u;
    let signs = vec2<f32>(
        select(1.0, -1.0, (index & 1u) != 0u),
        select(1.0, -1.0, (index & 2u) != 0u),
    );
    if (axis == 0u) { return vec3<f32>(signs.x, signs.y, 0.0); }
    if (axis == 1u) { return vec3<f32>(signs.x, 0.0, signs.y); }
    return vec3<f32>(0.0, signs.x, signs.y);
}

fn pattern_perlin_noise(point: vec3<f32>, salt: u32) -> f32 {
    let base = floor(point);
    let cell = vec3<i32>(base);
    let local = point - base;
    let fade = vec3<f32>(
        pattern_quintic(local.x), pattern_quintic(local.y), pattern_quintic(local.z));
    var accumulated = 0.0;
    for (var corner = 0u; corner < 8u; corner = corner + 1u) {
        let offset = vec3<u32>(corner & 1u, (corner >> 1u) & 1u, (corner >> 2u) & 1u);
        let corner_local = vec3<f32>(offset);
        let weight_x = select(1.0 - fade.x, fade.x, offset.x == 1u);
        let weight_y = select(1.0 - fade.y, fade.y, offset.y == 1u);
        let weight_z = select(1.0 - fade.z, fade.z, offset.z == 1u);
        let gradient = pattern_gradient(cell + vec3<i32>(offset), salt);
        accumulated = accumulated
            + weight_x * weight_y * weight_z * dot(gradient, local - corner_local);
    }
    // The 12-gradient set bounds |value| just under 1; map to 0..1 and clamp so the
    // guarantee every consumer relies on is exact rather than nearly true.
    return clamp(0.5 + 0.5 * accumulated, 0.0, 1.0);
}

fn pattern_fractal_perlin(point: vec3<f32>, octaves: u32, salt_base: u32) -> f32 {
    let count = clamp(octaves, 1u, 4u);
    var frequency = 1.0;
    var amplitude = 1.0;
    var total = 0.0;
    var normalisation = 0.0;
    for (var octave = 0u; octave < count; octave = octave + 1u) {
        total = total + amplitude * pattern_perlin_noise(point * frequency, salt_base ^ octave);
        normalisation = normalisation + amplitude;
        frequency = frequency * 2.0;
        amplitude = amplitude * 0.5;
    }
    return total / normalisation;
}

// ---- Simplex-lattice gradient noise (the four-corner contender) --------------
//
// Value noise reads EIGHT cube corners per octave. The simplex lattice skews space
// so the cell is a tetrahedron, which has FOUR — half the hashes for the same
// spatial frequency, which is the whole reason this exists as a contender.
//
// It is not free of that: the skew costs two dot products, and picking which of the
// six tetrahedra the point landed in is a comparison chain that DIVERGES inside a
// warp. Whether four hashes plus a branchy ordering beats eight hashes and no
// branches is a hardware question, not an arithmetic one, so it ships as a bench
// column rather than as a replacement.
//
// Gradients come from the same `pattern_hash_u32`, so the CPU reference reproduces
// this bit for bit exactly as it does the value-noise path.
const PATTERN_SIMPLEX_SKEW: f32 = 0.3333333333;   // 1/3 for 3D
const PATTERN_SIMPLEX_UNSKEW: f32 = 0.1666666667; // 1/6 for 3D

// One corner's contribution: the classic `(0.6 - r^2)^4` falloff, zero outside the
// corner's radius of influence so the four contributions sum without a seam.
fn pattern_simplex_corner(offset: vec3<f32>, cell: vec3<i32>, salt: u32) -> f32 {
    let falloff = 0.6 - dot(offset, offset);
    if (falloff <= 0.0) {
        return 0.0;
    }
    let squared = falloff * falloff;
    return squared * squared * dot(pattern_gradient(cell, salt), offset);
}

fn pattern_simplex_noise(point: vec3<f32>, salt: u32) -> f32 {
    // Skew into the lattice where the simplex cell is a unit tetrahedron.
    let skew = (point.x + point.y + point.z) * PATTERN_SIMPLEX_SKEW;
    let skewed = floor(point + vec3<f32>(skew));
    let unskew = (skewed.x + skewed.y + skewed.z) * PATTERN_SIMPLEX_UNSKEW;
    let origin = skewed - vec3<f32>(unskew);
    let offset0 = point - origin;

    // Which of the six tetrahedra: rank the components. This is the divergent part.
    var step1 = vec3<f32>(0.0);
    var step2 = vec3<f32>(0.0);
    if (offset0.x >= offset0.y) {
        if (offset0.y >= offset0.z) {
            step1 = vec3<f32>(1.0, 0.0, 0.0); step2 = vec3<f32>(1.0, 1.0, 0.0);
        } else if (offset0.x >= offset0.z) {
            step1 = vec3<f32>(1.0, 0.0, 0.0); step2 = vec3<f32>(1.0, 0.0, 1.0);
        } else {
            step1 = vec3<f32>(0.0, 0.0, 1.0); step2 = vec3<f32>(1.0, 0.0, 1.0);
        }
    } else {
        if (offset0.y < offset0.z) {
            step1 = vec3<f32>(0.0, 0.0, 1.0); step2 = vec3<f32>(0.0, 1.0, 1.0);
        } else if (offset0.x < offset0.z) {
            step1 = vec3<f32>(0.0, 1.0, 0.0); step2 = vec3<f32>(0.0, 1.0, 1.0);
        } else {
            step1 = vec3<f32>(0.0, 1.0, 0.0); step2 = vec3<f32>(1.0, 1.0, 0.0);
        }
    }

    let cell = vec3<i32>(skewed);
    let offset1 = offset0 - step1 + vec3<f32>(PATTERN_SIMPLEX_UNSKEW);
    let offset2 = offset0 - step2 + vec3<f32>(2.0 * PATTERN_SIMPLEX_UNSKEW);
    let offset3 = offset0 - vec3<f32>(1.0) + vec3<f32>(3.0 * PATTERN_SIMPLEX_UNSKEW);
    let total = pattern_simplex_corner(offset0, cell, salt)
        + pattern_simplex_corner(offset1, cell + vec3<i32>(step1), salt)
        + pattern_simplex_corner(offset2, cell + vec3<i32>(step2), salt)
        + pattern_simplex_corner(offset3, cell + vec3<i32>(1, 1, 1), salt);
    // 32 is the conventional 3D scale, which brings the sum to roughly [-1, 1];
    // the clamp makes "roughly" into a guarantee, since every consumer of a
    // generator value in this file assumes 0..1.
    return clamp(0.5 + 16.0 * total, 0.0, 1.0);
}

fn pattern_fractal_simplex(point: vec3<f32>, octaves: u32, salt_base: u32) -> f32 {
    let count = clamp(octaves, 1u, 4u);
    var frequency = 1.0;
    var amplitude = 1.0;
    var total = 0.0;
    var normalisation = 0.0;
    for (var octave = 0u; octave < count; octave = octave + 1u) {
        total = total + amplitude * pattern_simplex_noise(point * frequency, salt_base ^ octave);
        normalisation = normalisation + amplitude;
        frequency = frequency * 2.0;
        amplitude = amplitude * 0.5;
    }
    return total / normalisation;
}

// ---- Ridged multifractal ------------------------------------------------------
//
// The same value-noise lattice, folded: `1 - |2v - 1|` turns each octave's midline
// into a crease and its extremes into troughs, and squaring sharpens the crease.
// Reads as veins, erosion channels and rock strata rather than as grain.
//
// Deliberately built on `pattern_value_noise` and not on its own lattice: it costs
// the SAME eight hashes per octave as `noise`, so a bench column comparing the two
// isolates the fold — the look changes and the cost does not.
fn pattern_ridged_noise(point: vec3<f32>, octaves: u32, salt_base: u32) -> f32 {
    let count = clamp(octaves, 1u, 4u);
    var frequency = 1.0;
    var amplitude = 1.0;
    var total = 0.0;
    var normalisation = 0.0;
    for (var octave = 0u; octave < count; octave = octave + 1u) {
        let folded = 1.0 - abs(2.0 * pattern_value_noise(point * frequency, salt_base ^ octave) - 1.0);
        total = total + amplitude * folded * folded;
        normalisation = normalisation + amplitude;
        frequency = frequency * 2.0;
        amplitude = amplitude * 0.5;
    }
    return clamp(total / normalisation, 0.0, 1.0);
}

// ---- Turbulence ---------------------------------------------------------------
//
// `sum |2v - 1|` — the absolute value of a signed octave, which creases at the
// ZERO crossing rather than at the midline peak the way ridged does. Smoke, marble
// veining, weathering streaks. Same eight hashes per octave as `noise`, so it and
// `ridged` together isolate what the FOLD costs versus what the lattice costs:
// three generators, one lattice, three looks.
fn pattern_turbulence(point: vec3<f32>, octaves: u32, salt_base: u32) -> f32 {
    let count = clamp(octaves, 1u, 4u);
    var frequency = 1.0;
    var amplitude = 1.0;
    var total = 0.0;
    var normalisation = 0.0;
    for (var octave = 0u; octave < count; octave = octave + 1u) {
        total = total + amplitude
            * abs(2.0 * pattern_value_noise(point * frequency, salt_base ^ octave) - 1.0);
        normalisation = normalisation + amplitude;
        frequency = frequency * 2.0;
        amplitude = amplitude * 0.5;
    }
    return clamp(total / normalisation, 0.0, 1.0);
}

// ---- Wave: bands with a noise distortion --------------------------------------
//
// Blender's Wave texture, which is how wood grain and rock strata are actually
// authored: a 1D band function along one axis, with the coordinate pushed around
// by noise so the bands bend instead of ruling straight lines.
//
// Bands run along X of the sample coordinate, so the FRAME chooses their direction
// — face frame gives bands across a block face, world frame gives geological
// strata that continue across voxels. `param_a` is the distortion, in periods.
fn pattern_wave(point: vec3<f32>, distortion: f32, salt: u32) -> f32 {
    var coordinate = point.x + point.y * 0.25;
    if (distortion > 0.0) {
        coordinate = coordinate
            + distortion * (2.0 * pattern_value_noise(point, salt ^ 41u) - 1.0);
    }
    // Triangle rather than sine: it is two ALU ops instead of a transcendental, and
    // against a voxel grid the difference in profile is not visible.
    let phase = fract(coordinate);
    return 1.0 - abs(2.0 * phase - 1.0);
}

// ---- Worley / cellular F1 -----------------------------------------------------
//
// Distance to the nearest of one jittered feature point per cell. Cracked mud,
// pebbles, cell walls, lichen — the organic look none of the lattice generators
// reach, because its features have BOUNDARIES rather than gradients.
//
// 27 cells is the textbook neighbourhood. This walks all 27 rather than trying to
// prune: the jitter is unbounded within the cell, so a correct prune has to keep
// any cell whose nearest possible point beats the current best, and that test costs
// about what the distance it replaces costs. It is the dearest generator here by
// design — its bench column is the point.
const PATTERN_WORLEY_JITTER_X_SALT: u32 = 21u;
const PATTERN_WORLEY_JITTER_Y_SALT: u32 = 22u;
const PATTERN_WORLEY_JITTER_Z_SALT: u32 = 23u;

// All three Worley variants in one walk, because they differ only in what they do
// with the distances and NOT in how they find them. Three separate functions would
// have meant three copies of the 27-cell loop and three places to get the jitter
// salts wrong.
//
//   .x  F1              distance to the nearest feature point   — pebbles, cells
//   .y  F2              distance to the second nearest          — for the edge form
//   .z  smooth F1       exponential smooth-min over all of them — no hard creases
//
// The smooth minimum is iq's `-log(sum exp(-k*d))/k`, which reads as cell walls that
// swell and merge rather than meeting at a crease. It is accumulated in the same
// pass because it needs every distance, not just the best two.
const PATTERN_WORLEY_SMOOTH_K: f32 = 6.0;

fn pattern_worley_distances(point: vec3<f32>, salt_base: u32) -> vec3<f32> {
    let base = floor(point);
    let cell = vec3<i32>(base);
    let local = point - base;
    var nearest = 1e9;
    var second = 1e9;
    var smooth_sum = 0.0;
    for (var index = 0u; index < 27u; index = index + 1u) {
        let neighbour = vec3<i32>(
            i32(index % 3u) - 1,
            i32((index / 3u) % 3u) - 1,
            i32(index / 9u) - 1,
        );
        let neighbour_cell = cell + neighbour;
        let feature = vec3<f32>(neighbour) + vec3<f32>(
            pattern_hash_cell(neighbour_cell, salt_base ^ PATTERN_WORLEY_JITTER_X_SALT),
            pattern_hash_cell(neighbour_cell, salt_base ^ PATTERN_WORLEY_JITTER_Y_SALT),
            pattern_hash_cell(neighbour_cell, salt_base ^ PATTERN_WORLEY_JITTER_Z_SALT),
        );
        let offset = feature - local;
        let squared = dot(offset, offset);
        // Squared distance for the ranking — monotone, so the ordering is the same
        // and 27 square roots become one at the end.
        if (squared < nearest) {
            second = nearest;
            nearest = squared;
        } else if (squared < second) {
            second = squared;
        }
        smooth_sum = smooth_sum + exp(-PATTERN_WORLEY_SMOOTH_K * sqrt(squared));
    }
    return vec3<f32>(sqrt(nearest), sqrt(second), -log(smooth_sum) / PATTERN_WORLEY_SMOOTH_K);
}

// The nearest feature point is at most ~1.5 cells away in the worst case, so
// dividing by that normalises into 0..1 without clipping the tail off the common
// case.
const PATTERN_WORLEY_RANGE: f32 = 1.5;

fn pattern_worley(point: vec3<f32>, salt_base: u32) -> f32 {
    return clamp(pattern_worley_distances(point, salt_base).x / PATTERN_WORLEY_RANGE, 0.0, 1.0);
}

// F2 - F1: zero exactly on the boundary between two cells and rising away from it,
// inverted so the BOUNDARY is the bright feature. Cracked mud, dried paint, the
// mortar between irregular stones — the look no lattice noise produces, because it
// needs a global "which cell owns me" that only a cellular walk has.
fn pattern_worley_edge(point: vec3<f32>, salt_base: u32) -> f32 {
    let distances = pattern_worley_distances(point, salt_base);
    return clamp(1.0 - (distances.y - distances.x) / PATTERN_WORLEY_RANGE, 0.0, 1.0);
}

fn pattern_worley_smooth(point: vec3<f32>, salt_base: u32) -> f32 {
    return clamp(
        max(pattern_worley_distances(point, salt_base).z, 0.0) / PATTERN_WORLEY_RANGE, 0.0, 1.0);
}

// ---- Checker ------------------------------------------------------------------
//
// Alternating cells of the sampling lattice. Tiles, boards, and — the reason it is
// worth a generator slot rather than being dismissed as a toy — the bench's COST
// FLOOR: it produces a full-coverage pattern for two floors and a bit-and, so a
// column running it measures everything the layer mechanism costs AROUND the
// generator, with the generator itself at essentially zero. Every other column is
// read against it.
fn pattern_checker(point: vec3<f32>) -> f32 {
    let cell = vec3<i32>(floor(point));
    let parity = (cell.x + cell.y + cell.z) & 1;
    return select(0.0, 1.0, parity == 0);
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

/// The texel grid this layer samples on at a given distance — the layer's authored
/// count up close, halved once per doubling of distance beyond the fade start.
///
/// **This exists to make the pattern cache work at range, and it is not octave
/// LOD.** The cache pays off in proportion to how many pixels land on the same
/// texel: about a hundred at 2 m, and fewer than one further out, where each pixel
/// straddles several texels and every lookup misses. Measured, the cache removes
/// 37% of the pattern path on ground views and *nothing* on aerial ones, for
/// exactly that reason. Coarsening the grid with distance manufactures the reuse
/// that distance destroys.
///
/// Octave LOD tried to buy the same thing by dropping octaves per pixel and LOST at
/// ground level (+0.24 ms), because neighbouring pixels disagreeing about the
/// octave count is divergence. This is the opposite shape: a coarser grid makes
/// neighbouring pixels agree MORE, and the generator's work per sample is
/// unchanged. It also anti-aliases, since the thing being removed is detail finer
/// than a pixel.
///
/// The step is a power of two so the grid nests: a coarse texel is exactly a block
/// of fine ones, so the pattern does not swim as the LOD changes.
fn pattern_texels_at(layer: PatternLayer, distance_meters: f32) -> u32 {
    let texels = pattern_texels(layer);
    if (!MATERIAL_PATTERN_TEXEL_LOD || texels == 0u) {
        return texels;
    }
    let start = lighting.material_params.x;
    if (start <= 0.0 || distance_meters <= start) {
        return texels;
    }
    // `floor(log2(ratio))` for `ratio > 1` IS the IEEE-754 biased exponent minus
    // the bias, so the step count is a shift and a subtract rather than a
    // transcendental. Exact, not approximate: the mantissa cannot change which
    // power of two a value falls in, which is the entire content of `floor(log2)`.
    //
    // This is not a micro-optimization. Rung 8 of the entry probe measured the LAST
    // consumer of this function at 0.77-0.97 ms — the largest single item in the
    // pattern path, bigger than the generator — because with the snap stubbed the
    // drift path still needed it, and one `log2` per layer per pixel is a
    // transcendental in the middle of the shading loop.
    //
    // The guard above proves `ratio > 1`, so the biased exponent is at least 127 and
    // the subtraction cannot underflow. A denormal `start` sends `ratio` to infinity
    // and a NaN distance falls through the `<=` test; both land on exponent 255,
    // which `min(steps, 7u)` clamps to the coarsest grid — the right answer for
    // "unreasonably far away" either way.
    let ratio = distance_meters / start;
    let steps = (bitcast<u32>(ratio) >> 23u) - 127u;
    return max(texels >> min(steps, 7u), 1u);
}

// Normalised distance-LOD amount for the inspection view: 0 is the authored
// close-up grid and 1 is the coarsest grid the runtime can select. This exposes
// the actual texel LOD path, rather than an unrelated artistic noise value.
fn pattern_lod_level(layer: PatternLayer, distance_meters: f32) -> f32 {
    let authored = pattern_texels(layer);
    if (authored <= 1u) {
        return 0.0;
    }
    let sampled_texels = pattern_texels_at(layer, distance_meters);
    return clamp(log2(f32(authored) / f32(max(sampled_texels, 1u))) / 7.0, 0.0, 1.0);
}

fn pattern_debug_lod(material: u32, sample: PatternSample) -> f32 {
    let flags = materials[material].flags;
    if (!MATERIAL_PATTERNS || (flags & MATERIAL_FLAG_PATTERNS) == 0u) {
        return 0.0;
    }
    let count = min(
        min(material_pattern_count(flags), MATERIAL_PATTERN_MAX_LAYERS),
        MAX_PATTERN_LAYERS,
    );
    var level = 0.0;
    for (var slot = 0u; slot < count; slot = slot + 1u) {
        if (!material_debug_layer_enabled(slot)) {
            continue;
        }
        level = max(level, pattern_lod_level(
            materials[material].patterns[slot], sample.distance_meters));
    }
    return level;
}

fn pattern_snap_to_texels(meters: vec3<f32>, texels: u32,
                          voxel_size_meters: f32) -> vec3<f32> {
    if (MATERIAL_PATTERN_ENTRY_PROBE >= PATTERN_ENTRY_NO_SNAP || texels == 0u) {
        return meters;
    }
    // The brick is the authoritative one-metre world voxel. The smaller value in
    // BrickmapMeta is the ray-traversal/detail-cell size.
    let world_voxel_size_meters = voxel_size_meters * BRICK_SIZE;
    let texel = world_voxel_size_meters / f32(texels);
    return floor(meters / texel) * texel + vec3<f32>(texel * 0.5);
}

// ---- The tessellation ---------------------------------------------------------
//
// Divides a face into brick-bonded tiles and reports everything a masonry surface
// needs from that division. ONE walk, three outputs, for the same reason the three
// Worley variants share one loop: they differ in what they do with the result, not
// in how they find it.
//
//   .xy  the tile-local coordinate, 0..1 across the tile's interior
//   .z   a per-tile hash, 0..1 — the tone that makes one block differ from the next
//   .w   distance to the nearest tile edge, 0..1 of a half-tile — grout and bevel
//
// The bond is a per-ROW horizontal shift, which is what separates masonry from a
// grid: `tile_bond` of 0.5 offsets every other course by half a tile, and the
// courses stop lining up into continuous vertical joints. A grid of squares reads
// as tile; a bonded grid reads as a wall.
//
// The gap is subtracted from the tile's interior rather than added around it, so
// changing the grout width does not move the tiles. Dragging that slider should
// widen the joints, not slide the whole wall.
fn pattern_tessellate(local: vec2<f32>, aspect: f32, bond: f32, gap: f32) -> vec4<f32> {
    // Tiles are `aspect` wide for every 1 high, so the horizontal axis is scaled
    // rather than the tile being described by two sizes.
    let scaled = vec2<f32>(local.x / max(aspect, 1e-4), local.y);
    let row = floor(scaled.y);
    // Every course shifts by `bond` of a tile relative to the one below it, so the
    // vertical joints stagger instead of running the height of the wall.
    let shifted_x = scaled.x + row * bond;
    let column = floor(shifted_x);
    let cell = vec2<f32>(shifted_x - column, scaled.y - row);

    let hash = pattern_hash_cell(
        vec3<i32>(i32(column), i32(row), 0), PATTERN_TILE_SALT);

    // Distance to the nearest edge, as a fraction of a half-tile, with the grout
    // band consuming the outermost `gap`. Zero anywhere inside the joint, rising to
    // one at the tile's centre.
    let to_edge = min(min(cell.x, 1.0 - cell.x), min(cell.y, 1.0 - cell.y));
    let interior = max(0.5 - gap, 1e-4);
    let edge = clamp((to_edge - gap) / interior, 0.0, 1.0);

    // The tile-local coordinate is renormalised over the INTERIOR, so a generator
    // sampled in tile frame spans the stone and not the joint.
    let inner = clamp((cell - vec2<f32>(gap)) / max(1.0 - 2.0 * gap, 1e-4),
                      vec2<f32>(0.0), vec2<f32>(1.0));
    return vec4<f32>(inner, hash, edge);
}

const PATTERN_TILE_SALT: u32 = 61u;

// The edge distance, shaped from a bevel into a joint.
//
// The raw distance ramps linearly from the joint to the tile's centre, which reads
// as a pillow rather than as masonry — the whole face is a gradient and no part of
// it is flat. `sharpness` pushes the transition toward the joint: at 0 the ramp is
// the raw bevel, at 1 it is a narrow dark line around a flat tile.
//
// Implemented as a power rather than a smoothstep with a width, because a power
// keeps both endpoints pinned — 0 stays 0 in the joint and 1 stays 1 at the centre
// at every setting, so dragging the slider changes the profile without changing the
// grout's colour or the tile's.
fn pattern_tile_edge_shaped(edge: f32, sharpness: f32) -> f32 {
    let amount = clamp(sharpness, 0.0, 1.0);
    if (amount <= 0.0) {
        return edge;
    }
    // 1 -> 1/16 exponent: increasingly abrupt at the joint, flat everywhere else.
    return pow(edge, 1.0 / (1.0 + 15.0 * amount));
}

// The two world axes that lie IN a face, given the axis it faces along. The
// tessellation is 2D and needs to know which plane it is drawn on.
fn pattern_face_uv(meters: vec3<f32>, axis: u32) -> vec2<f32> {
    if (axis == 0u) { return vec2<f32>(meters.z, meters.y); }
    if (axis == 1u) { return vec2<f32>(meters.x, meters.z); }
    return vec2<f32>(meters.x, meters.y);
}

// The tessellation for this layer, from WORLD coordinates projected onto the hit
// face — world and not voxel-local, deliberately. Voxel-local would restart the
// courses at every block edge and cap the tile size at one metre; a wall of slate
// blocks bigger than a voxel, or a bond that continues across two, needs the
// tessellation to be a property of the WALL rather than of each cube in it.
fn pattern_tile_of(layer: PatternLayer, sample: PatternSample,
                   drift_meters: vec3<f32>) -> vec4<f32> {
    let period = max(layer.period_meters, 1e-4);
    let uv = pattern_face_uv(sample.world_meters - drift_meters, sample.axis) / period;
    return pattern_tessellate(uv, layer.tile_aspect, layer.tile_bond, layer.tile_gap);
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
                        voxel_size_meters: f32, distance_meters: f32) -> vec3<f32> {
    if (all(velocity == vec3<f32>(0.0))) {
        return vec3<f32>(0.0);
    }
    let offset = velocity * graph_animation_seconds();
    // The SAME grid the coordinate snaps to, or the drift steps off it and the
    // pattern shimmers as it moves.
    let texels = pattern_texels_at(layer, distance_meters);
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
    if (MATERIAL_PATTERN_ENTRY_PROBE >= PATTERN_ENTRY_NO_COORDINATE) {
        // Everything the frame branch reads goes with it: the hit position, the
        // world-voxel divide, the period divide — and `pattern_drift_meters`,
        // whose only consumer is this function. That is the rung's delta.
        return vec3<f32>(0.5);
    }
    let period = max(layer.period_meters, 1e-4);
    // Rung 7 collapses the frame to WORLD, which is what kills the world-voxel
    // divide: only the voxel and face branches read it. Written as a comparison
    // against a probe const rather than as an early return so the WORLD path below
    // stays the single implementation.
    var frame = pattern_frame(layer);
    if (MATERIAL_PATTERN_ENTRY_PROBE >= PATTERN_ENTRY_NO_FRAMES) {
        frame = PATTERN_FRAME_WORLD;
    } else if (MATERIAL_PATTERN_ENTRY_PROBE >= PATTERN_ENTRY_NO_TILE_FRAME
               && frame == PATTERN_FRAME_TILE) {
        // Rung 6 prices the TILE branch's mere PRESENCE. It is by far the largest
        // block in this function — `pattern_tile_of` pulls in the face projection,
        // the bonded tessellation and a per-tile hash — and the frame is runtime
        // data, so that code is resident and its registers are allocated on every
        // layer whether or not any material authors a tile frame. If this rung is
        // expensive, the fix is to get the tessellation out of the hot function,
        // not to make it faster.
        frame = PATTERN_FRAME_WORLD;
    }
    let world_voxel_size_meters = voxel_size_meters * BRICK_SIZE;
    let world_voxel = sample.voxel / vec3<i32>(i32(BRICK_SIZE));
    var meters = sample.world_meters;
    if (MATERIAL_PATTERN_ENTRY_PROBE < PATTERN_ENTRY_NO_DRIFT) {
        // Rung 8 takes the subtraction and `pattern_drift_meters` with it — this
        // is that function's only consumer, and with the texel LOD on it carries a
        // `log2` of its own.
        meters = meters - drift_meters;
    }
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
    else if (frame == PATTERN_FRAME_TILE) {
        let tile = pattern_tile_of(layer, sample, drift_meters);
        // The tile-local u,v, with the PER-TILE HASH as the third coordinate.
        //
        // That third component is the whole trick. A 3D generator sampled at a
        // different z is a completely different field, so pushing each tile's hash
        // into z gives every tile its own independent draw of the grain — which is
        // exactly what a slate wall looks like and what no other frame produces.
        // It costs nothing: the hash was already computed for the tone output.
        //
        // Scaled well past the generator's own feature size so neighbouring tiles
        // land in uncorrelated slices rather than in adjacent ones.
        return vec3<f32>(tile.x, tile.y, tile.z * 64.0);
    }
    // Otherwise WORLD: a field the world sits in, so it flows across neighbouring
    // voxels and CANNOT tile per voxel. The default, and the continuity argument.
    let snapped = pattern_snap_to_texels(
        meters, pattern_texels_at(layer, sample.distance_meters), voxel_size_meters);
    if (MATERIAL_PATTERN_ENTRY_PROBE >= PATTERN_ENTRY_NO_PERIOD) {
        // Rung 5: the reciprocal and three multiplies that put the coordinate in
        // units of the authored period. `period` itself dies with it.
        return snapped;
    }
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
    if (MATERIAL_PATTERN_ENTRY_PROBE >= PATTERN_ENTRY_NO_SALT) {
        return 0u;
    }
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

// ---- Pattern field cache -----------------------------------------------------
//
// A direct-mapped cache over the TEXEL LATTICE, filled lazily by the shading pass
// itself. It exists because `pattern_generator_value` is a pure function of the
// layer's configuration and the SNAPPED sample point: `pattern_snap_to_texels`
// quantises the coordinate, so every pixel landing on the same 1.56 cm texel asks
// the same question and gets the same answer. At 2 m from a wall that is roughly
// a hundred pixels per texel.
//
// It is generic by construction. Nothing here names a material, a slot or a
// generator — the key is built from the layer's own words, so two layers with
// identical configuration correctly share entries and a layer nobody has authored
// yet participates the day it is added.
//
// ANIMATION NEEDS NO SPECIAL CASE, which is the part worth understanding. Drift is
// applied to the coordinate BEFORE this point and is quantised to whole texels, so
// a drifting pattern simply asks about a different texel each step — the cached
// contents stay valid and the lookup moves. Gain is applied AFTER, outside the
// generator. So an animated surface hits the same cache as a still one, and no
// entry is ever invalidated by the clock.
const PATTERN_CACHE_MASK: u32 = 16777216u - 1u;

@group(G_WORLD) @binding(B_PATTERN_CACHE) var<storage, read_write> pattern_cache: array<atomic<u32>>;

/// Hash of everything the generator's answer depends on: the snapped coordinate,
/// the per-face salt, the octave count, and the layer's generator configuration.
///
/// The coordinate goes in as RAW BITS rather than as a rounded lattice index. It
/// is already quantised — that is the precondition checked by the caller — so its
/// bit pattern is exact and identical for every pixel on the texel, and hashing
/// the bits needs no knowledge of the units or the frame the coordinate is in.
fn pattern_cache_hashes(layer: PatternLayer, point: vec3<f32>, salt: u32,
                        octaves: u32) -> vec2<u32> {
    // Two INDEPENDENT folds over the original inputs: one chooses the slot and
    // the other supplies the tag. Rehashing the 32-bit slot hash cannot create
    // more entropy — after fixing its low 22 bits, only 10 bits remain — so that
    // version silently gave the nominally 16-bit tag only 10 effective bits.
    // These chains are independent and can be scheduled in parallel.
    var key = bitcast<u32>(point.x);
    key = key * 0x9e3779b9u ^ bitcast<u32>(point.y);
    key = key * 0x85ebca6bu ^ bitcast<u32>(point.z);
    key = key * 0xc2b2ae35u ^ salt;
    key = key * 0x27d4eb2fu ^ (octaves | (layer.packed << 3u));
    key = key * 0x165667b1u ^ bitcast<u32>(layer.period_meters);

    var tag_key = bitcast<u32>(point.z);
    tag_key = tag_key * 0x85ebca77u ^ bitcast<u32>(point.x);
    tag_key = tag_key * 0xc2b2ae3du ^ bitcast<u32>(point.y);
    tag_key = tag_key * 0x27d4eb4fu ^ salt;
    tag_key = tag_key * 0x165667c5u ^ (layer.packed | (octaves << 26u));
    tag_key = tag_key * 0xd3a2646du ^ bitcast<u32>(layer.period_meters);

    // `param_a` is density, distortion, edge sharpness or band width depending on
    // the generator; `param_b` is domain-warp strength except for EdgeBand, where
    // it is jaggedness. Both affect the raw answer,
    // and omitting them would leave stale entries behind after an authored edit.
    key = key * 0xd3a2646du ^ bitcast<u32>(layer.param_a);
    key = key * 0xfd7046c5u ^ bitcast<u32>(layer.param_b);
    tag_key = tag_key * 0xfd7046d7u ^ bitcast<u32>(layer.param_b);
    tag_key = tag_key * 0xb55a4f0du ^ bitcast<u32>(layer.param_a);
    // The extra flag word changes the raw answer (grid averaging), so toggling
    // it must move the key or the panel would serve stale point samples.
    key = key * 0x9e3779b1u ^ layer.flags_extra;
    tag_key = tag_key * 0x85ebca7bu ^ layer.flags_extra;
    return vec2<u32>(pattern_hash_u32(key), pattern_hash_u32(tag_key));
}

/// The probe's stand-in for a generator, and the reason the ladder can measure
/// anything above rung 1.
///
/// A stub that simply returned a constant would let naga prove the coordinate, the
/// salt and the octave count unused and delete all three — the entry path would
/// vanish with the generator and every rung above would read the same number. This
/// CONSUMES all three instead, for three adds and a `fract`, which is about as
/// close to free as a consumer that cannot be folded gets.
///
/// `salt` is masked to a byte before the convert so the f32 conversion is exact and
/// the sink stays deterministic frame over frame like everything else in the pass.
fn pattern_entry_sink(raw_point: vec3<f32>, salt: u32, octaves: u32) -> f32 {
    return fract(raw_point.x + raw_point.y + raw_point.z
                 + f32(salt & 0xffu) + f32(octaves));
}

fn pattern_generator_value(layer: PatternLayer, sample: PatternSample,
                           voxel_size_meters: f32, drift_meters: vec3<f32>) -> f32 {
    let raw_point = pattern_coordinate(layer, sample, voxel_size_meters, drift_meters);
    let salt = pattern_variation_salt(layer, sample);
    let octaves = pattern_layer_octaves(layer, sample);
    if (MATERIAL_PATTERN_ENTRY_PROBE >= PATTERN_ENTRY_NO_GENERATOR) {
        return pattern_entry_sink(raw_point, salt, octaves);
    }
    if (!MATERIAL_PATTERN_CACHE || pattern_texels_at(layer, sample.distance_meters) == 0u) {
        // Without the texel snap the coordinate is continuous, so no two pixels
        // ever agree on a key and the cache would be pure overhead. That is a
        // property of the LAYER, read from its own configuration — no material,
        // slot or generator is named anywhere in this path.
        return pattern_generator_resolved(
            layer, sample, drift_meters, raw_point, salt, octaves, voxel_size_meters);
    }
    let hashes = pattern_cache_hashes(layer, raw_point, salt, octaves);
    let slot = hashes.x & PATTERN_CACHE_MASK;
    // Forced non-zero: the buffer starts zeroed, so zero means empty.
    let tag = max(hashes.y >> 16u, 1u);
    let stored = atomicLoad(&pattern_cache[slot]);
    if ((stored >> 16u) == tag) {
        return f32(stored & 0xffffu) * (1.0 / 65535.0);
    }
    let value = pattern_generator_resolved(
        layer, sample, drift_meters, raw_point, salt, octaves, voxel_size_meters);
    let quantised = u32(clamp(value, 0.0, 1.0) * 65535.0 + 0.5);
    // ONE atomic 32-bit store, so an entry can never be seen half-written and
    // concurrent invocations cannot data-race. Same-key writers store the same
    // bits; colliding keys leave one complete, tag-checked entry behind.
    atomicStore(&pattern_cache[slot], (tag << 16u) | quantised);
    return value;
}

/// The generator itself, with the coordinate and salt already resolved so the
/// caching wrapper above can key on them.
/// Whether this build compiles the given generator's body at all.
///
/// EVERY generator's code is resident in `pattern_generator_at`, which is one
/// function inlined into the shading pass, so a project that authors two of them
/// still pays the register footprint of all fourteen. The entry probe measured
/// that effect directly and it is not small: rung 6 charged 0.146 ms to the tile
/// FRAME's mere presence on a table where nothing authors a tile frame. Code that
/// never executes cannot cost time unless it is costing registers, and this pass is
/// latency-bound, so occupancy is what converts registers into milliseconds.
///
/// The mask is not a quality knob and never trades detail for speed: a generator
/// outside it is one the material set does not use, so the output is bit-identical.
/// It is DERIVABLE — the authored table and the material graphs between them name
/// every generator a project can reach — which is the same move the cacheability
/// analysis already makes over the node declarations. The default is every bit set,
/// so an underived build is exactly the shipped renderer.
fn pattern_generator_enabled(generator: u32) -> bool {
    return (MATERIAL_PATTERN_GENERATOR_MASK & (1u << generator)) != 0u;
}

// A grass-side mask: one stable random edge height per horizontal face column,
// quantised to the layer's texel grid. The Pattern Layer's face mask restricts
// the result to sides; this function also returns zero for Y faces so the
// generator remains well-defined when previewed on its own.
fn pattern_edge_band_value(layer: PatternLayer, sample: PatternSample,
                           voxel_size_meters: f32, salt: u32) -> f32 {
    if (sample.axis == 1u) {
        return 0.0;
    }
    // A band is the exposed lip of a COLUMN, not a decal on every block:
    // stacked blocks only band where the column meets air. Mirrors the same
    // gate in `edge_band_value` (pattern.rs), whose CPU samples default to
    // fully exposed.
    if (pattern_edge_band_from_bottom(layer)) {
        if ((sample.exposure & PATTERN_EXPOSURE_BOTTOM) == 0u) {
            return 0.0;
        }
    } else if ((sample.exposure & PATTERN_EXPOSURE_TOP) == 0u) {
        return 0.0;
    }
    let world_voxel_size = voxel_size_meters * BRICK_SIZE;
    var local_u = 0.0;
    if (sample.axis == 0u) {
        local_u = fract(sample.world_meters.z / world_voxel_size);
    } else {
        local_u = fract(sample.world_meters.x / world_voxel_size);
    }
    let texels = max(pattern_texels(layer), 1u);
    let column = i32(floor(local_u * f32(texels)));
    let local_y = fract(sample.world_meters.y / world_voxel_size);
    let quantised_y = (floor(local_y * f32(texels)) + 0.5) / f32(texels);
    let block = vec3<i32>(floor(sample.world_meters / world_voxel_size));
    var cell = vec3<i32>(0, 0, 0);
    if (sample.axis == 0u) {
        cell = vec3<i32>(block.x, block.y, column);
    } else {
        cell = vec3<i32>(column, block.y, block.z);
    }
    let jitter = pattern_hash_cell(cell, salt) * 2.0 - 1.0;
    let width = clamp(layer.param_a, 0.0, 1.0);
    let jaggedness = clamp(layer.param_b, 0.0, 1.0);
    let edge = clamp(width * (1.0 + jitter * jaggedness), 0.0, 1.0);
    if (pattern_edge_band_from_bottom(layer)) {
        return select(0.0, 1.0, quantised_y <= edge);
    }
    return select(0.0, 1.0, quantised_y >= 1.0 - edge);
}

fn pattern_generator_at(layer: PatternLayer, sample: PatternSample,
                        drift_meters: vec3<f32>, raw_point: vec3<f32>, salt: u32,
                        octaves: u32) -> f32 {
    let generator = pattern_generator(layer);
    if (pattern_generator_enabled(PATTERN_GENERATOR_EDGE_BAND)
        && generator == PATTERN_GENERATOR_EDGE_BAND) {
        return pattern_edge_band_value(layer, sample, brickmap.voxel_size_meters, salt);
    }
    let point = pattern_warp(raw_point, layer, salt);
    if (pattern_generator_enabled(PATTERN_GENERATOR_NOISE) && generator == PATTERN_GENERATOR_NOISE) {
        return pattern_fractal_noise(point, octaves, salt);
    }
    if (pattern_generator_enabled(PATTERN_GENERATOR_SPECKLE) && generator == PATTERN_GENERATOR_SPECKLE) {
        return pattern_speckle(point, clamp(layer.param_a, 0.0, 1.0), salt);
    }
    if (pattern_generator_enabled(PATTERN_GENERATOR_PERLIN) && generator == PATTERN_GENERATOR_PERLIN) {
        return pattern_fractal_perlin(point, octaves, salt);
    }
    if (pattern_generator_enabled(PATTERN_GENERATOR_SIMPLEX) && generator == PATTERN_GENERATOR_SIMPLEX) {
        return pattern_fractal_simplex(point, octaves, salt);
    }
    if (pattern_generator_enabled(PATTERN_GENERATOR_RIDGED) && generator == PATTERN_GENERATOR_RIDGED) {
        return pattern_ridged_noise(point, octaves, salt);
    }
    if (pattern_generator_enabled(PATTERN_GENERATOR_TURBULENCE) && generator == PATTERN_GENERATOR_TURBULENCE) {
        return pattern_turbulence(point, octaves, salt);
    }
    if (pattern_generator_enabled(PATTERN_GENERATOR_WORLEY) && generator == PATTERN_GENERATOR_WORLEY) {
        return pattern_worley(point, salt);
    }
    if (pattern_generator_enabled(PATTERN_GENERATOR_WORLEY_EDGE) && generator == PATTERN_GENERATOR_WORLEY_EDGE) {
        return pattern_worley_edge(point, salt);
    }
    if (pattern_generator_enabled(PATTERN_GENERATOR_WORLEY_SMOOTH) && generator == PATTERN_GENERATOR_WORLEY_SMOOTH) {
        return pattern_worley_smooth(point, salt);
    }
    if (pattern_generator_enabled(PATTERN_GENERATOR_WAVE) && generator == PATTERN_GENERATOR_WAVE) {
        return pattern_wave(point, max(layer.param_a, 0.0), salt);
    }
    if (pattern_generator_enabled(PATTERN_GENERATOR_CHECKER) && generator == PATTERN_GENERATOR_CHECKER) {
        return pattern_checker(point);
    }
    // The two tessellation readouts. They re-walk rather than sharing the walk
    // `pattern_coordinate` already did, which is a deliberate ~15-op duplication:
    // threading a vec4 through the coordinate contract would complicate every frame
    // to save a floor and a hash against generators costing hundreds of ops.
    if (pattern_generator_enabled(PATTERN_GENERATOR_TILE_TONE) && generator == PATTERN_GENERATOR_TILE_TONE) {
        return pattern_tile_of(layer, sample, drift_meters).z;
    }
    if (pattern_generator_enabled(PATTERN_GENERATOR_TILE_EDGE) && generator == PATTERN_GENERATOR_TILE_EDGE) {
        return pattern_tile_edge_shaped(
            pattern_tile_of(layer, sample, drift_meters).w, layer.param_a);
    }
    return pattern_hash_cell(vec3<i32>(floor(point)), salt ^ PATTERN_FLAT_SALT);
}

// ---- Grid-average sampling ----------------------------------------------------
//
// The generator's EIGHT-OCTANT MEAN over the texel cell instead of the centre
// point sample. A point sample of a sub-texel-period noise hands every texel one
// independent random draw at FULL contrast; the mean concentrates the values
// around the field's local average — the calmer, solid-tone look. The taps sit at
// the centres of the cell's eight octants (±texel/4 along each axis around the
// snapped centre), the stratified box-filter estimate.
//
// Cost note: the result is a pure function of the snapped coordinate like every
// other generator value, so the texel cache holds it and the eightfold work is
// paid once per texel rather than once per pixel.

// Whether grid averaging changes anything for this layer. The tessellation
// readouts and the edge band ignore the mapped coordinate — their values are
// already constant per tile or per column, so eight taps would return eight
// copies of one number. Mirrors `grid_averaging_applies` and the generator
// early-returns in `generator_value_animated` on the CPU side.
fn pattern_grid_average_applies(layer: PatternLayer) -> bool {
    if ((layer.flags_extra & 1u) == 0u || pattern_texels(layer) == 0u
        || pattern_frame(layer) == PATTERN_FRAME_TILE) {
        return false;
    }
    let generator = pattern_generator(layer);
    return generator != PATTERN_GENERATOR_TILE_TONE
        && generator != PATTERN_GENERATOR_TILE_EDGE
        && generator != PATTERN_GENERATOR_EDGE_BAND;
}

// Mirrors `grid_averaged_value` in pattern.rs. One deliberate divergence: the
// offsets are sized from the ACTIVE texel grid, so a distance-coarsened cell is
// averaged over its own extent rather than over an eighth of it — the CPU
// reference has no texel LOD to coarsen.
fn pattern_grid_averaged_value(layer: PatternLayer, sample: PatternSample,
                               drift_meters: vec3<f32>, raw_point: vec3<f32>,
                               salt: u32, octaves: u32,
                               voxel_size_meters: f32) -> f32 {
    let texels = pattern_texels_at(layer, sample.distance_meters);
    let texel_meters = (voxel_size_meters * BRICK_SIZE) / f32(max(texels, 1u));
    let quarter = texel_meters * 0.25 / max(layer.period_meters, 1e-4);
    var total = 0.0;
    for (var corner = 0u; corner < 8u; corner = corner + 1u) {
        let tap = raw_point + quarter * vec3<f32>(
            select(-1.0, 1.0, (corner & 1u) != 0u),
            select(-1.0, 1.0, (corner & 2u) != 0u),
            select(-1.0, 1.0, (corner & 4u) != 0u),
        );
        total = total
            + pattern_generator_at(layer, sample, drift_meters, tap, salt, octaves);
    }
    return total * 0.125;
}

// The generator value with the layer's sampling mode applied — the one call site
// the cached and uncached paths share, so neither can forget the average.
fn pattern_generator_resolved(layer: PatternLayer, sample: PatternSample,
                              drift_meters: vec3<f32>, raw_point: vec3<f32>,
                              salt: u32, octaves: u32,
                              voxel_size_meters: f32) -> f32 {
    if (pattern_grid_average_applies(layer)) {
        return pattern_grid_averaged_value(
            layer, sample, drift_meters, raw_point, salt, octaves, voxel_size_meters);
    }
    return pattern_generator_at(layer, sample, drift_meters, raw_point, salt, octaves);
}

// Domain warping (iq, "domain warping"): sample a noise field at the point and
// displace the point by it before the generator ever sees it. `fbm(p + fbm(p))`.
//
// Applies to EVERY generator rather than being a generator of its own, which is the
// whole reason it earns a packed bit instead of a code: warped Worley is cracked
// stone, warped wave is wood grain, warped checker is a rippled tile floor. Twelve
// generators times on/off is a bigger library than twelve plus one.
//
// Three offset lattice reads rather than three independent noises: one
// `pattern_value_noise` per axis, at large fixed offsets so the three components
// decorrelate.
//
// That is 24 hashes, i.e. THREE octaves and not one — measured at +0.73 ms against
// a 3-octave noise layer's own +0.72 ms, so a warped layer costs about twice an
// unwarped one. Worth stating plainly because the arithmetic invites the wrong
// guess: a warp is not a cheap garnish on a generator, it is a second generator's
// worth of work in front of it.
const PATTERN_WARP_OFFSET_Y: vec3<f32> = vec3<f32>(31.416, 7.913, 19.264);
const PATTERN_WARP_OFFSET_Z: vec3<f32> = vec3<f32>(-13.077, 41.502, 5.731);
const PATTERN_WARP_SALT: u32 = 51u;

fn pattern_warp(point: vec3<f32>, layer: PatternLayer, salt: u32) -> vec3<f32> {
    if (!pattern_warps(layer)) {
        return point;
    }
    let strength = layer.param_b;
    if (strength == 0.0) {
        return point;
    }
    let warp_salt = salt ^ PATTERN_WARP_SALT;
    let displacement = vec3<f32>(
        pattern_value_noise(point, warp_salt),
        pattern_value_noise(point + PATTERN_WARP_OFFSET_Y, warp_salt),
        pattern_value_noise(point + PATTERN_WARP_OFFSET_Z, warp_salt),
    );
    // Centre on zero so the warp pushes both ways; an uncentred warp would slide the
    // whole pattern along the diagonal as strength rises, which reads as a bug.
    return point + (displacement * 2.0 - vec3<f32>(1.0)) * strength;
}

// The octave count this layer actually sums here: what it authored, cut down by
// the distance budget when PATTERN_OCTAVE_LOD is on. Folds to `pattern_octaves`
// exactly when the lever is off, which is what keeps the shipped output
// bit-identical.
fn pattern_layer_octaves(layer: PatternLayer, sample: PatternSample) -> u32 {
    return pattern_octave_budget(
        pattern_octaves(layer), layer.period_meters, sample.distance_meters);
}

// ---- Fade, mask and strength ------------------------------------------------

// How much of this layer survives at this distance, 0..1. Applied to the AMOUNT, so
// a faded layer converges on the material's unpatterned base rather than on grey.
fn pattern_fade(layer: PatternLayer, distance_meters: f32) -> f32 {
    if (MATERIAL_PATTERN_ENTRY_PROBE >= PATTERN_ENTRY_NO_FADE) {
        return 1.0;
    }
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

fn pattern_relief_covers_face(layer: PatternLayer, axis: u32, axis_sign: f32) -> bool {
    let mask = pattern_relief_face_mask(layer);
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
    if (MATERIAL_PATTERN_ENTRY_PROBE >= PATTERN_ENTRY_NO_STRENGTH) {
        // Still per-layer and still non-zero, so the blend below cannot fold and
        // the `strength <= 0.0` early-out cannot start firing — which would
        // silently delete the rest of the layer instead of pricing it. What goes
        // is the face-mask test and the animation gain.
        return clamp(layer.amount, 0.0, 1.0);
    }
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

// Displacement is a separate modifier from channel blending, so its mask is not
// scaled by a Pattern Layer's albedo/roughness/emission amount. The node owns the
// physical height; this helper only applies face scope, distance fade, and graph
// animation to that height field.
fn pattern_relief_strength(layer: PatternLayer, sample: PatternSample, gain: f32) -> f32 {
    if (!pattern_relief_covers_face(layer, sample.axis, sample.axis_sign)) {
        return 0.0;
    }
    return max(gain, 0.0)
        * MATERIAL_PATTERN_STRENGTH
        * pattern_fade(layer, sample.distance_meters);
}

// The generator as a HEIGHT mask: the raw value, or its complement when the
// displacement asked for the low end of the mask to be the raised one, then
// quantised into `relief_steps` flat levels. The quantise is what turns a
// continuous mask — every texel border a small random tilt, a face of wash —
// into plateaus with few, full-strength bevels: the normal-map look. Mirrors
// `relief_mask_value` in pattern.rs.
fn pattern_relief_value(layer: PatternLayer, sample: PatternSample,
                        voxel_size_meters: f32, drift_meters: vec3<f32>) -> f32 {
    var value = pattern_generator_value(layer, sample, voxel_size_meters, drift_meters);
    if (pattern_relief_inverted(layer)) {
        value = 1.0 - value;
    }
    if (layer.relief_steps >= 2u) {
        let count = f32(layer.relief_steps);
        value = floor(min(value * count, count - 1.0)) / (count - 1.0);
    }
    return value;
}

fn pattern_relief_height(material: u32, sample: PatternSample,
                         voxel_size_meters: f32,
                         animation: PatternAnimation) -> f32 {
    let flags = materials[material].flags;
    if (!MATERIAL_PATTERNS || (flags & MATERIAL_FLAG_PATTERNS) == 0u) {
        return 0.0;
    }
    let count = min(
        min(material_pattern_count(flags), MATERIAL_PATTERN_MAX_LAYERS),
        MAX_PATTERN_LAYERS,
    );
    var height = 0.0;
    for (var slot = 0u; slot < count; slot = slot + 1u) {
        if (!material_debug_layer_enabled(slot)) {
            continue;
        }
        let layer = materials[material].patterns[slot];
        // Height is the displacement output. It remains active even when the
        // author disables the optional derived normal.
        if (layer.relief_height_meters <= 0.0) {
            continue;
        }
        let gain = pattern_animation_gain(animation, slot);
        let strength = pattern_relief_strength(layer, sample, gain);
        let drift = pattern_animation_drift(animation, slot);
        let drift_meters = pattern_drift_meters(
            layer, drift, voxel_size_meters, sample.distance_meters);
        let value = pattern_relief_value(layer, sample, voxel_size_meters, drift_meters);
        height = height + layer.relief_height_meters * strength * value;
    }
    return height;
}

// Derive a tangent-space normal from the same snapped generator mask that drives
// albedo, roughness, and emission. This is an embossed-material experiment: the
// voxel silhouette stays unchanged, while the light responds as if selected
// texels were raised above the face. Because the height lives on PatternLayer,
// an emission speckle or any other mask can opt into the same relief independently.
fn pattern_relief_normal(material: u32, sample: PatternSample,
                         base_normal: vec3<f32>, voxel_size_meters: f32,
                         animation: PatternAnimation) -> vec3<f32> {
    let flags = materials[material].flags;
    if (!MATERIAL_PATTERNS || (flags & MATERIAL_FLAG_PATTERNS) == 0u) {
        return base_normal;
    }
    let count = min(
        min(material_pattern_count(flags), MATERIAL_PATTERN_MAX_LAYERS),
        MAX_PATTERN_LAYERS,
    );
    var tangent_u = vec3<f32>(1.0, 0.0, 0.0);
    var tangent_v = vec3<f32>(0.0, 1.0, 0.0);
    if (sample.axis == 0u) {
        tangent_u = vec3<f32>(0.0, 0.0, 1.0);
    } else if (sample.axis == 1u) {
        tangent_v = vec3<f32>(0.0, 0.0, 1.0);
    }
    var result = base_normal;
    let world_voxel_size = voxel_size_meters * BRICK_SIZE;
    for (var slot = 0u; slot < count; slot = slot + 1u) {
        if (!material_debug_layer_enabled(slot)) {
            continue;
        }
        let layer = materials[material].patterns[slot];
        // Normal derivation is an independent opt-in. The same height field
        // may still lift the surface while leaving its lighting normal alone.
        if (layer.relief_height_meters <= 0.0 || !pattern_relief_normal_enabled(layer)) {
            continue;
        }
        let texels = pattern_texels_at(layer, sample.distance_meters);
        // SUB-texel step. The height field is held flat across each texel, so a
        // full-texel step reads two neighbouring plateaus from everywhere inside
        // a texel and smears the discontinuity into a weak rolling gradient. A
        // fractional step keeps texel interiors flat (both taps land on the same
        // plateau) and concentrates the full height difference into a bevel
        // 2x this step wide at the borders — the emboss look. The fraction is
        // per-layer, authored on the Displacement node (at a full-texel step the
        // grass relief measured a mean tilt of 2.9 degrees — invisible; at 1/8
        // texel the same heights measure p95 29 degrees).
        let step = world_voxel_size / f32(max(texels, 1u))
            * layer.relief_bevel_fraction;
        let height_scale = layer.relief_height_meters
            * pattern_relief_strength(layer, sample, pattern_animation_gain(animation, slot));
        if (height_scale <= 0.0) {
            continue;
        }
        var u_plus = sample;
        var u_minus = sample;
        var v_plus = sample;
        var v_minus = sample;
        u_plus.world_meters = u_plus.world_meters + tangent_u * step;
        u_minus.world_meters = u_minus.world_meters - tangent_u * step;
        v_plus.world_meters = v_plus.world_meters + tangent_v * step;
        v_minus.world_meters = v_minus.world_meters - tangent_v * step;
        let drift = pattern_animation_drift(animation, slot);
        let drift_meters = pattern_drift_meters(
            layer, drift, voxel_size_meters, sample.distance_meters);
        let h_u_plus = pattern_relief_value(
            layer, u_plus, voxel_size_meters, drift_meters) * height_scale;
        let h_u_minus = pattern_relief_value(
            layer, u_minus, voxel_size_meters, drift_meters) * height_scale;
        let h_v_plus = pattern_relief_value(
            layer, v_plus, voxel_size_meters, drift_meters) * height_scale;
        let h_v_minus = pattern_relief_value(
            layer, v_minus, voxel_size_meters, drift_meters) * height_scale;
        // The strength multiplies the TILT rather than the height, so it shapes
        // the emboss lighting without feeding back into the height response the
        // traversal reads.
        let du = (h_u_plus - h_u_minus) / max(2.0 * step, 1e-5)
            * layer.relief_normal_strength;
        let dv = (h_v_plus - h_v_minus) / max(2.0 * step, 1e-5)
            * layer.relief_normal_strength;
        result = normalize(result - tangent_u * du - tangent_v * dv);
    }
    return result;
}

// ---- P1: the parallax occlusion march -----------------------------------------
//
// Classic POM adapted to a procedural, texel-quantised height field: intersect
// the ray with the virtual CEILING plane (face + tallest possible relief), walk
// it down toward the face sampling `pattern_relief_height`, and refine the
// crossing. Every tap goes through the texel cache, so marching the same wall
// costs generator work once per texel, not once per pixel.

// The tallest the material's relief can be (the sum of the authored heights —
// an upper bound; strength, faces and masks only lower it), plus the finest
// texel grid any relief layer samples on. The march prices itself from both:
// the ceiling bounds the offset, the grid tells it how many DISTINCT height
// values a tangential slide can even cross.
struct ReliefProfile {
    ceiling_meters: f32,
    finest_texels: u32,
}

fn pattern_relief_profile(material: u32) -> ReliefProfile {
    var profile: ReliefProfile;
    profile.ceiling_meters = 0.0;
    profile.finest_texels = 1u;
    let flags = materials[material].flags;
    if (!MATERIAL_PATTERNS || (flags & MATERIAL_FLAG_PATTERNS) == 0u) {
        return profile;
    }
    let count = min(
        min(material_pattern_count(flags), MATERIAL_PATTERN_MAX_LAYERS),
        MAX_PATTERN_LAYERS,
    );
    for (var slot = 0u; slot < count; slot = slot + 1u) {
        // Inactive slots author zero height, so summing the fixed row is safe.
        let height = materials[material].patterns[slot].relief_height_meters;
        if (height <= 0.0) {
            continue;
        }
        profile.ceiling_meters = profile.ceiling_meters + height;
        // A continuous (texels 0) relief layer — the tile plates — changes at
        // tile joints rather than texel borders; treat it as the default grid
        // so the budget stays honest without pricing every joint.
        var texels = pattern_texels(materials[material].patterns[slot]);
        if (texels == 0u) {
            texels = 8u;
        }
        profile.finest_texels = max(profile.finest_texels, texels);
    }
    return profile;
}

struct ReliefMarch {
    // Whether the march moved the shading point at all.
    displaced: bool,
    // The sample to shade with: `world_meters` slid along the face plane to
    // where the ray lands on the relief. Voxel, face and distance are the
    // original hit's — a P1 simplification that leaves voxel- and face-frame
    // layers unaware of a cross-voxel slide; world-frame layers, which are what
    // plate materials author, continue seamlessly.
    sample: PatternSample,
    // Height of the landing point above the face, in metres. The self-shadow
    // march starts here.
    height_meters: f32,
    // The landing surface: a plateau top keeps the face's derived normal; a
    // plate SIDE gets the wall's own axis normal.
    hit_wall: bool,
    wall_normal: vec3<f32>,
}

// The two world axes spanning the face, matching `pattern_relief_normal`.
fn pattern_face_tangent_u(axis: u32) -> vec3<f32> {
    if (axis == 0u) { return vec3<f32>(0.0, 0.0, 1.0); }
    return vec3<f32>(1.0, 0.0, 0.0);
}

fn pattern_face_tangent_v(axis: u32) -> vec3<f32> {
    if (axis == 1u) { return vec3<f32>(0.0, 0.0, 1.0); }
    return vec3<f32>(0.0, 1.0, 0.0);
}

fn pattern_parallax_march(material: u32, base_sample: PatternSample,
                          geometric_normal: vec3<f32>, ray_direction: vec3<f32>,
                          voxel_size_meters: f32,
                          animation: PatternAnimation) -> ReliefMarch {
    var result: ReliefMarch;
    result.displaced = false;
    result.sample = base_sample;
    result.height_meters = 0.0;
    result.hit_wall = false;
    result.wall_normal = geometric_normal;
    if (!MATERIAL_PARALLAX || MATERIAL_PARALLAX_SAMPLES == 0u) {
        return result;
    }
    // The distance cap comes BEFORE the profile loop: far pixels are most
    // pixels, and they should pay one compare, not a row walk.
    if (base_sample.distance_meters > MATERIAL_PARALLAX_END_METERS) {
        return result;
    }
    let profile = pattern_relief_profile(material);
    let ceiling = profile.ceiling_meters;
    if (ceiling <= 0.0) {
        return result;
    }
    // How fast the ray descends toward the face; a grazing ray gets a huge
    // lateral slide per metre of height, which the sample budget still bounds.
    let descent = -dot(ray_direction, geometric_normal);
    if (descent <= 1e-4) {
        return result;
    }
    // Tangential slide per metre of height ABOVE the face: walking the ray
    // backwards from the face hit, staying on the face plane.
    let slide = -(ray_direction / descent + geometric_normal);
    // Probes are CLAMPED to the hit block's own face, the way Minecraft POM
    // works per block. Unclamped, a slide across the block boundary wraps every
    // voxel-local generator — the edge band drew its lip in the middle of the
    // face — and marches phantom relief where no neighbour exists. World-frame
    // plates give up cross-block parallax continuity for it, which reads fine.
    let world_voxel_size = voxel_size_meters * BRICK_SIZE;
    let block_min = vec3<f32>((base_sample.voxel / vec3<i32>(8)) * vec3<i32>(8))
        * voxel_size_meters;
    // The upper bound backs off a hair: a probe clamped EXACTLY onto the block
    // boundary wraps every `fract`-based local coordinate to zero — the edge
    // band's field vanished at the top edge and the march tunnelled straight
    // past the lip it was supposed to land on.
    let block_max = block_min + vec3<f32>(world_voxel_size - 1e-4);
    // The budget prices the march by what it can actually resolve: the field is
    // piecewise constant on the finest relief grid, so a slide crossing two
    // texel columns cannot need twenty-four taps. Near-perpendicular views —
    // most terrain pixels — collapse to the floor of four; the authored cap
    // only pays off at grazing angles, where the slide really does cross many
    // columns. Two taps per column: one to find the plateau, one for the refine
    // to bracket its wall.
    let texel_meters = world_voxel_size / f32(profile.finest_texels);
    let slide_texels = length(slide) * ceiling / texel_meters;
    let samples = clamp(u32(slide_texels * 2.0) + 2u, 4u, MATERIAL_PARALLAX_SAMPLES);
    var above_height = ceiling;
    var below_height = -1.0;
    for (var step = 1u; step <= samples; step = step + 1u) {
        let height = ceiling * (1.0 - f32(step) / f32(samples));
        var probe = base_sample;
        probe.world_meters = clamp(
            base_sample.world_meters + slide * height, block_min, block_max);
        let field = pattern_relief_height(material, probe, voxel_size_meters, animation);
        if (field >= height) {
            below_height = height;
            break;
        }
        above_height = height;
    }
    if (below_height < 0.0) {
        // The ray reached the face without meeting the relief: shade the
        // original hit.
        return result;
    }
    // Binary refine between the last point above the field and the first at or
    // below it.
    var low = below_height;
    var high = above_height;
    for (var iteration = 0u; iteration < MATERIAL_PARALLAX_REFINE;
         iteration = iteration + 1u) {
        let middle = 0.5 * (low + high);
        var probe = base_sample;
        probe.world_meters = clamp(
            base_sample.world_meters + slide * middle, block_min, block_max);
        let field = pattern_relief_height(material, probe, voxel_size_meters, animation);
        if (field >= middle) {
            low = middle;
        } else {
            high = middle;
        }
    }
    var landed = base_sample;
    landed.world_meters = clamp(
        base_sample.world_meters + slide * low, block_min, block_max);
    let landed_field = pattern_relief_height(material, landed, voxel_size_meters, animation);
    result.displaced = true;
    result.sample = landed;
    result.height_meters = low;
    // A landing point well BELOW the local plateau top means the ray stopped on
    // the vertical wall between two plateaus rather than on a top. The wall
    // faces back against the ray's slide, along the slide's dominant axis.
    if (landed_field - low > max(0.002, 0.02 * ceiling)) {
        result.hit_wall = true;
        let tangent_u = pattern_face_tangent_u(base_sample.axis);
        let tangent_v = pattern_face_tangent_v(base_sample.axis);
        let slide_u = dot(slide, tangent_u);
        let slide_v = dot(slide, tangent_v);
        // The march advances tangentially OPPOSITE the slide (the slide walks
        // the ray backwards), so a blocking wall faces ALONG the slide — back
        // toward the camera. The first version had this negated, and every
        // crevice wall was backfacing: no sun, no ambient, pure black cracks.
        if (abs(slide_u) >= abs(slide_v)) {
            result.wall_normal = tangent_u * sign(slide_u);
        } else {
            result.wall_normal = tangent_v * sign(slide_v);
        }
    }
    return result;
}

// P2 — relief self-shadowing: march from the displaced point toward the sun
// through the same height field. Returns a visibility factor for the DIRECT sun
// term only; ambient and GI stay untouched. Soft rather than binary: the
// deeper the sun ray dips below a blocking plateau, the darker, which reads as
// a contact shadow at plate joints without a second shadow map.
fn pattern_parallax_sun_shadow(material: u32, displaced_sample: PatternSample,
                               start_height_meters: f32,
                               geometric_normal: vec3<f32>,
                               sun_direction: vec3<f32>,
                               voxel_size_meters: f32,
                               animation: PatternAnimation) -> f32 {
    if (!MATERIAL_PARALLAX || MATERIAL_PARALLAX_SHADOW_SAMPLES == 0u) {
        return 1.0;
    }
    // Same order as the march: one compare before any row walk.
    if (displaced_sample.distance_meters > MATERIAL_PARALLAX_END_METERS) {
        return 1.0;
    }
    let profile = pattern_relief_profile(material);
    let ceiling = profile.ceiling_meters;
    if (ceiling <= 0.0) {
        return 1.0;
    }
    let ascent = dot(sun_direction, geometric_normal);
    if (ascent <= 1e-4) {
        // The face's own n·l already handles a sun at or below its horizon.
        return 1.0;
    }
    // Tangential slide of the sun ray per metre of CLIMB above the face.
    let slide = sun_direction / ascent - geometric_normal;
    let climb_total = ceiling - start_height_meters;
    if (climb_total <= 0.0) {
        return 1.0;
    }
    // The same per-block clamp the march applies, for the same reasons —
    // including the backed-off upper bound that keeps `fract` from wrapping.
    let world_voxel_size = voxel_size_meters * BRICK_SIZE;
    let block_min = vec3<f32>((displaced_sample.voxel / vec3<i32>(8)) * vec3<i32>(8))
        * voxel_size_meters;
    let block_max = block_min + vec3<f32>(world_voxel_size - 1e-4);
    // Budgeted like the view march: the shadow ray's tangential run, priced in
    // finest-grid texels.
    let texel_meters = world_voxel_size / f32(profile.finest_texels);
    let slide_texels = length(slide) * climb_total / texel_meters;
    let samples = clamp(
        u32(slide_texels * 2.0) + 2u, 4u, MATERIAL_PARALLAX_SHADOW_SAMPLES);
    var deepest = 0.0;
    for (var step = 1u; step <= samples; step = step + 1u) {
        let climb = climb_total * f32(step) / f32(samples);
        let height = start_height_meters + climb;
        var probe = displaced_sample;
        probe.world_meters = clamp(
            displaced_sample.world_meters + slide * climb, block_min, block_max);
        let field = pattern_relief_height(material, probe, voxel_size_meters, animation);
        deepest = max(deepest, field - height);
    }
    // Full shadow once the sun ray is buried a quarter of the relief ceiling.
    return clamp(1.0 - deepest / max(0.25 * ceiling, 1e-4), 0.0, 1.0);
}

// ---- Blends -----------------------------------------------------------------

fn pattern_apply_color(layer: PatternLayer, base: vec3<f32>, sample: PatternSample,
                       voxel_size_meters: f32, gain: f32,
                       drift_velocity: vec3<f32>) -> vec3<f32> {
    let strength = pattern_strength(layer, sample, gain);
    if (strength <= 0.0) {
        return base;
    }
    let drift_meters = pattern_drift_meters(layer, drift_velocity, voxel_size_meters, sample.distance_meters);
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
    let drift_meters = pattern_drift_meters(layer, drift_velocity, voxel_size_meters, sample.distance_meters);
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
    // Column exposure, side faces only (a top face needs no lip). One
    // occupancy probe per neighbour, at the hit's own detail column just past
    // the block boundary — out-of-world counts as empty, which is the right
    // answer at the world's rim.
    sample.exposure = PATTERN_EXPOSURE_ALL;
    if (hit.axis != 1u) {
        let block_bottom = (hit.voxel.y / 8) * 8;
        let above = vec3<i32>(hit.voxel.x, block_bottom + 8, hit.voxel.z);
        let below = vec3<i32>(hit.voxel.x, block_bottom - 1, hit.voxel.z);
        sample.exposure = 0u;
        if (!voxel_occupied(above)) {
            sample.exposure = sample.exposure | PATTERN_EXPOSURE_TOP;
        }
        if (!voxel_occupied(below)) {
            sample.exposure = sample.exposure | PATTERN_EXPOSURE_BOTTOM;
        }
    }
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
// The complete surface payload that reaches the lighting model. Pattern layers
// currently author base color, roughness, and emission; the other PBR channels
// still travel through this same value unchanged. Keeping the payload intact is
// important: a modifier must not accidentally turn a PBR surface back into a
// loose collection of shader locals.
struct PbrSurface {
    base_color: vec3<f32>,
    roughness: f32,
    specular: f32,
    ambient_occlusion: f32,
    normal: vec3<f32>,
    emission: vec3<f32>,
}

fn material_pattern_surface_from_base(material: u32, sample: PatternSample,
                                      base: PbrSurface,
                                      animation: PatternAnimation) -> PbrSurface {
    var surface = base;
    let flags = materials[material].flags;
    if (!MATERIAL_PATTERNS || (flags & MATERIAL_FLAG_PATTERNS) == 0u) {
        return surface;
    }
    var count = min(
        min(material_pattern_count(flags), MATERIAL_PATTERN_MAX_LAYERS),
        MAX_PATTERN_LAYERS,
    );
    if (MATERIAL_PATTERN_ENTRY_PROBE >= PATTERN_ENTRY_NO_LAYERS) {
        // The ladder's floor, and its closure check: everything that survives here
        // is the flags read and the loop that never runs, so this rung must land on
        // the layers-off measurement. What the rung below it costs over this one is
        // the per-slot row load, the target branch and the blend.
        count = 0u;
    }
    let voxel_size_meters = brickmap.voxel_size_meters;
    for (var slot = 0u; slot < count; slot = slot + 1u) {
        if (!material_debug_layer_enabled(slot)) {
            continue;
        }
        let layer = materials[material].patterns[slot];
        let layer_target = pattern_target(layer);
        let gain = pattern_animation_gain(animation, slot);
        let drift = pattern_animation_drift(animation, slot);
        if (layer_target == PATTERN_TARGET_ALBEDO) {
            surface.base_color = pattern_apply_color(
                layer, surface.base_color, sample, voxel_size_meters, gain, drift);
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
fn material_pattern_surface(material: u32, sample: PatternSample) -> PbrSurface {
    var base: PbrSurface;
    base.base_color = material_face_albedo(material, sample.axis, sample.axis_sign);
    base.roughness = material_face_roughness(material, sample.axis, sample.axis_sign);
    base.specular = materials[material].specular;
    base.ambient_occlusion = 1.0;
    base.normal = vec3<f32>(0.0);
    base.emission = materials[material].emission;
    return material_pattern_surface_from_base(
        material,
        sample,
        base,
        pattern_animation_identity(),
    );
}
