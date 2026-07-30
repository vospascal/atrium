// dda.wgsl — Stage 2 renderer: two-level DDA over the brickmap, one sun
// shadow ray per primary hit, hemisphere sky ambient, ambient occlusion
// (E1 ray-traced / E1b analytic), Reinhard tonemap.
//
// Fullscreen compute pass (workgroup 8x8): one thread per output pixel builds
// a camera ray, traverses the two-level brickmap (coarse Amanatides & Woo DDA
// over 8^3-voxel bricks, fine DDA over the voxels inside occupied bricks),
// and writes a shaded color to an rgba8unorm storage texture. Misses get a
// vertical sky gradient. Each primary hit fires ONE shadow ray toward the sun
// through `trace_shadow_visibility()` plus whatever the AO_MODE lever asks for
// (AO_RAY_COUNT short occlusion rays, or a ray-free analytic estimate from the
// local occupancy bits) attenuating the hemisphere-ambient term only — the sun
// term keeps its own shadow ray (see the AO and shadow lever blocks).
//
// All traversal happens in VOXEL-space units: the camera position arrives in
// world meters and is divided by BrickmapMeta.voxel_size_meters once at ray
// setup; directions are unit-length either way (uniform scale).
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
// Color pipeline: palette colors are sRGB-encoded (as authored in
// voxel-sandbox's mesh.rs). This shader decodes them to linear (pow-2.2
// approximation — cheap and self-consistent with the encode below; the exact
// piecewise curve buys nothing at 8 bits), does ALL lighting math in linear
// (sun term + hemisphere ambient), applies a one-line Reinhard tonemap
// (Stage 4 refines the curve), then re-encodes to sRGB before textureStore.
// The storage-texture/blit contract is unchanged: the blit still receives
// sRGB-encoded bytes and undoes the swapchain's re-encode.
//
// Modularity (plan architecture rule): BOTH trace loops (primary + shadow)
// and BOTH cell levels (brick + voxel) share one stepping core — `DdaState` +
// `dda_setup` + `dda_step` — so the DDA math exists exactly once. The
// variants differ only in what they do at an occupied cell: `trace` /
// `trace_brick` build a full `Hit` (material, face, distance, voxel);
// `trace_shadow` / `shadow_brick_occluded` return a bool at the first set
// occupancy bit. Later stages (CAGI volume sampling, audio-ray port) reuse
// the same helpers.
//
// Bindings (group 0), matching the Rust-side layouts:
//   0  uniform  Camera        — camera.rs CameraUniform (80 bytes; position
//                               in world meters, ray basis vectors, resolution)
//   1  uniform  BrickmapMeta  — brickmap.rs BrickmapMetadata (48 bytes)
//   2  storage  brick_indices — dense brick-pointer grid, x-major then y then
//                               z; 0xffffffff = empty brick
//   3  storage  occupancy_words — 16 u32 words per occupied brick; bit index
//                               = local_x + local_y*8 + local_z*64
//   4  storage  material_words — 128 u32 words per occupied brick; one byte
//                               per voxel, same local index, little-endian
//                               (byte 0 = bits 0..8)
//   5  storage  palette       — array<vec4<f32>>, indexed by material id
//   6  texture  output        — rgba8unorm storage texture, write-only
//   7  uniform  Lighting      — lighting.rs LightingUniform (64 bytes; sun
//                               direction/color/intensity, hemisphere ambient)
//   8  storage  column_max_brick_y — per-XZ-brick-column max occupied brick
//                               Y (125 x 125 u32, x-major then z:
//                               column = x + z * grid_size.x); u32::MAX =
//                               empty column
//   9  storage  brick_occupancy_bits — one bit per brick cell (same x-major
//                               cell index as brick_indices; bit cell & 31 of
//                               word cell >> 5); set = brick occupied
//  10  storage  brick_skip_distances — one byte per brick cell, little-endian
//                               packed like material_words: chebyshev
//                               distance in bricks to the nearest occupied
//                               brick (0 = occupied, saturated at 255)

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
}

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> brickmap: BrickmapMeta;
@group(0) @binding(2) var<storage, read> brick_indices: array<u32>;
@group(0) @binding(3) var<storage, read> occupancy_words: array<u32>;
@group(0) @binding(4) var<storage, read> material_words: array<u32>;
@group(0) @binding(5) var<storage, read> palette: array<vec4<f32>>;
@group(0) @binding(6) var output: texture_storage_2d<rgba8unorm, write>;
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

// ---- Color pipeline ----------------------------------------------------------

// sRGB <-> linear via the pow-2.2 approximation: a self-consistent pair, one
// pow each way, indistinguishable from the exact piecewise curve at 8 bits.
fn srgb_decode(color: vec3<f32>) -> vec3<f32> {
    return pow(color, vec3<f32>(2.2, 2.2, 2.2));
}

fn srgb_encode(color: vec3<f32>) -> vec3<f32> {
    return pow(color, vec3<f32>(1.0 / 2.2, 1.0 / 2.2, 1.0 / 2.2));
}

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

// Occlusion in [0, 1] from `ray_count` short occlusion rays out of the hit
// face, averaged (E1's estimator). Rays reuse `shadow_ray_origin` (same
// integer-reconstructed, acne-free origin as the sun ray) and the shared
// `trace` with AO_MAX_DISTANCE — the chebyshev distance field makes short rays
// through open space nearly free. `ray_count` is AO_RAY_COUNT except under the
// sun-aware budget lever.
fn ray_traced_occlusion(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
                        normal: vec3<f32>, pixel: vec2<f32>, ray_count: u32) -> f32 {
    let surface_origin = shadow_ray_origin(hit, ray_origin, ray_direction, normal);
    var occlusion_sum = 0.0;
    for (var ray_index = 0u; ray_index < ray_count; ray_index = ray_index + 1u) {
        let direction = ao_ray_direction(normal, pixel, ray_index, ray_count);
        let occluder = trace(surface_origin, direction, AO_MAX_DISTANCE);
        if (occluder.material != 0u) {
            if (AO_DISTANCE_FALLOFF) {
                occlusion_sum += 1.0 - clamp(occluder.distance / AO_MAX_DISTANCE, 0.0, 1.0);
            } else {
                occlusion_sum += 1.0;
            }
        }
    }
    return occlusion_sum / f32(ray_count);
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
fn ambient_occlusion(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
                     normal: vec3<f32>, pixel: vec2<f32>, sun_weight: f32) -> f32 {
    var fade = 1.0;
    if (AO_DISTANCE_FADE) {
        fade = ao_distance_fade(hit.distance);
        if (fade <= 0.0) {
            return 1.0; // sub-pixel detail at this range — skip the work entirely
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
            occlusion = ray_traced_occlusion(hit, ray_origin, ray_direction, normal,
                                             pixel, ray_count);
        }
    } else if (AO_MODE == AO_MODE_ANALYTIC_CORNER) {
        occlusion = analytic_corner_occlusion(hit, ray_origin, ray_direction, normal);
    } else if (AO_MODE == AO_MODE_ANALYTIC_NEIGHBORHOOD) {
        occlusion = analytic_neighborhood_occlusion(hit, normal);
    }
    return 1.0 - lighting.shading_params.x * fade * occlusion;
}

// Linear-space direct light: albedo * (sun lambert * visibility + hemisphere
// ambient * AO). One shadow ray per hit through `trace_shadow_visibility`
// (binary in hard mode, a penumbra factor in soft mode); faces pointing away
// from the sun skip the trace outright (their lambert term is zero anyway). AO
// attenuates ONLY the ambient (indirect) term. E4 composition contract: when
// CAGI lands, the indirect term becomes `cagi_sample * ambient_occlusion(...)`
// — AO stays a pure multiplier on indirect light and never touches the direct
// sun term.
fn shade_hit(hit: Hit, ray_origin: vec3<f32>, ray_direction: vec3<f32>,
             pixel: vec2<f32>) -> vec3<f32> {
    let normal = hit_normal(hit);
    let albedo = srgb_decode(palette[hit.material].rgb);

    var sun_visibility = 0.0;
    let sun_facing = dot(normal, lighting.sun_direction);
    if (sun_facing > 0.0) {
        let shadow_origin = shadow_ray_origin(hit, ray_origin, ray_direction, normal);
        sun_visibility = trace_shadow_visibility(shadow_origin, lighting.sun_direction);
    }
    let sun = lighting.sun_color_intensity.rgb * lighting.sun_color_intensity.w
        * max(sun_facing, 0.0) * sun_visibility;
    var ambient = ambient_light(normal);
    if (AO_MODE != AO_MODE_OFF) {
        ambient = ambient * ambient_occlusion(hit, ray_origin, ray_direction, normal,
                                             pixel, max(sun_facing, 0.0) * sun_visibility);
    }
    return albedo * (sun + ambient);
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
