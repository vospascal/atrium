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

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;

    // Standard local-to-world transform
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    var world_pos = mesh_functions::mesh_position_local_to_world(world_from_local, vec4<f32>(vertex.position, 1.0));

    // Wind displacement — only blade tips move, roots stay planted.
    // vertex.position.y is in local space (0 to ~28 for Grass.glb),
    // so normalize to 0-1 range using a reasonable max height.
    let normalized_height = clamp(vertex.position.y / 30.0, 0.0, 1.0);
    let bend = normalized_height * normalized_height;

    let phase = world_pos.x * 0.7 + world_pos.z * 0.5;
    let sway = (sin(wind.time * 1.5 + phase) * 0.6 + sin(wind.time * 3.7 + phase * 2.3) * 0.25)
               * wind.wind_strength * bend * 0.03;

    world_pos.x += wind.wind_direction_x * sway;
    world_pos.z += wind.wind_direction_z * sway;
    world_pos.y -= abs(sway) * 0.15;

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
