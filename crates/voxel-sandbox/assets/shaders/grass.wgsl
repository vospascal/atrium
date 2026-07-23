// Grass main-pass vertex shader: Bevy's default mesh vertex transform (no
// skin/morph — grass is static geometry) plus a wind sway on the world
// position. The fragment is StandardMaterial's default (vertex-color tone), so
// this only overrides the vertex stage. Its positions MUST match
// grass_prepass.wgsl bit-for-bit (same shared wind fn) or the depth prepass
// z-fights.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io::{Vertex, VertexOutput},
}

// Wind time on the material bind group (x = now, y = previous frame). NOT
// `globals`, which isn't bound in the depth prepass — grass_prepass.wgsl reads
// this same uniform so both passes agree on depth.
@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> grass_wind_time: vec4<f32>;

// Wind offset, INLINED (kept byte-identical to grass_prepass.wgsl so the main
// and prepass passes compute the same depth). `base` = clump world origin,
// `height` = object-space height up the blade, `time` = sample time.
const WIND_STRENGTH: f32 = 0.35;
fn grass_wind_offset(base: vec3<f32>, height: f32, time: f32) -> vec3<f32> {
    let phase = base.x * 0.6 + base.z * 0.8;
    let sway_x = sin(time * 1.4 + phase) * 0.6 + sin(time * 2.7 + phase * 1.9) * 0.4;
    let sway_z = cos(time * 1.1 + phase * 1.3) * 0.6 + cos(time * 2.3 + phase * 0.7) * 0.4;
    let bend = max(height, 0.0) * 0.66 * WIND_STRENGTH;
    return vec3<f32>(sway_x * bend, 0.0, sway_z * bend);
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
    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    let wind = grass_wind_offset(world_from_local[3].xyz, vertex.position.y, grass_wind_time.x);
    out.world_position = vec4<f32>(out.world_position.xyz + wind, out.world_position.w);
    out.position = position_world_to_clip(out.world_position.xyz);
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
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index, world_from_local[3]);
#endif

    return out;
}
