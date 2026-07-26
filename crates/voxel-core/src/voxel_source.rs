//! One rendering interface for every voxel world.
//!
//! The greedy mesher and the terrain material were originally hardwired to the
//! fixed-footprint island ([`VoxelWorld`]): they read its dense RLE grid and
//! its whole-grid biome maps directly. `VoxelSource` lifts exactly what the
//! mesher needs — a dense voxel window plus the four per-column color
//! contexts — into a trait, so the infinite streamed world can implement it
//! too and be rendered by the *same* mesher and material. No second renderer.

use crate::world::{ChunkScratch, VoxelWorld, WORLD_SIZE_X, WORLD_SIZE_Z};

/// A source of voxel content the mesher can render. Implemented by both the
/// fixed island ([`VoxelWorld`]) and the infinite streamed world, so one
/// greedy mesher + one material serve both.
///
/// `Sync` because the island meshes all its chunks in parallel (`rayon`).
pub trait VoxelSource: Sync {
    /// Dense voxel window over `[x_start, x_end) × [z_start, z_end)` plus the
    /// 1-cell apron the mesher needs for neighbor culling and corner AO.
    /// Coordinates are world-voxel coordinates; see [`ChunkScratch::from_columns`].
    fn unpack_chunk(&self, x_start: i32, x_end: i32, z_start: i32, z_end: i32) -> ChunkScratch;

    /// Biome dryness at a column, `0.0` (lush) to `1.0` (desert).
    fn dryness_at(&self, x: i32, z: i32) -> f32;

    /// Grass-patch coverage at a column, `0.0` (bare dirt) to `1.0` (dense).
    fn cover_at(&self, x: i32, z: i32) -> f32;

    /// Distance to the nearest water surface at a column, in meters.
    fn water_distance_at(&self, x: i32, z: i32) -> f32;

    /// Per-tree color identity at a column (`0..1`, `0.5` where no tree grew).
    fn tree_tone_at(&self, x: i32, z: i32) -> f32;

    /// Vertex-centering offset in voxels `(half_x, half_z)`: meshed geometry is
    /// emitted at `(voxel - offset) * VOXEL_SIZE`. The island centers on its
    /// footprint so it straddles the origin; the infinite world returns
    /// `(0.0, 0.0)` and meshes in raw world-voxel coordinates.
    fn world_offset(&self) -> (f32, f32);
}

impl VoxelSource for VoxelWorld {
    fn unpack_chunk(&self, x_start: i32, x_end: i32, z_start: i32, z_end: i32) -> ChunkScratch {
        VoxelWorld::unpack_chunk(self, x_start, x_end, z_start, z_end)
    }

    fn dryness_at(&self, x: i32, z: i32) -> f32 {
        VoxelWorld::dryness_at(self, x, z)
    }

    fn cover_at(&self, x: i32, z: i32) -> f32 {
        VoxelWorld::cover_at(self, x, z)
    }

    fn water_distance_at(&self, x: i32, z: i32) -> f32 {
        VoxelWorld::water_distance_at(self, x, z)
    }

    fn tree_tone_at(&self, x: i32, z: i32) -> f32 {
        VoxelWorld::tree_tone_at(self, x, z)
    }

    fn world_offset(&self) -> (f32, f32) {
        (WORLD_SIZE_X as f32 / 2.0, WORLD_SIZE_Z as f32 / 2.0)
    }
}
