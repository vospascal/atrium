//! Procedural voxel plateau generation.
//!
//! One seed → one floating-island diorama ("a nice sized scene and
//! nothing more"): an organic noise-jittered rim, rolling fBm hills on
//! top, a carved river with sandy banks, layered dirt/stone cliff sides,
//! a sculpted underside tapering toward the island's center, and
//! decoration passes (tall grass, flowers, blob-canopy trees). The fog
//! sea the island floats in is a shader effect (`fog_ring.rs`).

use std::f32::consts::TAU;

use bevy::math::IVec3;

use crate::noise::{fractal_noise_2d, hash_3d, hash_to_unit, smoothstep};

pub const WORLD_SIZE_X: usize = 1000;
pub const WORLD_SIZE_Y: usize = 256;
pub const WORLD_SIZE_Z: usize = 1000;

/// River surface sits at the top of this voxel layer.
pub const WATER_LEVEL: i32 = 84;

/// The island's flat rim underside starts here; toward the center the
/// sculpted underside tapers well below (floating-island bottom).
pub const PLATEAU_FLOOR: i32 = 72;

/// The underside reaches at most this far below the rim lip (meters),
/// deepest toward the island's center.
const UNDERSIDE_MAX_DEPTH_METERS: f32 = 8.0;
/// How fast the underside deepens per meter of distance from the rim.
const UNDERSIDE_TAPER_RATIO: f32 = 0.55;

/// Land occupies roughly this fraction of the half-extent, modulated by noise.
const LAND_RADIUS_FRACTION: f32 = 0.72;

/// Edge length of one voxel in meters. Fine voxels: gentle slopes terrace
/// into contour-line steps and trees get enough cells for detailed canopies.
pub const VOXEL_SIZE: f32 = 0.125;

/// Marks a column with no land at all (open sky beyond the rim).
const NO_LAND: i32 = -1;

// ---- Biome classification -------------------------------------------------
// Biomes are DERIVED from the terrain, never authored: per-column altitude,
// slope, and distance-to-water decide what each column is. The same terrain
// shape therefore always produces the same beaches, rock faces, snow caps,
// and tree belts — regardless of whether the heightmap came from the
// built-in generator or a Blender export.

/// Columns this close to water (and low enough) become sandy beach.
const BEACH_DISTANCE_METERS: f32 = 2.5;
const BEACH_MAX_ALTITUDE_METERS: f32 = 1.0;
/// Steeper than this (rise/run, 1.0 = 45°) the soil gives way to bare rock.
const ROCK_SLOPE_RATIO: f32 = 0.95;
/// Above this altitude everything is bare rock (alpine zone)…
const ALPINE_LINE_METERS: f32 = 14.0;
/// …and above this it is snow, unless too steep for snow to settle.
const SNOW_LINE_METERS: f32 = 17.0;
const SNOW_MAX_SLOPE_RATIO: f32 = 1.3;
/// Trees need gentle slopes and stop below the alpine zone.
const TREE_LINE_METERS: f32 = 12.0;
const TREE_MAX_SLOPE_RATIO: f32 = 0.65;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Voxel {
    Air,
    Grass,
    TallGrass,
    Dirt,
    Sand,
    Stone,
    Water,
    Trunk,
    Leaves,
    FlowerPink,
    FlowerWhite,
    FlowerYellow,
    /// Second canopy tone: slab-built tree crowns alternate light/dark chunks.
    LeavesDark,
    Snow,
}

impl Voxel {
    /// Solid voxels occlude faces and cast ambient occlusion; air and water do not.
    pub fn is_solid(self) -> bool {
        !matches!(self, Voxel::Air | Voxel::Water)
    }
}

pub struct VoxelWorld {
    pub voxels: Vec<Voxel>,
    /// Per-column biome dryness: 0 = lush green, 1 = desert. Drives the
    /// ground-color gradient and vegetation density.
    dryness: Vec<f32>,
    /// Per-column grass-patch coverage: 1 = dense clump, 0 = bare dirt
    /// between the patches. Drives tuft clustering and ground tint.
    ground_cover: Vec<f32>,
    /// Per-column steepness (rise over run, 1.0 = 45°), from the heightmap.
    slope: Vec<f32>,
    /// Per-column distance to the nearest water surface, meters.
    water_distance: Vec<f32>,
}

impl VoxelWorld {
    fn index(x: usize, y: usize, z: usize) -> usize {
        (y * WORLD_SIZE_Z + z) * WORLD_SIZE_X + x
    }

    pub fn get(&self, x: i32, y: i32, z: i32) -> Voxel {
        if x < 0
            || y < 0
            || z < 0
            || x >= WORLD_SIZE_X as i32
            || y >= WORLD_SIZE_Y as i32
            || z >= WORLD_SIZE_Z as i32
        {
            return Voxel::Air;
        }
        self.voxels[Self::index(x as usize, y as usize, z as usize)]
    }

    pub fn get_offset(&self, position: IVec3) -> Voxel {
        self.get(position.x, position.y, position.z)
    }

    fn set(&mut self, x: i32, y: i32, z: i32, voxel: Voxel) {
        if x < 0
            || y < 0
            || z < 0
            || x >= WORLD_SIZE_X as i32
            || y >= WORLD_SIZE_Y as i32
            || z >= WORLD_SIZE_Z as i32
        {
            return;
        }
        self.voxels[Self::index(x as usize, y as usize, z as usize)] = voxel;
    }

    /// Biome dryness at a column, `0.0` (lush) to `1.0` (desert).
    pub fn dryness_at(&self, x: i32, z: i32) -> f32 {
        if x < 0 || z < 0 || x >= WORLD_SIZE_X as i32 || z >= WORLD_SIZE_Z as i32 {
            return 0.0;
        }
        self.dryness[(z as usize) * WORLD_SIZE_X + x as usize]
    }

    /// Grass-patch coverage at a column, `0.0` (bare dirt) to `1.0` (dense clump).
    pub fn cover_at(&self, x: i32, z: i32) -> f32 {
        if x < 0 || z < 0 || x >= WORLD_SIZE_X as i32 || z >= WORLD_SIZE_Z as i32 {
            return 0.0;
        }
        self.ground_cover[(z as usize) * WORLD_SIZE_X + x as usize]
    }

    /// Fully procedural plateau (fBm heightmap + noise rim).
    pub fn generate(seed: u32) -> Self {
        Self::build(compute_heightmap(seed), seed, None)
    }

    /// Plateau from a Blender-exported heightmap (meters relative to the
    /// water plane, NaN = open sky), with optional authored tree positions.
    pub fn from_imported(terrain: &crate::terrain_import::ImportedTerrain, seed: u32) -> Self {
        let mut heights = vec![NO_LAND; WORLD_SIZE_X * WORLD_SIZE_Z];
        for z in 0..WORLD_SIZE_Z {
            for x in 0..WORLD_SIZE_X {
                let u = x as f32 / (WORLD_SIZE_X - 1) as f32;
                let v = z as f32 / (WORLD_SIZE_Z - 1) as f32;
                let height_meters = terrain.sample_height(u, v);
                if height_meters.is_nan() {
                    continue;
                }
                let height_voxels = WATER_LEVEL + (height_meters / VOXEL_SIZE).round() as i32;
                // Trees respect the tree line on their own, so only a thin
                // ceiling margin is reserved — peaks may climb to ~20 m.
                heights[z * WORLD_SIZE_X + x] =
                    height_voxels.clamp(PLATEAU_FLOOR + 2, WORLD_SIZE_Y as i32 - 8);
            }
        }

        let tree_positions: Option<Vec<(i32, i32)>> =
            terrain.tree_points_uv.as_ref().map(|points| {
                points
                    .iter()
                    .map(|&[u, v]| {
                        (
                            (u * (WORLD_SIZE_X - 1) as f32).round() as i32,
                            (v * (WORLD_SIZE_Z - 1) as f32).round() as i32,
                        )
                    })
                    .collect()
            });

        Self::build(heights, seed, tree_positions.as_deref())
    }

    /// Shared tail of every construction path: fill columns from the
    /// heightmap, then decorate and plant trees. (The fog ring hiding the
    /// world edge is a shader effect, not voxels — see `fog_ring.rs`.)
    fn build(heights: Vec<i32>, seed: u32, tree_positions: Option<&[(i32, i32)]>) -> Self {
        let mut world = Self {
            voxels: vec![Voxel::Air; WORLD_SIZE_X * WORLD_SIZE_Y * WORLD_SIZE_Z],
            dryness: compute_dryness_map(seed),
            ground_cover: compute_cover_map(seed),
            slope: compute_slope_map(&heights),
            water_distance: compute_water_distance_map(&heights),
        };

        let underside = compute_underside_map(&heights, seed);
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                let column_index = (z as usize) * WORLD_SIZE_X + x as usize;
                let column_height = heights[column_index];
                if column_height != NO_LAND {
                    world.fill_column(x, z, column_height, underside[column_index]);
                }
            }
        }

        world.decorate(&heights, seed);
        match tree_positions {
            Some(positions) => world.plant_trees_at(positions, &heights, seed),
            None => world.plant_trees(&heights, seed),
        }
        world
    }

    /// One terrain column: sculpted underside bottom, stone core, subsoil,
    /// biome-classified cap, water fill up to the water line.
    fn fill_column(&mut self, x: i32, z: i32, column_height: i32, underside_y: i32) {
        let altitude_meters = (column_height - WATER_LEVEL) as f32 * VOXEL_SIZE;
        let slope = self.slope_at(x, z);
        let water_distance = self.water_distance_at(x, z);

        let cap = if altitude_meters > SNOW_LINE_METERS && slope <= SNOW_MAX_SLOPE_RATIO {
            Voxel::Snow
        } else if slope > ROCK_SLOPE_RATIO || altitude_meters > ALPINE_LINE_METERS {
            Voxel::Stone
        } else if water_distance <= BEACH_DISTANCE_METERS
            && altitude_meters <= BEACH_MAX_ALTITUDE_METERS
        {
            Voxel::Sand
        } else {
            Voxel::Grass
        };
        let subsoil = match cap {
            Voxel::Sand => Voxel::Sand,
            Voxel::Grass => Voxel::Dirt,
            _ => Voxel::Stone,
        };

        for y in underside_y..=column_height {
            let voxel = if y == column_height {
                cap
            } else if y >= column_height - 3 {
                subsoil
            } else if y < underside_y + 10 {
                // Earthy skin on the island's sculpted bottom, so the
                // underside reads as hanging soil rather than gray slab.
                Voxel::Dirt
            } else {
                Voxel::Stone
            };
            self.set(x, y, z, voxel);
        }
        for y in (column_height + 1)..=WATER_LEVEL {
            self.set(x, y, z, Voxel::Water);
        }
    }

    /// Terrain steepness at a column (rise over run, 1.0 = 45°).
    pub fn slope_at(&self, x: i32, z: i32) -> f32 {
        if x < 0 || z < 0 || x >= WORLD_SIZE_X as i32 || z >= WORLD_SIZE_Z as i32 {
            return 0.0;
        }
        self.slope[(z as usize) * WORLD_SIZE_X + x as usize]
    }

    /// Distance to the nearest water surface at a column, meters.
    pub fn water_distance_at(&self, x: i32, z: i32) -> f32 {
        if x < 0 || z < 0 || x >= WORLD_SIZE_X as i32 || z >= WORLD_SIZE_Z as i32 {
            return f32::MAX;
        }
        self.water_distance[(z as usize) * WORLD_SIZE_X + x as usize]
    }

    /// Trees at authored positions (e.g. a Blender scatter), still subject
    /// to the "dry grassland only" rule.
    fn plant_trees_at(&mut self, positions: &[(i32, i32)], heights: &[i32], seed: u32) {
        for &(x, z) in positions {
            if x < 2 || z < 2 || x >= WORLD_SIZE_X as i32 - 2 || z >= WORLD_SIZE_Z as i32 - 2 {
                continue;
            }
            let column_height = heights[(z as usize) * WORLD_SIZE_X + x as usize];
            if column_height <= WATER_LEVEL + 1
                || self.get(x, column_height, z) != Voxel::Grass
                || !self.tree_can_grow(x, z, column_height)
            {
                continue;
            }
            self.grow_tree(x, column_height, z, seed);
        }
    }

    /// Trees follow the same derived fields as the ground cap: below the
    /// tree line, on gentle slopes only.
    fn tree_can_grow(&self, x: i32, z: i32, column_height: i32) -> bool {
        let altitude_meters = (column_height - WATER_LEVEL) as f32 * VOXEL_SIZE;
        altitude_meters < TREE_LINE_METERS && self.slope_at(x, z) <= TREE_MAX_SLOPE_RATIO
    }

    /// Tall grass tufts and flowers on dry grassland.
    fn decorate(&mut self, heights: &[i32], seed: u32) {
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                let column_height = heights[(z as usize) * WORLD_SIZE_X + x as usize];
                if column_height <= WATER_LEVEL + 1 {
                    continue;
                }
                if self.get(x, column_height, z) != Voxel::Grass {
                    continue;
                }
                // Grass grows in clumpy patches with bare dirt between them
                // (reference look), thinning toward the desert side.
                let lushness = 1.0 - self.dryness_at(x, z);
                let clump = smoothstep(0.40, 0.70, self.cover_at(x, z)) * lushness;
                let roll = hash_to_unit(hash_3d(x, 900, z, seed.wrapping_add(51)));
                if roll < 0.60 * clump {
                    self.set(x, column_height + 1, z, Voxel::TallGrass);
                } else if clump > 0.3 && roll > 0.9975 {
                    let flower = match hash_3d(x, 901, z, seed.wrapping_add(52)) % 3 {
                        0 => Voxel::FlowerPink,
                        1 => Voxel::FlowerWhite,
                        _ => Voxel::FlowerYellow,
                    };
                    self.set(x, column_height + 1, z, flower);
                }
            }
        }
    }

    /// Scatter blob-canopy trees on grassland with a minimum spacing.
    fn plant_trees(&mut self, heights: &[i32], seed: u32) {
        let mut placed_trees: Vec<(i32, i32)> = Vec::new();
        let minimum_spacing_squared = 46 * 46;

        for z in 16..(WORLD_SIZE_Z as i32 - 16) {
            for x in 16..(WORLD_SIZE_X as i32 - 16) {
                let column_height = heights[(z as usize) * WORLD_SIZE_X + x as usize];
                if column_height <= WATER_LEVEL + 3 {
                    continue;
                }
                if self.get(x, column_height, z) != Voxel::Grass
                    || !self.tree_can_grow(x, z, column_height)
                {
                    continue;
                }
                // Dense stands on the lush side, lone trees near the desert.
                let tree_probability = 0.0035 * (0.15 + 0.85 * (1.0 - self.dryness_at(x, z)));
                if hash_to_unit(hash_3d(x, 700, z, seed.wrapping_add(31))) >= tree_probability {
                    continue;
                }
                let too_close = placed_trees.iter().any(|&(tree_x, tree_z)| {
                    let dx = tree_x - x;
                    let dz = tree_z - z;
                    dx * dx + dz * dz < minimum_spacing_squared
                });
                if too_close {
                    continue;
                }
                placed_trees.push((x, z));
                self.grow_tree(x, column_height, z, seed);
            }
        }
    }

    fn grow_tree(&mut self, x: i32, ground_height: i32, z: i32, seed: u32) {
        let tree_hash = hash_3d(x, 800, z, seed.wrapping_add(41));
        // Chunky reference-style tree: thick 3×3 trunk and a crown built
        // from overlapping rectangular slabs in two leaf tones, instead of
        // a smooth ellipsoid blob. Tall — real trees tower over the 1.7 m
        // first-person eye, they don't sit at shoulder height.
        let trunk_height = 34 + (tree_hash % 18) as i32;

        for y in 1..=trunk_height {
            for offset_x in -1..=1 {
                for offset_z in -1..=1 {
                    self.set(x + offset_x, ground_height + y, z + offset_z, Voxel::Trunk);
                }
            }
        }

        let crown_center_y = ground_height + trunk_height + 5;
        let slab_count = 7 + (tree_hash >> 8) % 4;
        for slab_index in 0..slab_count as i32 {
            let slab_hash = hash_3d(
                x + slab_index * 37,
                810 + slab_index,
                z + slab_index * 53,
                seed.wrapping_add(43),
            );
            let unit_a = hash_to_unit(slab_hash);
            let unit_b = hash_to_unit(slab_hash.wrapping_mul(0x9E37_79B9));
            let unit_c = hash_to_unit(slab_hash.wrapping_mul(0x85EB_CA6B));
            let unit_d = hash_to_unit(slab_hash.wrapping_mul(0xC2B2_AE35));

            // The first slab is centered so the trunk always carries a crown.
            let (center_x, center_y, center_z) = if slab_index == 0 {
                (x, crown_center_y, z)
            } else {
                (
                    x + ((unit_a - 0.5) * 22.0) as i32,
                    crown_center_y + ((unit_b - 0.5) * 14.0) as i32,
                    z + ((unit_c - 0.5) * 22.0) as i32,
                )
            };
            let half_extent_x = 6 + (unit_d * 6.0) as i32;
            let half_extent_y = 3 + (unit_a * 3.0) as i32;
            let half_extent_z = 6 + (unit_b * 6.0) as i32;
            let leaf_tone = if slab_hash & 1 == 0 {
                Voxel::Leaves
            } else {
                Voxel::LeavesDark
            };

            for offset_y in -half_extent_y..=half_extent_y {
                for offset_z in -half_extent_z..=half_extent_z {
                    for offset_x in -half_extent_x..=half_extent_x {
                        let cell_x = center_x + offset_x;
                        let cell_y = center_y + offset_y;
                        let cell_z = center_z + offset_z;
                        if self.get(cell_x, cell_y, cell_z) == Voxel::Air {
                            self.set(cell_x, cell_y, cell_z, leaf_tone);
                        }
                    }
                }
            }
        }
    }
}

/// Steepness per column: height span over a 5×5 window, as rise over run
/// (1.0 = 45°). Windowed rather than nearest-neighbor so single voxel
/// terrace steps on gentle slopes don't read as cliffs.
fn compute_slope_map(heights: &[i32]) -> Vec<f32> {
    const WINDOW_RADIUS: i32 = 2;
    let mut slope_map = vec![0.0_f32; WORLD_SIZE_X * WORLD_SIZE_Z];
    for z in 0..WORLD_SIZE_Z as i32 {
        for x in 0..WORLD_SIZE_X as i32 {
            let center = heights[(z as usize) * WORLD_SIZE_X + x as usize];
            if center == NO_LAND {
                continue;
            }
            let mut lowest = center;
            let mut highest = center;
            for offset_z in -WINDOW_RADIUS..=WINDOW_RADIUS {
                for offset_x in -WINDOW_RADIUS..=WINDOW_RADIUS {
                    let sample_x = x + offset_x;
                    let sample_z = z + offset_z;
                    if sample_x < 0
                        || sample_z < 0
                        || sample_x >= WORLD_SIZE_X as i32
                        || sample_z >= WORLD_SIZE_Z as i32
                    {
                        continue;
                    }
                    let neighbor = heights[(sample_z as usize) * WORLD_SIZE_X + sample_x as usize];
                    if neighbor == NO_LAND {
                        continue;
                    }
                    lowest = lowest.min(neighbor);
                    highest = highest.max(neighbor);
                }
            }
            slope_map[(z as usize) * WORLD_SIZE_X + x as usize] =
                (highest - lowest) as f32 / (2 * WINDOW_RADIUS) as f32;
        }
    }
    slope_map
}

/// Distance to the nearest water column, meters, via a two-pass chamfer
/// transform (exact enough for beach bands at a fraction of a BFS's cost).
fn compute_water_distance_map(heights: &[i32]) -> Vec<f32> {
    const DIAGONAL: f32 = 1.414;
    let far = (WORLD_SIZE_X + WORLD_SIZE_Z) as f32;
    let mut distances: Vec<f32> = heights
        .iter()
        .map(|&height| {
            if height != NO_LAND && height < WATER_LEVEL {
                0.0
            } else {
                far
            }
        })
        .collect();

    let index_of = |x: usize, z: usize| z * WORLD_SIZE_X + x;
    for z in 0..WORLD_SIZE_Z {
        for x in 0..WORLD_SIZE_X {
            let mut best = distances[index_of(x, z)];
            if x > 0 {
                best = best.min(distances[index_of(x - 1, z)] + 1.0);
            }
            if z > 0 {
                best = best.min(distances[index_of(x, z - 1)] + 1.0);
                if x > 0 {
                    best = best.min(distances[index_of(x - 1, z - 1)] + DIAGONAL);
                }
                if x + 1 < WORLD_SIZE_X {
                    best = best.min(distances[index_of(x + 1, z - 1)] + DIAGONAL);
                }
            }
            distances[index_of(x, z)] = best;
        }
    }
    for z in (0..WORLD_SIZE_Z).rev() {
        for x in (0..WORLD_SIZE_X).rev() {
            let mut best = distances[index_of(x, z)];
            if x + 1 < WORLD_SIZE_X {
                best = best.min(distances[index_of(x + 1, z)] + 1.0);
            }
            if z + 1 < WORLD_SIZE_Z {
                best = best.min(distances[index_of(x, z + 1)] + 1.0);
                if x + 1 < WORLD_SIZE_X {
                    best = best.min(distances[index_of(x + 1, z + 1)] + DIAGONAL);
                }
                if x > 0 {
                    best = best.min(distances[index_of(x - 1, z + 1)] + DIAGONAL);
                }
            }
            distances[index_of(x, z)] = best;
        }
    }

    for distance in &mut distances {
        *distance *= VOXEL_SIZE;
    }
    distances
}

/// Bottom voxel of the island per column: at the rim the underside meets
/// the lip (`PLATEAU_FLOOR`); toward the center it tapers down like a
/// floating island's belly, roughened by noise and the occasional hanging
/// spike of rock.
fn compute_underside_map(heights: &[i32], seed: u32) -> Vec<i32> {
    let edge_distance = compute_edge_distance_map(heights);
    let mut underside = vec![PLATEAU_FLOOR; WORLD_SIZE_X * WORLD_SIZE_Z];
    for z in 0..WORLD_SIZE_Z {
        for x in 0..WORLD_SIZE_X {
            let column_index = z * WORLD_SIZE_X + x;
            if heights[column_index] == NO_LAND {
                continue;
            }
            let taper = (edge_distance[column_index] * UNDERSIDE_TAPER_RATIO)
                .min(UNDERSIDE_MAX_DEPTH_METERS);
            let roughness = fractal_noise_2d(
                x as f32 * 0.02 + 5200.0,
                z as f32 * 0.02,
                3,
                seed.wrapping_add(41),
            );
            let mut depth_meters = taper * (0.6 + 0.8 * roughness);
            // Occasional hanging spikes, like roots of rock.
            let spike = fractal_noise_2d(
                x as f32 * 0.085 + 6400.0,
                z as f32 * 0.085,
                2,
                seed.wrapping_add(43),
            );
            if spike > 0.78 && taper > 1.0 {
                depth_meters += (spike - 0.78) * 22.0;
            }
            let depth_voxels = (depth_meters / VOXEL_SIZE).round() as i32;
            underside[column_index] = (PLATEAU_FLOOR - depth_voxels).max(6);
        }
    }
    underside
}

/// Distance to the island's rim (the nearest `NO_LAND` column or the
/// world border), meters — same two-pass chamfer as the water map.
fn compute_edge_distance_map(heights: &[i32]) -> Vec<f32> {
    const DIAGONAL: f32 = 1.414;
    let far = (WORLD_SIZE_X + WORLD_SIZE_Z) as f32;
    let index_of = |x: usize, z: usize| z * WORLD_SIZE_X + x;
    let mut distances: Vec<f32> = heights
        .iter()
        .map(|&height| if height == NO_LAND { 0.0 } else { far })
        .collect();
    for z in 0..WORLD_SIZE_Z {
        distances[index_of(0, z)] = 0.0;
        distances[index_of(WORLD_SIZE_X - 1, z)] = 0.0;
    }
    for x in 0..WORLD_SIZE_X {
        distances[index_of(x, 0)] = 0.0;
        distances[index_of(x, WORLD_SIZE_Z - 1)] = 0.0;
    }

    for z in 0..WORLD_SIZE_Z {
        for x in 0..WORLD_SIZE_X {
            let mut best = distances[index_of(x, z)];
            if x > 0 {
                best = best.min(distances[index_of(x - 1, z)] + 1.0);
            }
            if z > 0 {
                best = best.min(distances[index_of(x, z - 1)] + 1.0);
                if x > 0 {
                    best = best.min(distances[index_of(x - 1, z - 1)] + DIAGONAL);
                }
                if x + 1 < WORLD_SIZE_X {
                    best = best.min(distances[index_of(x + 1, z - 1)] + DIAGONAL);
                }
            }
            distances[index_of(x, z)] = best;
        }
    }
    for z in (0..WORLD_SIZE_Z).rev() {
        for x in (0..WORLD_SIZE_X).rev() {
            let mut best = distances[index_of(x, z)];
            if x + 1 < WORLD_SIZE_X {
                best = best.min(distances[index_of(x + 1, z)] + 1.0);
            }
            if z + 1 < WORLD_SIZE_Z {
                best = best.min(distances[index_of(x, z + 1)] + 1.0);
                if x + 1 < WORLD_SIZE_X {
                    best = best.min(distances[index_of(x + 1, z + 1)] + DIAGONAL);
                }
                if x > 0 {
                    best = best.min(distances[index_of(x - 1, z + 1)] + DIAGONAL);
                }
            }
            distances[index_of(x, z)] = best;
        }
    }

    for distance in &mut distances {
        *distance *= VOXEL_SIZE;
    }
    distances
}

/// Grass-patch coverage: mid-frequency noise sharpened into clumps, so
/// tufts cluster into dense patches with bare dirt showing between.
fn compute_cover_map(seed: u32) -> Vec<f32> {
    let mut cover_map = vec![0.0_f32; WORLD_SIZE_X * WORLD_SIZE_Z];
    for z in 0..WORLD_SIZE_Z {
        for x in 0..WORLD_SIZE_X {
            cover_map[z * WORLD_SIZE_X + x] = fractal_noise_2d(
                x as f32 * 0.045 + 3100.0,
                z as f32 * 0.045,
                3,
                seed.wrapping_add(29),
            );
        }
    }
    cover_map
}

/// Biome gradient: a directional lush→desert sweep across the plateau,
/// broken up by low-frequency noise so the transition meanders.
fn compute_dryness_map(seed: u32) -> Vec<f32> {
    let mut dryness_map = vec![0.0_f32; WORLD_SIZE_X * WORLD_SIZE_Z];
    // Gradient direction varies per seed so regenerated plateaus differ.
    let gradient_angle = hash_to_unit(hash_3d(17, 23, 29, seed.wrapping_add(97))) * TAU;
    let gradient_x = gradient_angle.cos();
    let gradient_z = gradient_angle.sin();
    let half_x = WORLD_SIZE_X as f32 / 2.0;
    let half_z = WORLD_SIZE_Z as f32 / 2.0;

    for z in 0..WORLD_SIZE_Z {
        for x in 0..WORLD_SIZE_X {
            let centered_x = (x as f32 - half_x) / half_x;
            let centered_z = (z as f32 - half_z) / half_z;
            let sweep = (centered_x * gradient_x + centered_z * gradient_z) * 0.5 + 0.5;
            let wobble = fractal_noise_2d(
                x as f32 * 0.01 + 1700.0,
                z as f32 * 0.01,
                3,
                seed.wrapping_add(23),
            );
            dryness_map[z * WORLD_SIZE_X + x] =
                (sweep * 1.3 - 0.15 + (wobble - 0.5) * 0.55).clamp(0.0, 1.0);
        }
    }
    dryness_map
}

/// Heightmap: rolling fBm hills on a plateau slab with an organic rim,
/// and a river channel carved below the water line. `NO_LAND` beyond the rim.
fn compute_heightmap(seed: u32) -> Vec<i32> {
    let mut heights = vec![NO_LAND; WORLD_SIZE_X * WORLD_SIZE_Z];
    let half_x = WORLD_SIZE_X as f32 / 2.0;
    let half_z = WORLD_SIZE_Z as f32 / 2.0;

    for z in 0..WORLD_SIZE_Z {
        for x in 0..WORLD_SIZE_X {
            let world_x = x as f32;
            let world_z = z as f32;

            let centered_x = (world_x - half_x) / half_x;
            let centered_z = (world_z - half_z) / half_z;
            let radial_distance = (centered_x * centered_x + centered_z * centered_z).sqrt();

            // Organic rim: land radius wobbles with low-frequency noise.
            let rim_wobble = fractal_noise_2d(
                world_x * 0.015 + 900.0,
                world_z * 0.015,
                3,
                seed.wrapping_add(21),
            );
            let land_radius = LAND_RADIUS_FRACTION + (rim_wobble - 0.5) * 0.16;
            if radial_distance > land_radius {
                continue;
            }

            // Gentle rolling hills: long slopes that terrace into single-voxel
            // contour steps, not steep blocky terrain.
            let rolling = fractal_noise_2d(world_x * 0.007, world_z * 0.007, 5, seed);
            let detail = fractal_noise_2d(world_x * 0.03, world_z * 0.03, 4, seed.wrapping_add(7));
            let hill_shape = rolling * 0.85 + detail * 0.15;
            let mut height = (WATER_LEVEL + 4) as f32 + hill_shape * 12.0;

            // River: carve wherever the channel noise crosses its midline.
            let river_noise = fractal_noise_2d(
                world_x * 0.006 + 400.0,
                world_z * 0.006,
                4,
                seed.wrapping_add(13),
            );
            let channel_distance = (river_noise - 0.5).abs();
            // Wide gentle banks (out to channel_width) around a full-depth
            // channel core, so the river reads as a river, not a slot canyon.
            let channel_width = 0.085;
            if channel_distance < channel_width {
                let carve = smoothstep(channel_width, 0.02, channel_distance);
                let river_bed = (WATER_LEVEL - 3) as f32 - carve * 2.0;
                height += (river_bed - height) * carve;
            }

            heights[z * WORLD_SIZE_X + x] =
                (height.round() as i32).clamp(PLATEAU_FLOOR + 2, WORLD_SIZE_Y as i32 - 8);
        }
    }
    heights
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn beyond_the_rim_is_open_sky() {
        let world = VoxelWorld::generate(1);
        for &(x, z) in &[
            (1, 1),
            (WORLD_SIZE_X as i32 - 2, 1),
            (1, WORLD_SIZE_Z as i32 - 2),
            (WORLD_SIZE_X as i32 - 2, WORLD_SIZE_Z as i32 - 2),
        ] {
            for y in 0..WORLD_SIZE_Y as i32 {
                let voxel = world.get(x, y, z);
                assert!(
                    matches!(voxel, Voxel::Air),
                    "expected open sky at corner ({x},{y},{z}), got {voxel:?}"
                );
            }
        }
    }

    #[test]
    fn plateau_has_grass_and_river() {
        let world = VoxelWorld::generate(1);
        let mut grass_count = 0;
        let mut water_count = 0;
        let mut underside_count = 0;
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                for y in 0..WORLD_SIZE_Y as i32 {
                    match world.get(x, y, z) {
                        Voxel::Grass => {
                            grass_count += 1;
                            assert!(
                                world.get(x, y - 1, z).is_solid(),
                                "grass floating at ({x},{y},{z})"
                            );
                        }
                        Voxel::Water => water_count += 1,
                        voxel => {
                            if voxel.is_solid() && y < PLATEAU_FLOOR {
                                underside_count += 1;
                            }
                        }
                    }
                }
            }
        }
        assert!(
            grass_count > 2000,
            "expected a plateau, got {grass_count} grass voxels"
        );
        assert!(
            water_count > 200,
            "expected a river, got {water_count} water voxels"
        );
        assert!(
            underside_count > 100_000,
            "expected a sculpted floating-island underside below the rim lip, \
             got {underside_count} voxels"
        );
    }

    #[test]
    fn biomes_derive_from_terrain_shape() {
        // Synthetic cone mountain: 24 m peak in the middle, shore at the
        // edges dipping underwater. Biomes must fall out of the shape alone.
        let grid_side = 65;
        let mut grid = vec![0.0_f32; grid_side * grid_side];
        for z in 0..grid_side {
            for x in 0..grid_side {
                let dx = (x as f32 / (grid_side - 1) as f32 - 0.5) * 2.0;
                let dz = (z as f32 / (grid_side - 1) as f32 - 0.5) * 2.0;
                let radial = (dx * dx + dz * dz).sqrt();
                grid[z * grid_side + x] = 26.0 - radial * 30.0; // -4 m rim → 26 m peak
            }
        }
        let terrain =
            crate::terrain_import::ImportedTerrain::from_grid(grid_side, grid_side, grid, None);
        let world = VoxelWorld::from_imported(&terrain, 3);

        let mut snow_count = 0;
        let mut sand_count = 0;
        let mut grass_count = 0;
        let mut trunk_above_tree_line = 0;
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                for y in 0..WORLD_SIZE_Y as i32 {
                    match world.get(x, y, z) {
                        Voxel::Snow => snow_count += 1,
                        Voxel::Sand => sand_count += 1,
                        Voxel::Grass => grass_count += 1,
                        Voxel::Trunk => {
                            // Only trunk BASES count — a legal tree's trunk
                            // may extend above the line, its roots may not.
                            let is_base =
                                self::VoxelWorld::get(&world, x, y - 1, z) != Voxel::Trunk;
                            let base_altitude_meters = (y - 1 - WATER_LEVEL) as f32 * VOXEL_SIZE;
                            if is_base && base_altitude_meters > TREE_LINE_METERS + 0.5 {
                                trunk_above_tree_line += 1;
                            }
                        }
                        _ => {}
                    }
                }
            }
        }
        assert!(snow_count > 1000, "expected a snow cap, got {snow_count}");
        assert!(sand_count > 1000, "expected a beach ring, got {sand_count}");
        assert!(
            grass_count > 10000,
            "expected grass slopes, got {grass_count}"
        );
        assert_eq!(trunk_above_tree_line, 0, "trees must stop at the tree line");
    }

    #[test]
    fn generation_is_deterministic() {
        let world_a = VoxelWorld::generate(7);
        let world_b = VoxelWorld::generate(7);
        assert_eq!(world_a.voxels, world_b.voxels);
    }
}
