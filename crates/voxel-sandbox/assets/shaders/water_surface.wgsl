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
    view_transformations::position_world_to_clip,
    forward_io::{Vertex, VertexOutput},
}

// Per-corner surface heights (render metres), indexed by corner id (UV.x).
@group(#{MATERIAL_BIND_GROUP}) @binding(4)
var<storage, read> heights: array<f32>;

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
    // Lift this corner to its live sim height (X/Z are baked into the mesh).
    let corner_id = u32(vertex.uv.x + 0.5);
    world_position.y = heights[corner_id];
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
