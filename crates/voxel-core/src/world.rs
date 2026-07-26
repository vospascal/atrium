//! Procedural voxel plateau generation.
//!
//! One seed → one floating-island diorama ("a nice sized scene and
//! nothing more"): an organic noise-jittered rim, rolling fBm hills on
//! top, a carved river with sandy banks, layered dirt/stone cliff sides,
//! a sculpted underside tapering toward the island's center, and
//! decoration passes (tall grass, flowers, blob-canopy trees). The fog
//! sea the island floats in is a shader effect (`fog_ring.rs`).

use std::f32::consts::TAU;

use glam::IVec3;

use crate::noise::{fractal_noise_2d, hash_3d, hash_to_unit, smoothstep};

pub const WORLD_SIZE_X: usize = 1000;
pub const WORLD_SIZE_Y: usize = 256;
pub const WORLD_SIZE_Z: usize = 1000;

/// River surface sits at the top of this voxel layer.
pub const WATER_LEVEL: i32 = 84;

/// The island's flat rim underside starts here; toward the center the
/// sculpted underside tapers well below (floating-island bottom). Lowered so
/// lake basins can carve a proper ~3 m deep pool with rock beneath (the clamp
/// min is `PLATEAU_FLOOR + 2`).
pub const PLATEAU_FLOOR: i32 = 58;

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

/// Beach width scales with biome dryness: the lush side keeps grass to
/// the water's edge (a sliver of wet sand), the dry side gets real beaches.
pub(crate) const BEACH_LUSH_METERS: f32 = 0.4;
pub(crate) const BEACH_DRY_METERS: f32 = 2.6;
pub(crate) const BEACH_MAX_ALTITUDE_METERS: f32 = 0.8;
/// Steeper than this (rise/run, 1.0 = 45°) the soil gives way to bare rock.
pub(crate) const ROCK_SLOPE_RATIO: f32 = 0.95;
/// Above this altitude everything is bare rock (alpine zone)…
pub(crate) const ALPINE_LINE_METERS: f32 = 14.0;
/// …and above this it is snow, unless too steep for snow to settle.
pub(crate) const SNOW_LINE_METERS: f32 = 17.0;
pub(crate) const SNOW_MAX_SLOPE_RATIO: f32 = 1.3;
/// Trees need gentle slopes and stop below the alpine zone.
pub(crate) const TREE_LINE_METERS: f32 = 12.0;
pub(crate) const TREE_MAX_SLOPE_RATIO: f32 = 0.65;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Voxel {
    Air,
    Grass,
    TallGrass,
    Dirt,
    Sand,
    /// Dark lake-bottom muck where the water is too deep for sand.
    Sediment,
    Stone,
    Water,
    Trunk,
    /// White bark with dark flecks (birches).
    TrunkBirch,
    Leaves,
    /// Second canopy tone: slab-built tree crowns alternate light/dark chunks.
    LeavesDark,
    /// Light yellow-green birch foliage — turns gold early in autumn.
    LeavesBirch,
    /// Dark blue-green conifer needles — barely turn with the seasons.
    LeavesPine,
    FlowerPink,
    FlowerWhite,
    FlowerYellow,
    FlowerBlue,
    /// Underwater grass tufts swaying on the river/lake bed.
    WaterWeed,
    /// Flat pad floating on the water surface.
    LilyPad,
    /// A pad carrying a white blossom.
    LilyBloom,
    /// Tall waterline grass, stacked a few voxels high.
    Reed,
    /// The brown seed head topping a cattail (Typha) stalk.
    CattailHead,
    Snow,
}

impl Voxel {
    /// Solid voxels occlude faces and cast ambient occlusion. Air, water,
    /// and thin ground cover (tufts, flowers, pads, reeds) do not — cover
    /// renders below full voxel height, so treating it as solid would bake
    /// shadows against gaps that are visibly open.
    pub fn is_solid(self) -> bool {
        !matches!(
            self,
            Voxel::Air
                | Voxel::Water
                | Voxel::TallGrass
                | Voxel::FlowerPink
                | Voxel::FlowerWhite
                | Voxel::FlowerYellow
                | Voxel::FlowerBlue
                | Voxel::WaterWeed
                | Voxel::LilyPad
                | Voxel::LilyBloom
                | Voxel::Reed
                | Voxel::CattailHead
        )
    }
}

/// One vertical run of identical voxels inside a column.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Run {
    pub voxel: Voxel,
    pub length: u16,
}

/// The finished world, stored as per-column RLE: every (x, z) column is a
/// short list of vertical runs (air above, a few material bands, stone
/// core — typically well under a dozen). ~15 MB instead of the 256 MB
/// dense grid, and a column's runs fit in a cache line or two.
///
/// Generation happens in a dense [`WorldBuilder`] (fast random writes for
/// trees and boulders) and compresses into this on completion. Readers
/// that sweep volumes never random-access the RLE: the mesher unpacks one
/// chunk at a time into a dense [`ChunkScratch`], and column sweeps use
/// [`VoxelWorld::column_runs`].
pub struct VoxelWorld {
    /// All columns' runs, concatenated in column order (`z * X + x`).
    runs: Vec<Run>,
    /// Per-column offsets into `runs`; `column_starts[c]..column_starts[c+1]`.
    column_starts: Vec<u32>,
    /// Per-column biome dryness: 0 = lush green, 1 = desert. Drives the
    /// ground-color gradient and vegetation density.
    dryness: Vec<f32>,
    /// Per-column grass-patch coverage: 1 = dense clump, 0 = bare dirt
    /// between the patches. Drives tuft clustering and ground tint.
    ground_cover: Vec<f32>,
    /// Per-column distance to the nearest water surface, meters.
    water_distance: Vec<f32>,
    /// Per-tree color identity (0..1), stamped in a disc around each tree
    /// and bush when it grows. The mesher derives hue variation and the
    /// autumn turning order from it, so every tree has its own shade.
    tree_tone: Vec<f32>,
}

/// A dense one-chunk (+1 apron) window unpacked from the RLE world, so the
/// mesher's neighbor and ambient-occlusion lookups are plain array reads.
/// Cells are y-contiguous per column (fast unpack, fast vertical scans).
pub struct ChunkScratch {
    origin_x: i32,
    origin_z: i32,
    span_x: i32,
    span_z: i32,
    cells: Vec<Voxel>,
}

impl ChunkScratch {
    /// Voxel at WORLD coordinates; air outside the window or the world.
    pub fn get(&self, x: i32, y: i32, z: i32) -> Voxel {
        let local_x = x - self.origin_x;
        let local_z = z - self.origin_z;
        if y < 0
            || y >= WORLD_SIZE_Y as i32
            || local_x < 0
            || local_z < 0
            || local_x >= self.span_x
            || local_z >= self.span_z
        {
            return Voxel::Air;
        }
        self.cells[((local_z * self.span_x + local_x) * WORLD_SIZE_Y as i32 + y) as usize]
    }

    pub fn get_offset(&self, position: IVec3) -> Voxel {
        self.get(position.x, position.y, position.z)
    }

    /// Build a window by filling one column at a time. `fill(world_x, world_z,
    /// column)` receives a `WORLD_SIZE_Y`-long slice (index = y) to write into.
    /// Lets any [`crate::voxel_source::VoxelSource`] produce a scratch window
    /// without depending on the internal cell layout.
    pub fn from_columns(
        origin_x: i32,
        origin_z: i32,
        span_x: i32,
        span_z: i32,
        mut fill: impl FnMut(i32, i32, &mut [Voxel]),
    ) -> ChunkScratch {
        let column_height = WORLD_SIZE_Y as i32;
        let mut cells = vec![Voxel::Air; (span_x * span_z * column_height) as usize];
        for local_z in 0..span_z {
            for local_x in 0..span_x {
                let base = ((local_z * span_x + local_x) * column_height) as usize;
                fill(
                    origin_x + local_x,
                    origin_z + local_z,
                    &mut cells[base..base + WORLD_SIZE_Y],
                );
            }
        }
        ChunkScratch {
            origin_x,
            origin_z,
            span_x,
            span_z,
            cells,
        }
    }

    /// This window's `(origin_x, origin_z, span_x, span_z)` in world voxels
    /// (origin includes the 1-cell apron). The terrain shader binds these as the
    /// per-chunk occupancy origin + span so `is_solid` can localize world coords.
    pub fn window(&self) -> (i32, i32, i32, i32) {
        (self.origin_x, self.origin_z, self.span_x, self.span_z)
    }

    /// Packed solid-occupancy bitset for this window, 1 bit per cell, laid out
    /// exactly as [`ChunkScratch`]'s cells — `((local_z * span_x + local_x) *
    /// WORLD_SIZE_Y + y)`. The streamed terrain shader recomputes ambient
    /// occlusion from this per-chunk buffer (see `chunk_origin` in
    /// `voxel_terrain.wgsl`), the infinite-world analogue of the island's global
    /// [`VoxelWorld::solid_occupancy_bits`]. Solid = the same [`Voxel::is_solid`]
    /// the mesher's culling uses (cover/water are not solid).
    pub fn solid_occupancy_bits(&self) -> Vec<u32> {
        let total_bits = (self.span_x * self.span_z * WORLD_SIZE_Y as i32) as usize;
        let mut bits = vec![0_u32; total_bits.div_ceil(32)];
        for (index, voxel) in self.cells.iter().enumerate() {
            if voxel.is_solid() {
                bits[index >> 5] |= 1 << (index & 31);
            }
        }
        bits
    }
}

impl VoxelWorld {
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
        let column = (z as usize) * WORLD_SIZE_X + x as usize;
        let start = self.column_starts[column] as usize;
        let end = self.column_starts[column + 1] as usize;
        let mut top = 0;
        for run in &self.runs[start..end] {
            top += run.length as i32;
            if y < top {
                return run.voxel;
            }
        }
        Voxel::Air
    }

    /// The runs of one column, bottom to top, as `(voxel, y_start, length)`.
    /// The cheap way to sweep columns (ground heights, save/load) without
    /// per-cell run walks.
    pub fn column_runs(&self, x: i32, z: i32) -> impl Iterator<Item = (Voxel, i32, i32)> + '_ {
        let (start, end) = if x < 0 || z < 0 || x >= WORLD_SIZE_X as i32 || z >= WORLD_SIZE_Z as i32
        {
            (0, 0)
        } else {
            let column = (z as usize) * WORLD_SIZE_X + x as usize;
            (
                self.column_starts[column] as usize,
                self.column_starts[column + 1] as usize,
            )
        };
        self.runs[start..end].iter().scan(0_i32, |y_cursor, run| {
            let y_start = *y_cursor;
            *y_cursor += run.length as i32;
            Some((run.voxel, y_start, run.length as i32))
        })
    }

    /// Unpack the columns `x_range`/`z_range` (the mesher passes its chunk
    /// plus a 1-cell apron) into a dense scratch window.
    pub fn unpack_chunk(&self, x_start: i32, x_end: i32, z_start: i32, z_end: i32) -> ChunkScratch {
        let origin_x = (x_start - 1).max(0);
        let origin_z = (z_start - 1).max(0);
        let span_x = (x_end + 1).min(WORLD_SIZE_X as i32) - origin_x;
        let span_z = (z_end + 1).min(WORLD_SIZE_Z as i32) - origin_z;
        let mut cells = vec![Voxel::Air; (span_x * span_z * WORLD_SIZE_Y as i32) as usize];
        for local_z in 0..span_z {
            for local_x in 0..span_x {
                let column_base = ((local_z * span_x + local_x) * WORLD_SIZE_Y as i32) as usize;
                let mut y_cursor = 0_usize;
                for (voxel, _, length) in self.column_runs(origin_x + local_x, origin_z + local_z) {
                    if voxel != Voxel::Air {
                        cells[column_base + y_cursor..column_base + y_cursor + length as usize]
                            .fill(voxel);
                    }
                    y_cursor += length as usize;
                }
            }
        }
        ChunkScratch {
            origin_x,
            origin_z,
            span_x,
            span_z,
            cells,
        }
    }

    /// (run count, resident bytes of the RLE + column index).
    /// Packed solid-occupancy bitset, 1 bit per voxel, for the terrain shader
    /// to recompute ambient occlusion per fragment. Bit index is
    /// `(z * WORLD_SIZE_X + x) * WORLD_SIZE_Y + y` — y-contiguous within a
    /// column, so each solid run sets a contiguous bit range. Solid = the same
    /// `Voxel::is_solid` the mesher's baked AO used (cover/water are not solid).
    pub fn solid_occupancy_bits(&self) -> Vec<u32> {
        let total_bits = WORLD_SIZE_X * WORLD_SIZE_Y * WORLD_SIZE_Z;
        let mut bits = vec![0_u32; total_bits.div_ceil(32)];
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                let column_base = ((z as usize) * WORLD_SIZE_X + x as usize) * WORLD_SIZE_Y;
                for (voxel, y_start, length) in self.column_runs(x, z) {
                    if !voxel.is_solid() {
                        continue;
                    }
                    for y in y_start..(y_start + length) {
                        let index = column_base + y as usize;
                        bits[index >> 5] |= 1 << (index & 31);
                    }
                }
            }
        }
        bits
    }

    pub fn memory_stats(&self) -> (usize, usize) {
        let bytes = self.runs.len() * std::mem::size_of::<Run>()
            + self.column_starts.len() * std::mem::size_of::<u32>();
        (self.runs.len(), bytes)
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

    /// Fully procedural plateau (fBm heightmap + noise rim). `season` runs
    /// 0.0 (high summer) to 1.0 (deep autumn) and thins the flower meadows;
    /// the mesher applies the matching foliage colors.
    pub fn generate(seed: u32, season: f32) -> Self {
        let mut heights = compute_heightmap(seed);
        smooth_shorelines(&mut heights);
        quantize_cliffs(&mut heights);
        WorldBuilder::build(heights, seed, season, None)
    }

    /// Plateau from a Blender-exported heightmap (meters relative to the
    /// water plane, NaN = open sky), with optional authored tree positions.
    pub fn from_imported(
        terrain: &crate::terrain_import::ImportedTerrain,
        seed: u32,
        season: f32,
    ) -> Self {
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

        WorldBuilder::build(heights, seed, season, tree_positions.as_deref())
    }

    /// Distance to the nearest water surface at a column, meters.
    pub fn water_distance_at(&self, x: i32, z: i32) -> f32 {
        if x < 0 || z < 0 || x >= WORLD_SIZE_X as i32 || z >= WORLD_SIZE_Z as i32 {
            return f32::MAX;
        }
        self.water_distance[(z as usize) * WORLD_SIZE_X + x as usize]
    }

    /// Per-tree color identity at a column (0..1, 0.5 where no tree grew).
    pub fn tree_tone_at(&self, x: i32, z: i32) -> f32 {
        if x < 0 || z < 0 || x >= WORLD_SIZE_X as i32 || z >= WORLD_SIZE_Z as i32 {
            return 0.5;
        }
        self.tree_tone[(z as usize) * WORLD_SIZE_X + x as usize]
    }
}

/// Dense scratch world used ONLY during generation: trees, boulders, and
/// decoration need fast random writes, which RLE is bad at. Compresses
/// into the RLE [`VoxelWorld`] when every pass has run.
struct WorldBuilder {
    voxels: Vec<Voxel>,
    dryness: Vec<f32>,
    ground_cover: Vec<f32>,
    /// Per-column steepness (rise over run, 1.0 = 45°), from the heightmap.
    /// Generation-only — nothing at runtime needs it.
    slope: Vec<f32>,
    water_distance: Vec<f32>,
    tree_tone: Vec<f32>,
}

impl WorldBuilder {
    fn index(x: usize, y: usize, z: usize) -> usize {
        (y * WORLD_SIZE_Z + z) * WORLD_SIZE_X + x
    }

    fn get(&self, x: i32, y: i32, z: i32) -> Voxel {
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

    fn dryness_at(&self, x: i32, z: i32) -> f32 {
        if x < 0 || z < 0 || x >= WORLD_SIZE_X as i32 || z >= WORLD_SIZE_Z as i32 {
            return 0.0;
        }
        self.dryness[(z as usize) * WORLD_SIZE_X + x as usize]
    }

    fn cover_at(&self, x: i32, z: i32) -> f32 {
        if x < 0 || z < 0 || x >= WORLD_SIZE_X as i32 || z >= WORLD_SIZE_Z as i32 {
            return 0.0;
        }
        self.ground_cover[(z as usize) * WORLD_SIZE_X + x as usize]
    }

    /// Shared tail of every construction path: fill columns from the
    /// heightmap, then decorate, plant trees, scatter bushes and boulders,
    /// and compress the result into the RLE world. (The fog ring hiding
    /// the world edge is a shader effect, not voxels — see `fog_ring.rs`.)
    fn build(
        heights: Vec<i32>,
        seed: u32,
        season: f32,
        tree_positions: Option<&[(i32, i32)]>,
    ) -> VoxelWorld {
        let mut world = Self {
            voxels: vec![Voxel::Air; WORLD_SIZE_X * WORLD_SIZE_Y * WORLD_SIZE_Z],
            dryness: compute_dryness_map(seed),
            ground_cover: compute_cover_map(seed),
            slope: compute_slope_map(&heights),
            water_distance: compute_water_distance_map(&heights),
            tree_tone: vec![0.5; WORLD_SIZE_X * WORLD_SIZE_Z],
        };

        let underside = compute_underside_map(&heights, seed);
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                let column_index = (z as usize) * WORLD_SIZE_X + x as usize;
                let column_height = heights[column_index];
                if column_height != NO_LAND {
                    world.fill_column(x, z, column_height, underside[column_index], seed);
                }
            }
        }

        world.decorate(&heights, seed, season);
        match tree_positions {
            Some(positions) => world.plant_trees_at(positions, &heights, seed),
            None => world.plant_trees(&heights, seed),
        }
        world.scatter_bushes(&heights, seed);
        world.scatter_boulders(&heights, seed);
        world.compress()
    }

    /// Compress the dense grid into per-column RLE. Two fully sequential
    /// passes over the 256 MB array (never column-strided): pass 1 counts
    /// runs per column, pass 2 fills them into exact slots — the dense
    /// grid is dropped afterwards.
    fn compress(self) -> VoxelWorld {
        let column_count = WORLD_SIZE_X * WORLD_SIZE_Z;

        let mut run_counts = vec![0_u32; column_count];
        let mut previous = vec![Voxel::Air; column_count];
        for y in 0..WORLD_SIZE_Y {
            for column in 0..column_count {
                let voxel = self.voxels[y * column_count + column];
                if y == 0 || previous[column] != voxel {
                    run_counts[column] += 1;
                    previous[column] = voxel;
                }
            }
        }

        let mut column_starts = vec![0_u32; column_count + 1];
        for column in 0..column_count {
            column_starts[column + 1] = column_starts[column] + run_counts[column];
        }

        let total_runs = column_starts[column_count] as usize;
        let mut runs = vec![
            Run {
                voxel: Voxel::Air,
                length: 0,
            };
            total_runs
        ];
        let mut cursors: Vec<u32> = column_starts[..column_count].to_vec();
        for y in 0..WORLD_SIZE_Y {
            let row_base = y * column_count;
            for (column, cursor) in cursors.iter_mut().enumerate() {
                let voxel = self.voxels[row_base + column];
                if y == 0 || runs[*cursor as usize - 1].voxel != voxel {
                    runs[*cursor as usize] = Run { voxel, length: 1 };
                    *cursor += 1;
                } else {
                    runs[*cursor as usize - 1].length += 1;
                }
            }
        }

        VoxelWorld {
            runs,
            column_starts,
            dryness: self.dryness,
            ground_cover: self.ground_cover,
            water_distance: self.water_distance,
            tree_tone: self.tree_tone,
        }
    }

    /// One terrain column: sculpted underside bottom, stone core, subsoil,
    /// biome-classified cap, water fill up to the water line.
    fn fill_column(&mut self, x: i32, z: i32, column_height: i32, underside_y: i32, seed: u32) {
        let altitude_meters = (column_height - WATER_LEVEL) as f32 * VOXEL_SIZE;
        let slope = self.slope_at(x, z);
        let water_distance = self.water_distance_at(x, z);

        let cap = if column_height < WATER_LEVEL {
            // Underwater bed: sandy shallows fading into dark sediment,
            // with a noise-wavy boundary so the transition meanders.
            let depth_meters = (WATER_LEVEL - column_height) as f32 * VOXEL_SIZE;
            let sand_limit = 0.20
                + 0.40
                    * fractal_noise_2d(
                        x as f32 * 0.03 + 7100.0,
                        z as f32 * 0.03,
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
            // Slope-rock needs some altitude: lake and river banks are
            // just steep enough to trip the rule, but they're soil, not
            // cliffs — gray rings around every pond look wrong.
            Voxel::Stone
        } else if water_distance
            <= BEACH_LUSH_METERS + (BEACH_DRY_METERS - BEACH_LUSH_METERS) * self.dryness_at(x, z)
            && altitude_meters <= BEACH_MAX_ALTITUDE_METERS
        {
            Voxel::Sand
        } else {
            Voxel::Grass
        };
        let subsoil = match cap {
            Voxel::Sand => Voxel::Sand,
            Voxel::Sediment => Voxel::Sediment,
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
    fn slope_at(&self, x: i32, z: i32) -> f32 {
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

    /// Stamp a tree's color identity in a disc around its base, so the
    /// mesher can recover "which tree does this leaf belong to" from x/z.
    fn stamp_tree_tone(&mut self, x: i32, z: i32, radius: i32, tone: f32) {
        for offset_z in -radius..=radius {
            for offset_x in -radius..=radius {
                if offset_x * offset_x + offset_z * offset_z > radius * radius {
                    continue;
                }
                let column_x = x + offset_x;
                let column_z = z + offset_z;
                if column_x < 0
                    || column_z < 0
                    || column_x >= WORLD_SIZE_X as i32
                    || column_z >= WORLD_SIZE_Z as i32
                {
                    continue;
                }
                self.tree_tone[(column_z as usize) * WORLD_SIZE_X + column_x as usize] = tone;
            }
        }
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
                || !self.plantable_cap(x, column_height, z)
                || !self.tree_can_grow(x, z, column_height)
            {
                continue;
            }
            self.grow_tree(x, column_height, z, seed);
        }
    }

    /// Trees root in grass anywhere, and in shoreline sand (willow country).
    fn plantable_cap(&self, x: i32, column_height: i32, z: i32) -> bool {
        match self.get(x, column_height, z) {
            Voxel::Grass => true,
            Voxel::Sand => self.water_distance_at(x, z) <= 4.0,
            _ => false,
        }
    }

    /// Trees follow the same derived fields as the ground cap: below the
    /// tree line, on gentle slopes only.
    fn tree_can_grow(&self, x: i32, z: i32, column_height: i32) -> bool {
        let altitude_meters = (column_height - WATER_LEVEL) as f32 * VOXEL_SIZE;
        altitude_meters < TREE_LINE_METERS && self.slope_at(x, z) <= TREE_MAX_SLOPE_RATIO
    }

    /// Ground cover, above and below the waterline: tall grass and flower
    /// meadows on land, reed belts at the shore, waterweed and lily pads
    /// in the shallows.
    fn decorate(&mut self, heights: &[i32], seed: u32, season: f32) {
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                let column_height = heights[(z as usize) * WORLD_SIZE_X + x as usize];
                if column_height == NO_LAND {
                    continue;
                }

                // Underwater: reeds wade out into the shallowest water, weed
                // tufts cover the bed, lily pads cluster on the surface.
                if column_height < WATER_LEVEL {
                    let depth = WATER_LEVEL - column_height;
                    let roll = hash_to_unit(hash_3d(x, 902, z, seed.wrapping_add(53)));
                    if depth <= 2 {
                        // Same patch field as the shore reeds, so the belts
                        // run continuously across the waterline. A thin
                        // fringe grows everywhere along the edge; the patch
                        // noise thickens it into full beds.
                        let reed_patch = fractal_noise_2d(
                            x as f32 * 0.06 + 9700.0,
                            z as f32 * 0.06,
                            3,
                            seed.wrapping_add(77),
                        );
                        if roll < 0.12 + 0.30 * smoothstep(0.48, 0.70, reed_patch) {
                            let stalk_hash = hash_3d(x, 904, z, seed.wrapping_add(55));
                            if stalk_hash.is_multiple_of(3) {
                                // Cattail: the stalk clears the surface and
                                // carries a brown seed head.
                                for y in (column_height + 1)..=(WATER_LEVEL + 2) {
                                    self.set(x, y, z, Voxel::Reed);
                                }
                                self.set(x, WATER_LEVEL + 3, z, Voxel::CattailHead);
                            } else {
                                for y in (column_height + 1)..=(WATER_LEVEL + 1) {
                                    self.set(x, y, z, Voxel::Reed);
                                }
                            }
                            continue;
                        }
                    }
                    let weed_patch = fractal_noise_2d(
                        x as f32 * 0.05 + 8300.0,
                        z as f32 * 0.05,
                        3,
                        seed.wrapping_add(71),
                    );
                    let weed_chance = if depth >= 2 { 0.62 } else { 0.30 };
                    if roll < weed_chance * smoothstep(0.42, 0.72, weed_patch) {
                        self.set(x, column_height + 1, z, Voxel::WaterWeed);
                    } else if (1..=6).contains(&depth) {
                        let pad_patch = fractal_noise_2d(
                            x as f32 * 0.045 + 9100.0,
                            z as f32 * 0.045,
                            3,
                            seed.wrapping_add(73),
                        );
                        // Tight patches, dense inside: pads raft together in
                        // clusters instead of sprinkling across the water.
                        if pad_patch > 0.66
                            && roll > 0.55
                            && self.get(x, WATER_LEVEL + 1, z) == Voxel::Air
                        {
                            let pad = if hash_3d(x, 908, z, seed.wrapping_add(65)).is_multiple_of(6)
                            {
                                Voxel::LilyBloom
                            } else {
                                Voxel::LilyPad
                            };
                            self.set(x, WATER_LEVEL + 1, z, pad);
                        }
                    }
                    continue;
                }

                let cap = self.get(x, column_height, z);
                let altitude_meters = (column_height - WATER_LEVEL) as f32 * VOXEL_SIZE;
                let water_distance = self.water_distance_at(x, z);

                // Reed belt right at the waterline, on sand or grass.
                if matches!(cap, Voxel::Sand | Voxel::Grass)
                    && altitude_meters <= 0.6
                    && water_distance <= 1.1
                {
                    let reed_patch = fractal_noise_2d(
                        x as f32 * 0.06 + 9700.0,
                        z as f32 * 0.06,
                        3,
                        seed.wrapping_add(77),
                    );
                    let roll = hash_to_unit(hash_3d(x, 903, z, seed.wrapping_add(54)));
                    if roll < 0.18 + 0.35 * smoothstep(0.48, 0.70, reed_patch) {
                        let stalk_hash = hash_3d(x, 904, z, seed.wrapping_add(55));
                        let is_cattail = stalk_hash.is_multiple_of(3);
                        let reed_height = if is_cattail {
                            3 + ((stalk_hash >> 4) % 2) as i32
                        } else {
                            2 + (stalk_hash % 3) as i32
                        };
                        for step in 1..=reed_height {
                            if self.get(x, column_height + step, z) == Voxel::Air {
                                self.set(x, column_height + step, z, Voxel::Reed);
                            }
                        }
                        if is_cattail
                            && self.get(x, column_height + reed_height + 1, z) == Voxel::Air
                        {
                            self.set(x, column_height + reed_height + 1, z, Voxel::CattailHead);
                        }
                        continue;
                    }
                }

                if cap != Voxel::Grass || column_height <= WATER_LEVEL {
                    continue;
                }
                // Grass grows in clumpy patches with bare dirt between them
                // (reference look), thinning toward the desert side.
                let lushness = 1.0 - self.dryness_at(x, z);
                let clump = smoothstep(0.40, 0.70, self.cover_at(x, z)) * lushness;
                let roll = hash_to_unit(hash_3d(x, 900, z, seed.wrapping_add(51)));

                // Flower meadows: noise blobs where flowers bloom in dense
                // two-tone drifts. Each ~9 m patch keeps one palette so the
                // drifts read as intentional plantings, not confetti; autumn
                // thins them out.
                let meadow = fractal_noise_2d(
                    x as f32 * 0.02 + 4700.0,
                    z as f32 * 0.02,
                    3,
                    seed.wrapping_add(61),
                );
                let meadow_amount =
                    smoothstep(0.60, 0.72, meadow) * lushness * (1.0 - season * 0.75);
                if meadow_amount > 0.0 && roll < 0.14 * meadow_amount {
                    let palette = hash_3d(
                        x.div_euclid(72),
                        905,
                        z.div_euclid(72),
                        seed.wrapping_add(62),
                    ) % 4;
                    let pick = hash_3d(x, 906, z, seed.wrapping_add(63));
                    let flower = match palette {
                        0 => {
                            if pick.is_multiple_of(3) {
                                Voxel::FlowerYellow
                            } else {
                                Voxel::FlowerWhite
                            }
                        }
                        1 => {
                            if pick.is_multiple_of(3) {
                                Voxel::FlowerWhite
                            } else {
                                Voxel::FlowerPink
                            }
                        }
                        2 => {
                            if pick.is_multiple_of(3) {
                                Voxel::FlowerWhite
                            } else {
                                Voxel::FlowerBlue
                            }
                        }
                        _ => {
                            if pick.is_multiple_of(2) {
                                Voxel::FlowerYellow
                            } else {
                                Voxel::FlowerBlue
                            }
                        }
                    };
                    // Half the meadow flowers stand on a grass stalk, so
                    // blossoms bob above the carpet instead of hiding in it.
                    if pick % 5 < 2 {
                        self.set(x, column_height + 1, z, Voxel::TallGrass);
                        self.set(x, column_height + 2, z, flower);
                    } else {
                        self.set(x, column_height + 1, z, flower);
                    }
                } else if roll < 0.70 * clump {
                    // Lush clumps grow knee-high: stalks stack 2-3 voxels
                    // where the meadow is densest, single tufts elsewhere.
                    let stalk_roll = hash_to_unit(hash_3d(x, 907, z, seed.wrapping_add(64)));
                    let stalk_height = if stalk_roll < clump * 0.5 {
                        3
                    } else if stalk_roll < clump * 1.4 {
                        2
                    } else {
                        1
                    };
                    for step in 1..=stalk_height {
                        if self.get(x, column_height + step, z) == Voxel::Air {
                            self.set(x, column_height + step, z, Voxel::TallGrass);
                        }
                    }
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
                if column_height <= WATER_LEVEL + 1 {
                    continue;
                }
                if !self.plantable_cap(x, column_height, z)
                    || !self.tree_can_grow(x, z, column_height)
                {
                    continue;
                }
                // Dense stands on the lush side, lone trees near the desert,
                // and a bonus along the waterline so shores get willows.
                let shore_bonus = 1.0 + 1.5 * smoothstep(6.0, 2.0, self.water_distance_at(x, z));
                let tree_probability =
                    0.0035 * (0.15 + 0.85 * (1.0 - self.dryness_at(x, z))) * shore_bonus;
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

    /// Pick a species from the terrain and a per-tree hash, stamp the
    /// tree's color identity, and grow it. Willows crowd the waterline,
    /// pines take the heights and the dry side, birches mix into the rest.
    fn grow_tree(&mut self, x: i32, ground_height: i32, z: i32, seed: u32) {
        let tree_hash = hash_3d(x, 800, z, seed.wrapping_add(41));
        let tone = hash_to_unit(tree_hash.wrapping_mul(0x85EB_CA6B).wrapping_add(0x9E37));
        let species_roll = hash_to_unit(tree_hash.wrapping_mul(0x27D4_EB2F));
        let altitude_meters = (ground_height - WATER_LEVEL) as f32 * VOXEL_SIZE;
        let water_distance = self.water_distance_at(x, z);
        let dryness = self.dryness_at(x, z);

        if water_distance <= 4.0 && species_roll < 0.70 {
            self.stamp_tree_tone(x, z, 18, tone);
            self.grow_willow(x, ground_height, z, tree_hash);
        } else if altitude_meters > 6.5
            || (dryness > 0.55 && species_roll < 0.45)
            || species_roll > 0.90
        {
            self.stamp_tree_tone(x, z, 13, tone);
            self.grow_pine(x, ground_height, z, tree_hash);
        } else if species_roll < 0.30 {
            self.stamp_tree_tone(x, z, 10, tone);
            self.grow_birch(x, ground_height, z, tree_hash);
        } else {
            self.stamp_tree_tone(x, z, 22, tone);
            self.grow_oak(x, ground_height, z, tree_hash);
        }
    }

    /// Chunky reference-style tree: thick 3×3 trunk and a crown built
    /// from overlapping rectangular slabs in two leaf tones, instead of
    /// a smooth ellipsoid blob. Tall — real trees tower over the 1.7 m
    /// first-person eye, they don't sit at shoulder height.
    fn grow_oak(&mut self, x: i32, ground_height: i32, z: i32, tree_hash: u32) {
        let trunk_height = 34 + (tree_hash % 18) as i32;

        for y in 1..=trunk_height {
            for offset_x in -1..=1 {
                for offset_z in -1..=1 {
                    self.set(x + offset_x, ground_height + y, z + offset_z, Voxel::Trunk);
                }
            }
        }

        let crown_center_y = ground_height + trunk_height + 5;
        // Many small overlapping slabs beat a few huge ones: the crown
        // silhouette turns puffy and irregular instead of flat-topped.
        let slab_count = 12 + (tree_hash >> 8) % 5;
        for slab_index in 0..slab_count as i32 {
            let slab_hash = hash_3d(
                x + slab_index * 37,
                810 + slab_index,
                z + slab_index * 53,
                tree_hash.wrapping_add(43),
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
                    x + ((unit_a - 0.5) * 20.0) as i32,
                    crown_center_y + ((unit_b - 0.5) * 13.0) as i32,
                    z + ((unit_c - 0.5) * 20.0) as i32,
                )
            };
            let half_extent_x = 3 + (unit_d * 5.0) as i32;
            let half_extent_y = 2 + (unit_a * 2.0) as i32;
            let half_extent_z = 3 + (unit_b * 5.0) as i32;
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

    /// Slender white-barked tree: thin 2×2 trunk, a narrow stack of small
    /// leaf blobs high up. Reads as a lighter accent between the oaks.
    fn grow_birch(&mut self, x: i32, ground_height: i32, z: i32, tree_hash: u32) {
        let trunk_height = 42 + (tree_hash % 16) as i32;
        for y in 1..=trunk_height {
            for offset_x in 0..=1 {
                for offset_z in 0..=1 {
                    self.set(
                        x + offset_x,
                        ground_height + y,
                        z + offset_z,
                        Voxel::TrunkBirch,
                    );
                }
            }
        }

        let crown_base = ground_height + trunk_height - 12;
        let blob_count = 5 + (tree_hash >> 7) % 3;
        for blob_index in 0..blob_count as i32 {
            let blob_hash = hash_3d(
                x + blob_index * 41,
                830 + blob_index,
                z + blob_index * 59,
                tree_hash.wrapping_add(47),
            );
            let unit_a = hash_to_unit(blob_hash);
            let unit_b = hash_to_unit(blob_hash.wrapping_mul(0x9E37_79B9));
            let unit_c = hash_to_unit(blob_hash.wrapping_mul(0x85EB_CA6B));

            let center_x = x + ((unit_a - 0.5) * 9.0) as i32;
            let center_y = crown_base + blob_index * 4 + ((unit_b - 0.5) * 4.0) as i32;
            let center_z = z + ((unit_c - 0.5) * 9.0) as i32;
            let half_extent = 3 + (unit_a * 3.0) as i32;
            let half_extent_y = 2 + (unit_b * 2.0) as i32;

            for offset_y in -half_extent_y..=half_extent_y {
                for offset_z in -half_extent..=half_extent {
                    for offset_x in -half_extent..=half_extent {
                        let cell_x = center_x + offset_x;
                        let cell_y = center_y + offset_y;
                        let cell_z = center_z + offset_z;
                        if self.get(cell_x, cell_y, cell_z) == Voxel::Air {
                            self.set(cell_x, cell_y, cell_z, Voxel::LeavesBirch);
                        }
                    }
                }
            }
        }
    }

    /// Conifer: stacked shrinking discs with one-voxel gaps between them —
    /// the pagoda silhouette MagicaVoxel pines are known for.
    fn grow_pine(&mut self, x: i32, ground_height: i32, z: i32, tree_hash: u32) {
        let total_height = 46 + (tree_hash % 22) as i32;
        for y in 1..=total_height {
            for offset_x in 0..=1 {
                for offset_z in 0..=1 {
                    self.set(x + offset_x, ground_height + y, z + offset_z, Voxel::Trunk);
                }
            }
        }

        let canopy_bottom = 8 + ((tree_hash >> 5) % 6) as i32;
        let base_extent = 8.0 + hash_to_unit(tree_hash.wrapping_mul(0xC2B2_AE35)) * 4.0;
        let layer_count = (total_height - canopy_bottom) / 4;
        for layer_index in 0..=layer_count {
            let progress = layer_index as f32 / layer_count.max(1) as f32;
            let extent = ((1.0 - progress) * base_extent) as i32 + 1;
            let layer_y = ground_height + canopy_bottom + layer_index * 4;
            for offset_y in 0..3 {
                for offset_z in -extent..=extent {
                    for offset_x in -extent..=extent {
                        if offset_x * offset_x + offset_z * offset_z > extent * extent {
                            continue;
                        }
                        let cell_x = x + offset_x;
                        let cell_y = layer_y + offset_y;
                        let cell_z = z + offset_z;
                        if self.get(cell_x, cell_y, cell_z) == Voxel::Air {
                            self.set(cell_x, cell_y, cell_z, Voxel::LeavesPine);
                        }
                    }
                }
            }
        }
    }

    /// Waterline tree: short thick trunk, a wide dome, and leaf strands
    /// hanging from the dome's rim — they drape until they meet ground or
    /// water, like a willow trailing in a pond.
    fn grow_willow(&mut self, x: i32, ground_height: i32, z: i32, tree_hash: u32) {
        let trunk_height = 18 + (tree_hash % 8) as i32;
        for y in 1..=trunk_height {
            for offset_x in -1..=1 {
                for offset_z in -1..=1 {
                    self.set(x + offset_x, ground_height + y, z + offset_z, Voxel::Trunk);
                }
            }
        }

        let dome_center_y = ground_height + trunk_height + 3;
        let dome_radius = 10 + (hash_to_unit(tree_hash.wrapping_mul(0x9E37_79B9)) * 4.0) as i32;
        let dome_height = 4 + ((tree_hash >> 9) % 3) as i32;
        for offset_y in -1..=dome_height {
            for offset_z in -dome_radius..=dome_radius {
                for offset_x in -dome_radius..=dome_radius {
                    let planar = (offset_x * offset_x + offset_z * offset_z) as f32
                        / (dome_radius * dome_radius) as f32;
                    let vertical = if offset_y < 0 {
                        0.0
                    } else {
                        (offset_y * offset_y) as f32 / (dome_height * dome_height) as f32
                    };
                    if planar + vertical > 1.05 {
                        continue;
                    }
                    let cell_x = x + offset_x;
                    let cell_y = dome_center_y + offset_y;
                    let cell_z = z + offset_z;
                    if self.get(cell_x, cell_y, cell_z) == Voxel::Air {
                        let mottle = hash_3d(cell_x, cell_y, cell_z, tree_hash);
                        self.set(
                            cell_x,
                            cell_y,
                            cell_z,
                            if mottle.is_multiple_of(3) {
                                Voxel::LeavesDark
                            } else {
                                Voxel::Leaves
                            },
                        );
                    }
                }
            }
        }

        // Hanging strands around the dome rim.
        let strand_count = 26;
        for strand_index in 0..strand_count {
            let strand_hash = hash_3d(x + strand_index, 850, z - strand_index, tree_hash);
            if hash_to_unit(strand_hash) > 0.80 {
                continue;
            }
            let angle = TAU * strand_index as f32 / strand_count as f32;
            let strand_x = x + (angle.cos() * (dome_radius as f32 - 0.5)).round() as i32;
            let strand_z = z + (angle.sin() * (dome_radius as f32 - 0.5)).round() as i32;
            let length = 8 + (strand_hash % 14) as i32;
            let strand_leaf = if strand_hash.is_multiple_of(3) {
                Voxel::LeavesDark
            } else {
                Voxel::Leaves
            };
            for drop in 0..length {
                let cell_y = dome_center_y - 1 - drop;
                if self.get(strand_x, cell_y, strand_z) != Voxel::Air {
                    break;
                }
                self.set(strand_x, cell_y, strand_z, strand_leaf);
            }
        }
    }

    /// Low leaf blobs scattered on grassland — undergrowth between trees,
    /// denser near the water.
    fn scatter_bushes(&mut self, heights: &[i32], seed: u32) {
        for z in 8..(WORLD_SIZE_Z as i32 - 8) {
            for x in 8..(WORLD_SIZE_X as i32 - 8) {
                let column_height = heights[(z as usize) * WORLD_SIZE_X + x as usize];
                if column_height <= WATER_LEVEL + 1 {
                    continue;
                }
                if self.get(x, column_height, z) != Voxel::Grass || self.slope_at(x, z) > 0.85 {
                    continue;
                }
                let lushness = 1.0 - self.dryness_at(x, z);
                let shore_bonus = 1.0 + smoothstep(8.0, 2.0, self.water_distance_at(x, z));
                let probability = 0.0009 * (0.2 + 0.8 * lushness) * shore_bonus;
                if hash_to_unit(hash_3d(x, 710, z, seed.wrapping_add(33))) >= probability {
                    continue;
                }
                self.grow_bush(x, column_height, z, seed);
            }
        }
    }

    fn grow_bush(&mut self, x: i32, ground_height: i32, z: i32, seed: u32) {
        let bush_hash = hash_3d(x, 860, z, seed.wrapping_add(35));
        let tone = hash_to_unit(bush_hash.wrapping_mul(0x85EB_CA6B));
        let half_extent_x = 2 + (bush_hash % 3) as i32;
        let half_extent_z = 2 + ((bush_hash >> 3) % 3) as i32;
        let half_extent_y = 1 + ((bush_hash >> 6) % 2) as i32;
        self.stamp_tree_tone(x, z, half_extent_x.max(half_extent_z) + 1, tone);

        let center_y = ground_height + half_extent_y;
        for offset_y in -half_extent_y..=half_extent_y {
            for offset_z in -half_extent_z..=half_extent_z {
                for offset_x in -half_extent_x..=half_extent_x {
                    let roundness = (offset_x * offset_x) as f32
                        / (half_extent_x * half_extent_x) as f32
                        + (offset_y * offset_y) as f32 / (half_extent_y * half_extent_y) as f32
                        + (offset_z * offset_z) as f32 / (half_extent_z * half_extent_z) as f32;
                    let bumpy = hash_to_unit(hash_3d(
                        x + offset_x,
                        ground_height + offset_y,
                        z + offset_z,
                        bush_hash,
                    ));
                    if roundness > 0.85 + bumpy * 0.45 {
                        continue;
                    }
                    let cell_x = x + offset_x;
                    let cell_y = center_y + offset_y;
                    let cell_z = z + offset_z;
                    if self.get(cell_x, cell_y, cell_z) == Voxel::Air {
                        self.set(
                            cell_x,
                            cell_y,
                            cell_z,
                            if bumpy > 0.6 {
                                Voxel::LeavesDark
                            } else {
                                Voxel::Leaves
                            },
                        );
                    }
                }
            }
        }
    }

    /// Half-buried boulder clusters: lone field stones on the meadow,
    /// rock gardens where the rocky-patch noise runs high, and shore
    /// stones poking out of the shallows.
    fn scatter_boulders(&mut self, heights: &[i32], seed: u32) {
        for z in 8..(WORLD_SIZE_Z as i32 - 8) {
            for x in 8..(WORLD_SIZE_X as i32 - 8) {
                let column_height = heights[(z as usize) * WORLD_SIZE_X + x as usize];
                if column_height == NO_LAND || column_height < WATER_LEVEL - 4 {
                    continue;
                }
                let cap = self.get(x, column_height, z);
                if !matches!(cap, Voxel::Grass | Voxel::Sand) {
                    continue;
                }
                let rocky_patch = fractal_noise_2d(
                    x as f32 * 0.012 + 6100.0,
                    z as f32 * 0.012,
                    3,
                    seed.wrapping_add(37),
                );
                let patch_boost = 1.0 + 5.0 * smoothstep(0.58, 0.72, rocky_patch);
                let probability = 0.00022 * patch_boost;
                if hash_to_unit(hash_3d(x, 720, z, seed.wrapping_add(39))) >= probability {
                    continue;
                }
                self.grow_boulder(x, column_height, z, seed);
            }
        }
    }

    fn grow_boulder(&mut self, x: i32, ground_height: i32, z: i32, seed: u32) {
        let boulder_hash = hash_3d(x, 870, z, seed.wrapping_add(45));
        let lobe_count = 1 + (boulder_hash % 3) as i32;
        for lobe_index in 0..lobe_count {
            let lobe_hash = hash_3d(x + lobe_index * 13, 871, z - lobe_index * 7, boulder_hash);
            let radius = 2 + (lobe_hash % 3) as i32;
            let lobe_x = x + ((hash_to_unit(lobe_hash) - 0.5) * 5.0) as i32;
            let lobe_z =
                z + ((hash_to_unit(lobe_hash.wrapping_mul(0x9E37_79B9)) - 0.5) * 5.0) as i32;
            // Sunk about halfway, so it reads as bedded in the ground.
            let lobe_y = ground_height + radius / 2;
            for offset_y in -radius..=radius {
                for offset_z in -radius..=radius {
                    for offset_x in -radius..=radius {
                        if offset_x * offset_x + offset_y * offset_y + offset_z * offset_z
                            > radius * radius
                        {
                            continue;
                        }
                        self.set(
                            lobe_x + offset_x,
                            lobe_y + offset_y,
                            lobe_z + offset_z,
                            Voxel::Stone,
                        );
                    }
                }
            }
            // A shore boulder may have displaced the water under a lily
            // pad — clear any pad it stranded.
            for offset_z in -radius..=radius {
                for offset_x in -radius..=radius {
                    let column_x = lobe_x + offset_x;
                    let column_z = lobe_z + offset_z;
                    if matches!(
                        self.get(column_x, WATER_LEVEL + 1, column_z),
                        Voxel::LilyPad | Voxel::LilyBloom
                    ) && self.get(column_x, WATER_LEVEL, column_z) != Voxel::Water
                    {
                        self.set(column_x, WATER_LEVEL + 1, column_z, Voxel::Air);
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

/// Two-pass chamfer distance transform: distance in meters from every
/// column to the nearest seed column (exact enough for shore bands at a
/// fraction of a BFS's cost).
fn chamfer_distance_map(is_seed: impl Fn(usize) -> bool) -> Vec<f32> {
    const DIAGONAL: f32 = 1.414;
    let far = (WORLD_SIZE_X + WORLD_SIZE_Z) as f32;
    let index_of = |x: usize, z: usize| z * WORLD_SIZE_X + x;
    let mut distances: Vec<f32> = (0..WORLD_SIZE_X * WORLD_SIZE_Z)
        .map(|index| if is_seed(index) { 0.0 } else { far })
        .collect();

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

/// Distance to the nearest water column, meters.
fn compute_water_distance_map(heights: &[i32]) -> Vec<f32> {
    chamfer_distance_map(|index| heights[index] != NO_LAND && heights[index] < WATER_LEVEL)
}

/// Quantize the heightmap into chunky vertical steps where the terrain is
/// steep, so cliffs and rock faces read as big MagicaVoxel-style blocks while
/// gentle rolling ground stays smooth (procedural terrain only — imported
/// heightmaps keep their authored shape). Runs after shoreline smoothing.
///
/// The slope is measured on the *pre-quantized* heightmap, and the snap is
/// blended in by slope (`smoothstep`), so only genuinely steep columns terrace
/// and there is no hard seam at the threshold. Sky rim and underwater beds are
/// left untouched.
fn quantize_cliffs(heights: &mut [i32]) {
    /// Voxels per chunky terrace (~0.375 m at VOXEL_SIZE 0.125).
    const CLIFF_STEP: f32 = 3.0;
    let slope = compute_slope_map(heights);
    let original: Vec<i32> = heights.to_vec();
    for index in 0..heights.len() {
        let height = original[index];
        if height == NO_LAND || height < WATER_LEVEL {
            continue;
        }
        let cliff_strength = smoothstep(0.7, 1.3, slope[index]);
        if cliff_strength <= 0.0 {
            continue;
        }
        let snapped = (height as f32 / CLIFF_STEP).round() * CLIFF_STEP;
        let blended = height as f32 + (snapped - height as f32) * cliff_strength;
        heights[index] = blended.round() as i32;
    }
}

/// Ease every shoreline (procedural terrain only — imported heightmaps
/// keep their authored cliffs). Above the water line, land within the
/// ramp may only rise gently, so banks meet the water as floodplain
/// aprons instead of trench walls. Below it, the bed near the shore is
/// held up to a shallow sunlit shelf — the grass visibly slides under
/// the water before the bottom drops away, like a natural beach entry.
fn smooth_shorelines(heights: &mut [i32]) {
    /// Land flattens toward the water inside this ramp…
    const LAND_RAMP_METERS: f32 = 7.0;
    /// …down to roughly one voxel right at the waterline…
    const SHORE_STEP_METERS: f32 = 0.10;
    /// …and may rise back to this by the ramp's outer edge.
    const LAND_RAMP_TOP_METERS: f32 = 3.5;
    /// The underwater shelf reaches this far out from the shore…
    const SHELF_RAMP_METERS: f32 = 4.0;
    /// …starting at ankle depth right against the bank.
    const SHELF_EDGE_DEPTH_METERS: f32 = 0.15;

    let water_distance = compute_water_distance_map(heights);
    let land_distance =
        chamfer_distance_map(|index| heights[index] != NO_LAND && heights[index] >= WATER_LEVEL);

    for index in 0..heights.len() {
        let height = heights[index];
        if height == NO_LAND {
            continue;
        }
        if height >= WATER_LEVEL {
            let ramp = (water_distance[index] / LAND_RAMP_METERS).min(1.0);
            let allowed_altitude =
                SHORE_STEP_METERS + (LAND_RAMP_TOP_METERS - SHORE_STEP_METERS) * ramp * ramp;
            let allowed_height = WATER_LEVEL + (allowed_altitude / VOXEL_SIZE).round() as i32;
            heights[index] = height.min(allowed_height);
        } else {
            let ramp = (land_distance[index] / SHELF_RAMP_METERS).min(1.0);
            let allowed_depth = SHELF_EDGE_DEPTH_METERS + ramp * ramp * 1.6;
            let shallowest_bed = WATER_LEVEL - (allowed_depth / VOXEL_SIZE).round() as i32;
            heights[index] = height.max(shallowest_bed);
        }
    }
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
/// world border), meters.
fn compute_edge_distance_map(heights: &[i32]) -> Vec<f32> {
    chamfer_distance_map(|index| {
        let x = index % WORLD_SIZE_X;
        let z = index / WORLD_SIZE_X;
        heights[index] == NO_LAND
            || x == 0
            || z == 0
            || x == WORLD_SIZE_X - 1
            || z == WORLD_SIZE_Z - 1
    })
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

/// Infinite terrain height for a single column, in voxels, from position-based
/// noise ONLY — no island falloff, so it tiles forever. This is the foundation
/// for chunk streaming (Stage 9): a column at world `(x, z)` generates the same
/// height no matter which chunk asks, so chunk seams are seamless. The finite
/// island ([`compute_heightmap`]) masks this radially on top.
pub fn terrain_column_height(world_x: i32, world_z: i32, seed: u32) -> i32 {
    let world_x = world_x as f32;
    let world_z = world_z as f32;

    // Gentle rolling hills, barely above the water line so shores flood shallow.
    let rolling = fractal_noise_2d(world_x * 0.007, world_z * 0.007, 5, seed);
    let detail = fractal_noise_2d(world_x * 0.03, world_z * 0.03, 4, seed.wrapping_add(7));
    let hill_shape = rolling * 0.85 + detail * 0.15;
    let mut height = (WATER_LEVEL + 4) as f32 + hill_shape * 12.0;

    // River: carve where the channel noise crosses its midline (wide banks).
    let river_noise = fractal_noise_2d(
        world_x * 0.006 + 400.0,
        world_z * 0.006,
        4,
        seed.wrapping_add(13),
    );
    let channel_distance = (river_noise - 0.5).abs();
    let channel_width = 0.105;
    if channel_distance < channel_width {
        let carve = smoothstep(channel_width, 0.02, channel_distance);
        let river_bed = (WATER_LEVEL - 4) as f32 - carve * 2.5;
        height += (river_bed - height) * carve;
    }

    // Lake basins: broad noise blobs sink below the water line.
    let lake_noise = fractal_noise_2d(
        world_x * 0.004 + 2300.0,
        world_z * 0.004,
        3,
        seed.wrapping_add(19),
    );
    let lake_amount = smoothstep(0.66, 0.80, lake_noise);
    if lake_amount > 0.0 {
        let lake_bed = (WATER_LEVEL - 3) as f32 - lake_amount * 20.0;
        height += (lake_bed - height) * lake_amount;
    }

    (height.round() as i32).clamp(PLATEAU_FLOOR + 2, WORLD_SIZE_Y as i32 - 8)
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

            // The island is the infinite terrain masked to a radius; the hills /
            // river / lakes themselves are position-based and shared with the
            // streaming path.
            heights[z * WORLD_SIZE_X + x] = terrain_column_height(x as i32, z as i32, seed);
        }
    }
    heights
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terrain_height_deterministic_and_seamless() {
        // A column's height depends only on (x, z, seed) — never on which chunk
        // asks — so streamed chunks meet seamlessly at their borders.
        for &(x, z) in &[(0, 0), (63, 64), (1000, -1000), (-5, 12), (i32::MIN / 4, 7)] {
            let a = terrain_column_height(x, z, 42);
            let b = terrain_column_height(x, z, 42);
            assert_eq!(a, b, "non-deterministic height at ({x},{z})");
        }
        // Adjacent columns across a chunk boundary are independent + in range.
        for x in 60..70 {
            let h = terrain_column_height(x, 0, 42);
            assert!(
                (PLATEAU_FLOOR + 2..=WORLD_SIZE_Y as i32 - 8).contains(&h),
                "height {h} out of range at x={x}"
            );
        }
    }

    #[test]
    fn terrain_height_infinite_generates_land_everywhere() {
        // Unlike the island, the raw terrain has no radial cutoff — it produces
        // valid land arbitrarily far from the origin (the streaming base).
        let far = terrain_column_height(500_000, -500_000, 7);
        assert!(
            far > PLATEAU_FLOOR,
            "expected land far from origin, got {far}"
        );
    }

    #[test]
    fn beyond_the_rim_is_open_sky() {
        let world = VoxelWorld::generate(1, 0.0);
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
        let world = VoxelWorld::generate(1, 0.0);
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
    fn water_bodies_have_life() {
        let world = VoxelWorld::generate(1, 0.0);
        let mut weed_count = 0;
        let mut pad_count = 0;
        let mut reed_count = 0;
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                for y in 0..WORLD_SIZE_Y as i32 {
                    match world.get(x, y, z) {
                        Voxel::WaterWeed => {
                            weed_count += 1;
                            assert!(
                                world.get(x, y - 1, z).is_solid(),
                                "waterweed floating at ({x},{y},{z})"
                            );
                        }
                        Voxel::LilyPad | Voxel::LilyBloom => {
                            pad_count += 1;
                            assert_eq!(
                                world.get(x, y - 1, z),
                                Voxel::Water,
                                "lily pad not on the water surface at ({x},{y},{z})"
                            );
                            assert_eq!(y, WATER_LEVEL + 1, "lily pad off the surface layer");
                        }
                        Voxel::Reed => reed_count += 1,
                        _ => {}
                    }
                }
            }
        }
        assert!(weed_count > 100, "expected waterweed, got {weed_count}");
        assert!(pad_count > 5, "expected lily pads, got {pad_count}");
        assert!(reed_count > 50, "expected shore reeds, got {reed_count}");
    }

    #[test]
    fn shorelines_are_gentle() {
        // The easing pass must leave no trench walls: ground right at the
        // water's edge stays within a few voxels of the surface.
        let world = VoxelWorld::generate(1, 0.0);
        for z in 1..WORLD_SIZE_Z as i32 - 1 {
            for x in 1..WORLD_SIZE_X as i32 - 1 {
                if world.get(x, WATER_LEVEL, z) != Voxel::Water {
                    continue;
                }
                for (dx, dz) in [(1, 0), (-1, 0), (0, 1), (0, -1)] {
                    let neighbor_x = x + dx;
                    let neighbor_z = z + dz;
                    if world.get(neighbor_x, WATER_LEVEL, neighbor_z) == Voxel::Water {
                        continue;
                    }
                    // Top of the neighbor's GROUND (ignoring trees,
                    // boulders, and ground cover).
                    let ground_top = (WATER_LEVEL - 8..WATER_LEVEL + 40).rev().find(|&y| {
                        matches!(
                            world.get(neighbor_x, y, neighbor_z),
                            Voxel::Grass | Voxel::Sand | Voxel::Dirt | Voxel::Sediment
                        )
                    });
                    if let Some(top) = ground_top {
                        assert!(
                            top - WATER_LEVEL <= 3,
                            "trench wall at ({neighbor_x},{neighbor_z}): shore rises {} voxels \
                             above the water surface",
                            top - WATER_LEVEL
                        );
                    }
                }
            }
        }
    }

    #[test]
    fn forests_have_species_variety() {
        let world = VoxelWorld::generate(1, 0.0);
        let mut oak_leaves = 0;
        let mut birch_leaves = 0;
        let mut pine_leaves = 0;
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                for y in WATER_LEVEL..WORLD_SIZE_Y as i32 {
                    match world.get(x, y, z) {
                        Voxel::Leaves | Voxel::LeavesDark => oak_leaves += 1,
                        Voxel::LeavesBirch => birch_leaves += 1,
                        Voxel::LeavesPine => pine_leaves += 1,
                        _ => {}
                    }
                }
            }
        }
        assert!(
            oak_leaves > 500,
            "expected oak/willow canopy, got {oak_leaves}"
        );
        assert!(birch_leaves > 200, "expected birches, got {birch_leaves}");
        assert!(pine_leaves > 200, "expected pines, got {pine_leaves}");
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
        let world = VoxelWorld::from_imported(&terrain, 3, 0.0);

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
                        Voxel::Trunk | Voxel::TrunkBirch => {
                            // Only trunk BASES count — a legal tree's trunk
                            // may extend above the line, its roots may not.
                            let below = self::VoxelWorld::get(&world, x, y - 1, z);
                            let is_base = !matches!(below, Voxel::Trunk | Voxel::TrunkBirch);
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
        let world_a = VoxelWorld::generate(7, 0.0);
        let world_b = VoxelWorld::generate(7, 0.0);
        assert_eq!(world_a.column_starts, world_b.column_starts);
        assert_eq!(world_a.runs, world_b.runs);
    }

    #[test]
    fn rle_round_trips_and_compresses() {
        let world = VoxelWorld::generate(1, 0.0);

        // Runs must reconstruct exactly what get() reports, and every
        // column must span the full world height.
        for z in [0, 250, 500, 750, WORLD_SIZE_Z as i32 - 1] {
            for x in 0..WORLD_SIZE_X as i32 {
                let mut spanned = 0;
                for (voxel, y_start, length) in world.column_runs(x, z) {
                    assert_eq!(spanned, y_start, "run gap at ({x},{z})");
                    for y in y_start..y_start + length {
                        assert_eq!(world.get(x, y, z), voxel);
                    }
                    spanned += length;
                }
                assert_eq!(spanned, WORLD_SIZE_Y as i32, "column ({x},{z}) short");
            }
        }

        // A scratch window must agree with get() everywhere inside it.
        let scratch = world.unpack_chunk(448, 512, 448, 512);
        for z in 447..513 {
            for x in 447..513 {
                for y in (WATER_LEVEL - 20)..(WATER_LEVEL + 60) {
                    assert_eq!(scratch.get(x, y, z), world.get(x, y, z));
                }
            }
        }

        let (run_count, rle_bytes) = world.memory_stats();
        assert!(run_count > 1_000_000, "suspiciously few runs: {run_count}");
        assert!(
            rle_bytes < 64_000_000,
            "RLE failed to compress: {rle_bytes} bytes"
        );
    }
}
