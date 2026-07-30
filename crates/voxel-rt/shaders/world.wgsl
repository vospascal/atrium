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
//   5  storage  materials     — array<Material> (48 bytes/row), indexed by
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
}

@group(0) @binding(1) var<uniform> brickmap: BrickmapMeta;
@group(0) @binding(2) var<storage, read> brick_indices: array<u32>;
@group(0) @binding(3) var<storage, read> occupancy_words: array<u32>;
@group(0) @binding(4) var<storage, read> material_words: array<u32>;
// One row of the M1 material table (src/material.rs `GpuMaterial`). Laid out as
// three 16-byte rows, each a vec3 followed by the scalar filling its w slot, so
// std430 adds no implicit padding and the Rust upload matches byte for byte.
struct Material {
    albedo: vec3<f32>,      // sRGB-encoded, as authored
    transmittance: f32,     // light passing THROUGH (transport; M2 reads it)
    emission: vec3<f32>,    // linear radiance; all-zero until M1b
    roughness: f32,
    opacity: f32,           // < 1.0 => traversal must continue through
    specular: f32,
    flags: u32,             // MATERIAL_FLAG_* below
    _pad: u32,
}

// Mirrors `MaterialFlags` in src/material.rs.
const MATERIAL_FLAG_FOLIAGE: u32 = 1u;
const MATERIAL_FLAG_EMISSIVE: u32 = 2u;
const MATERIAL_FLAG_TRANSPARENT: u32 = 4u;
const MATERIAL_FLAG_LIQUID: u32 = 8u;

@group(0) @binding(5) var<storage, read> materials: array<Material>;
@group(0) @binding(7) var<uniform> lighting: Lighting;
@group(0) @binding(8) var<storage, read> column_max_brick_y: array<u32>;
@group(0) @binding(9) var<storage, read> brick_occupancy_bits: array<u32>;
@group(0) @binding(10) var<storage, read> brick_skip_distances: array<u32>;

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

const EMPTY_BRICK: u32 = 0xffffffffu;
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
    let box_min = vec3<f32>((*state).cell - vec3<i32>(skip_cells - 1)) * BRICK_SIZE;
    let box_max = vec3<f32>((*state).cell + vec3<i32>(skip_cells)) * BRICK_SIZE;
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

fn occupancy_bit_set(pointer: u32, bit: u32) -> bool {
    return ((occupancy_words[pointer * 16u + (bit >> 5u)] >> (bit & 31u)) & 1u) == 1u;
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
    return occupancy_bit_set(pointer, local_bit_index(cell, brick_cell));
}

// Full fine traversal for primary rays: first occupied voxel wins and the
// complete hit record (material, entry face, distance, voxel coordinate) is
// reconstructed. `t_enter`/`t_exit` bracket the ray's overlap with this
// brick; `entry_axis` is the axis whose face the ray crossed to arrive (it
// becomes the hit normal if the first voxel tested is already occupied).
fn trace_brick(origin: vec3<f32>, direction: vec3<f32>, inverse_direction: vec3<f32>,
               pointer: u32, brick_cell: vec3<i32>, t_enter: f32, t_exit: f32,
               entry_axis: u32) -> Hit {
    var result: Hit;
    result.material = 0u;

    let brick_min_cell = brick_cell * 8;
    let brick_max_cell = brick_min_cell + vec3<i32>(7, 7, 7);
    var state = dda_setup(origin, direction, inverse_direction, t_enter, 1.0,
                          brick_min_cell, brick_max_cell);
    state.face_axis = entry_axis;

    for (var step_index = 0u; step_index < MAX_VOXEL_STEPS; step_index = step_index + 1u) {
        let bit = local_bit_index(state.cell, brick_cell);
        if (occupancy_bit_set(pointer, bit)) {
            let packed = material_words[pointer * 128u + (bit >> 2u)];
            result.material = (packed >> ((bit & 3u) * 8u)) & 0xffu;
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
                         brick_cell: vec3<i32>, t_enter: f32, t_exit: f32) -> bool {
    let brick_min_cell = brick_cell * 8;
    let brick_max_cell = brick_min_cell + vec3<i32>(7, 7, 7);
    var state = dda_setup(origin, direction, inverse_direction, t_enter, 1.0,
                          brick_min_cell, brick_max_cell);

    for (var step_index = 0u; step_index < MAX_VOXEL_STEPS; step_index = step_index + 1u) {
        if (occupancy_bit_set(pointer, local_bit_index(state.cell, brick_cell))) {
            return true;
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
fn trace(origin: vec3<f32>, direction: vec3<f32>, max_distance: f32) -> Hit {
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
                                   state.cell, state.t, brick_exit, state.face_axis);
            if (fine.material != 0u) {
                return fine;
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
                      trace(origin, direction, MAX_TRACE_DISTANCE).material != 0u);
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
                                      state.cell, state.t, brick_exit)) {
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
            if (ENABLE_DISTANCE_SKIP && skip_cells >= 2u) {
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
