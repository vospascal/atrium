// dda.wgsl — Stage 1 primary-ray renderer: two-level DDA over the brickmap.
//
// Fullscreen compute pass (workgroup 8x8): one thread per output pixel builds
// a camera ray, traverses the two-level brickmap (coarse Amanatides & Woo DDA
// over 8^3-voxel bricks, fine DDA over the voxels inside occupied bricks),
// and writes a shaded color to an rgba8unorm storage texture. Misses get a
// vertical sky gradient.
//
// All traversal happens in VOXEL-space units: the camera position arrives in
// world meters and is divided by BrickmapMeta.voxel_size_meters once at ray
// setup; directions are unit-length either way (uniform scale). Palette
// colors are sRGB-encoded (as authored in voxel-sandbox's mesh.rs); this
// shader writes them shaded-as-is — the blit to the swapchain must not
// re-encode.
//
// Modularity (plan architecture rule): the trace pipeline is split into small
// named functions — ray_setup / intersect_world_bounds / trace (coarse brick
// DDA) / trace_brick (fine voxel DDA) / shade_hit / sky_color — so later
// stages (sun shadow ray, CAGI volume sampling) plug in as additional
// functions reusing `trace` without rewriting traversal.
//
// Bindings (group 0), matching the Rust-side layouts:
//   0  uniform  Camera        — camera.rs CameraUniform (80 bytes; position
//                               in world meters, ray basis vectors, resolution)
//   1  uniform  BrickmapMeta  — brickmap.rs BrickmapMetadata (32 bytes)
//   2  storage  brick_indices — dense brick-pointer grid, x-major then y then
//                               z; 0xffffffff = empty brick
//   3  storage  occupancy_words — 16 u32 words per occupied brick; bit index
//                               = local_x + local_y*8 + local_z*64
//   4  storage  material_words — 128 u32 words per occupied brick; one byte
//                               per voxel, same local index, little-endian
//                               (byte 0 = bits 0..8)
//   5  storage  palette       — array<vec4<f32>>, indexed by material id
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

struct BrickmapMeta {
    brick_grid_size: vec3<u32>,   // (125, 32, 125)
    occupied_brick_count: u32,
    world_size_voxels: vec3<u32>, // (1000, 256, 1000)
    voxel_size_meters: f32,       // 0.125
}

@group(0) @binding(0) var<uniform> camera: Camera;
@group(0) @binding(1) var<uniform> brickmap: BrickmapMeta;
@group(0) @binding(2) var<storage, read> brick_indices: array<u32>;
@group(0) @binding(3) var<storage, read> occupancy_words: array<u32>;
@group(0) @binding(4) var<storage, read> material_words: array<u32>;
@group(0) @binding(5) var<storage, read> palette: array<vec4<f32>>;
@group(0) @binding(6) var output: texture_storage_2d<rgba8unorm, write>;

const EMPTY_BRICK: u32 = 0xffffffffu;
const BRICK_SIZE: f32 = 8.0;
// Worst-case bricks crossed: 125 + 32 + 125 axis crossings plus slack.
const MAX_BRICK_STEPS: u32 = 512u;
// Worst-case voxels crossed inside one 8^3 brick: 8+8+8-2 = 22, plus slack.
const MAX_VOXEL_STEPS: u32 = 24u;
// Max trace distance, voxel units (world diagonal is ~1437).
const MAX_TRACE_DISTANCE: f32 = 2048.0;
const RAY_EPSILON: f32 = 1e-4;
// normalize(vec3(0.55, 0.8, 0.35)), precomputed.
const SUN_DIRECTION: vec3<f32> = vec3<f32>(0.53295, 0.77520, 0.33915);

struct Hit {
    material: u32,   // 0 = miss (Air is never occupied)
    axis: u32,       // face axis last stepped: 0 = x, 1 = y, 2 = z
    axis_sign: f32,  // sign of the ray direction along that axis
    distance: f32,   // ray parameter t, voxel units
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

// ---- Fine DDA: voxels inside one occupied brick -----------------------------

// Amanatides & Woo over the 8^3 voxels of the brick at `brick_cell`, using
// the occupancy bitmask. `t_enter`/`t_exit` bracket the ray's overlap with
// this brick (global t, voxel units); `entry_axis` is the axis whose face the
// ray crossed to arrive here (it becomes the hit normal if the first voxel
// tested is already occupied).
fn trace_brick(origin: vec3<f32>, direction: vec3<f32>, inverse_direction: vec3<f32>,
               pointer: u32, brick_cell: vec3<i32>, t_enter: f32, t_exit: f32,
               entry_axis: u32) -> Hit {
    var result: Hit;
    result.material = 0u;

    let brick_min = vec3<f32>(brick_cell) * BRICK_SIZE;
    let entry_point = origin + direction * (t_enter + RAY_EPSILON);
    var cell = clamp(vec3<i32>(floor(entry_point - brick_min)),
                     vec3<i32>(0, 0, 0), vec3<i32>(7, 7, 7));
    let step_direction = vec3<i32>(
        select(-1, 1, direction.x >= 0.0),
        select(-1, 1, direction.y >= 0.0),
        select(-1, 1, direction.z >= 0.0),
    );
    var t_max = vec3<f32>(
        boundary_t(origin.x, inverse_direction.x, brick_min.x + f32(cell.x), 1.0, step_direction.x),
        boundary_t(origin.y, inverse_direction.y, brick_min.y + f32(cell.y), 1.0, step_direction.y),
        boundary_t(origin.z, inverse_direction.z, brick_min.z + f32(cell.z), 1.0, step_direction.z),
    );
    let t_delta = abs(inverse_direction);

    var face_axis = entry_axis;
    var t = t_enter;
    for (var step_index = 0u; step_index < MAX_VOXEL_STEPS; step_index = step_index + 1u) {
        let bit = u32(cell.x) + u32(cell.y) * 8u + u32(cell.z) * 64u;
        let occupancy = occupancy_words[pointer * 16u + (bit >> 5u)];
        if (((occupancy >> (bit & 31u)) & 1u) == 1u) {
            let packed = material_words[pointer * 128u + (bit >> 2u)];
            result.material = (packed >> ((bit & 3u) * 8u)) & 0xffu;
            result.axis = face_axis;
            var direction_component = direction.x;
            if (face_axis == 1u) {
                direction_component = direction.y;
            } else if (face_axis == 2u) {
                direction_component = direction.z;
            }
            result.axis_sign = sign(direction_component);
            result.distance = t;
            return result;
        }

        // Step to the neighboring voxel across the nearest boundary.
        if (t_max.x <= t_max.y && t_max.x <= t_max.z) {
            t = t_max.x;
            cell.x = cell.x + step_direction.x;
            t_max.x = t_max.x + t_delta.x;
            face_axis = 0u;
        } else if (t_max.y <= t_max.z) {
            t = t_max.y;
            cell.y = cell.y + step_direction.y;
            t_max.y = t_max.y + t_delta.y;
            face_axis = 1u;
        } else {
            t = t_max.z;
            cell.z = cell.z + step_direction.z;
            t_max.z = t_max.z + t_delta.z;
            face_axis = 2u;
        }
        if (cell.x < 0 || cell.y < 0 || cell.z < 0 || cell.x > 7 || cell.y > 7 || cell.z > 7) {
            break; // left the brick — hand back to the coarse level
        }
        if (t > t_exit + RAY_EPSILON) {
            break;
        }
    }
    return result;
}

// ---- Coarse DDA: the brick grid ---------------------------------------------

// Trace one ray through the brick grid. Empty bricks (sentinel pointer) are
// skipped in a single step; occupied bricks run the fine DDA. Everything is
// in voxel units; `origin` must already be voxel-space.
fn trace(origin: vec3<f32>, direction: vec3<f32>) -> Hit {
    var result: Hit;
    result.material = 0u;

    let inverse_direction = vec3<f32>(
        safe_inverse(direction.x),
        safe_inverse(direction.y),
        safe_inverse(direction.z),
    );
    let bounds = intersect_world_bounds(origin, inverse_direction);
    let t_enter = bounds.x;
    let t_exit = bounds.y;
    if (t_enter > t_exit) {
        return result; // ray misses the world entirely
    }

    var t = t_enter + RAY_EPSILON;
    let start = origin + direction * t;
    let grid_size = vec3<i32>(brickmap.brick_grid_size);
    var brick = clamp(vec3<i32>(floor(start / BRICK_SIZE)),
                      vec3<i32>(0, 0, 0), grid_size - vec3<i32>(1, 1, 1));
    let step_direction = vec3<i32>(
        select(-1, 1, direction.x >= 0.0),
        select(-1, 1, direction.y >= 0.0),
        select(-1, 1, direction.z >= 0.0),
    );
    var t_max = vec3<f32>(
        boundary_t(origin.x, inverse_direction.x, f32(brick.x) * BRICK_SIZE, BRICK_SIZE, step_direction.x),
        boundary_t(origin.y, inverse_direction.y, f32(brick.y) * BRICK_SIZE, BRICK_SIZE, step_direction.y),
        boundary_t(origin.z, inverse_direction.z, f32(brick.z) * BRICK_SIZE, BRICK_SIZE, step_direction.z),
    );
    let t_delta = abs(inverse_direction) * BRICK_SIZE;

    var face_axis = u32(bounds.z); // world-entry face seeds the first normal
    let t_limit = min(t_exit, MAX_TRACE_DISTANCE);
    for (var step_index = 0u; step_index < MAX_BRICK_STEPS; step_index = step_index + 1u) {
        if (brick.x < 0 || brick.y < 0 || brick.z < 0 ||
            brick.x >= grid_size.x || brick.y >= grid_size.y || brick.z >= grid_size.z) {
            break; // exited the grid → sky
        }
        if (t > t_limit) {
            break; // beyond the world or the trace-distance guard → sky
        }

        let brick_cell = u32(brick.x)
            + u32(brick.y) * brickmap.brick_grid_size.x
            + u32(brick.z) * brickmap.brick_grid_size.x * brickmap.brick_grid_size.y;
        let pointer = brick_indices[brick_cell];
        if (pointer != EMPTY_BRICK) {
            let brick_exit = min(min(t_max.x, t_max.y), t_max.z);
            let fine = trace_brick(origin, direction, inverse_direction, pointer,
                                   brick, t, brick_exit, face_axis);
            if (fine.material != 0u) {
                return fine;
            }
        }

        // Step to the neighboring brick across the nearest boundary.
        if (t_max.x <= t_max.y && t_max.x <= t_max.z) {
            t = t_max.x;
            brick.x = brick.x + step_direction.x;
            t_max.x = t_max.x + t_delta.x;
            face_axis = 0u;
        } else if (t_max.y <= t_max.z) {
            t = t_max.y;
            brick.y = brick.y + step_direction.y;
            t_max.y = t_max.y + t_delta.y;
            face_axis = 1u;
        } else {
            t = t_max.z;
            brick.z = brick.z + step_direction.z;
            t_max.z = t_max.z + t_delta.z;
            face_axis = 2u;
        }
    }
    return result;
}

// ---- Shading ----------------------------------------------------------------

// Warm horizon fading into a blue zenith, with a soft sun glow.
fn sky_color(direction: vec3<f32>) -> vec3<f32> {
    let horizon = vec3<f32>(0.86, 0.78, 0.65);
    let zenith = vec3<f32>(0.30, 0.52, 0.86);
    let elevation = clamp(direction.y * 0.5 + 0.5, 0.0, 1.0);
    var sky = mix(horizon, zenith, smoothstep(0.42, 0.78, elevation));
    let sun_amount = pow(max(dot(direction, SUN_DIRECTION), 0.0), 64.0);
    sky = sky + vec3<f32>(1.0, 0.9, 0.7) * sun_amount * 0.5;
    return sky;
}

// Slight distinct tint per face axis so cube geometry reads even where the
// sun term is flat: tops full, bottoms dark, x faces warm-dim, z faces
// cool-dim.
fn face_tint(normal: vec3<f32>) -> vec3<f32> {
    if (normal.y > 0.5) {
        return vec3<f32>(1.0, 1.0, 1.0);
    }
    if (normal.y < -0.5) {
        return vec3<f32>(0.62, 0.62, 0.66);
    }
    if (abs(normal.x) > 0.5) {
        return vec3<f32>(0.93, 0.90, 0.86);
    }
    return vec3<f32>(0.84, 0.86, 0.93);
}

// Flat palette color, fixed-sun lambert lifted by a floor, per-axis tint.
fn shade_hit(hit: Hit) -> vec3<f32> {
    var normal = vec3<f32>(0.0, 0.0, 0.0);
    if (hit.axis == 0u) {
        normal.x = -hit.axis_sign;
    } else if (hit.axis == 1u) {
        normal.y = -hit.axis_sign;
    } else {
        normal.z = -hit.axis_sign;
    }
    let base = palette[hit.material].rgb;
    let diffuse = 0.55 + 0.45 * max(dot(normal, SUN_DIRECTION), 0.0);
    return base * diffuse * face_tint(normal);
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

    let hit = trace(origin, direction);
    var color = vec3<f32>(0.0, 0.0, 0.0);
    if (hit.material == 0u) {
        color = sky_color(direction);
    } else {
        color = shade_hit(hit);
    }
    textureStore(output, vec2<i32>(i32(invocation.x), i32(invocation.y)),
                 vec4<f32>(color, 1.0));
}
