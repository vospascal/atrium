//! One rendering interface for every voxel world.
//!
//! The greedy mesher and the terrain material were originally hardwired to the
//! fixed-footprint island ([`VoxelWorld`]): they read its dense RLE grid and
//! its whole-grid biome maps directly. `VoxelSource` lifts exactly what the
//! mesher needs — a dense voxel window plus the four per-column color
//! contexts — into a trait, so the infinite streamed world can implement it
//! too and be rendered by the *same* mesher and material. No second renderer.

use crate::world::{ChunkScratch, VoxelWorld, WORLD_SIZE_X, WORLD_SIZE_Z};
use std::sync::Arc;

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

/// Half the island's footprint, in voxels — the shift between the island's own
/// grid (`0..WORLD_SIZE`) and the shared world frame it is rendered in.
pub const ISLAND_HALF_X: i32 = WORLD_SIZE_X as i32 / 2;
/// See [`ISLAND_HALF_X`].
pub const ISLAND_HALF_Z: i32 = WORLD_SIZE_Z as i32 / 2;

/// The fixed island, presented as a **bounded source in the shared world
/// frame** so the chunk streamer can render it exactly like the infinite world.
///
/// [`VoxelWorld`] indexes its grid from `0..WORLD_SIZE`; the streamed world
/// meshes in signed world-voxel coordinates centred on the origin. Rather than
/// teach the streamer two coordinate conventions, this wrapper translates: a
/// world coordinate maps to the island cell `+ (ISLAND_HALF_X, ISLAND_HALF_Z)`,
/// which puts the island's centre at the world origin. Because the old
/// `world_offset` of `(half, half)` placed it there too, the emitted geometry
/// lands at byte-identical render positions — the island simply stops being a
/// special case.
///
/// Reads outside the footprint fall through to air / neutral column values, so
/// the streamer's apron lookups at the rim behave like any other empty chunk.
pub struct IslandSource {
    world: Arc<VoxelWorld>,
}

impl IslandSource {
    pub fn new(world: Arc<VoxelWorld>) -> Self {
        Self { world }
    }

    /// The wrapped island, for the consumers that still work in island-local
    /// coordinates (collision heights, the fluid sim, water proximity).
    pub fn world(&self) -> &Arc<VoxelWorld> {
        &self.world
    }

    /// Half-extent in chunks of `chunk_size` voxels: the streamer generates no
    /// chunk outside `[-bounds, bounds]`, which is what makes the island finite.
    /// Rounded up, so the rim chunks that only partly overlap the footprint are
    /// still meshed.
    pub fn chunk_bounds(chunk_size: i32) -> i32 {
        (ISLAND_HALF_X.max(ISLAND_HALF_Z) + chunk_size - 1) / chunk_size
    }
}

impl VoxelSource for IslandSource {
    fn unpack_chunk(&self, x_start: i32, x_end: i32, z_start: i32, z_end: i32) -> ChunkScratch {
        // Fill by run-walking the island's own grid, then re-anchor the window
        // into the world frame so the mesher's world-coordinate reads land.
        self.world
            .unpack_chunk(
                x_start + ISLAND_HALF_X,
                x_end + ISLAND_HALF_X,
                z_start + ISLAND_HALF_Z,
                z_end + ISLAND_HALF_Z,
            )
            .translated(-ISLAND_HALF_X, -ISLAND_HALF_Z)
    }

    fn dryness_at(&self, x: i32, z: i32) -> f32 {
        self.world.dryness_at(x + ISLAND_HALF_X, z + ISLAND_HALF_Z)
    }

    fn cover_at(&self, x: i32, z: i32) -> f32 {
        self.world.cover_at(x + ISLAND_HALF_X, z + ISLAND_HALF_Z)
    }

    fn water_distance_at(&self, x: i32, z: i32) -> f32 {
        self.world
            .water_distance_at(x + ISLAND_HALF_X, z + ISLAND_HALF_Z)
    }

    fn tree_tone_at(&self, x: i32, z: i32) -> f32 {
        self.world
            .tree_tone_at(x + ISLAND_HALF_X, z + ISLAND_HALF_Z)
    }

    fn world_offset(&self) -> (f32, f32) {
        // Already centred by the coordinate shift above.
        (0.0, 0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world::WORLD_SIZE_Y;

    fn island() -> IslandSource {
        IslandSource::new(Arc::new(VoxelWorld::generate(1, 0.0)))
    }

    /// The whole point of the wrapper: a world coordinate reads the island cell
    /// half a footprint away, so the island's centre sits at the world origin.
    #[test]
    fn column_reads_are_shifted_by_half_the_footprint() {
        let source = island();
        let world = source.world().clone();
        for (world_x, world_z) in [(0, 0), (-200, 137), (321, -64), (-499, 499)] {
            let (island_x, island_z) = (world_x + ISLAND_HALF_X, world_z + ISLAND_HALF_Z);
            assert_eq!(
                source.dryness_at(world_x, world_z),
                world.dryness_at(island_x, island_z)
            );
            assert_eq!(
                source.cover_at(world_x, world_z),
                world.cover_at(island_x, island_z)
            );
            assert_eq!(
                source.water_distance_at(world_x, world_z),
                world.water_distance_at(island_x, island_z)
            );
            assert_eq!(
                source.tree_tone_at(world_x, world_z),
                world.tree_tone_at(island_x, island_z)
            );
        }
    }

    /// The re-anchored window must hand the mesher exactly the voxels the
    /// island's own grid holds — same cells, addressed in the world frame.
    #[test]
    fn unpacked_window_matches_the_island_grid() {
        let source = island();
        let world = source.world().clone();
        // A chunk-sized window straddling the origin, i.e. the island's middle.
        let (x_start, x_end, z_start, z_end) = (-64, 0, -64, 0);
        let shifted = source.unpack_chunk(x_start, x_end, z_start, z_end);
        let direct = world.unpack_chunk(
            x_start + ISLAND_HALF_X,
            x_end + ISLAND_HALF_X,
            z_start + ISLAND_HALF_Z,
            z_end + ISLAND_HALF_Z,
        );

        let mut solid_seen = 0;
        for world_z in z_start..z_end {
            for world_x in x_start..x_end {
                for y in 0..WORLD_SIZE_Y as i32 {
                    let from_shifted = shifted.get(world_x, y, world_z);
                    let from_direct =
                        direct.get(world_x + ISLAND_HALF_X, y, world_z + ISLAND_HALF_Z);
                    assert_eq!(
                        from_shifted, from_direct,
                        "mismatch at world ({world_x},{y},{world_z})"
                    );
                    if from_shifted != crate::world::Voxel::Air {
                        solid_seen += 1;
                    }
                }
            }
        }
        // Guard against the test passing because both windows are empty air.
        assert!(
            solid_seen > 0,
            "window over the island centre held no terrain"
        );
    }

    /// Geometry must land where it always did. The old path meshed island cells
    /// and subtracted `world_offset = (half, half)`; the new path meshes world
    /// cells and subtracts nothing. Those agree exactly when `world = island -
    /// half`, which is the shift this source applies.
    #[test]
    fn render_positions_are_unchanged_by_the_reframing() {
        let source = island();
        assert_eq!(source.world_offset(), (0.0, 0.0));
        for island_x in [0, 1, 250, 500, 999] {
            let old_render_x = island_x as f32 - ISLAND_HALF_X as f32;
            let world_x = island_x - ISLAND_HALF_X;
            let new_render_x = world_x as f32 - source.world_offset().0;
            assert_eq!(old_render_x, new_render_x);
        }
    }

    /// The bounds the streamer clamps to must cover the whole footprint, or the
    /// rim of the island would silently never be meshed.
    #[test]
    fn chunk_bounds_cover_the_footprint() {
        let chunk_size = 64;
        let bounds = IslandSource::chunk_bounds(chunk_size);
        let reach = bounds * chunk_size;
        assert!(
            reach >= ISLAND_HALF_X && reach >= ISLAND_HALF_Z,
            "bounds {bounds} chunks reach {reach} voxels, footprint half-extent is \
             ({ISLAND_HALF_X}, {ISLAND_HALF_Z})"
        );
    }
}
