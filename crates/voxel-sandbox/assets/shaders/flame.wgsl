// Flame material: unlit vertex-colored voxels that sway (anchored at the
// base, licking at the tip) and flicker. Emits HDR values above 1.0 so the
// bloom pass makes the fire glow.
//
// flame_params: x = flame height (m), y = sway amplitude (m),
//               z = sway speed, w = emissive gain.

#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::globals,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> flame_params: vec4<f32>;

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

    var position = vertex.position;
    let height_factor = clamp(position.y / max(flame_params.x, 0.001), 0.0, 1.0);
    let sway = height_factor * height_factor * flame_params.y;
    let t = globals.time * flame_params.z;
    let phase = position.x * 9.0 + position.z * 7.0;
    position.x += sway * (sin(t + phase) * 0.7 + sin(t * 1.9 + phase * 1.3) * 0.3);
    position.z += sway * (cos(t * 1.3 + phase) * 0.7 + sin(t * 2.3 + phase * 0.7) * 0.3);
    position.y += sway * 0.5 * sin(t * 2.7 + phase);

    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(position, 1.0),
    );
    out.position = position_world_to_clip(out.world_position.xyz);
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
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

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let t = globals.time;
    let phase = in.world_position.x * 13.0 + in.world_position.z * 11.0
        + in.world_position.y * 5.0;
    let flicker = 0.8 + 0.2 * sin(t * 11.0 + phase) + 0.12 * sin(t * 27.0 + phase * 1.7);
#ifdef VERTEX_COLORS
    let base_color = in.color.rgb;
#else
    let base_color = vec3<f32>(1.0, 0.5, 0.1);
#endif
    return vec4<f32>(base_color * flame_params.w * flicker, 1.0);
}
