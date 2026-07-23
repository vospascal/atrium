// Grass depth-prepass vertex shader. Mirrors Bevy's default prepass vertex
// (bevy_pbr prepass.wgsl), minus skin/morph, PLUS the identical grass wind from
// grass_wind.wgsl. Applying the SAME displacement here as in grass.wgsl is the
// whole point: the prepass and main pass then write the same depth, so the
// swaying grass doesn't z-fight. The fragment is Bevy's default prepass
// fragment (not overridden).

#import bevy_pbr::{
    mesh_functions,
    prepass_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
}

// Same wind time uniform as grass.wgsl (globals isn't bound in the prepass).
@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> grass_wind_time: vec4<f32>;

// Wind offset, INLINED — kept byte-identical to grass.wgsl so the prepass and
// main pass compute the same displaced depth (otherwise the grass z-fights).
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

    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    let wind = grass_wind_offset(world_from_local[3].xyz, vertex.position.y, grass_wind_time.x);
    out.world_position = vec4<f32>(out.world_position.xyz + wind, out.world_position.w);
    out.position = position_world_to_clip(out.world_position.xyz);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif

#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex.instance_index,
    );
#endif
#endif // NORMAL_PREPASS_OR_DEFERRED_PREPASS

#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif

#ifdef MOTION_VECTOR_PREPASS
    let prev_model = mesh_functions::get_previous_world_from_local(vertex.instance_index);
    let prev_world = mesh_functions::mesh_position_local_to_world(
        prev_model,
        vec4<f32>(vertex.position, 1.0),
    );
    // Displace the previous position with the previous frame's wind so motion
    // vectors track the sway correctly.
    let prev_wind = grass_wind_offset(
        prev_model[3].xyz,
        vertex.position.y,
        grass_wind_time.y,
    );
    out.previous_world_position = vec4<f32>(prev_world.xyz + prev_wind, prev_world.w);
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
