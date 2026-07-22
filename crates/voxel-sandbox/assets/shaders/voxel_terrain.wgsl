// Terrain jitter extension for StandardMaterial.
//
// Recomputes the per-voxel brightness speckle in the fragment shader (from the
// fragment's world position) instead of reading it from baked vertex colors,
// so the look survives greedy meshing. The hash matches
// `voxel_core::noise::{hash_3d, hash_to_unit}` exactly, and the amplitude is
// carried in the vertex-color alpha by the mesher. See `voxel_material.rs`.

#import bevy_pbr::pbr_fragment::pbr_input_from_standard_material
#import bevy_pbr::pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing}
#import bevy_pbr::forward_io::{VertexOutput, FragmentOutput}

struct VoxelExtension {
    // x = seed (bitcast to u32), y = VOXEL_SIZE, z = half_x, w = half_z
    params: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> voxel_ext: VoxelExtension;

// Integer lattice hash — identical to voxel_core::noise::hash_3d.
fn hash_3d(x: i32, y: i32, z: i32, seed: u32) -> u32 {
    var h: u32 = (seed * 0x9E3779B9u)
        ^ (u32(x) * 0x85EBCA6Bu)
        ^ (u32(y) * 0xC2B2AE35u)
        ^ (u32(z) * 0x27D4EB2Fu);
    h = h ^ (h >> 15u);
    h = h * 0x2C1B3C6Du;
    h = h ^ (h >> 12u);
    h = h * 0x297A2D39u;
    h = h ^ (h >> 15u);
    return h;
}

// Map a hash to [0, 1) — identical to voxel_core::noise::hash_to_unit.
fn hash_to_unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // Jitter amplitude rides in the vertex-color alpha (0 = no jitter, e.g.
    // flowers). Grab it before the standard material consumes the color.
    let amplitude = in.color.a;

    // Evaluate the standard material (this multiplies base_color by the
    // vertex color rgb — the un-jittered voxel color the mesher baked).
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    // Recover the owning voxel coordinate. Vertices sit on face boundaries, so
    // step half a voxel back along the normal to land inside the cell — that
    // way all six faces of a voxel share one jitter value, and merged quads
    // still vary per cell across their surface.
    let seed = bitcast<u32>(voxel_ext.params.x);
    let voxel_size = voxel_ext.params.y;
    let half_x = voxel_ext.params.z;
    let half_z = voxel_ext.params.w;
    let sample = in.world_position.xyz - in.world_normal * (0.5 * voxel_size);
    let vx = i32(floor(sample.x / voxel_size + half_x));
    let vy = i32(floor(sample.y / voxel_size));
    let vz = i32(floor(sample.z / voxel_size + half_z));

    // roll in [0,1); jitter is mean-1.0: 1 + amplitude*(2*roll - 1).
    let roll = hash_to_unit(hash_3d(vx, vy, vz, seed + 3u));
    let jitter = 1.0 + amplitude * (2.0 * roll - 1.0);
    pbr_input.material.base_color = vec4<f32>(pbr_input.material.base_color.rgb * jitter, 1.0);

    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);
    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
