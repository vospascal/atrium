// world.wgsl — the SHARED world-traversal prelude: brickmap bindings, the
// traversal + shadow levers, and the two-level DDA core. Concatenated in front
// of EVERY pass shader that traverses the world (`dda.wgsl` for the shading
// pass, `cagi.wgsl` for the E4 light-volume cellular automaton), so the DDA
// math and the traversal levers exist exactly once — a pass shader can never
// drift from the traversal the bench measures.
//
// Split out of `dda.wgsl` in E4 (it was that file's first two thirds). Nothing
// here knows about pixels, cameras, output textures or light volumes; the
// consumers add those in their own headers, which is why the camera (binding 0)
// and the output texture (binding 6) are absent below and their numbers are
// left free for the consumer.
//
// Shared bindings (group 0) — every consumer must expose exactly these:
//   1  uniform  BrickmapMeta  — brickmap.rs BrickmapMetadata (48 bytes)
//   2  storage  brick_indices — dense brick-pointer grid, x-major then y then
//                               z; 0xffffffff = empty brick
//   3  storage  occupancy_words — 16 u32 words per occupied brick; bit index
//                               = local_x + local_y*8 + local_z*64
//   4  storage  material_words — 128 u32 words per occupied brick; one byte
//                               per voxel, same local index, little-endian
//                               (byte 0 = bits 0..8)
//   5  storage  materials     — array<Material> (80 bytes/row), indexed by
//                               material id; see src/material.rs, which owns
//                               the table and pins this layout by test
//   7  uniform  Lighting      — lighting.rs LightingUniform (96 bytes; sun
//                               direction/color/intensity, hemisphere ambient,
//                               the runtime quality knobs, the E4 GI knobs)
//   8  storage  column_max_brick_y — per-XZ-brick-column max occupied brick
//                               Y (125 x 125 u32, x-major then z:
//                               column = x + z * grid_size.x); u32::MAX =
//                               empty column. E4 reuses it as the sky test.
//   9  storage  brick_occupancy_bits — one bit per brick cell (same x-major
//                               cell index as brick_indices; bit cell & 31 of
//                               word cell >> 5); set = brick occupied
//  15  storage  brick_bounds  — one u32 per brick cell: six 5-bit AADF
//                               directional bounds (-x, +x, -y, +y, -z, +z).
//                               Numbered 15 because 11-14 belong to the CAGI
//                               pass's own bindings.
//  10  storage  brick_skip_distances — one byte per brick cell, little-endian
//                               packed like material_words: chebyshev
//                               distance in bricks to the nearest occupied
//                               brick (0 = occupied, saturated at 255)
//
// All traversal happens in VOXEL-space units: a world-meter position must be
// divided by BrickmapMeta.voxel_size_meters before it enters `trace`.
//
// Traversal fast paths (Stage 2 optimization round, reworked after the
// bench_dda regression hunt — see examples/bench_dda.rs). NOTE: the shipped
// defaults are set by the lever block below; the column-height paths and the
// any-hit shadow loop are default-OFF (the chebyshev distance skip covers
// the same empty space better on M3 Max) but stay levered for per-GPU
// re-evaluation.
//
// - Column-height skip: binding 8 holds, per XZ brick column, the max
//   occupied brick Y (u32::MAX sentinel = empty column, which reads as -1
//   after i32 conversion so everything counts as "above" it). The coarse
//   loops HOIST the current column's max into a register and refresh it only
//   when a step crosses into a new XZ column (`face_axis != 1`), so vertical
//   stepping never touches the storage buffer. A ray above its column's max
//   can hit nothing in that column:
//     * heading UP — fast-forward straight to the next x/z column boundary
//       in one step (`column_fast_forward`) instead of climbing brick by
//       brick;
//     * heading DOWN — jump straight to the top plane of the column's
//       highest occupied brick (`descend_fast_forward`), which is what makes
//       top-down primary rays cheap (the empty air between the world ceiling
//       and the terrain collapses into one jump per column), or to the
//       lateral column exit when that comes first.
//   The next column's max is re-checked on entry either way, so geometry
//   taller than the ray farther along (a mountain behind a lake, a tree
//   crown one column over) still occludes correctly — fast-forward, NEVER
//   terminate: terminating at the first cleared column would drop long
//   low-sun shadows across water and valleys.
// - Global-height early exit: `BrickmapMeta.max_occupied_brick_y` — an
//   upward ray above the tallest brick in the WORLD can never hit; both
//   coarse loops break to a miss immediately. This also kills sky-pixel
//   primary rays above the island without walking the grid.
// - Empty-space acceleration (bindings 9/10): every brick carries a
//   chebyshev distance to the nearest occupied brick (byte-packed, binding
//   10). The coarse loops use it as BOTH the occupancy test (0 = occupied —
//   the 2 MB pointer grid is only read for occupied bricks) and the skip
//   stride: a brick at distance d sits centered in a guaranteed-empty cube
//   of half-width d-1 bricks, so the ray jumps to that cube's exit in ONE
//   re-seeded step (`distance_skip`) instead of crossing it brick by brick.
//   Binding 9 is a 1-bit-per-brick occupancy grid: an optional occupancy-test
//   lever for the traversal (default-off — no win on M3 Max) and the data
//   E1b's AO brick early-out reads.
//
// Modularity (plan architecture rule): BOTH trace loops (primary + shadow)
// and BOTH cell levels (brick + voxel) share one stepping core — `DdaState` +
// `dda_setup` + `dda_step` — so the DDA math exists exactly once. The
// variants differ only in what they do at an occupied cell: `trace` /
// `trace_brick` build a full `Hit` (material, face, distance, voxel);
// `trace_shadow` / `shadow_brick_occluded` return a bool at the first set
// occupancy bit. E4's CAGI injection calls `trace_shadow_visibility` for its
// per-cell sun test through this very file; the audio-ray port will reuse the
// same helpers.

struct BrickmapMeta {
    brick_grid_size: vec3<u32>,   // (125, 32, 125)
    occupied_brick_count: u32,
    world_size_voxels: vec3<u32>, // (1000, 256, 1000)
    voxel_size_meters: f32,       // 0.125
    max_occupied_brick_y: u32,    // tallest occupied brick Y in the world
    _pad5: u32,
    _pad6: u32,
    _pad7: u32,
}

// S3 — the per-layer animation values a material graph supplies to the pattern
// stack. One entry per pattern slot, in surface-chain order.
//
// It lives HERE, in the first concatenated file, so both pattern.wgsl and
// graph_prelude.wgsl can name it without depending on out-of-order module-scope
// resolution. The values ride in registers rather than the material row, so
// GpuPatternLayer stays 32 bytes and the table upload is unchanged.
struct PatternAnimation {
    // Multiplies each slot's authored amount. The authored `amount` keeps its
    // single meaning: this is a SEPARATE gain, so an unconnected socket is
    // plainly the identity rather than a second copy of the same number.
    gain: vec4<f32>,
    // Metres per second, world space, per slot. A velocity and not an offset —
    // the shader applies the clock — so a constant vector wired straight in is
    // a flow rather than a static displacement that merely looks like one.
    drift_velocity: array<vec4<f32>, 4>,
}

fn pattern_animation_identity() -> PatternAnimation {
    return PatternAnimation(
        vec4<f32>(1.0, 1.0, 1.0, 1.0),
        array<vec4<f32>, 4>(
            vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0)
        ),
    );
}

fn pattern_animation_gain(animation: PatternAnimation, slot: u32) -> f32 {
    if (slot == 0u) { return animation.gain.x; }
    if (slot == 1u) { return animation.gain.y; }
    if (slot == 2u) { return animation.gain.z; }
    return animation.gain.w;
}

struct Lighting {
    sun_direction: vec3<f32>,       // unit vector, surface -> sun
    _pad0: f32,
    sun_color_intensity: vec4<f32>, // rgb = linear sun color, w = intensity
    sky_ambient: vec4<f32>,         // rgb = linear sky ambient, w = strength
    ground_ambient: vec4<f32>,      // rgb = linear ground bounce, w unused
    // The RUNTIME quality levers (E1c; lighting.rs ShadingParams):
    //   x = AO strength [0, 1]                       (E1)
    //   y = soft-shadow penumbra scale               (E1b)
    //   z = AO distance-fade ramp start, voxel units (E1b lever 2)
    //   w = AO distance-fade ramp end, voxel units
    // Each is ignored when its lever is compiled off. Everything else is a
    // compile-time const in the lever blocks below, because it lives inside the
    // traversal loops or selects an estimator — see src/variants.rs for the
    // registry row, the measured verdict and which kind each lever is.
    shading_params: vec4<f32>,
    // The E4 CAGI runtime levers (lighting.rs GiParams):
    //   x = GI strength — multiplier on the sampled light volume
    //   y = ambient floor — how much of the E1c hemisphere ambient survives
    //       under CAGI (0 = the volume is the only indirect light)
    //   z = sun bounce fraction — the share of the sun's radiance a sunlit
    //       surface injects into the volume
    //   w = unused
    gi_params: vec4<f32>,
    // The E6 water runtime levers (lighting.rs WaterParams):
    //   x = extinction scale — multiplier on the per-meter absorption
    //       coefficients, i.e. how clear the water is
    //   y = scatter strength — what the absorbed light is replaced by
    //   z = ray cutoff — the smallest Fresnel weight worth a secondary ray
    //   w = reserved (B6's flow)
    water_params: vec4<f32>,
    // The E6 water LOOK levers, the two Pascal asked to be able to drag
    // (lighting.rs WaterParams again — a second vector because the first is full):
    //   x = refraction strength — how far the material's authored index of
    //       refraction is pulled toward 1.0, i.e. how WIDE Snell's window is
    //   y = tint — how coloured the water is (0 = neutral, 1 = physical)
    //   z, w = reserved (E7's water look pass)
    water_optics: vec4<f32>,
    // Day/night sky state (lighting.rs CelestialState). The active directional
    // light above becomes the moon at night; these retain both physical bodies
    // so the background, reflections and water all see the same sky.
    celestial_sun: vec4<f32>,  // xyz direction, w daylight
    celestial_moon: vec4<f32>, // xyz direction, w moon phase
    sky_zenith: vec4<f32>,     // rgb radiance, w star rotation
    sky_horizon: vec4<f32>,    // rgb radiance, w moonlight
    material_params: vec4<f32>, // x/y = absolute pattern fade start/end, metres
    // The S3 animation clock (lighting.rs AnimationParams). Split into whole
    // epochs and a remainder inside one, rather than one monotonic second
    // count: a single f32 loses the fraction an oscillator needs within hours
    // of uptime, and any wrapped single clock steps every rate that is not
    // harmonic with the wrap. src/animation_clock.rs carries the argument.
    //   x = seconds within the current epoch, [0, ANIMATION_EPOCH_SECONDS)
    //   y = whole epochs elapsed
    //   z = live world-event count — sensors loop to THIS, never to the
    //       array capacity, so a world with no entities costs one comparison
    //   w = reserved (the wind arc's global flow vector)
    animation_params: vec4<f32>,
}

@group(0) @binding(1) var<uniform> brickmap: BrickmapMeta;
@group(0) @binding(2) var<storage, read> brick_indices: array<u32>;
@group(0) @binding(3) var<storage, read> occupancy_words: array<u32>;
@group(0) @binding(4) var<storage, read> material_words: array<u32>;
// One row of the material table (src/material.rs `GpuMaterial`). Laid out as
// SIXTEEN 16-byte rows — each a vec3 followed by the scalar filling its w slot, or
// four scalars — so std430 adds no implicit padding and the Rust upload matches
// byte for byte. 256 bytes per row, 6.6 KB for the table.
//
// This is the FLAT form on purpose. The authored row on the CPU is a tagged union
// (`MaterialKind`: Air / Solid / Cover / Medium), because a sentinel there is
// indistinguishable from a real value to whoever is authoring it. Here the
// opposite is true: this is the hottest read in the renderer, so every field is
// present unconditionally and the shading path never branches to find out whether
// a column applies. `to_gpu()` expands the union and fills the sentinels in.
// S2 — layer slots per material row. Mirrors MAX_PATTERN_LAYERS in src/pattern.rs.
const MAX_PATTERN_LAYERS: u32 = 4u;

// S2 — one uploaded pattern layer: 32 bytes, two std430 16-byte rows. Mirrors
// `GpuPatternLayer` in src/pattern.rs.
//
// The LAYOUT lives here, with the rest of the row, because `struct Material` embeds
// it and both pass shaders share this file. The BEHAVIOUR — the generators, the
// sampling frames, the blends, and the three functions that apply a stack — lives in
// `shaders/pattern.wgsl`, which only the shading pass includes.
struct PatternLayer {
    // Generator, frame, target, blend, face mask and octave count, unpacked by the
    // accessors in pattern.wgsl.
    packed: u32,
    period_meters: f32,
    amount: f32,
    // The generator's first free parameter: speckle density today.
    param_a: f32,
    // The second colour, sRGB-encoded, for mix-to-colour and add. Its first channel
    // is the target VALUE for a scalar target.
    target_color: vec3<f32>,
    // The second free parameter. No generator reads it today; kept because the slot is
    // free anyway (the vec3 below needs its w filled) and the next generator with two
    // knobs would otherwise have to grow the row.
    param_b: f32,
}

struct Material {
    albedo: vec3<f32>,      // sRGB-encoded, as authored
    transmittance: f32,     // light passing THROUGH (transport; M2 reads it)
    emission: vec3<f32>,    // linear radiance; zero on every non-emitting row
    roughness: f32,
    opacity: f32,           // < 1.0 => traversal must continue through
    specular: f32,
    flags: u32,             // MATERIAL_FLAG_* below
    // E6: how hard this material bends a ray that ENTERS it, and (through
    // ((n-1)/(n+1))^2) how much it mirrors head-on. 1.0 = does not refract, the
    // honest value for every opaque row. Per-material because transparency is a
    // material CLASS — water 1.333, oil ~1.47, honey ~1.50.
    index_of_refraction: f32,
    // E6: the participating-medium PAIR, per metre, per channel. Absorption
    // destroys light; scattering redirects it, and is therefore the light a ray
    // picks up along its path. Extinction is their sum, and the medium's apparent
    // COLOUR is the derived single-scattering albedo scattering/extinction — so
    // nothing here paints the water. All-zero for anything a ray cannot enter.
    absorption_per_meter: vec3<f32>,
    _pad_absorption: f32,
    scattering_per_meter: vec3<f32>,
    _pad_scattering: f32,
    // S1: per-face-role values. On a row WITHOUT face roles all three hold the base
    // albedo/roughness, so a per-face read is the identity — which is why the
    // MATERIAL_FLAG_FACE_ROLES bit exists: it lets the shading path skip the
    // selection entirely rather than discover it is pointless three floats later.
    top_albedo: vec3<f32>,
    top_roughness: f32,
    side_albedo: vec3<f32>,
    side_roughness: f32,
    bottom_albedo: vec3<f32>,
    bottom_roughness: f32,
    // S2: the pattern stack, always MAX_PATTERN_LAYERS slots whatever the row
    // authored. Slots past the row's count (in the flag word, see
    // material_pattern_count) are zeroed with amount = 0, which is the identity even
    // if the count were ever wrong.
    patterns: array<PatternLayer, 4>,
}

// Mirrors `MaterialFlags` in src/material.rs, where these are DERIVED from the
// authored row's kind rather than written beside it — so a row can no longer
// claim LIQUID with no medium, or claim foliage while blocking all light.
const MATERIAL_FLAG_FOLIAGE: u32 = 1u;
const MATERIAL_FLAG_EMISSIVE: u32 = 2u;
const MATERIAL_FLAG_TRANSPARENT: u32 = 4u;
const MATERIAL_FLAG_LIQUID: u32 = 8u;
// S1: this row's top and bottom differ from its sides.
const MATERIAL_FLAG_FACE_ROLES: u32 = 16u;
// S2: this row has at least one pattern layer.
const MATERIAL_FLAG_PATTERNS: u32 = 32u;

// S2: how many pattern slots this row's stack fills. Carried in the flag word's
// bits 8-10 rather than in a field of its own, because a u32 count would cost a
// whole 16-byte std430 row (three quarters of it padding) for three bits.
fn material_pattern_count(flags: u32) -> u32 {
    return (flags >> 8u) & 0x7u;
}

// S1 — MATERIAL_FACE_ROLES is patched in by the variant registry. The shipped
// path reads authored per-face slots; rows without them are identical either way.
const MATERIAL_FACE_ROLES: bool = true;

// S1 — this hit's albedo, picked by which face was struck.
//
// The face is free: the DDA already records the axis it last stepped and the sign
// of the ray along it, because the analytic corner AO needed an integer face frame.
// `hit_normal` builds its normal as `-axis_sign` on `axis`, so a `+Y` normal — the
// top face — is `axis == 1` with a NEGATIVE sign, which is the one thing about this
// that reads backwards and is therefore worth stating.
fn material_face_albedo(material: u32, axis: u32, axis_sign: f32) -> vec3<f32> {
    let row = materials[material];
    if (!MATERIAL_FACE_ROLES || (row.flags & MATERIAL_FLAG_FACE_ROLES) == 0u) {
        return row.albedo;
    }
    if (axis != 1u) {
        return row.side_albedo;
    }
    return select(row.bottom_albedo, row.top_albedo, axis_sign < 0.0);
}

/// S1 — this hit's roughness, by the same rule.
fn material_face_roughness(material: u32, axis: u32, axis_sign: f32) -> f32 {
    let row = materials[material];
    if (!MATERIAL_FACE_ROLES || (row.flags & MATERIAL_FLAG_FACE_ROLES) == 0u) {
        return row.roughness;
    }
    if (axis != 1u) {
        return row.side_roughness;
    }
    return select(row.bottom_roughness, row.top_roughness, axis_sign < 0.0);
}

@group(0) @binding(5) var<storage, read> materials: array<Material>;
@group(0) @binding(7) var<uniform> lighting: Lighting;
@group(0) @binding(8) var<storage, read> column_max_brick_y: array<u32>;
@group(0) @binding(9) var<storage, read> brick_occupancy_bits: array<u32>;
@group(0) @binding(10) var<storage, read> brick_skip_distances: array<u32>;
@group(0) @binding(15) var<storage, read> brick_bounds: array<u32>;

// AADF field layout — mirrors BOUND_BITS / BOUND_DIRECTIONS in src/brickmap.rs.
// Six 5-bit bounds per brick cell in order -x, +x, -y, +y, -z, +z: how many
// FURTHER cells the ray may cross in that direction such that the whole box
// spanned by all six is empty.
const BOUND_BITS: u32 = 5u;
const BOUND_MASK: u32 = 31u;

// ---- A/B benchmark levers ----------------------------------------------------
// Compile-time flags (naga folds them; disabled paths cost nothing). The
// headless benchmark (`examples/bench_dda.rs`) string-patches individual
// flags to measure each traversal optimization in isolation and to
// reconstruct the unoptimized Stage 2 baseline. Defaults = the fastest
// combination measured on Apple M3 Max (see the bench table in the plan
// notes); re-run the bench before changing them, and re-evaluate on Quest 3
// hardware (Stage 6) — the trade-offs are architecture-specific.
//
// LEVER REGISTRY (E1c): every const in this block and the two below has one
// row in `src/variants.rs::REGISTRY` carrying its kind, its default, its
// measured verdict and the bench columns that sweep it — and a pinning test
// fails if a lever exists here and not there (or vice versa). The Rust mirrors
// that patch these consts are `src/traversal.rs`, `src/ao.rs`, `src/shadows.rs`,
// composed by `passes::dda::build_shader_source`. Losing levers stay compiled
// out but runnable (an M3 Max loss can be a Quest win); their implementations
// live in named functions OUTSIDE the traversal loops so the hot path reads as
// the algorithm.
//
// BOTH column-height fast paths default OFF: measured SLOWER in every
// scenario on M3 Max once the chebyshev distance skip landed. The upward
// lateral jump (COLUMN_FAST_FORWARD) already lost before that — shadow rays
// cross columns about once per step at typical elevations, so the jump saves
// few steps while its per-column resync math and scattered column reads cost
// more than a plain step. The downward plane jump (DESCEND_FAST_FORWARD) was
// the Stage 2 top-down win, but the distance field now covers the same empty
// air in ALL directions and turning the column machinery off besides saves
// its storage reads and branches (bench: -8% to -14% on every scenario).
const ENABLE_COLUMN_FAST_FORWARD: bool = false;
const ENABLE_DESCEND_FAST_FORWARD: bool = false;
const ENABLE_GLOBAL_MAX_TERMINATE: bool = true;
// Any-hit shadow rays measured a consistent 1-3% SLOWER than reusing the
// closest-hit trace on M3 Max (three bench rounds) — the second specialized
// coarse loop costs more than the material/face bookkeeping it skips.
const ENABLE_ANY_HIT_SHADOW: bool = false;
// 1-bit-per-brick occupancy grid: no measurable win on M3 Max — the skip
// distance byte already doubles as the occupancy test, making the bit read
// a second redundant load, and even standalone (no distance skip) it only
// matched the plain pointer read. Candidate to re-try on small-cache GPUs.
const ENABLE_BRICK_BIT_GRID: bool = false;
// Chebyshev empty-space skip: jump guaranteed-empty cubes in one step. The
// distance byte doubles as the occupancy test (0 = occupied), so an empty
// brick costs one byte-load from the 500 KB grid instead of a u32 from the
// 2 MB pointer grid — and skips whole cubes of empty air besides.
const ENABLE_DISTANCE_SKIP: bool = true;
// AADF directional skip: jump the box spanned by the cell's six directional
// bounds (binding 15) instead of the chebyshev cube. DOCUMENTED NEGATIVE on M3
// Max — default OFF; see the registry verdict in src/variants.rs for the four
// scenario numbers. The FIELD is better than chebyshev and that is measurable
// (27,578 grazing cells go from reach 0 to a mean 5.19 cells); reading it is
// what costs more than it returns:
//   - the chebyshev byte doubles as the occupancy test, so one load answers
//     both questions; the bound word is a SECOND load on top of it;
//   - 2 MB of bounds against 500 KB of distance bytes, so the cache-resident
//     grid stops being cache-resident;
//   - unpacking six 5-bit fields costs shifts where a byte compare costs one.
// Kept as a lever because the reach advantage is hardware-independent while the
// cache cost is not — Quest has a different hierarchy and may flip this.
// From NAADF (Ulschmid et al., CGF 2026, MIT).
const ENABLE_DIRECTIONAL_SKIP: bool = false;

// ---- E1b: shadow levers (technique bank T1) -----------------------------------
// 0 = hard (one binary any-hit shadow ray — the voxel-purist default and the
//     Stage 2 correctness gate's reference);
// 1 = soft from the chebyshev distance field: IQ's single-ray penumbra trick,
//     tracking min(penumbra_scale * clearance / t) along the SAME shadow ray
//     using the distance bytes the traversal already fetches. Penumbra scale
//     is the RUNTIME knob (lighting.shading_params.y).
const SHADOW_MODE_HARD: u32 = 0u;
const SHADOW_MODE_SOFT_DISTANCE_FIELD: u32 = 1u;
const SHADOW_MODE: u32 = 0u;
// Ray parameter (voxel units) below which the penumbra term is ignored: the
// shadow ray starts inside the hit voxel's own brick, whose conservative
// clearance is 0 by definition, so sampling there would blacken every lit
// pixel. One brick of lead-in is the minimum that works.
const SHADOW_PENUMBRA_MIN_DISTANCE: f32 = 8.0;

// Either column-height fast path needs the per-column max hoisted into a
// register (and the guaranteed-empty-brick skip that comes with it).
const USE_COLUMN_HEIGHTS: bool = ENABLE_COLUMN_FAST_FORWARD || ENABLE_DESCEND_FAST_FORWARD;

// ---- E6: the sun and liquids (lever) ------------------------------------------
// Whether the SUN's rays pass THROUGH liquids instead of stopping on them.
//
// Why it matters: with it off, every submerged surface is in shadow, so a pool
// bed one metre down is lit by ambient alone and shallow water reads DARKER than
// the opaque water it replaced (measured 2026-07-31 on the top-down lakes
// scenario: the refracted bed came out dark navy against opaque water's bright
// cyan). With it on, a sunlit pool bed is sunlit and the whole point of refraction
// — seeing the ground under the water — survives.
//
// Why it is a lever and not simply on: it is the single most expensive thing in
// E6, because a shadow ray that no longer stops at the water surface walks the
// whole body voxel by voxel (water bricks are occupied, so the coarse skip cannot
// help). Numbers and the per-tier verdict are in the registry row
// (`src/variants.rs`, `WaterSunThroughLiquid`) and the bench doc's E6 section.
//
// Why it lives HERE rather than in `water.wgsl`: the CA pass's per-cell sun test
// goes through the same `trace_shadow_visibility`, and it must agree — otherwise
// the light volume would shadow the bed the shading pass now lights. `world.wgsl`
// is the half both passes concatenate, so `WaterSettings::patch_shader_source`
// writes this const into whichever source it is handed.
//
// The simplification that ships with it: the sun's own path through the water is
// NOT attenuated by Beer-Lambert (only the camera's is), so a deep bed is lit as
// brightly as a shallow one. Correcting it needs the wet path length of the SUN
// ray, i.e. a second medium march per shaded point.
const WATER_SUN_THROUGH_LIQUID: bool = true;

const EMPTY_BRICK: u32 = 0xffffffffu;
// Level-0 pointer tagging — the GPU half of the scheme documented in
// src/brickmap.rs. Top two bits: 0b00 UNIQUE (payload = level-1 slot), 0b01
// UNIFORM (payload = material id, brick is that material in all 512 cells),
// 0b10 TEMPLATE (reserved, not emitted yet), 0b11 EMPTY (EMPTY_BRICK carries it
// already). UNIQUE is tag 0 so an untagged slot index stays a valid pointer.
const BRICK_TAG_SHIFT: u32 = 30u;
const BRICK_PAYLOAD_MASK: u32 = 0x3fffffffu;
const BRICK_TAG_UNIFORM: u32 = 1u;
const BRICK_SIZE: f32 = 8.0;
// Worst-case bricks crossed: 125 + 32 + 125 axis crossings plus slack.
const MAX_BRICK_STEPS: u32 = 512u;
// Worst-case voxels crossed inside one 8^3 brick: 8+8+8-2 = 22, plus slack.
const MAX_VOXEL_STEPS: u32 = 24u;
// Max trace distance, voxel units (world diagonal is ~1437).
const MAX_TRACE_DISTANCE: f32 = 2048.0;
const RAY_EPSILON: f32 = 1e-4;
// Shadow-ray offset from the hit face, voxel units (1e-3 voxel = 0.125 mm
// world). Must stay above RAY_EPSILON so the traversal's entry nudge cannot
// step the shadow origin back through the face it was lifted from.
const SHADOW_BIAS: f32 = 1e-3;

// ---- Shared color helpers ----------------------------------------------------

// sRGB <-> linear via the pow-2.2 approximation: a self-consistent pair, one
// pow each way, indistinguishable from the exact piecewise curve at 8 bits.
// Shared because the CAGI pass decodes table-derived albedo with the same
// curve the shading pass decodes material colors with.
fn srgb_decode(color: vec3<f32>) -> vec3<f32> {
    return pow(color, vec3<f32>(2.2, 2.2, 2.2));
}

fn srgb_encode(color: vec3<f32>) -> vec3<f32> {
    return pow(color, vec3<f32>(1.0 / 2.2, 1.0 / 2.2, 1.0 / 2.2));
}

struct Hit {
    material: u32,     // 0 = miss (Air is never occupied)
    axis: u32,         // face axis last stepped: 0 = x, 1 = y, 2 = z
    axis_sign: f32,    // sign of the ray direction along that axis
    distance: f32,     // ray parameter t, voxel units
    voxel: vec3<i32>,  // world-voxel coordinate of the hit voxel
}

// ---- Ray setup --------------------------------------------------------------

// 1/d with sign-preserving huge magnitude for near-zero components, so DDA
// t values along a non-moving axis land at +infinity-ish instead of NaN.
fn safe_inverse(direction_component: f32) -> f32 {
    if (abs(direction_component) < 1e-8) {
        return select(1e30, -1e30, direction_component < 0.0);
    }
    return 1.0 / direction_component;
}

// Global ray parameter t of the next cell boundary along one axis.
// cell_min is the low edge of the current cell; a positive step crosses at
// cell_min + cell_size, a negative step at cell_min.
fn boundary_t(origin_component: f32, inverse_direction: f32, cell_min: f32,
              cell_size: f32, step_direction: i32) -> f32 {
    var boundary = cell_min;
    if (step_direction > 0) {
        boundary = cell_min + cell_size;
    }
    return (boundary - origin_component) * inverse_direction;
}

// ---- World-bounds entry -----------------------------------------------------

// Slab-test the world AABB [0, world_size]^3 (voxel units).
// Returns (t_enter, t_exit, entry_axis); miss when t_enter > t_exit.
// t_enter is clamped to 0 for a camera already inside the bounds.
fn intersect_world_bounds(origin: vec3<f32>, inverse_direction: vec3<f32>) -> vec3<f32> {
    let world_max = vec3<f32>(brickmap.world_size_voxels);
    let t_low = (vec3<f32>(0.0, 0.0, 0.0) - origin) * inverse_direction;
    let t_high = (world_max - origin) * inverse_direction;
    let t_near = min(t_low, t_high);
    let t_far = max(t_low, t_high);

    var entry_axis = 0u;
    var t_enter = t_near.x;
    if (t_near.y > t_enter) {
        t_enter = t_near.y;
        entry_axis = 1u;
    }
    if (t_near.z > t_enter) {
        t_enter = t_near.z;
        entry_axis = 2u;
    }
    let t_exit = min(min(t_far.x, t_far.y), t_far.z);
    return vec3<f32>(max(t_enter, 0.0), t_exit, f32(entry_axis));
}

// ---- Shared DDA core ---------------------------------------------------------

// One Amanatides & Woo traversal state, used at BOTH levels: coarse (cell =
// brick coordinate, cell size 8) and fine (cell = world-voxel coordinate,
// cell size 1). `t`/`t_max` are global ray parameters in voxel units either
// way, so a fine traversal can be seeded directly from the coarse `t`.
struct DdaState {
    cell: vec3<i32>,
    face_axis: u32,        // axis of the boundary last crossed (entry face)
    t: f32,                // ray parameter at entry into `cell`
    t_max: vec3<f32>,      // ray parameter of the NEXT boundary per axis
    t_delta: vec3<f32>,    // ray parameter width of one cell per axis
    step_direction: vec3<i32>,
}

// Initialize a traversal at ray parameter `t_start` on a grid of
// `cell_size`-sized cells, clamping the start cell into [clamp_min, clamp_max].
fn dda_setup(origin: vec3<f32>, direction: vec3<f32>, inverse_direction: vec3<f32>,
             t_start: f32, cell_size: f32, clamp_min: vec3<i32>,
             clamp_max: vec3<i32>) -> DdaState {
    var state: DdaState;
    let start_point = origin + direction * (t_start + RAY_EPSILON);
    state.cell = clamp(vec3<i32>(floor(start_point / cell_size)), clamp_min, clamp_max);
    state.step_direction = vec3<i32>(
        select(-1, 1, direction.x >= 0.0),
        select(-1, 1, direction.y >= 0.0),
        select(-1, 1, direction.z >= 0.0),
    );
    state.t_max = vec3<f32>(
        boundary_t(origin.x, inverse_direction.x, f32(state.cell.x) * cell_size,
                   cell_size, state.step_direction.x),
        boundary_t(origin.y, inverse_direction.y, f32(state.cell.y) * cell_size,
                   cell_size, state.step_direction.y),
        boundary_t(origin.z, inverse_direction.z, f32(state.cell.z) * cell_size,
                   cell_size, state.step_direction.z),
    );
    state.t_delta = abs(inverse_direction) * cell_size;
    state.t = t_start;
    state.face_axis = 0u; // callers with a known entry face override this
    return state;
}

// Advance the state across the current cell's boundary along `axis`.
fn step_along_axis(state: ptr<function, DdaState>, axis: u32) {
    if (axis == 0u) {
        (*state).t = (*state).t_max.x;
        (*state).cell.x = (*state).cell.x + (*state).step_direction.x;
        (*state).t_max.x = (*state).t_max.x + (*state).t_delta.x;
    } else if (axis == 1u) {
        (*state).t = (*state).t_max.y;
        (*state).cell.y = (*state).cell.y + (*state).step_direction.y;
        (*state).t_max.y = (*state).t_max.y + (*state).t_delta.y;
    } else {
        (*state).t = (*state).t_max.z;
        (*state).cell.z = (*state).cell.z + (*state).step_direction.z;
        (*state).t_max.z = (*state).t_max.z + (*state).t_delta.z;
    }
    (*state).face_axis = axis;
}

// One standard DDA step: cross the nearest boundary. Branchless: the mask
// selects every axis whose boundary is nearest (bench: -1% to -5.4% vs the
// if/else chain on M3 Max), so an exact float tie steps diagonally through
// the shared edge in one go — the intermediate cells a two-step crossing
// would test are grazed with measure-zero overlap anyway. face_axis keeps
// the branchy version's x > y > z tie priority.
fn dda_step(state: ptr<function, DdaState>) {
    let t_max = (*state).t_max;
    let mask = t_max <= min(t_max.yzx, t_max.zxy);
    (*state).t = min(t_max.x, min(t_max.y, t_max.z));
    (*state).cell = (*state).cell + vec3<i32>(mask) * (*state).step_direction;
    (*state).t_max = t_max + vec3<f32>(mask) * (*state).t_delta;
    var axis = 2u;
    if (mask.x) {
        axis = 0u;
    } else if (mask.y) {
        axis = 1u;
    }
    (*state).face_axis = axis;
}

// ---- Column-height early exit (coarse level only) ----------------------------

// Max occupied brick Y of the XZ column holding `cell`. Empty columns store
// u32::MAX, which converts to -1 (modular u32 -> i32), so every brick Y
// counts as above. The coarse loops HOIST this value into a register and
// refresh it only when a step changes the XZ column (`face_axis != 1`) — a
// ray climbing or descending inside one column pays zero storage reads.
// Callers must pass an in-grid cell.
fn column_max_of(cell: vec3<i32>) -> i32 {
    let column = u32(cell.x) + u32(cell.z) * brickmap.brick_grid_size.x;
    return i32(column_max_brick_y[column]);
}

// Fast-forward an UPWARD ray that cleared its current column: nothing in
// this column can be hit anymore, so jump straight to the next x/z column
// boundary (skipping every brick-Y crossing in between) and resynchronize
// the vertical component to the jump target. The caller re-checks the NEW
// column's max on the next iteration, which keeps taller geometry farther
// along the ray occluding correctly — fast-forward, never terminate.
fn column_fast_forward(state: ptr<function, DdaState>, origin: vec3<f32>,
                       direction: vec3<f32>, inverse_direction: vec3<f32>) {
    if ((*state).t_max.x <= (*state).t_max.z) {
        step_along_axis(state, 0u);
    } else {
        step_along_axis(state, 2u);
    }
    // max() guards against float undershoot in the reconstruction re-testing
    // a brick row the incremental traversal already cleared.
    let jumped_y = i32(floor((origin.y + direction.y * ((*state).t + RAY_EPSILON)) / BRICK_SIZE));
    (*state).cell.y = max((*state).cell.y, jumped_y);
    (*state).t_max.y = boundary_t(origin.y, inverse_direction.y,
                                  f32((*state).cell.y) * BRICK_SIZE, BRICK_SIZE,
                                  (*state).step_direction.y);
}

// Fast-forward a DOWNWARD ray that is above every occupied brick of its
// column: nothing can be hit until the ray reaches the top plane of the
// column's highest occupied brick row, so jump straight to that plane in one
// step (this is what makes top-down primary rays cheap — the ~20 empty brick
// rows between the world ceiling and the terrain collapse into one jump; the
// plane crossing is exact integer-derived math, no float reconstruction).
// When a lateral column exit comes FIRST, jump there instead, exactly like
// `column_fast_forward`, and let the caller re-check the new column.
fn descend_fast_forward(state: ptr<function, DdaState>, origin: vec3<f32>,
                        direction: vec3<f32>, inverse_direction: vec3<f32>,
                        column_max_y: i32) {
    let plane_y = f32(column_max_y + 1) * BRICK_SIZE;
    let t_plane = (plane_y - origin.y) * inverse_direction.y;
    if (t_plane <= (*state).t_max.x && t_plane <= (*state).t_max.z) {
        (*state).cell.y = column_max_y;
        (*state).t = max(t_plane, (*state).t);
        (*state).t_max.y = boundary_t(origin.y, inverse_direction.y,
                                      f32(column_max_y) * BRICK_SIZE, BRICK_SIZE,
                                      (*state).step_direction.y);
        (*state).face_axis = 1u;
        return;
    }
    if ((*state).t_max.x <= (*state).t_max.z) {
        step_along_axis(state, 0u);
    } else {
        step_along_axis(state, 2u);
    }
    // min() mirrors the upward variant's max(): reconstruction undershoot
    // may only re-test a row, never re-ascend past the incremental state.
    let jumped_y = i32(floor((origin.y + direction.y * ((*state).t + RAY_EPSILON)) / BRICK_SIZE));
    (*state).cell.y = min((*state).cell.y, jumped_y);
    (*state).t_max.y = boundary_t(origin.y, inverse_direction.y,
                                  f32((*state).cell.y) * BRICK_SIZE, BRICK_SIZE,
                                  (*state).step_direction.y);
}

// What the caller must do after the coarse-loop height levers ran: test the
// brick it is standing in, take another loop iteration (a fast-forward moved
// the state), or stop as a miss.
const HEIGHT_LEVER_TEST_BRICK: u32 = 0u;
const HEIGHT_LEVER_CONTINUE: u32 = 1u;
const HEIGHT_LEVER_MISS: u32 = 2u;

// All height-based coarse-loop levers in ONE place (E1c): the hoisted column
// max refresh, the global-max sky-out, and both column fast-forwards. Both
// coarse loops call this instead of carrying twenty lines of nested `if` each,
// so the loop body reads as the algorithm — and with the shipped defaults
// (column heights off, global max on) naga folds this down to the single
// `cell.y > world_max_brick_y` compare.
//
// `column_max_y` and `column_stale` are the caller's hoisted registers: the
// column max is only re-read when a step changed the XZ column (`face_axis != 1`)
// or a distance skip invalidated it, so climbing inside one column costs zero
// storage reads.
fn coarse_height_levers(state: ptr<function, DdaState>,
                        column_max_y: ptr<function, i32>,
                        column_stale: ptr<function, bool>,
                        origin: vec3<f32>, direction: vec3<f32>,
                        inverse_direction: vec3<f32>, heading_upward: bool,
                        world_max_brick_y: i32) -> u32 {
    if (USE_COLUMN_HEIGHTS) {
        if ((*state).face_axis != 1u || *column_stale) {
            *column_max_y = column_max_of((*state).cell);
            *column_stale = false;
        }
        if ((*state).cell.y > *column_max_y) {
            // Above every occupied brick of this column: fast-forward, NEVER
            // terminate — taller terrain in columns farther along the ray must
            // stay reachable (long low-sun shadows across water depend on it).
            if (heading_upward) {
                if (ENABLE_GLOBAL_MAX_TERMINATE && (*state).cell.y > world_max_brick_y) {
                    return HEIGHT_LEVER_MISS; // above everything in the world
                }
                if (ENABLE_COLUMN_FAST_FORWARD) {
                    column_fast_forward(state, origin, direction, inverse_direction);
                    return HEIGHT_LEVER_CONTINUE;
                }
            } else if (ENABLE_DESCEND_FAST_FORWARD) {
                descend_fast_forward(state, origin, direction, inverse_direction,
                                     *column_max_y);
                return HEIGHT_LEVER_CONTINUE;
            }
        }
        return HEIGHT_LEVER_TEST_BRICK;
    }
    if (ENABLE_GLOBAL_MAX_TERMINATE && heading_upward
        && (*state).cell.y > world_max_brick_y) {
        return HEIGHT_LEVER_MISS; // above everything in the world → sky
    }
    return HEIGHT_LEVER_TEST_BRICK;
}

// ---- Empty-space acceleration (coarse level only) -----------------------------

// Flat cell index of an in-grid brick coordinate (x-major, then y, then z —
// the shared layout of brick_indices / brick_occupancy_bits /
// brick_skip_distances).
fn brick_cell_index(cell: vec3<i32>) -> u32 {
    return u32(cell.x)
        + u32(cell.y) * brickmap.brick_grid_size.x
        + u32(cell.z) * brickmap.brick_grid_size.x * brickmap.brick_grid_size.y;
}

// Whether the brick at `cell_index` holds any voxels. The bit grid is the
// fast path (62.5 KB stays cache-resident); the skip-distance byte doubles
// as an occupancy test when the bit grid is levered off; the pointer grid is
// the lever-everything-off fallback.
fn brick_occupied(cell_index: u32) -> bool {
    if (ENABLE_BRICK_BIT_GRID) {
        return ((brick_occupancy_bits[cell_index >> 5u] >> (cell_index & 31u)) & 1u) == 1u;
    }
    if (ENABLE_DISTANCE_SKIP) {
        return skip_distance_of(cell_index) == 0u;
    }
    return brick_indices[cell_index] != EMPTY_BRICK;
}

// Chebyshev distance (in bricks) from `cell_index` to the nearest occupied
// brick: 0 = occupied, byte-packed little-endian like the material bytes.
fn skip_distance_of(cell_index: u32) -> u32 {
    return (brick_skip_distances[cell_index >> 2u] >> ((cell_index & 3u) * 8u)) & 0xffu;
}

// Distance in VOXEL units from `point` (inside the brick `cell`, whose
// chebyshev skip distance is `skip_cells`) to the nearest possibly occupied
// voxel — the quantity E1b's soft shadows use as IQ's `h`.
//
// The brick sits centered in an all-empty cube of half-width skip_cells - 1
// bricks, so every occupied voxel lies outside that cube and the L-infinity
// distance from `point` to the cube's boundary bounds the true distance from
// below. Using the boundary distance rather than the flat
// (skip_cells - 1) * BRICK_SIZE floor adds the point's own offset inside the
// cube, which is what turns a per-brick step function into something varying
// continuously along the ray — the cheap refinement E1b evaluates against
// brick-granular banding. Callers must pass a point INSIDE the brick, not on
// its entry face: at skip_cells = 1 the cube is the brick itself, so a
// face-point evaluates to 0 and would black out every ray that grazes near
// geometry (measured — it darkened 55% of the frame regardless of penumbra
// scale before the sample point moved to the segment midpoint).
// GRANULARITY CAVEAT: the underlying field is per-BRICK, so the bound still
// jumps in 8-voxel (1 m) increments as skip_cells changes.
fn brick_clearance(cell: vec3<i32>, skip_cells: u32, point: vec3<f32>) -> f32 {
    let half_width = f32(i32(skip_cells) - 1) * BRICK_SIZE;
    let cube_min = vec3<f32>(cell) * BRICK_SIZE - vec3<f32>(half_width);
    let cube_max = vec3<f32>(cell + vec3<i32>(1, 1, 1)) * BRICK_SIZE + vec3<f32>(half_width);
    let to_boundary = min(point - cube_min, cube_max - point);
    return max(min(min(to_boundary.x, to_boundary.y), to_boundary.z), 0.0);
}

// Whether every brick of the 3x3x3 brick neighbourhood around `brick_cell` is
// empty, the OWN brick excluded (it holds the hit voxel, so it is occupied by
// definition). Out-of-grid neighbours count as empty. Reads the
// 1-bit-per-brick occupancy grid (binding 9) — 62.5 KB, cache-resident, and
// the only consumer that grid currently has. Used by AO_BRICK_EARLY_OUT.
fn brick_neighborhood_empty(brick_cell: vec3<i32>) -> bool {
    let grid_size = vec3<i32>(brickmap.brick_grid_size);
    for (var offset_z = -1; offset_z <= 1; offset_z = offset_z + 1) {
        for (var offset_y = -1; offset_y <= 1; offset_y = offset_y + 1) {
            for (var offset_x = -1; offset_x <= 1; offset_x = offset_x + 1) {
                if (offset_x == 0 && offset_y == 0 && offset_z == 0) {
                    continue;
                }
                let neighbor = brick_cell + vec3<i32>(offset_x, offset_y, offset_z);
                if (any(neighbor < vec3<i32>(0, 0, 0)) || any(neighbor >= grid_size)) {
                    continue;
                }
                let cell_index = brick_cell_index(neighbor);
                if (((brick_occupancy_bits[cell_index >> 5u] >> (cell_index & 31u)) & 1u) == 1u) {
                    return false;
                }
            }
        }
    }
    return true;
}

// Jump a ray standing in an empty brick with chebyshev distance
// `skip_cells` >= 2 to the exit of its guaranteed-empty cube (half-width
// skip_cells - 1 bricks, so every brick strictly inside is empty) and
// re-seed the coarse DDA there with the existing verified setup math —
// no hand-patched state. The first potentially occupied brick lies exactly
// ON the cube boundary, and the re-seed lands at most RAY_EPSILON past it,
// so the jump can never tunnel. face_axis = the cube's exit axis, keeping
// the entry normal correct if the very next brick scores a hit.
fn distance_skip(state: ptr<function, DdaState>, origin: vec3<f32>, direction: vec3<f32>,
                 inverse_direction: vec3<f32>, skip_cells: i32,
                 clamp_min: vec3<i32>, clamp_max: vec3<i32>) {
    // The chebyshev cube is the symmetric case of the general box below.
    let half_width = vec3<i32>(skip_cells - 1);
    bounded_skip(state, origin, direction, inverse_direction, half_width, half_width,
                 clamp_min, clamp_max);
}

// One directional bound out of a packed AADF word.
fn bound_of(packed: u32, direction: u32) -> u32 {
    return (packed >> (direction * BOUND_BITS)) & BOUND_MASK;
}

// The AADF box of an empty brick cell, as (low, high) cell counts. Returns zero
// extents when the cell has no room to claim, which the callers read as "no skip
// available, take a single dda_step".
fn directional_box(cell_index: u32) -> array<vec3<i32>, 2> {
    let packed = brick_bounds[cell_index];
    let low = vec3<i32>(i32(bound_of(packed, 0u)), i32(bound_of(packed, 2u)),
                        i32(bound_of(packed, 4u)));
    let high = vec3<i32>(i32(bound_of(packed, 1u)), i32(bound_of(packed, 3u)),
                         i32(bound_of(packed, 5u)));
    return array<vec3<i32>, 2>(low, high);
}

// The general form of the jump above: `low`/`high` are per-axis counts of
// guaranteed-empty cells on each side of the current one, so the empty region is
// the box of cells [cell - low, cell + high]. The chebyshev cube passes the same
// value for both; AADF passes its six directional bounds. ONE routine, so the
// tunnel-safety argument is made once.
fn bounded_skip(state: ptr<function, DdaState>, origin: vec3<f32>, direction: vec3<f32>,
                inverse_direction: vec3<f32>, low: vec3<i32>, high: vec3<i32>,
                clamp_min: vec3<i32>, clamp_max: vec3<i32>) {
    let box_min = vec3<f32>((*state).cell - low) * BRICK_SIZE;
    let box_max = vec3<f32>((*state).cell + high + vec3<i32>(1, 1, 1)) * BRICK_SIZE;
    let t_far = max((box_min - origin) * inverse_direction,
                    (box_max - origin) * inverse_direction);
    var exit_axis = 2u;
    if (t_far.x <= t_far.y && t_far.x <= t_far.z) {
        exit_axis = 0u;
    } else if (t_far.y <= t_far.z) {
        exit_axis = 1u;
    }
    let t_exit = max(min(min(t_far.x, t_far.y), t_far.z), (*state).t);
    let previous_cell = (*state).cell;
    *state = dda_setup(origin, direction, inverse_direction, t_exit, BRICK_SIZE,
                       clamp_min, clamp_max);
    (*state).face_axis = exit_axis;
    if (all((*state).cell == previous_cell)) {
        // Float undershoot re-derived the same cell — force progress so the
        // skip can never spin in place.
        dda_step(state);
    }
}

// ---- Fine level: voxels inside one occupied brick -----------------------------

// Occupancy bit index of a world-voxel cell inside the brick at `brick_cell`.
fn local_bit_index(cell: vec3<i32>, brick_cell: vec3<i32>) -> u32 {
    let local = cell - brick_cell * 8;
    return u32(local.x) + u32(local.y) * 8u + u32(local.z) * 64u;
}

// ---- Level-0 pointer tag accessors -------------------------------------------

// The level-1 slot a UNIQUE pointer addresses. A no-op mask on an untagged
// pointer, so call sites can apply it unconditionally.
fn brick_slot(pointer: u32) -> u32 {
    return pointer & BRICK_PAYLOAD_MASK;
}

// Whether the brick is one material through and through — solid in all 512
// cells, so a ray hits it at the face it entered and never descends.
fn brick_is_uniform(pointer: u32) -> bool {
    return (pointer >> BRICK_TAG_SHIFT) == BRICK_TAG_UNIFORM;
}

// The material filling a UNIFORM brick. Meaningless for any other tag.
fn brick_uniform_material(pointer: u32) -> u32 {
    return pointer & 0xffu;
}

fn occupancy_bit_set(pointer: u32, bit: u32) -> bool {
    let slot = brick_slot(pointer);
    return ((occupancy_words[slot * 16u + (bit >> 5u)] >> (bit & 31u)) & 1u) == 1u;
}

// Random-access occupancy test at a world-voxel coordinate (out of world =
// empty), reusing the traversal's own index math — `brick_cell_index` for the
// coarse cell, `local_bit_index` + `occupancy_bit_set` for the voxel inside the
// brick — so the layout knowledge stays in one place. The traversal itself
// never needs this (it already has the brick pointer hoisted); E1b's analytic
// AO does, and it crosses brick boundaries freely.
fn voxel_occupied(cell: vec3<i32>) -> bool {
    if (any(cell < vec3<i32>(0, 0, 0))
        || any(cell >= vec3<i32>(brickmap.world_size_voxels))) {
        return false;
    }
    let brick_cell = cell / vec3<i32>(8, 8, 8);
    let pointer = brick_indices[brick_cell_index(brick_cell)];
    if (pointer == EMPTY_BRICK) {
        return false;
    }
    if (brick_is_uniform(pointer)) {
        return true;
    }
    return occupancy_bit_set(pointer, local_bit_index(cell, brick_cell));
}

// Random-access MATERIAL read at a world-voxel coordinate: the material byte, or
// 0 (air — the miss sentinel) for an empty voxel, an empty brick or a cell
// outside the world. The occupancy-only twin of `voxel_occupied`, sharing the
// same index math, so the packing knowledge stays in one place.
//
// The two-level traversal never needs this (it has the brick pointer hoisted and
// reads the material byte on the hit it already found); E6's water-medium march
// does, because "is the voxel I am standing in still the same liquid" is a
// question about a voxel rather than about a hit.
fn voxel_material_at(cell: vec3<i32>) -> u32 {
    if (any(cell < vec3<i32>(0, 0, 0))
        || any(cell >= vec3<i32>(brickmap.world_size_voxels))) {
        return 0u;
    }
    let brick_cell = cell / vec3<i32>(8, 8, 8);
    let pointer = brick_indices[brick_cell_index(brick_cell)];
    if (pointer == EMPTY_BRICK) {
        return 0u;
    }
    if (brick_is_uniform(pointer)) {
        return brick_uniform_material(pointer);
    }
    let bit = local_bit_index(cell, brick_cell);
    if (!occupancy_bit_set(pointer, bit)) {
        return 0u;
    }
    let packed = material_words[brick_slot(pointer) * 128u + (bit >> 2u)];
    return (packed >> ((bit & 3u) * 8u)) & 0xffu;
}

// Whether a material id is a fluid, from the material table's LIQUID flag
// (src/material.rs) rather than from a hardcoded row — so a second liquid at B6
// needs no shader change. Air (row 0) is not a liquid.
fn material_is_liquid(material: u32) -> bool {
    return (materials[material].flags & MATERIAL_FLAG_LIQUID) != 0u;
}

// One vector component by axis index (0 = x, 1 = y, 2 = z) — the DDA reports a
// `face_axis`, and several callers need the ray's component along it.
fn component_of(vector: vec3<f32>, axis: u32) -> f32 {
    if (axis == 0u) {
        return vector.x;
    }
    if (axis == 1u) {
        return vector.y;
    }
    return vector.z;
}

// Full fine traversal for primary rays: first occupied voxel wins and the
// complete hit record (material, entry face, distance, voxel coordinate) is
// reconstructed. `t_enter`/`t_exit` bracket the ray's overlap with this
// brick; `entry_axis` is the axis whose face the ray crossed to arrive (it
// becomes the hit normal if the first voxel tested is already occupied).
fn trace_brick(origin: vec3<f32>, direction: vec3<f32>, inverse_direction: vec3<f32>,
               pointer: u32, brick_cell: vec3<i32>, t_enter: f32, t_exit: f32,
               entry_axis: u32, skip_liquids: bool) -> Hit {
    var result: Hit;
    result.material = 0u;

    let brick_min_cell = brick_cell * 8;
    let brick_max_cell = brick_min_cell + vec3<i32>(7, 7, 7);

    // UNIFORM fast path: the brick is one material in all 512 cells, so the
    // first voxel the ray meets is the one it entered through. The hit is the
    // brick's entry face — no dda_setup, no descent, no level-1 fetch at all.
    // 58.6% of the island's occupied bricks qualify (see the brick_census
    // probe in voxel-core), which is most of the ground under most rays.
    if (brick_is_uniform(pointer)) {
        let material = brick_uniform_material(pointer);
        // E6 again: a liquid does not stop a ray told to ignore liquids. Leaving
        // material at 0 reports a miss, and the coarse level steps past the
        // whole brick — which is the correct behaviour AND cheaper than the
        // per-voxel walk it replaces.
        if (skip_liquids && material_is_liquid(material)) {
            return result;
        }
        result.material = material;
        result.axis = entry_axis;
        result.axis_sign = sign(component_of(direction, entry_axis));
        result.distance = t_enter;
        // The voxel the ray is standing in one epsilon past the face, clamped
        // into the brick so a face-exact intersection cannot name a neighbour.
        result.voxel = clamp(vec3<i32>(floor(origin + direction * (t_enter + RAY_EPSILON))),
                             brick_min_cell, brick_max_cell);
        return result;
    }

    var state = dda_setup(origin, direction, inverse_direction, t_enter, 1.0,
                          brick_min_cell, brick_max_cell);
    state.face_axis = entry_axis;

    for (var step_index = 0u; step_index < MAX_VOXEL_STEPS; step_index = step_index + 1u) {
        let bit = local_bit_index(state.cell, brick_cell);
        if (occupancy_bit_set(pointer, bit)) {
            let packed = material_words[brick_slot(pointer) * 128u + (bit >> 2u)];
            let material = (packed >> ((bit & 3u) * 8u)) & 0xffu;
            // E6: a ray that has been told liquids do not block it walks through
            // them. The test costs one material-table load on a HIT, i.e. once per
            // ray rather than once per step, and short-circuits away entirely for
            // the callers that pass false (every ray but the sun's).
            if (!(skip_liquids && material_is_liquid(material))) {
                result.material = material;
                result.axis = state.face_axis;
                var direction_component = direction.x;
                if (state.face_axis == 1u) {
                    direction_component = direction.y;
                } else if (state.face_axis == 2u) {
                    direction_component = direction.z;
                }
                result.axis_sign = sign(direction_component);
                result.distance = state.t;
                result.voxel = state.cell;
                return result;
            }
        }
        dda_step(&state);
        if (any(state.cell < brick_min_cell) || any(state.cell > brick_max_cell)) {
            break; // left the brick — hand back to the coarse level
        }
        if (state.t > t_exit + RAY_EPSILON) {
            break;
        }
    }
    return result;
}

// Any-hit fine traversal for shadow rays: the first set occupancy bit
// occludes — no material fetch, no face/distance bookkeeping.
fn shadow_brick_occluded(origin: vec3<f32>, direction: vec3<f32>,
                         inverse_direction: vec3<f32>, pointer: u32,
                         brick_cell: vec3<i32>, t_enter: f32, t_exit: f32,
                         skip_liquids: bool) -> bool {
    // UNIFORM fast path, the any-hit twin of `trace_brick`'s: a solid brick
    // occludes, full stop — unless it is a liquid this ray ignores. Shadow rays
    // graze terrain at shallow angles and cross many ground bricks, so this is
    // the case that walks the most voxels for the least information.
    if (brick_is_uniform(pointer)) {
        return !(skip_liquids && material_is_liquid(brick_uniform_material(pointer)));
    }

    let brick_min_cell = brick_cell * 8;
    let brick_max_cell = brick_min_cell + vec3<i32>(7, 7, 7);
    var state = dda_setup(origin, direction, inverse_direction, t_enter, 1.0,
                          brick_min_cell, brick_max_cell);

    for (var step_index = 0u; step_index < MAX_VOXEL_STEPS; step_index = step_index + 1u) {
        let bit = local_bit_index(state.cell, brick_cell);
        if (occupancy_bit_set(pointer, bit)) {
            // Same E6 rule as `trace_brick`: a liquid does not occlude a ray that
            // was told liquids are transparent to it.
            if (!skip_liquids) {
                return true;
            }
            let packed = material_words[brick_slot(pointer) * 128u + (bit >> 2u)];
            let material = (packed >> ((bit & 3u) * 8u)) & 0xffu;
            if (!material_is_liquid(material)) {
                return true;
            }
        }
        dda_step(&state);
        if (any(state.cell < brick_min_cell) || any(state.cell > brick_max_cell)) {
            return false;
        }
        if (state.t > t_exit + RAY_EPSILON) {
            return false;
        }
    }
    return false;
}

// ---- Coarse level: the brick grid ---------------------------------------------

// Trace one closest-hit ray through the brick grid. Occupancy comes from the
// bit grid; empty bricks either distance-skip their guaranteed-empty cube or
// step once; occupied bricks run the fine DDA; cleared columns fast-forward.
// Everything is in voxel units; `origin` must already be voxel-space.
// `max_distance` caps the traversal (primary/shadow rays pass
// MAX_TRACE_DISTANCE; E1's short AO rays pass AO_MAX_DISTANCE).
fn trace(origin: vec3<f32>, direction: vec3<f32>, max_distance: f32,
         skip_liquids: bool) -> Hit {
    var result: Hit;
    result.material = 0u;

    let inverse_direction = vec3<f32>(
        safe_inverse(direction.x),
        safe_inverse(direction.y),
        safe_inverse(direction.z),
    );
    let bounds = intersect_world_bounds(origin, inverse_direction);
    if (bounds.x > bounds.y) {
        return result; // ray misses the world entirely
    }

    let grid_size = vec3<i32>(brickmap.brick_grid_size);
    var state = dda_setup(origin, direction, inverse_direction, bounds.x, BRICK_SIZE,
                          vec3<i32>(0, 0, 0), grid_size - vec3<i32>(1, 1, 1));
    state.face_axis = u32(bounds.z); // world-entry face seeds the first normal

    let heading_upward = direction.y >= 0.0;
    let world_max_brick_y = i32(brickmap.max_occupied_brick_y); // -1 when empty
    let t_limit = min(bounds.y, max_distance);
    // Hoisted column max of the CURRENT XZ column (see column_max_of).
    var column_max_y = 0;
    if (USE_COLUMN_HEIGHTS) {
        column_max_y = column_max_of(state.cell);
    }
    // A distance skip can change the XZ column even when it exits through a
    // Y face, so it forces a refresh instead of relying on face_axis.
    var column_stale = false;

    for (var step_index = 0u; step_index < MAX_BRICK_STEPS; step_index = step_index + 1u) {
        if (any(state.cell < vec3<i32>(0, 0, 0)) || any(state.cell >= grid_size)) {
            break; // exited the grid → sky
        }
        if (state.t > t_limit) {
            break; // beyond the world or the trace-distance guard → sky
        }
        let height_action = coarse_height_levers(&state, &column_max_y, &column_stale,
                                                 origin, direction, inverse_direction,
                                                 heading_upward, world_max_brick_y);
        if (height_action == HEIGHT_LEVER_MISS) {
            break;
        }
        if (height_action == HEIGHT_LEVER_CONTINUE) {
            continue;
        }

        let cell_index = brick_cell_index(state.cell);
        if (brick_occupied(cell_index)) {
            let pointer = brick_indices[cell_index];
            let brick_exit = min(min(state.t_max.x, state.t_max.y), state.t_max.z);
            let fine = trace_brick(origin, direction, inverse_direction, pointer,
                                   state.cell, state.t, brick_exit, state.face_axis,
                                   skip_liquids);
            if (fine.material != 0u) {
                return fine;
            }
        } else if (ENABLE_DIRECTIONAL_SKIP) {
            let extents = directional_box(cell_index);
            if (any(extents[0] + extents[1] > vec3<i32>(0, 0, 0))) {
                bounded_skip(&state, origin, direction, inverse_direction,
                             extents[0], extents[1], vec3<i32>(0, 0, 0),
                             grid_size - vec3<i32>(1, 1, 1));
                column_stale = true;
                continue;
            }
        } else if (ENABLE_DISTANCE_SKIP) {
            let skip_cells = skip_distance_of(cell_index);
            if (skip_cells >= 2u) {
                distance_skip(&state, origin, direction, inverse_direction,
                              i32(skip_cells), vec3<i32>(0, 0, 0),
                              grid_size - vec3<i32>(1, 1, 1));
                column_stale = true;
                continue;
            }
        }
        dda_step(&state);
    }
    return result;
}

// E1b soft shadows (technique bank T1), kept OUT of the shadow loop's body:
// IQ's single-ray penumbra term `min(penumbra_scale * clearance / t)` folded
// into the running visibility, from the chebyshev distance bytes the traversal
// already fetched. Sampled at the MIDPOINT of the ray's segment through this
// brick — see `brick_clearance` on why a face point blacks out the frame — and
// ignored for the first brick of lead-in, whose conservative clearance is 0 by
// definition. DOCUMENTED NEGATIVE (per-brick granularity prints a 1 m lattice);
// see the registry verdict in src/variants.rs.
fn soft_penumbra_update(visibility: f32, state: ptr<function, DdaState>,
                        skip_cells: u32, origin: vec3<f32>,
                        direction: vec3<f32>) -> f32 {
    if ((*state).t <= SHADOW_PENUMBRA_MIN_DISTANCE) {
        return visibility;
    }
    let brick_exit = min(min((*state).t_max.x, (*state).t_max.y), (*state).t_max.z);
    let sample_t = ((*state).t + brick_exit) * 0.5;
    let clearance = brick_clearance((*state).cell, skip_cells,
                                    origin + direction * sample_t);
    return min(visibility, lighting.shading_params.y * clearance / sample_t);
}

// Sun visibility in [0, 1] along a shadow ray.
//
// HARD mode (SHADOW_MODE = 0, the default) returns exactly 0.0 or 1.0. With
// the shipped ENABLE_ANY_HIT_SHADOW = false it delegates to the closest-hit
// `trace` — measured faster than the specialized any-hit loop below — which
// keeps the renderer bit-identical to Stage 2.
//
// SOFT mode (E1b, technique bank T1) runs the any-hit coarse loop and tracks
// IQ's single-ray penumbra term `min(penumbra_scale * clearance / t)` from the
// chebyshev distance bytes the traversal already fetches (`brick_clearance`),
// then smoothsteps it. No extra rays, no extra data — but see the granularity
// caveat on `brick_clearance` and the E1b verdict in the bench doc.
//
// Occupied bricks always run the occupancy-only fine variant; the first
// occluding voxel ends the ray at zero visibility either way.
fn trace_shadow_visibility(origin: vec3<f32>, direction: vec3<f32>) -> f32 {
    if (SHADOW_MODE == SHADOW_MODE_HARD && !ENABLE_ANY_HIT_SHADOW) {
        return select(1.0, 0.0,
                      trace(origin, direction, MAX_TRACE_DISTANCE,
                            WATER_SUN_THROUGH_LIQUID).material != 0u);
    }
    var visibility = 1.0;
    let inverse_direction = vec3<f32>(
        safe_inverse(direction.x),
        safe_inverse(direction.y),
        safe_inverse(direction.z),
    );
    let bounds = intersect_world_bounds(origin, inverse_direction);
    if (bounds.x > bounds.y) {
        return 1.0;
    }

    let grid_size = vec3<i32>(brickmap.brick_grid_size);
    var state = dda_setup(origin, direction, inverse_direction, bounds.x, BRICK_SIZE,
                          vec3<i32>(0, 0, 0), grid_size - vec3<i32>(1, 1, 1));

    let heading_upward = direction.y >= 0.0;
    let world_max_brick_y = i32(brickmap.max_occupied_brick_y); // -1 when empty
    let t_limit = min(bounds.y, MAX_TRACE_DISTANCE);
    // Hoisted column max of the CURRENT XZ column (see column_max_of).
    var column_max_y = 0;
    if (USE_COLUMN_HEIGHTS) {
        column_max_y = column_max_of(state.cell);
    }
    // Set by distance_skip, which can change the XZ column via a Y-face exit.
    var column_stale = false;

    for (var step_index = 0u; step_index < MAX_BRICK_STEPS; step_index = step_index + 1u) {
        if (any(state.cell < vec3<i32>(0, 0, 0)) || any(state.cell >= grid_size)) {
            break; // left the grid → nothing more can occlude
        }
        if (state.t > t_limit) {
            break;
        }
        let height_action = coarse_height_levers(&state, &column_max_y, &column_stale,
                                                 origin, direction, inverse_direction,
                                                 heading_upward, world_max_brick_y);
        if (height_action == HEIGHT_LEVER_MISS) {
            break; // nothing left that could occlude → unoccluded
        }
        if (height_action == HEIGHT_LEVER_CONTINUE) {
            continue;
        }

        let cell_index = brick_cell_index(state.cell);
        if (brick_occupied(cell_index)) {
            let pointer = brick_indices[cell_index];
            let brick_exit = min(min(state.t_max.x, state.t_max.y), state.t_max.z);
            if (shadow_brick_occluded(origin, direction, inverse_direction, pointer,
                                      state.cell, state.t, brick_exit,
                                      WATER_SUN_THROUGH_LIQUID)) {
                return 0.0;
            }
        } else {
            var skip_cells = 0u;
            if (ENABLE_DISTANCE_SKIP || SHADOW_MODE == SHADOW_MODE_SOFT_DISTANCE_FIELD) {
                skip_cells = skip_distance_of(cell_index);
            }
            if (SHADOW_MODE == SHADOW_MODE_SOFT_DISTANCE_FIELD) {
                visibility = soft_penumbra_update(visibility, &state, skip_cells,
                                                  origin, direction);
            }
            if (ENABLE_DIRECTIONAL_SKIP) {
                let extents = directional_box(cell_index);
                if (any(extents[0] + extents[1] > vec3<i32>(0, 0, 0))) {
                    bounded_skip(&state, origin, direction, inverse_direction,
                                 extents[0], extents[1], vec3<i32>(0, 0, 0),
                                 grid_size - vec3<i32>(1, 1, 1));
                    column_stale = true;
                    continue;
                }
            } else if (ENABLE_DISTANCE_SKIP && skip_cells >= 2u) {
                distance_skip(&state, origin, direction, inverse_direction,
                              i32(skip_cells), vec3<i32>(0, 0, 0),
                              grid_size - vec3<i32>(1, 1, 1));
                column_stale = true;
                continue;
            }
        }
        dda_step(&state);
    }
    if (SHADOW_MODE == SHADOW_MODE_SOFT_DISTANCE_FIELD) {
        let clamped = clamp(visibility, 0.0, 1.0);
        return clamped * clamped * (3.0 - 2.0 * clamped); // smoothstep
    }
    return visibility;
}
