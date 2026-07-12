#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
}

struct GrassWindUniforms {
    time: f32,
    wind_strength: f32,
    wind_direction_x: f32,
    wind_direction_z: f32,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> wind: GrassWindUniforms;

// Simple hash for per-blade variation (deterministic from position)
fn hash12(p: vec2<f32>) -> f32 {
    var p3 = fract(vec3<f32>(p.x, p.y, p.x) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;

    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var world_pos = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));

    // UV.y = 0 at base, 1 at tip
    let height_percent = vertex.uv.y;

    // Per-blade random lean from world position hash
    let blade_root = world_pos.xz - vertex.position.xz * 0.01;
    let random_lean = (hash12(floor(blade_root * 100.0)) - 0.5) * 0.015;

    // Wind sway: sinusoidal wave
    let wave = sin(wind.time * 1.8 + (world_pos.x + world_pos.z) * 0.5);
    let sway = wave * wind.wind_strength * height_percent * 0.02;

    // Curve: random lean + wind, applied to tips only
    let curve = (random_lean * height_percent * height_percent) + sway;

    world_pos.x += wind.wind_direction_x * curve;
    world_pos.z += wind.wind_direction_z * curve;

    out.world_position = world_pos;
    out.position = position_world_to_clip(world_pos.xyz);

    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif

#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        vertex.tangent,
        vertex.instance_index,
    );
#endif

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

    return out;
}
