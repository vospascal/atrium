// Water surface vertex shader (F4 GPU displacement).
//
// The water mesh is a STATIC flat grid built once over the sim's wet region:
// each vertex sits at its final world X/Z with Y = 0, and carries its corner id
// in UV.x. Every fluid tick the CPU (or, in the compute path, the GPU) only
// rewrites the small `heights` storage buffer — this shader lifts each corner
// to its live surface height, so the surface animates without ever rebuilding
// or re-uploading the mesh itself.
//
// Water is alpha-blended and therefore excluded from the depth prepass, so
// (unlike grass) there is no prepass vertex shader to keep in lockstep.

#import bevy_pbr::{
    mesh_functions,
    mesh_view_bindings::globals,
    view_transformations::position_world_to_clip,
    forward_io::{Vertex, VertexOutput},
}

struct WaterUniform {
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    light_direction: vec4<f32>,
    // rgb = light colour, w = wave choppiness 0..1 (from the gusting wind).
    light_color: vec4<f32>,
    reflection: vec4<f32>,
    surface: vec4<f32>,
    // x = depth darkening, y = underside opacity, z = wave HEIGHT (metres).
    surface_extra: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> water: WaterUniform;

// Per-corner surface heights (render metres), indexed by corner id (UV.x).
@group(#{MATERIAL_BIND_GROUP}) @binding(4)
var<storage, read> heights: array<f32>;

// The wave field, COPIED byte-for-byte from water.wgsl. The fragment shader
// builds its wave normals from this same field, so displacing the surface with
// anything else would light the water as if it were a different shape. There is
// no depth prepass for water (it is alpha-blended and excluded), so unlike grass
// the risk here is normals disagreeing with geometry, not z-fighting.
fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(0.1031, 0.1030));
    q += dot(q, q.yx + 33.33);
    return fract((q.x + q.y) * q.x);
}

fn value_noise_2d(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let t = fract(p);
    let u = t * t * (3.0 - 2.0 * t);
    let n00 = hash21(cell);
    let n10 = hash21(cell + vec2<f32>(1.0, 0.0));
    let n01 = hash21(cell + vec2<f32>(0.0, 1.0));
    let n11 = hash21(cell + vec2<f32>(1.0, 1.0));
    return mix(mix(n00, n10, u.x), mix(n01, n11, u.x), u.y);
}

// Wave heightfield: three octaves, direction rotated ~113° each octave so
// crests never line up, with a light domain warp from the running total.
fn wave_height(p_in: vec2<f32>, t_in: f32) -> f32 {
    var p = p_in;
    var direction = vec2<f32>(0.35, 0.94);
    let rotation = mat2x2<f32>(vec2<f32>(-0.39, 0.92), vec2<f32>(-0.92, -0.39));
    var amplitude = 1.0;
    var frequency = 1.4;
    var time = t_in;
    var total = 0.0;
    var norm = 0.0;
    for (var octave = 0; octave < 3; octave++) {
        total += value_noise_2d(p * frequency + direction * time) * amplitude;
        norm += amplitude;
        p += direction * total * 0.3;
        direction = rotation * direction;
        frequency *= 1.9;
        amplitude *= 0.55;
        time *= 1.35;
    }
    return total / norm;
}

// Vertical wave displacement in metres at a world XZ, matching the fragment's
// `t` and `p` exactly. `surface_extra.y` is the wave height the panel sets; the
// choppiness from the wind both raises the swell and speeds it up.
fn wave_displacement(world_xz: vec2<f32>) -> f32 {
    let choppiness = clamp(water.light_color.w, 0.0, 1.0);
    let t = globals.time * (0.5 + 1.2 * choppiness);
    let p = world_xz * 0.8;
    let height = water.surface_extra.z * (0.15 + 0.85 * choppiness);
    // `wave_height` returns 0..1, so centre it to swing symmetrically.
    return (wave_height(p, t) - 0.5) * 2.0 * height;
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;

    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );
#endif

#ifdef VERTEX_POSITIONS
    var world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    // Lift this corner to its live sim height (X/Z are baked into the mesh),
    // then ride the wind-driven swell on top of it. The streamed world has no
    // fluid sim — its `heights` buffer is one flat entry — so this displacement
    // is the only thing that makes its water actually move.
    let corner_id = u32(vertex.uv.x + 0.5);
    world_position.y = heights[corner_id] + wave_displacement(world_position.xz);
    out.world_position = world_position;
    out.position = position_world_to_clip(world_position.xyz);
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex.instance_index,
    );
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif

    return out;
}
