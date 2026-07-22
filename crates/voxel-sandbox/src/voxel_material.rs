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
use bevy::render::storage::ShaderStorageBuffer;
use bevy::shader::ShaderRef;
use voxel_core::world::{VOXEL_SIZE, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};

/// The terrain material: StandardMaterial PBR + the voxel jitter/AO extension.
pub type VoxelTerrainMaterial = ExtendedMaterial<StandardMaterial, VoxelExtension>;

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct VoxelExtension {
    /// `x` = world seed reinterpreted as f32 bits (`bitcast<u32>` in WGSL),
    /// `y` = VOXEL_SIZE (m), `z` = half world-X (voxels), `w` = half world-Z.
    /// Lets the shader recover each fragment's voxel coordinate for both the
    /// jitter hash and the ambient-occlusion neighbor lookups.
    #[uniform(100)]
    pub params: Vec4,
    /// World voxel dimensions `(x, y, z, _)` — for indexing the occupancy bits.
    #[uniform(101)]
    pub dimensions: Vec4,
    /// Packed solid-occupancy bitset (1 bit/voxel). The shader reads it to
    /// recompute per-voxel ambient occlusion in the fragment stage, so greedy
    /// meshing can merge flat faces regardless of their AO.
    #[storage(102, read_only)]
    pub occupancy: Handle<ShaderStorageBuffer>,
}

impl MaterialExtension for VoxelExtension {
    fn fragment_shader() -> ShaderRef {
        "shaders/voxel_terrain.wgsl".into()
    }
}

/// Build the extension for a given world seed and a handle to its
/// solid-occupancy bitset (upload the bits from
/// [`voxel_core::world::VoxelWorld::solid_occupancy_bits`] into an
/// `Assets<ShaderStorageBuffer>` and pass the handle here).
pub fn voxel_extension(seed: u32, occupancy: Handle<ShaderStorageBuffer>) -> VoxelExtension {
    VoxelExtension {
        params: Vec4::new(
            f32::from_bits(seed),
            VOXEL_SIZE,
            WORLD_SIZE_X as f32 / 2.0,
            WORLD_SIZE_Z as f32 / 2.0,
        ),
        dimensions: Vec4::new(
            WORLD_SIZE_X as f32,
            WORLD_SIZE_Y as f32,
            WORLD_SIZE_Z as f32,
            0.0,
        ),
        occupancy,
    }
}
