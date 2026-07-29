// Terrain main-pass vertex shader: unpacks the 12-byte voxel vertex (see
// `voxel_material::ATTRIBUTE_VOXEL_POSITION`) and otherwise does Bevy's default
// mesh vertex transform. The fragment stage is `voxel_terrain.wgsl`.
//
// Its unpacking MUST match voxel_terrain_prepass_vertex.wgsl bit-for-bit, or
// the depth prepass and the main pass disagree on depth and the terrain
// z-fights against itself.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io::VertexOutput,
}

// The packed layout, declared by hand because these are custom attributes.
struct Vertex {
    @builtin(instance_index) instance_index: u32,
    // xyz = chunk-local position, 16-bit fixed point; w = face word.
    @location(0) packed_position: vec4<u32>,
    @location(1) color: vec4<f32>,
};

// Must match `voxel_material::PACKED_POSITION_ORIGIN` / `_SPAN`.
const PACKED_POSITION_ORIGIN: f32 = -1.0;
const PACKED_POSITION_SPAN: f32 = 34.0;
// Must match `mesh::BAKED_AO_SENTINEL` — the offset the fragment shader reads
// off vertex alpha to mean "ambient occlusion is already in this colour".
const BAKED_AO_SENTINEL: f32 = 10.0;

fn unpack_local_position(packed: vec4<u32>) -> vec3<f32> {
    let normalized = vec3<f32>(f32(packed.x), f32(packed.y), f32(packed.z)) / 65535.0;
    return normalized * PACKED_POSITION_SPAN + PACKED_POSITION_ORIGIN;
}

// Voxel faces only ever point six ways, so the normal travels as a 3-bit index
// into `mesh::FACE_DIRECTIONS` rather than three floats.
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

// Rebuild the alpha convention the fragment shader still expects: the jitter
// amplitude, offset by the sentinel where AO is baked into the colour.
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

    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        unpack_face_normal(face_word),
        vertex.instance_index,
    );
    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(local_position, 1.0),
    );
    out.position = position_world_to_clip(out.world_position.xyz);

#ifdef VERTEX_COLORS
    out.color = vec4<f32>(vertex.color.rgb, unpack_alpha(face_word));
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
