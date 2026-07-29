// Terrain depth-prepass vertex shader. Mirrors Bevy's default prepass vertex
// (bevy_pbr prepass.wgsl), minus skin/morph, PLUS the identical unpacking of
// the 12-byte voxel vertex that voxel_terrain_vertex.wgsl does.
//
// Applying the SAME unpacking here is the whole point: the prepass and the main
// pass then compute the same position, so the terrain writes one depth and
// doesn't z-fight against itself. The fragment is Bevy's default prepass
// fragment (not overridden).

#import bevy_pbr::{
    mesh_functions,
    prepass_io::VertexOutput,
    view_transformations::position_world_to_clip,
}

// The packed layout, declared by hand because these are custom attributes.
struct Vertex {
    @builtin(instance_index) instance_index: u32,
    // xyz = chunk-local position, 16-bit fixed point; w = face word.
    @location(0) packed_position: vec4<u32>,
    @location(1) color: vec4<f32>,
};

// Kept byte-identical to voxel_terrain_vertex.wgsl.
const PACKED_POSITION_ORIGIN: f32 = -1.0;
const PACKED_POSITION_SPAN: f32 = 34.0;
const BAKED_AO_SENTINEL: f32 = 10.0;

fn unpack_local_position(packed: vec4<u32>) -> vec3<f32> {
    let normalized = vec3<f32>(f32(packed.x), f32(packed.y), f32(packed.z)) / 65535.0;
    return normalized * PACKED_POSITION_SPAN + PACKED_POSITION_ORIGIN;
}

fn unpack_face_normal(face_word: u32) -> vec3<f32> {
    switch face_word & 7u {
        case 0u: { return vec3<f32>(1.0, 0.0, 0.0); }
        case 1u: { return vec3<f32>(-1.0, 0.0, 0.0); }
        case 2u: { return vec3<f32>(0.0, 1.0, 0.0); }
        case 3u: { return vec3<f32>(0.0, -1.0, 0.0); }
        case 4u: { return vec3<f32>(0.0, 0.0, 1.0); }
        default: { return vec3<f32>(0.0, 0.0, -1.0); }
    }
}

fn unpack_alpha(face_word: u32) -> f32 {
    let amplitude = f32(face_word >> 4u) / 4095.0;
    let baked = (face_word & 8u) != 0u;
    return select(amplitude, amplitude + BAKED_AO_SENTINEL, baked);
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;

    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let face_word = vertex.packed_position.w;
    let local_position = unpack_local_position(vertex.packed_position);

    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(local_position, 1.0),
    );
    out.position = position_world_to_clip(out.world_position.xyz);
#ifdef UNCLIPPED_DEPTH_ORTHO_EMULATION
    out.unclipped_depth = out.position.z;
    out.position.z = min(out.position.z, 1.0);
#endif

#ifdef NORMAL_PREPASS_OR_DEFERRED_PREPASS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        unpack_face_normal(face_word),
        vertex.instance_index,
    );
#endif

#ifdef VERTEX_COLORS
    out.color = vec4<f32>(vertex.color.rgb, unpack_alpha(face_word));
#endif

#ifdef MOTION_VECTOR_PREPASS
    // Terrain geometry is static, so the previous position is the same local
    // vertex under the previous transform.
    let prev_model = mesh_functions::get_previous_world_from_local(vertex.instance_index);
    out.previous_world_position = mesh_functions::mesh_position_local_to_world(
        prev_model,
        vec4<f32>(local_position, 1.0),
    );
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
