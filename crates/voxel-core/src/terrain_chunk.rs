//! On-demand base-terrain chunk generation for an infinite, streamable world.
//!
//! Where [`crate::world::VoxelWorld`] builds one finite floating island in a
//! single dense pass, this module generates the *base terrain* (stone, dirt,
//! grass, sand, snow, water — no trees or ground cover) of an **unbounded**
//! world one chunk at a time, on demand. Every property of a column at world
//! `(x, z)` is derived purely from its position via
//! [`terrain_column_height`], so a column generates identically no matter
//! which chunk asks for it — chunk seams are seamless (Stage 9, slice S2).
//!
//! The biome classification (which cap a column gets) is reproduced exactly
//! from [`crate::world`]'s `fill_column`, but the per-column inputs it needs —
//! slope, distance-to-water, dryness — are recomputed *per column* from
//! neighboring heights and position noise instead of from whole-grid maps, so
//! no full-world state is required.

use std::f32::consts::TAU;

use crate::noise::{fractal_noise_2d, hash_3d, hash_to_unit};
use crate::world::{
    terrain_column_height, Voxel, ALPINE_LINE_METERS, BEACH_DRY_METERS, BEACH_LUSH_METERS,
    BEACH_MAX_ALTITUDE_METERS, ROCK_SLOPE_RATIO, SNOW_LINE_METERS, SNOW_MAX_SLOPE_RATIO,
    VOXEL_SIZE, WATER_LEVEL, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z,
};

/// Columns per side of a terrain chunk. Matches the sandbox mesher's
/// `CHUNK_SIZE` so a generated chunk maps one-to-one onto a mesh chunk.
pub const CHUNK_COLUMNS: usize = 64;

/// Slope is measured over this ±radius window of columns (a 5×5 window),
/// matching `compute_slope_map`'s window so the cap classification agrees
/// with the island generator.
pub(crate) const SLOPE_WINDOW_RADIUS: i32 = 2;

/// Distance-to-water is searched within this world-space radius, in columns.
/// APPROXIMATION: the island path uses a whole-grid chamfer transform, which
/// finds water at any distance; here we cap the search so it stays per-column
/// and cheap. The cap comfortably exceeds the widest beach band
/// (`BEACH_DRY_METERS` ≈ 21 columns), so no beach classification is lost — a
/// column with no water inside the radius simply reports "no water".
pub(crate) const WATER_SEARCH_RADIUS: i32 = 24;

/// One streamed chunk of base terrain: a dense column-major voxel block for a
/// `CHUNK_COLUMNS`×`CHUNK_COLUMNS` footprint spanning the full world height.
pub struct TerrainChunk {
    /// World X of the chunk's `local_x == 0` edge.
    pub origin_x: i32,
    /// World Z of the chunk's `local_z == 0` edge.
    pub origin_z: i32,
    /// Columns per side (always [`CHUNK_COLUMNS`]).
    pub size: usize,
    /// Dense voxels, indexed `(y * size + local_z) * size + local_x`, with
    /// `y` in `0..WORLD_SIZE_Y` and `local_x`/`local_z` in `0..size`.
    pub voxels: Vec<Voxel>,
}

impl TerrainChunk {
    /// Voxel at chunk-local coordinates; [`Voxel::Air`] outside the block.
    pub fn get(&self, local_x: usize, y: usize, local_z: usize) -> Voxel {
        if local_x >= self.size || local_z >= self.size || y >= WORLD_SIZE_Y {
            return Voxel::Air;
        }
        self.voxels[(y * self.size + local_z) * self.size + local_x]
    }
}

/// Generate one chunk of base terrain. The chunk's world origin is
/// `(chunk_x * CHUNK_COLUMNS, chunk_z * CHUNK_COLUMNS)`; every column is
/// classified and filled from position-derived fields alone, so adjacent
/// chunks share their border columns exactly.
pub fn generate_terrain_chunk(chunk_x: i32, chunk_z: i32, seed: u32) -> TerrainChunk {
    let size = CHUNK_COLUMNS;
    let origin_x = chunk_x * CHUNK_COLUMNS as i32;
    let origin_z = chunk_z * CHUNK_COLUMNS as i32;
    let mut voxels = vec![Voxel::Air; size * size * WORLD_SIZE_Y];

    for local_z in 0..size {
        for local_x in 0..size {
            let world_x = origin_x + local_x as i32;
            let world_z = origin_z + local_z as i32;
            fill_terrain_column(&mut voxels, size, local_x, local_z, world_x, world_z, seed);
        }
    }

    TerrainChunk {
        origin_x,
        origin_z,
        size,
        voxels,
    }
}

/// Fill one column of the dense block: stone core from bedrock (`y = 0`) up to
/// the column height, a subsoil band under the cap, the biome cap on top, then
/// water up to the water line. Unlike the island's `fill_column`, an infinite
/// world has no sculpted underside — the stone core starts at bedrock.
#[allow(clippy::too_many_arguments)]
fn fill_terrain_column(
    voxels: &mut [Voxel],
    size: usize,
    local_x: usize,
    local_z: usize,
    world_x: i32,
    world_z: i32,
    seed: u32,
) {
    let column_height = terrain_column_height(world_x, world_z, seed);
    let cap = classify_cap(world_x, world_z, column_height, seed);
    let subsoil = match cap {
        Voxel::Sand => Voxel::Sand,
        Voxel::Sediment => Voxel::Sediment,
        Voxel::Grass => Voxel::Dirt,
        _ => Voxel::Stone,
    };

    let mut set = |y: i32, voxel: Voxel| {
        if (0..WORLD_SIZE_Y as i32).contains(&y) {
            voxels[(y as usize * size + local_z) * size + local_x] = voxel;
        }
    };

    for y in 0..=column_height {
        let voxel = if y == column_height {
            cap
        } else if y >= column_height - 3 {
            subsoil
        } else {
            Voxel::Stone
        };
        set(y, voxel);
    }
    for y in (column_height + 1)..=WATER_LEVEL {
        set(y, Voxel::Water);
    }
}

/// The surface cap for a column, reproducing `world::WorldBuilder::fill_column`
/// exactly: underwater sand/sediment by depth, then snow, rock, beach sand, or
/// grass by altitude, slope, and distance to water.
fn classify_cap(world_x: i32, world_z: i32, column_height: i32, seed: u32) -> Voxel {
    let slope = slope_at(world_x, world_z, seed);
    let water_distance = water_distance_at(world_x, world_z, seed);
    let dryness = dryness_at(world_x, world_z, seed);
    classify_cap_with(
        world_x,
        world_z,
        column_height,
        seed,
        slope,
        water_distance,
        dryness,
    )
}

/// The cap classification with its per-column inputs already computed, so a
/// caller that needs slope/water-distance/dryness for its own decisions (the
/// infinite [`crate::streamed_source::StreamedSource`]) can share them instead
/// of recomputing the costly water-distance search. Identical result to
/// [`classify_cap`].
#[allow(clippy::too_many_arguments)]
pub(crate) fn classify_cap_with(
    world_x: i32,
    world_z: i32,
    column_height: i32,
    seed: u32,
    slope: f32,
    water_distance: f32,
    dryness: f32,
) -> Voxel {
    let altitude_meters = (column_height - WATER_LEVEL) as f32 * VOXEL_SIZE;

    if column_height < WATER_LEVEL {
        // Underwater bed: sandy shallows fading into dark sediment, with a
        // noise-wavy boundary so the transition meanders.
        let depth_meters = (WATER_LEVEL - column_height) as f32 * VOXEL_SIZE;
        let sand_limit = 0.20
            + 0.40
                * fractal_noise_2d(
                    world_x as f32 * 0.03 + 7100.0,
                    world_z as f32 * 0.03,
                    2,
                    seed.wrapping_add(67),
                );
        if depth_meters <= sand_limit {
            Voxel::Sand
        } else {
            Voxel::Sediment
        }
    } else if altitude_meters > SNOW_LINE_METERS && slope <= SNOW_MAX_SLOPE_RATIO {
        Voxel::Snow
    } else if (slope > ROCK_SLOPE_RATIO && altitude_meters > 1.2)
        || altitude_meters > ALPINE_LINE_METERS
    {
        // Slope-rock needs some altitude: pond banks are steep enough to trip
        // the rule but are soil, not cliffs.
        Voxel::Stone
    } else if water_distance <= BEACH_LUSH_METERS + (BEACH_DRY_METERS - BEACH_LUSH_METERS) * dryness
        && altitude_meters <= BEACH_MAX_ALTITUDE_METERS
    {
        Voxel::Sand
    } else {
        Voxel::Grass
    }
}

/// Terrain steepness at a column (rise over run, 1.0 = 45°), from the height
/// span over a 5×5 window. Reproduces `compute_slope_map` per column, drawing
/// neighbor heights straight from [`terrain_column_height`] (an infinite world
/// has no `NO_LAND` columns to skip).
pub(crate) fn slope_at(world_x: i32, world_z: i32, seed: u32) -> f32 {
    let center = terrain_column_height(world_x, world_z, seed);
    let mut lowest = center;
    let mut highest = center;
    for offset_z in -SLOPE_WINDOW_RADIUS..=SLOPE_WINDOW_RADIUS {
        for offset_x in -SLOPE_WINDOW_RADIUS..=SLOPE_WINDOW_RADIUS {
            let neighbor = terrain_column_height(world_x + offset_x, world_z + offset_z, seed);
            lowest = lowest.min(neighbor);
            highest = highest.max(neighbor);
        }
    }
    (highest - lowest) as f32 / (2 * SLOPE_WINDOW_RADIUS) as f32
}

/// Distance to the nearest water column, in meters. Searches a fixed
/// world-space radius ([`WATER_SEARCH_RADIUS`]) for the nearest column whose
/// terrain sits below the water line; returns `f32::MAX` if none is within
/// the radius (see the constant's note on this approximation).
pub(crate) fn water_distance_at(world_x: i32, world_z: i32, seed: u32) -> f32 {
    let mut nearest_squared: Option<i32> = None;
    for offset_z in -WATER_SEARCH_RADIUS..=WATER_SEARCH_RADIUS {
        for offset_x in -WATER_SEARCH_RADIUS..=WATER_SEARCH_RADIUS {
            let distance_squared = offset_x * offset_x + offset_z * offset_z;
            if distance_squared > WATER_SEARCH_RADIUS * WATER_SEARCH_RADIUS {
                continue;
            }
            if nearest_squared.is_some_and(|best| distance_squared >= best) {
                continue;
            }
            if terrain_column_height(world_x + offset_x, world_z + offset_z, seed) < WATER_LEVEL {
                nearest_squared = Some(distance_squared);
            }
        }
    }
    match nearest_squared {
        Some(distance_squared) => (distance_squared as f32).sqrt() * VOXEL_SIZE,
        None => f32::MAX,
    }
}

/// Biome dryness at a column, 0.0 (lush) to 1.0 (desert). Reproduces
/// `compute_dryness_map`'s directional sweep plus low-frequency wobble. The
/// sweep is anchored to the island's center (`WORLD_SIZE_X`/`WORLD_SIZE_Z`
/// halves) so an infinite world's gradient reads identically to the island's;
/// the value is still a pure function of position and seed.
pub(crate) fn dryness_at(world_x: i32, world_z: i32, seed: u32) -> f32 {
    let gradient_angle = hash_to_unit(hash_3d(17, 23, 29, seed.wrapping_add(97))) * TAU;
    let gradient_x = gradient_angle.cos();
    let gradient_z = gradient_angle.sin();
    let half_x = WORLD_SIZE_X as f32 / 2.0;
    let half_z = WORLD_SIZE_Z as f32 / 2.0;

    let centered_x = (world_x as f32 - half_x) / half_x;
    let centered_z = (world_z as f32 - half_z) / half_z;
    let sweep = (centered_x * gradient_x + centered_z * gradient_z) * 0.5 + 0.5;
    let wobble = fractal_noise_2d(
        world_x as f32 * 0.01 + 1700.0,
        world_z as f32 * 0.01,
        3,
        seed.wrapping_add(23),
    );
    (sweep * 1.3 - 0.15 + (wobble - 0.5) * 0.55).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_generation_is_deterministic() {
        let first = generate_terrain_chunk(2, 3, 42);
        let second = generate_terrain_chunk(2, 3, 42);
        assert_eq!(first.origin_x, second.origin_x);
        assert_eq!(first.origin_z, second.origin_z);
        assert_eq!(first.voxels, second.voxels);
    }

    #[test]
    fn chunk_borders_are_seamless() {
        // Chunk (0,0) owns world x 0..CHUNK_COLUMNS; chunk (1,0) owns the next
        // strip. Their abutting columns — the left chunk's last (world x =
        // CHUNK_COLUMNS-1) and the right chunk's first (world x = CHUNK_COLUMNS)
        // — must each match an independent single-column generation at the same
        // world coordinate. Position-only determinism ⇒ the seam is invisible.
        let left = generate_terrain_chunk(0, 0, 7);
        let right = generate_terrain_chunk(1, 0, 7);
        assert_eq!(right.origin_x, CHUNK_COLUMNS as i32);

        let left_edge_world_x = CHUNK_COLUMNS as i32 - 1;
        let right_edge_world_x = CHUNK_COLUMNS as i32;
        for local_z in 0..CHUNK_COLUMNS {
            let world_z = local_z as i32;
            for y in 0..WORLD_SIZE_Y {
                let left_edge = left.get(CHUNK_COLUMNS - 1, y, local_z);
                assert_eq!(
                    left_edge,
                    reference_column_voxel(left_edge_world_x, world_z, y, 7),
                    "left edge not position-deterministic at world \
                     ({left_edge_world_x},{world_z}) y={y}"
                );
                let right_edge = right.get(0, y, local_z);
                assert_eq!(
                    right_edge,
                    reference_column_voxel(right_edge_world_x, world_z, y, 7),
                    "right edge not position-deterministic at world \
                     ({right_edge_world_x},{world_z}) y={y}"
                );
            }
        }
    }

    /// Generate a single world column's voxel at `y` independently of any
    /// chunk, so seam tests can assert per-`(x,z)` determinism.
    fn reference_column_voxel(world_x: i32, world_z: i32, y: usize, seed: u32) -> Voxel {
        let mut voxels = vec![Voxel::Air; WORLD_SIZE_Y];
        fill_terrain_column(&mut voxels, 1, 0, 0, world_x, world_z, seed);
        voxels[y]
    }

    #[test]
    fn base_materials_are_present() {
        let mut has_grass = false;
        let mut has_stone = false;
        let mut has_water = false;
        // A spread of chunks so at least one hits hills, cliffs, and water.
        for chunk_x in -1..=2 {
            for chunk_z in -1..=2 {
                let chunk = generate_terrain_chunk(chunk_x, chunk_z, 1);
                for voxel in &chunk.voxels {
                    match voxel {
                        Voxel::Grass => has_grass = true,
                        Voxel::Stone => has_stone = true,
                        Voxel::Water => has_water = true,
                        _ => {}
                    }
                }
            }
        }
        assert!(has_grass, "expected some grass across the sampled chunks");
        assert!(has_stone, "expected some stone across the sampled chunks");
        assert!(has_water, "expected some water across the sampled chunks");
    }

    #[test]
    fn columns_are_solid_to_bedrock() {
        // Infinite terrain fills the stone core from bedrock, with no sculpted
        // underside — the very bottom voxel of every column is solid.
        let chunk = generate_terrain_chunk(0, 0, 5);
        for local_z in 0..CHUNK_COLUMNS {
            for local_x in 0..CHUNK_COLUMNS {
                assert!(
                    chunk.get(local_x, 0, local_z).is_solid(),
                    "expected bedrock at chunk-local ({local_x},{local_z})"
                );
            }
        }
    }
}
