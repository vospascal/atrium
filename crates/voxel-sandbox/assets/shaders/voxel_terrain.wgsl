// Terrain jitter + ambient-occlusion extension for StandardMaterial.
//
// Both the per-voxel brightness speckle (jitter) and the corner ambient
// occlusion are recomputed here in the fragment shader — from the fragment's
// world position and a global solid-occupancy bitset — instead of being baked
// into vertex colors. That lets greedy meshing merge flat faces regardless of
// their AO without smearing it across the merged quad.
//
// Cover geometry (grass/flowers/reeds) is squashed, so its world position
// doesn't map cleanly to a voxel cell; it keeps its baked AO and is flagged by
// a sentinel in the vertex-color alpha (>= 1.0) so this shader skips AO for it.
// The jitter hash matches `voxel_core::noise::{hash_3d, hash_to_unit}`.

#import bevy_pbr::pbr_fragment::pbr_input_from_standard_material
#import bevy_pbr::pbr_functions::{alpha_discard, apply_pbr_lighting, main_pass_post_lighting_processing}
#import bevy_pbr::forward_io::{VertexOutput, FragmentOutput}

struct VoxelExtension {
    // x = seed (bitcast to u32), y = VOXEL_SIZE, z = half_x, w = half_z
    params: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> voxel_ext: VoxelExtension;
// x = WORLD_SIZE_X, y = WORLD_SIZE_Y, z = WORLD_SIZE_Z
@group(#{MATERIAL_BIND_GROUP}) @binding(101)
var<uniform> voxel_dims: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(102)
var<storage, read> occupancy: array<u32>;
// Hemisphere ambient (GI feel): sky = up-facing colour + strength (w),
// ground = down-facing bounce colour.
@group(#{MATERIAL_BIND_GROUP}) @binding(103)
var<uniform> ambient_sky: vec4<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(104)
var<uniform> ambient_ground: vec4<f32>;

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

fn hash_to_unit(h: u32) -> f32 {
    return f32(h >> 8u) / 16777216.0;
}

// Solid-occupancy lookup, matching voxel_core::world::solid_occupancy_bits:
// bit index = (z * WX + x) * WY + y.
fn is_solid(vx: i32, vy: i32, vz: i32) -> bool {
    let wx = i32(voxel_dims.x);
    let wy = i32(voxel_dims.y);
    let wz = i32(voxel_dims.z);
    if vx < 0 || vy < 0 || vz < 0 || vx >= wx || vy >= wy || vz >= wz {
        return false;
    }
    let index = (u32(vz) * u32(wx) + u32(vx)) * u32(wy) + u32(vy);
    return ((occupancy[index >> 5u] >> (index & 31u)) & 1u) == 1u;
}

// Vertex-corner ambient occlusion level (0..3), matching the CPU
// voxel_core / mesh::ambient_occlusion_level.
fn occlusion_level(side_1: bool, side_2: bool, corner: bool) -> f32 {
    if side_1 && side_2 {
        return 0.0;
    }
    return 3.0 - (f32(side_1) + f32(side_2) + f32(corner));
}

// Per-fragment ambient occlusion for a full-height terrain face. Reproduces
// the mesher's per-corner AO bilinearly interpolated across the cell, but
// evaluated per fragment so it stays tight to occluders on merged quads.
fn terrain_ao(world_position: vec3<f32>, normal: vec3<f32>) -> f32 {
    let voxel_size = voxel_ext.params.y;
    let offset = vec3<f32>(voxel_ext.params.z, 0.0, voxel_ext.params.w);

    // Two positive world-axis tangents perpendicular to the (axis-aligned)
    // normal. Labeling is arbitrary — AO is symmetric in the two tangents.
    let axis = abs(normal);
    var tangent_1: vec3<f32>;
    var tangent_2: vec3<f32>;
    if axis.x > 0.5 {
        tangent_1 = vec3<f32>(0.0, 1.0, 0.0);
        tangent_2 = vec3<f32>(0.0, 0.0, 1.0);
    } else if axis.y > 0.5 {
        tangent_1 = vec3<f32>(1.0, 0.0, 0.0);
        tangent_2 = vec3<f32>(0.0, 0.0, 1.0);
    } else {
        tangent_1 = vec3<f32>(1.0, 0.0, 0.0);
        tangent_2 = vec3<f32>(0.0, 1.0, 0.0);
    }

    // Owning cell (step half a voxel back along the normal), and the air layer
    // just outside the face where the occluders are sampled.
    let cell = floor(world_position / voxel_size + offset - 0.5 * normal);
    let base = cell + normal;
    let base_i = vec3<i32>(base);
    let t1_i = vec3<i32>(tangent_1);
    let t2_i = vec3<i32>(tangent_2);

    // Fractional position within the cell along each tangent (0..1).
    let frac = fract(world_position / voxel_size + offset);
    let fu = dot(frac, tangent_1);
    let fv = dot(frac, tangent_2);

    // Four corner AO levels: (a1, a2) in {-1,+1}^2 → (fu,fv) in {0,1}^2.
    let l00 = occlusion_level(
        is_solid(base_i.x - t1_i.x, base_i.y - t1_i.y, base_i.z - t1_i.z),
        is_solid(base_i.x - t2_i.x, base_i.y - t2_i.y, base_i.z - t2_i.z),
        is_solid(base_i.x - t1_i.x - t2_i.x, base_i.y - t1_i.y - t2_i.y, base_i.z - t1_i.z - t2_i.z),
    );
    let l10 = occlusion_level(
        is_solid(base_i.x + t1_i.x, base_i.y + t1_i.y, base_i.z + t1_i.z),
        is_solid(base_i.x - t2_i.x, base_i.y - t2_i.y, base_i.z - t2_i.z),
        is_solid(base_i.x + t1_i.x - t2_i.x, base_i.y + t1_i.y - t2_i.y, base_i.z + t1_i.z - t2_i.z),
    );
    let l11 = occlusion_level(
        is_solid(base_i.x + t1_i.x, base_i.y + t1_i.y, base_i.z + t1_i.z),
        is_solid(base_i.x + t2_i.x, base_i.y + t2_i.y, base_i.z + t2_i.z),
        is_solid(base_i.x + t1_i.x + t2_i.x, base_i.y + t1_i.y + t2_i.y, base_i.z + t1_i.z + t2_i.z),
    );
    let l01 = occlusion_level(
        is_solid(base_i.x - t1_i.x, base_i.y - t1_i.y, base_i.z - t1_i.z),
        is_solid(base_i.x + t2_i.x, base_i.y + t2_i.y, base_i.z + t2_i.z),
        is_solid(base_i.x - t1_i.x + t2_i.x, base_i.y - t1_i.y + t2_i.y, base_i.z - t1_i.z + t2_i.z),
    );

    let level = mix(mix(l00, l10, fu), mix(l01, l11, fu), fv);
    return 0.55 + 0.15 * level;
}

@fragment
fn fragment(in: VertexOutput, @builtin(front_facing) is_front: bool) -> FragmentOutput {
    // Alpha carries the jitter amplitude. A sentinel offset of +10 marks cover
    // geometry, which keeps its baked AO and skips the shader AO below.
    let raw_alpha = in.color.a;
    let is_cover = raw_alpha >= 1.0;
    let amplitude = select(raw_alpha, raw_alpha - 10.0, is_cover);

    // Evaluate the standard material (multiplies base_color by vertex rgb).
    var pbr_input = pbr_input_from_standard_material(in, is_front);

    // Per-voxel jitter (mean-1.0), recovered from world position.
    let seed = bitcast<u32>(voxel_ext.params.x);
    let voxel_size = voxel_ext.params.y;
    let offset = vec3<f32>(voxel_ext.params.z, 0.0, voxel_ext.params.w);
    let sample = in.world_position.xyz - in.world_normal * (0.5 * voxel_size);
    let voxel_coord = sample / voxel_size + offset;
    let voxel = vec3<i32>(floor(voxel_coord));
    let roll = hash_to_unit(hash_3d(voxel.x, voxel.y, voxel.z, seed + 3u));
    // Fade the hard per-voxel hash out under minification: when one pixel spans
    // ~a voxel or more (distance / the orbit miniature), the un-mipmapped hash
    // aliases into sparkling speckle across the whole scene. `fwidth` measures
    // voxels-per-pixel; keep full jitter up close, fade to a flat tone by the
    // time a pixel covers ~1.5 voxels.
    let voxels_per_pixel = length(fwidth(voxel_coord));
    let jitter_fade = clamp(1.5 - voxels_per_pixel, 0.0, 1.0);
    let jitter = 1.0 + amplitude * jitter_fade * (2.0 * roll - 1.0);

    var color = pbr_input.material.base_color.rgb * jitter;
    if !is_cover {
        // Terrain: baked AO was dropped from the mesh; apply it here so merged
        // flat faces still show tight corner occlusion. `ambient_ground.w` is a
        // live strength (1 = baked look, >1 deepens crevices, 0 = AO off).
        let ao = terrain_ao(in.world_position.xyz, in.world_normal);
        let ao_strength = clamp(1.0 - (1.0 - ao) * ambient_ground.w, 0.0, 1.0);
        color = color * ao_strength;
    }
    pbr_input.material.base_color = vec4<f32>(color, 1.0);

    pbr_input.material.base_color = alpha_discard(pbr_input.material, pbr_input.material.base_color);

    var out: FragmentOutput;
    out.color = apply_pbr_lighting(pbr_input);

    // Hemisphere ambient (Stage-7 GI feel): add sky colour to up-facing faces
    // and a warm ground-bounce to down-facing ones, tinted by the surface
    // colour. `ambient_sky.w` is the strength (0 = off, look unchanged).
    if ambient_sky.w > 0.0 {
        let up = in.world_normal.y * 0.5 + 0.5;
        let hemi = mix(ambient_ground.rgb, ambient_sky.rgb, up);
        out.color = vec4<f32>(out.color.rgb + color * hemi * ambient_sky.w, out.color.a);
    }

    out.color = main_pass_post_lighting_processing(pbr_input, out.color);
    return out;
}
