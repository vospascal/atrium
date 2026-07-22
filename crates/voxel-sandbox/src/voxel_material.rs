//! Terrain material that moves per-voxel brightness **jitter** off the
//! vertices and into the fragment shader.
//!
//! The MagicaVoxel look needs a small per-voxel brightness speckle. Baking it
//! into vertex colors blocks greedy meshing (merging N voxels into one quad
//! collapses the per-voxel values). Instead we recompute the speckle in the
//! fragment shader from the fragment's world position, so it survives any
//! future merge. Every voxel type's jitter is mean-1.0
//! (`center + span·roll`, `center = 1 − span/2`), so only the per-type
//! **amplitude** (`span/2`) has to travel — the mesher packs it into the
//! otherwise-unused vertex-color alpha (terrain is opaque; water is a separate
//! material). See `docs/voxel-engine-plan.md` Stage 2.
//!
//! This is an [`ExtendedMaterial`] on top of [`StandardMaterial`], so all of
//! Bevy's PBR lighting, shadows, and fog still apply — we only add the jitter
//! multiply after the standard material is evaluated.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

/// The terrain material: StandardMaterial PBR + the voxel jitter extension.
pub type VoxelTerrainMaterial = ExtendedMaterial<StandardMaterial, VoxelExtension>;

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct VoxelExtension {
    /// `x` = world seed reinterpreted as f32 bits (`bitcast<u32>` in WGSL),
    /// `y` = VOXEL_SIZE (m), `z` = half world-X (voxels), `w` = half world-Z.
    /// Together these let the shader recover each fragment's voxel coordinate
    /// and hash it to the same jitter the CPU used to bake.
    #[uniform(100)]
    pub params: Vec4,
}

impl MaterialExtension for VoxelExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/voxel_terrain.wgsl".into()
    }
}

/// Build the extension uniform for a given world seed and grid dimensions.
pub fn voxel_extension(seed: u32, voxel_size: f32, half_x: f32, half_z: f32) -> VoxelExtension {
    VoxelExtension {
        params: Vec4::new(f32::from_bits(seed), voxel_size, half_x, half_z),
    }
}
