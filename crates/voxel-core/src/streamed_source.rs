//! The infinite streamed world as a [`VoxelSource`].
//!
//! [`crate::terrain_chunk`] already generates the infinite world's *base
//! terrain* (stone, dirt, grass, sand, snow, water) seamlessly per column.
//! This module wraps that terrain, adds the island's decoration passes —
//! ground cover, trees, and bushes — and exposes the whole thing through the
//! [`VoxelSource`] trait, so the **same** greedy mesher and material that
//! render the fixed island ([`crate::world::VoxelWorld`]) render the infinite
//! world too. No second renderer, no second look.
//!
//! # The seamlessness invariant
//!
//! Every voxel and every per-column color context a column at world `(x, z)`
//! reports is a *pure function of `(x, z, seed)`* — never of which
//! `unpack_chunk` window asked. Terrain, cover, dryness, cover density, and
//! water distance are position-only by construction (they reuse the
//! [`crate::terrain_chunk`] formulas). Trees are the subtle case: a tree's
//! canopy spans a radius around its trunk, so a column can be covered by a
//! tree whose trunk lies in a *different* window. We keep it seamless by:
//!
//! 1. Placing trees on a **jittered grid of cells**: each cell holds at most
//!    one candidate trunk at a hashed position, and whether a tree grows there
//!    is a pure function of that trunk cell — no global spacing pass, no scan
//!    order dependence.
//! 2. When building a window, growing **every** tree whose reach
//!    ([`MAX_TREE_REACH`]) overlaps the window — including trunks outside it —
//!    in a fixed canonical cell order, writing only the voxels that land inside
//!    the window. Two overlapping windows therefore see the identical set of
//!    trees, grown in the identical order, so their shared columns agree
//!    voxel-for-voxel.
//! 3. Deriving [`VoxelSource::tree_tone_at`] from the *same* candidate scan, so
//!    a leaf's tint matches the tree that stamped it.
//!
//! # Memoization (and why it cannot change the world)
//!
//! Everything above is a pure function of `(x, z, seed)`, which the mesher then
//! hammers: it unpacks one window but asks for the four per-column color
//! contexts roughly once per emitted face — tens of thousands of times per
//! chunk, mostly on columns it already asked about. Recomputing each time meant
//! minutes per chunk, dominated by the water-distance search (a radius-24
//! neighborhood scan of fBm terrain heights) run again and again.
//!
//! So a `StreamedSource` memoizes, behind a lock (the trait is `Sync`, and the
//! streamer builds one source per chunk, so a cache is effectively per-chunk):
//!
//! * [`TerrainTile`] — a 64-column tile of terrain heights, padded by the
//!   water-search radius, plus that tile's whole water-distance field computed
//!   in one **exact** two-pass distance transform instead of a per-column
//!   search. Serves heights, slope, and water distance.
//! * Per-column dryness + grass cover, per-cell tree/bush candidates, and the
//!   resulting per-column tree tone.
//!
//! These are memos of pure functions, so nothing observable moves: a cached
//! value is the value the old code computed, and the cache is never consulted
//! for anything but the coordinate it was keyed on. The caches sit in
//! dependency layers (tiles → column noise → candidates → tone) so a lookup
//! never re-enters the layer it came from.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use crate::noise::{fractal_noise_2d, hash_3d, hash_to_unit, smoothstep};
use crate::terrain_chunk::{
    classify_cap_with, dryness_at, SLOPE_WINDOW_RADIUS, WATER_SEARCH_RADIUS,
};
use crate::voxel_source::VoxelSource;
use crate::world::{
    terrain_column_height, ChunkScratch, Voxel, TREE_LINE_METERS, TREE_MAX_SLOPE_RATIO, VOXEL_SIZE,
    WATER_LEVEL, WORLD_SIZE_Y,
};

/// Farthest a tree's canopy or tone disc reaches from its trunk, in voxels.
/// Oak crowns reach ~18 voxels of leaves and stamp a 22-voxel tone disc; every
/// other species is tighter. Windows include every trunk within this reach so
/// no canopy that should spill in is missed.
const MAX_TREE_REACH: i32 = 24;

/// Side of a tree placement cell, in voxels. One candidate trunk per cell sets
/// the rough tree spacing; the per-cell existence roll thins it by biome.
///
/// Matched to the island's `minimum_spacing_squared` (46² in `plant_trees`),
/// which is what actually caps *its* tree density: the island rolls a small
/// per-column probability and then rejects any trunk within 46 voxels of an
/// earlier one, so its lush stands settle at roughly one tree per 46² columns.
/// A cell grid can't run that global rejection pass (it would not be a pure
/// function of position, so chunk borders would disagree), so the cell *is* the
/// spacing. At 22 the streamed world came out ~4.5× denser than the island — a
/// jungle instead of scattered stands over open meadow.
const TREE_CELL: i32 = 46;

/// Farthest a bush's leaves or tone disc reaches from its center, in voxels.
const MAX_BUSH_REACH: i32 = 6;

/// Side of a bush placement cell, in voxels — denser than the tree grid, for
/// scattered undergrowth. Sized so the per-cell roll lands near the island's
/// per-column bush probability (`0.0009` in `scatter_bushes`, ≈ one bush per
/// 1100 lush columns); at 10 the undergrowth was about twice the island's.
const BUSH_CELL: i32 = 14;

/// The streamed world bakes high summer; nothing thins the flower meadows.
const SEASON: f32 = 0.0;

/// Columns per side of one cached [`TerrainTile`]. Matches the mesher's chunk
/// side, so a chunk's own columns fall in a single tile and only its apron and
/// tree reach touch the neighbors.
const TILE_COLUMNS: i32 = 64;

/// Side of a tile's height grid: the tile's columns plus [`WATER_SEARCH_RADIUS`]
/// on every side. That halo is exactly what the water-distance transform needs,
/// so a tile's interior water distances are computable from the tile alone.
const TILE_PADDED_SPAN: i32 = TILE_COLUMNS + 2 * WATER_SEARCH_RADIUS;

/// An infinite, seamless voxel world rendered by the island's mesher.
///
/// Every column is still *defined* purely by `(x, z, seed)` — there is no
/// authored grid — but the source memoizes what it has already derived (see the
/// module docs), because the mesher asks for the same columns thousands of
/// times per chunk.
pub struct StreamedSource {
    /// The generation seed shared with [`terrain_column_height`] and the
    /// island, so a given seed produces one consistent world.
    pub seed: u32,
    /// Memoized derivations of that seed. Behind a lock so the source stays
    /// `Sync`; only ever holds already-computed values, never window state.
    cache: Mutex<Cache>,
}

/// Everything a [`StreamedSource`] has already derived, in dependency order:
/// tiles depend on nothing, column noise on nothing, candidates on tiles +
/// column noise, tones on candidates. A miss therefore never needs the map it
/// was looked up in, so computing a value while the lock is *released* can
/// never deadlock or recurse endlessly.
#[derive(Default)]
struct Cache {
    /// Height + water-distance tiles, keyed by tile coordinate.
    tiles: HashMap<(i32, i32), Arc<TerrainTile>>,
    /// Per-column dryness and grass cover, keyed by world column.
    column_noise: HashMap<(i32, i32), ColumnNoise>,
    /// Per-cell tree candidate (`None` = no tree grows in that cell).
    tree_candidates: HashMap<(i32, i32), Option<TreeCandidate>>,
    /// Per-cell bush candidate.
    bush_candidates: HashMap<(i32, i32), Option<BushCandidate>>,
    /// Per-column tree/bush tint, keyed by world column.
    tree_tones: HashMap<(i32, i32), f32>,
}

/// The two cheap-but-repeated position noises of one column.
#[derive(Clone, Copy)]
struct ColumnNoise {
    dryness: f32,
    cover: f32,
}

impl StreamedSource {
    /// A streamed world for `seed`, matching the terrain the island generator
    /// produces for the same seed (minus the radial island mask).
    pub fn new(seed: u32) -> Self {
        StreamedSource {
            seed,
            cache: Mutex::new(Cache::default()),
        }
    }

    /// Lock the cache, panicking on poisoning — a poisoned cache means a panic
    /// happened mid-generation, and half a world is not worth recovering.
    fn cache(&self) -> std::sync::MutexGuard<'_, Cache> {
        self.cache.lock().expect("streamed-source cache poisoned")
    }

    /// The tile covering a column, generating it on first touch. The lock is
    /// held only around the map itself; generation runs unlocked, so a
    /// concurrent duplicate build is possible and harmless (identical output).
    fn tile(&self, world_x: i32, world_z: i32) -> Arc<TerrainTile> {
        let key = (
            world_x.div_euclid(TILE_COLUMNS),
            world_z.div_euclid(TILE_COLUMNS),
        );
        if let Some(tile) = self.cache().tiles.get(&key) {
            return Arc::clone(tile);
        }
        let tile = Arc::new(TerrainTile::generate(key.0, key.1, self.seed));
        Arc::clone(self.cache().tiles.entry(key).or_insert(tile))
    }

    /// Terrain height at a column, from the covering tile's height grid.
    fn height(&self, world_x: i32, world_z: i32) -> i32 {
        self.tile(world_x, world_z).height(world_x, world_z)
    }

    /// Terrain steepness at a column (see [`TerrainTile::slope`]).
    fn slope(&self, world_x: i32, world_z: i32) -> f32 {
        self.tile(world_x, world_z).slope(world_x, world_z)
    }

    /// Distance to the nearest water column in meters, read out of the covering
    /// tile's precomputed field.
    fn water_distance(&self, world_x: i32, world_z: i32) -> f32 {
        self.tile(world_x, world_z).water_distance(world_x, world_z)
    }

    /// Dryness and grass cover at a column, computed together on first touch.
    fn column_noise(&self, world_x: i32, world_z: i32) -> ColumnNoise {
        let key = (world_x, world_z);
        if let Some(noise) = self.cache().column_noise.get(&key) {
            return *noise;
        }
        let noise = ColumnNoise {
            dryness: dryness_at(world_x, world_z, self.seed),
            cover: cover_density(world_x, world_z, self.seed),
        };
        self.cache().column_noise.insert(key, noise);
        noise
    }

    /// The tree (if any) rooted in a placement cell, memoized per cell.
    fn tree_candidate(&self, cell_x: i32, cell_z: i32) -> Option<TreeCandidate> {
        let key = (cell_x, cell_z);
        if let Some(candidate) = self.cache().tree_candidates.get(&key) {
            return *candidate;
        }
        let candidate = tree_candidate(self, cell_x, cell_z);
        self.cache().tree_candidates.insert(key, candidate);
        candidate
    }

    /// The bush (if any) rooted in a placement cell, memoized per cell.
    fn bush_candidate(&self, cell_x: i32, cell_z: i32) -> Option<BushCandidate> {
        let key = (cell_x, cell_z);
        if let Some(candidate) = self.cache().bush_candidates.get(&key) {
            return *candidate;
        }
        let candidate = bush_candidate(self, cell_x, cell_z);
        self.cache().bush_candidates.insert(key, candidate);
        candidate
    }
}

/// One tile's cached terrain: a padded grid of column heights, plus the water
/// distance of every interior column.
///
/// The heights are what makes the tile worth keeping — [`terrain_column_height`]
/// is an fBm stack, and slope alone reads a 5×5 window of it per column — but
/// the water-distance field is the real win: one transform over the tile
/// replaces a radius-24 search *per query*.
struct TerrainTile {
    /// Seed the tile was generated with, so an out-of-halo height query can
    /// fall back to the same pure function.
    seed: u32,
    /// World coordinates of `heights`' first cell (interior origin minus the
    /// water-search halo).
    padded_origin_x: i32,
    padded_origin_z: i32,
    /// World coordinates of the tile's first *interior* column.
    interior_origin_x: i32,
    interior_origin_z: i32,
    /// Column heights, `TILE_PADDED_SPAN²`, indexed `local_z * span + local_x`.
    heights: Vec<i32>,
    /// Interior water distances in meters, `TILE_COLUMNS²`, `f32::MAX` where no
    /// water lies within [`WATER_SEARCH_RADIUS`].
    water_distances: Vec<f32>,
}

impl TerrainTile {
    /// Generate the tile at tile coordinates `(tile_x, tile_z)`.
    fn generate(tile_x: i32, tile_z: i32, seed: u32) -> TerrainTile {
        let interior_origin_x = tile_x * TILE_COLUMNS;
        let interior_origin_z = tile_z * TILE_COLUMNS;
        let padded_origin_x = interior_origin_x - WATER_SEARCH_RADIUS;
        let padded_origin_z = interior_origin_z - WATER_SEARCH_RADIUS;

        let mut heights = Vec::with_capacity((TILE_PADDED_SPAN * TILE_PADDED_SPAN) as usize);
        for local_z in 0..TILE_PADDED_SPAN {
            for local_x in 0..TILE_PADDED_SPAN {
                heights.push(terrain_column_height(
                    padded_origin_x + local_x,
                    padded_origin_z + local_z,
                    seed,
                ));
            }
        }
        let water_distances = water_distance_field(&heights);

        TerrainTile {
            seed,
            padded_origin_x,
            padded_origin_z,
            interior_origin_x,
            interior_origin_z,
            heights,
            water_distances,
        }
    }

    /// Height of a column. Inside the tile's padded grid (which covers every
    /// interior column's whole slope window and water halo) this is an array
    /// read; anywhere else it falls back to the generator, so the value is the
    /// same either way.
    fn height(&self, world_x: i32, world_z: i32) -> i32 {
        let local_x = world_x - self.padded_origin_x;
        let local_z = world_z - self.padded_origin_z;
        if (0..TILE_PADDED_SPAN).contains(&local_x) && (0..TILE_PADDED_SPAN).contains(&local_z) {
            self.heights[(local_z * TILE_PADDED_SPAN + local_x) as usize]
        } else {
            terrain_column_height(world_x, world_z, self.seed)
        }
    }

    /// Terrain steepness at a column (rise over run, 1.0 = 45°), from the height
    /// span over a 5×5 window — the same min/max over the same position-only
    /// heights as `terrain_chunk::slope_at`, reading this tile's grid instead of
    /// re-evaluating the noise 25 times.
    fn slope(&self, world_x: i32, world_z: i32) -> f32 {
        let center = self.height(world_x, world_z);
        let mut lowest = center;
        let mut highest = center;
        for offset_z in -SLOPE_WINDOW_RADIUS..=SLOPE_WINDOW_RADIUS {
            for offset_x in -SLOPE_WINDOW_RADIUS..=SLOPE_WINDOW_RADIUS {
                let neighbor = self.height(world_x + offset_x, world_z + offset_z);
                lowest = lowest.min(neighbor);
                highest = highest.max(neighbor);
            }
        }
        (highest - lowest) as f32 / (2 * SLOPE_WINDOW_RADIUS) as f32
    }

    /// Water distance of an interior column, in meters. Callers reach a tile
    /// through [`StreamedSource::tile`], which picks the tile *containing* the
    /// column, so the column is always interior here.
    fn water_distance(&self, world_x: i32, world_z: i32) -> f32 {
        let interior_x = world_x - self.interior_origin_x;
        let interior_z = world_z - self.interior_origin_z;
        debug_assert!(
            (0..TILE_COLUMNS).contains(&interior_x) && (0..TILE_COLUMNS).contains(&interior_z),
            "water distance asked of a tile that does not contain ({world_x},{world_z})"
        );
        self.water_distances[(interior_z * TILE_COLUMNS + interior_x) as usize]
    }
}

/// Water distance for every interior column of a tile, from its padded height
/// grid — an **exact** Euclidean distance transform in two separable passes
/// (squared distances along z, then combined along x), replacing the radius-24
/// neighborhood search `terrain_chunk::water_distance_at` runs per column.
///
/// The result is bit-identical to that search, cap behavior included: a column
/// with water inside the radius reports `sqrt(nearest squared) * VOXEL_SIZE`
/// from the very same integer offset, and a column with none reports
/// `f32::MAX`. Exactness matters — these values pick beach sand, tree
/// plantability, and shoreline tint, so an approximation (a true chamfer
/// transform, say) would visibly move the world.
fn water_distance_field(heights: &[i32]) -> Vec<f32> {
    let span = TILE_PADDED_SPAN;
    let cap_squared = WATER_SEARCH_RADIUS * WATER_SEARCH_RADIUS;
    // One past the cap: stands in for "no water in reach". Adding any squared
    // offset to it keeps it past the cap, so it can never fake a near hit.
    let beyond_cap = cap_squared + 1;

    // Pass 1, along z: squared distance from every padded cell to the nearest
    // water cell in its own column, as a forward then backward scan.
    let mut column_distance_squared = vec![beyond_cap; (span * span) as usize];
    for local_x in 0..span {
        let mut nearest_water_z: Option<i32> = None;
        for local_z in 0..span {
            let index = (local_z * span + local_x) as usize;
            if heights[index] < WATER_LEVEL {
                nearest_water_z = Some(local_z);
            }
            if let Some(water_z) = nearest_water_z {
                let delta = local_z - water_z;
                column_distance_squared[index] = delta * delta;
            }
        }
        nearest_water_z = None;
        for local_z in (0..span).rev() {
            let index = (local_z * span + local_x) as usize;
            if heights[index] < WATER_LEVEL {
                nearest_water_z = Some(local_z);
            }
            if let Some(water_z) = nearest_water_z {
                let delta = water_z - local_z;
                column_distance_squared[index] = column_distance_squared[index].min(delta * delta);
            }
        }
    }

    // Pass 2, along x: an interior cell's nearest water is the best of its
    // neighbors' column results offset by dx². Only |dx| <= radius can land
    // inside the cap, so that is the whole window to scan.
    let mut distances = Vec::with_capacity((TILE_COLUMNS * TILE_COLUMNS) as usize);
    for interior_z in 0..TILE_COLUMNS {
        let padded_z = interior_z + WATER_SEARCH_RADIUS;
        for interior_x in 0..TILE_COLUMNS {
            let padded_x = interior_x + WATER_SEARCH_RADIUS;
            let row_base = padded_z * span;
            let mut nearest_squared = beyond_cap;
            for offset_x in -WATER_SEARCH_RADIUS..=WATER_SEARCH_RADIUS {
                let candidate = offset_x * offset_x
                    + column_distance_squared[(row_base + padded_x + offset_x) as usize];
                nearest_squared = nearest_squared.min(candidate);
            }
            distances.push(if nearest_squared > cap_squared {
                f32::MAX
            } else {
                (nearest_squared as f32).sqrt() * VOXEL_SIZE
            });
        }
    }
    distances
}

impl VoxelSource for StreamedSource {
    fn unpack_chunk(&self, x_start: i32, x_end: i32, z_start: i32, z_end: i32) -> ChunkScratch {
        // Window plus the mesher's 1-cell apron. No world-size clamp — the
        // world is infinite, so any coordinate is valid.
        let origin_x = x_start - 1;
        let origin_z = z_start - 1;
        let span_x = (x_end + 1) - (x_start - 1);
        let span_z = (z_end + 1) - (z_start - 1);

        let mut work = WorkGrid::new(origin_x, origin_z, span_x, span_z);

        // 1. Base terrain + ground cover: purely per column.
        for local_z in 0..span_z {
            for local_x in 0..span_x {
                let world_x = origin_x + local_x;
                let world_z = origin_z + local_z;
                work.fill_terrain_and_cover(world_x, world_z, self);
            }
        }

        // 2. Trees: every candidate trunk whose reach overlaps the window,
        //    grown in canonical (cell_z, cell_x) order so overlapping crowns
        //    resolve identically regardless of the window.
        for_each_cell_in_reach(
            origin_x,
            origin_z,
            span_x,
            span_z,
            MAX_TREE_REACH,
            TREE_CELL,
            |cell_x, cell_z| {
                if let Some(tree) = self.tree_candidate(cell_x, cell_z) {
                    work.grow_tree(&tree);
                }
            },
        );

        // 3. Bushes, after the trees (matching the island's pass order, so a
        //    bush only fills the air a tree left).
        for_each_cell_in_reach(
            origin_x,
            origin_z,
            span_x,
            span_z,
            MAX_BUSH_REACH,
            BUSH_CELL,
            |cell_x, cell_z| {
                if let Some(bush) = self.bush_candidate(cell_x, cell_z) {
                    work.grow_bush(&bush);
                }
            },
        );

        work.into_scratch()
    }

    fn dryness_at(&self, x: i32, z: i32) -> f32 {
        self.column_noise(x, z).dryness
    }

    fn cover_at(&self, x: i32, z: i32) -> f32 {
        self.column_noise(x, z).cover
    }

    fn water_distance_at(&self, x: i32, z: i32) -> f32 {
        self.water_distance(x, z)
    }

    fn tree_tone_at(&self, x: i32, z: i32) -> f32 {
        if let Some(tone) = self.cache().tree_tones.get(&(x, z)) {
            return *tone;
        }

        let mut tone = 0.5;

        // Trees first, then bushes — matching the stamp order in `unpack_chunk`
        // and the island, so the last feature to cover a column wins its tint.
        let tree_cell = x.div_euclid(TREE_CELL);
        let tree_cell_z = z.div_euclid(TREE_CELL);
        let tree_cell_span = MAX_TREE_REACH.div_euclid(TREE_CELL) + 1;
        for cell_z in (tree_cell_z - tree_cell_span)..=(tree_cell_z + tree_cell_span) {
            for cell_x in (tree_cell - tree_cell_span)..=(tree_cell + tree_cell_span) {
                if let Some(tree) = self.tree_candidate(cell_x, cell_z) {
                    let delta_x = x - tree.trunk_x;
                    let delta_z = z - tree.trunk_z;
                    if delta_x * delta_x + delta_z * delta_z <= tree.tone_radius * tree.tone_radius
                    {
                        tone = tree.tone;
                    }
                }
            }
        }

        let bush_cell = x.div_euclid(BUSH_CELL);
        let bush_cell_z = z.div_euclid(BUSH_CELL);
        let bush_cell_span = MAX_BUSH_REACH.div_euclid(BUSH_CELL) + 1;
        for cell_z in (bush_cell_z - bush_cell_span)..=(bush_cell_z + bush_cell_span) {
            for cell_x in (bush_cell - bush_cell_span)..=(bush_cell + bush_cell_span) {
                if let Some(bush) = self.bush_candidate(cell_x, cell_z) {
                    let delta_x = x - bush.center_x;
                    let delta_z = z - bush.center_z;
                    if delta_x * delta_x + delta_z * delta_z <= bush.tone_radius * bush.tone_radius
                    {
                        tone = bush.tone;
                    }
                }
            }
        }

        self.cache().tree_tones.insert((x, z), tone);
        tone
    }

    fn world_offset(&self) -> (f32, f32) {
        // The infinite world meshes in raw world-voxel coordinates.
        (0.0, 0.0)
    }
}

/// Grass-patch coverage at a column, `0.0` (bare) to `1.0` (dense clump).
/// Reproduces `world::compute_cover_map`'s per-column noise.
fn cover_density(world_x: i32, world_z: i32, seed: u32) -> f32 {
    fractal_noise_2d(
        world_x as f32 * 0.045 + 3100.0,
        world_z as f32 * 0.045,
        3,
        seed.wrapping_add(29),
    )
}

/// Visit every placement cell of side `cell_size` whose reach overlaps the
/// window `[origin, origin + span)` expanded by `reach`, in canonical
/// `(cell_z, cell_x)` order. That order is independent of the window, so
/// features grown across it resolve identically in any overlapping window.
fn for_each_cell_in_reach(
    origin_x: i32,
    origin_z: i32,
    span_x: i32,
    span_z: i32,
    reach: i32,
    cell_size: i32,
    mut visit: impl FnMut(i32, i32),
) {
    let cell_min_x = (origin_x - reach).div_euclid(cell_size);
    let cell_max_x = (origin_x + span_x - 1 + reach).div_euclid(cell_size);
    let cell_min_z = (origin_z - reach).div_euclid(cell_size);
    let cell_max_z = (origin_z + span_z - 1 + reach).div_euclid(cell_size);
    for cell_z in cell_min_z..=cell_max_z {
        for cell_x in cell_min_x..=cell_max_x {
            visit(cell_x, cell_z);
        }
    }
}

/// Tree species, picked from the terrain and a per-tree hash exactly as
/// `world::WorldBuilder::grow_tree` picks it.
#[derive(Clone, Copy)]
enum Species {
    Oak,
    Birch,
    Pine,
    Willow,
}

/// A tree that grew in one placement cell: its trunk column, ground height,
/// per-tree hash, species, and color identity.
#[derive(Clone, Copy)]
struct TreeCandidate {
    trunk_x: i32,
    trunk_z: i32,
    ground_height: i32,
    tree_hash: u32,
    species: Species,
    tone: f32,
    tone_radius: i32,
}

/// The tree (if any) rooted in placement cell `(cell_x, cell_z)`. Pure function
/// of the cell and seed: the trunk position, whether it grows, and its species
/// all derive from position hashes and the position-only terrain fields, so it
/// never depends on which window asks. Reached through
/// [`StreamedSource::tree_candidate`], which memoizes it per cell.
fn tree_candidate(source: &StreamedSource, cell_x: i32, cell_z: i32) -> Option<TreeCandidate> {
    let seed = source.seed;
    let cell_hash = hash_3d(cell_x, 701, cell_z, seed.wrapping_add(31));
    let trunk_x = cell_x * TREE_CELL + (cell_hash % TREE_CELL as u32) as i32;
    let trunk_z = cell_z * TREE_CELL + ((cell_hash >> 8) % TREE_CELL as u32) as i32;

    let ground_height = source.height(trunk_x, trunk_z);
    if ground_height <= WATER_LEVEL + 1 {
        return None;
    }

    let slope = source.slope(trunk_x, trunk_z);
    let water_distance = source.water_distance(trunk_x, trunk_z);
    let dryness = source.column_noise(trunk_x, trunk_z).dryness;
    let cap = classify_cap_with(
        trunk_x,
        trunk_z,
        ground_height,
        seed,
        slope,
        water_distance,
        dryness,
    );

    // Trees root in grass anywhere, and in shoreline sand (willow country).
    let plantable = match cap {
        Voxel::Grass => true,
        Voxel::Sand => water_distance <= 4.0,
        _ => false,
    };
    if !plantable {
        return None;
    }

    // Below the tree line, on gentle slopes only.
    let altitude_meters = (ground_height - WATER_LEVEL) as f32 * VOXEL_SIZE;
    if altitude_meters >= TREE_LINE_METERS || slope > TREE_MAX_SLOPE_RATIO {
        return None;
    }

    // Dense stands on the lush side, lone trees near the desert, a bonus along
    // the waterline so shores get willows. One roll per cell (no global spacing
    // pass), so placement is a pure function of position.
    let lushness = 1.0 - dryness;
    let shore_bonus = 1.0 + 1.5 * smoothstep(6.0, 2.0, water_distance);
    let probability = ((0.18 + 0.42 * lushness) * shore_bonus).min(0.9);
    let existence_roll = hash_to_unit(hash_3d(trunk_x, 700, trunk_z, seed.wrapping_add(31)));
    if existence_roll >= probability {
        return None;
    }

    // Species selection, reproducing `grow_tree`'s branch order and tone discs.
    let tree_hash = hash_3d(trunk_x, 800, trunk_z, seed.wrapping_add(41));
    let tone = hash_to_unit(tree_hash.wrapping_mul(0x85EB_CA6B).wrapping_add(0x9E37));
    let species_roll = hash_to_unit(tree_hash.wrapping_mul(0x27D4_EB2F));
    let (species, tone_radius) = if water_distance <= 4.0 && species_roll < 0.70 {
        (Species::Willow, 18)
    } else if altitude_meters > 6.5
        || (dryness > 0.55 && species_roll < 0.45)
        || species_roll > 0.90
    {
        (Species::Pine, 13)
    } else if species_roll < 0.30 {
        (Species::Birch, 10)
    } else {
        (Species::Oak, 22)
    };

    Some(TreeCandidate {
        trunk_x,
        trunk_z,
        ground_height,
        tree_hash,
        species,
        tone,
        tone_radius,
    })
}

/// A bush that grew in one placement cell.
#[derive(Clone, Copy)]
struct BushCandidate {
    center_x: i32,
    center_z: i32,
    ground_height: i32,
    bush_hash: u32,
    tone: f32,
    tone_radius: i32,
}

/// The bush (if any) rooted in placement cell `(cell_x, cell_z)`. Pure function
/// of the cell and seed, mirroring `scatter_bushes`'s per-column rules. Reached
/// through [`StreamedSource::bush_candidate`], which memoizes it per cell.
fn bush_candidate(source: &StreamedSource, cell_x: i32, cell_z: i32) -> Option<BushCandidate> {
    let seed = source.seed;
    let cell_hash = hash_3d(cell_x, 711, cell_z, seed.wrapping_add(33));
    let center_x = cell_x * BUSH_CELL + (cell_hash % BUSH_CELL as u32) as i32;
    let center_z = cell_z * BUSH_CELL + ((cell_hash >> 8) % BUSH_CELL as u32) as i32;

    let ground_height = source.height(center_x, center_z);
    if ground_height <= WATER_LEVEL + 1 {
        return None;
    }

    let slope = source.slope(center_x, center_z);
    let water_distance = source.water_distance(center_x, center_z);
    let dryness = source.column_noise(center_x, center_z).dryness;
    let cap = classify_cap_with(
        center_x,
        center_z,
        ground_height,
        seed,
        slope,
        water_distance,
        dryness,
    );
    if cap != Voxel::Grass || slope > 0.85 {
        return None;
    }

    let lushness = 1.0 - dryness;
    let shore_bonus = 1.0 + smoothstep(8.0, 2.0, water_distance);
    let probability = ((0.06 + 0.12 * lushness) * shore_bonus).min(0.6);
    let existence_roll = hash_to_unit(hash_3d(center_x, 710, center_z, seed.wrapping_add(33)));
    if existence_roll >= probability {
        return None;
    }

    let bush_hash = hash_3d(center_x, 860, center_z, seed.wrapping_add(35));
    let tone = hash_to_unit(bush_hash.wrapping_mul(0x85EB_CA6B));
    let half_extent_x = 2 + (bush_hash % 3) as i32;
    let half_extent_z = 2 + ((bush_hash >> 3) % 3) as i32;
    let tone_radius = half_extent_x.max(half_extent_z) + 1;

    Some(BushCandidate {
        center_x,
        center_z,
        ground_height,
        bush_hash,
        tone,
        tone_radius,
    })
}

/// A dense scratch block covering exactly one window (`[origin, origin +
/// span)`), y-contiguous per column. Cells outside the window read [`Voxel::Air`]
/// and swallow writes, so a tree rooted outside the window can be grown with its
/// full stamp and only its in-window voxels land — the rest fall off the edge
/// without affecting anything. Cells are laid out to copy straight into
/// [`ChunkScratch::from_columns`].
struct WorkGrid {
    origin_x: i32,
    origin_z: i32,
    span_x: i32,
    span_z: i32,
    cells: Vec<Voxel>,
}

impl WorkGrid {
    fn new(origin_x: i32, origin_z: i32, span_x: i32, span_z: i32) -> Self {
        WorkGrid {
            origin_x,
            origin_z,
            span_x,
            span_z,
            cells: vec![Voxel::Air; (span_x * span_z * WORLD_SIZE_Y as i32) as usize],
        }
    }

    fn get(&self, x: i32, y: i32, z: i32) -> Voxel {
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

    fn set(&mut self, x: i32, y: i32, z: i32, voxel: Voxel) {
        let local_x = x - self.origin_x;
        let local_z = z - self.origin_z;
        if y < 0
            || y >= WORLD_SIZE_Y as i32
            || local_x < 0
            || local_z < 0
            || local_x >= self.span_x
            || local_z >= self.span_z
        {
            return;
        }
        self.cells[((local_z * self.span_x + local_x) * WORLD_SIZE_Y as i32 + y) as usize] = voxel;
    }

    fn into_scratch(self) -> ChunkScratch {
        let span_x = self.span_x;
        let origin_x = self.origin_x;
        let origin_z = self.origin_z;
        ChunkScratch::from_columns(
            origin_x,
            origin_z,
            span_x,
            self.span_z,
            |world_x, world_z, column| {
                let local_x = world_x - origin_x;
                let local_z = world_z - origin_z;
                let base = ((local_z * span_x + local_x) * WORLD_SIZE_Y as i32) as usize;
                column.copy_from_slice(&self.cells[base..base + WORLD_SIZE_Y]);
            },
        )
    }

    /// Fill one column's base terrain (bedrock-up stone core, subsoil band,
    /// biome cap, water to the water line) and its ground cover, reproducing
    /// `terrain_chunk::fill_terrain_column` and `world::WorldBuilder::decorate`
    /// from position-only fields.
    fn fill_terrain_and_cover(&mut self, world_x: i32, world_z: i32, source: &StreamedSource) {
        let seed = source.seed;
        let column_height = source.height(world_x, world_z);
        let slope = source.slope(world_x, world_z);
        let water_distance = source.water_distance(world_x, world_z);
        let ColumnNoise { dryness, cover } = source.column_noise(world_x, world_z);
        let cap = classify_cap_with(
            world_x,
            world_z,
            column_height,
            seed,
            slope,
            water_distance,
            dryness,
        );
        let subsoil = match cap {
            Voxel::Sand => Voxel::Sand,
            Voxel::Sediment => Voxel::Sediment,
            Voxel::Grass => Voxel::Dirt,
            _ => Voxel::Stone,
        };

        // Stone core from bedrock, subsoil band, cap on top. An infinite world
        // has no sculpted underside, so the core starts at y = 0.
        for y in 0..=column_height {
            let voxel = if y == column_height {
                cap
            } else if y >= column_height - 3 {
                subsoil
            } else {
                Voxel::Stone
            };
            self.set(world_x, y, world_z, voxel);
        }
        for y in (column_height + 1)..=WATER_LEVEL {
            self.set(world_x, y, world_z, Voxel::Water);
        }

        self.decorate_column(
            world_x,
            world_z,
            column_height,
            cap,
            water_distance,
            dryness,
            cover,
            seed,
        );
    }

    /// Ground cover for one column, above and below the waterline, reproducing
    /// `world::WorldBuilder::decorate`: reeds/weed/lily pads in the shallows,
    /// tall-grass clumps and flower meadows on land.
    #[allow(clippy::too_many_arguments)]
    fn decorate_column(
        &mut self,
        world_x: i32,
        world_z: i32,
        column_height: i32,
        cap: Voxel,
        water_distance: f32,
        dryness: f32,
        cover: f32,
        seed: u32,
    ) {
        // Underwater: reeds wade out into the shallowest water, weed tufts cover
        // the bed, lily pads cluster on the surface.
        if column_height < WATER_LEVEL {
            let depth = WATER_LEVEL - column_height;
            let roll = hash_to_unit(hash_3d(world_x, 902, world_z, seed.wrapping_add(53)));
            if depth <= 2 {
                let reed_patch = fractal_noise_2d(
                    world_x as f32 * 0.06 + 9700.0,
                    world_z as f32 * 0.06,
                    3,
                    seed.wrapping_add(77),
                );
                if roll < 0.12 + 0.30 * smoothstep(0.48, 0.70, reed_patch) {
                    let stalk_hash = hash_3d(world_x, 904, world_z, seed.wrapping_add(55));
                    if stalk_hash.is_multiple_of(3) {
                        for y in (column_height + 1)..=(WATER_LEVEL + 2) {
                            self.set(world_x, y, world_z, Voxel::Reed);
                        }
                        self.set(world_x, WATER_LEVEL + 3, world_z, Voxel::CattailHead);
                    } else {
                        for y in (column_height + 1)..=(WATER_LEVEL + 1) {
                            self.set(world_x, y, world_z, Voxel::Reed);
                        }
                    }
                    return;
                }
            }
            let weed_patch = fractal_noise_2d(
                world_x as f32 * 0.05 + 8300.0,
                world_z as f32 * 0.05,
                3,
                seed.wrapping_add(71),
            );
            let weed_chance = if depth >= 2 { 0.62 } else { 0.30 };
            if roll < weed_chance * smoothstep(0.42, 0.72, weed_patch) {
                self.set(world_x, column_height + 1, world_z, Voxel::WaterWeed);
            } else if (1..=6).contains(&depth) {
                let pad_patch = fractal_noise_2d(
                    world_x as f32 * 0.045 + 9100.0,
                    world_z as f32 * 0.045,
                    3,
                    seed.wrapping_add(73),
                );
                if pad_patch > 0.66
                    && roll > 0.55
                    && self.get(world_x, WATER_LEVEL + 1, world_z) == Voxel::Air
                {
                    let pad = if hash_3d(world_x, 908, world_z, seed.wrapping_add(65))
                        .is_multiple_of(6)
                    {
                        Voxel::LilyBloom
                    } else {
                        Voxel::LilyPad
                    };
                    self.set(world_x, WATER_LEVEL + 1, world_z, pad);
                }
            }
            return;
        }

        let altitude_meters = (column_height - WATER_LEVEL) as f32 * VOXEL_SIZE;

        // Reed belt right at the waterline, on sand or grass.
        if matches!(cap, Voxel::Sand | Voxel::Grass)
            && altitude_meters <= 0.6
            && water_distance <= 1.1
        {
            let reed_patch = fractal_noise_2d(
                world_x as f32 * 0.06 + 9700.0,
                world_z as f32 * 0.06,
                3,
                seed.wrapping_add(77),
            );
            let roll = hash_to_unit(hash_3d(world_x, 903, world_z, seed.wrapping_add(54)));
            if roll < 0.18 + 0.35 * smoothstep(0.48, 0.70, reed_patch) {
                let stalk_hash = hash_3d(world_x, 904, world_z, seed.wrapping_add(55));
                let is_cattail = stalk_hash.is_multiple_of(3);
                let reed_height = if is_cattail {
                    3 + ((stalk_hash >> 4) % 2) as i32
                } else {
                    2 + (stalk_hash % 3) as i32
                };
                for step in 1..=reed_height {
                    if self.get(world_x, column_height + step, world_z) == Voxel::Air {
                        self.set(world_x, column_height + step, world_z, Voxel::Reed);
                    }
                }
                if is_cattail
                    && self.get(world_x, column_height + reed_height + 1, world_z) == Voxel::Air
                {
                    self.set(
                        world_x,
                        column_height + reed_height + 1,
                        world_z,
                        Voxel::CattailHead,
                    );
                }
                return;
            }
        }

        if cap != Voxel::Grass || column_height <= WATER_LEVEL {
            return;
        }

        // Grass grows in clumpy patches with bare dirt between them, thinning
        // toward the desert side.
        let lushness = 1.0 - dryness;
        let clump = smoothstep(0.40, 0.70, cover) * lushness;
        let roll = hash_to_unit(hash_3d(world_x, 900, world_z, seed.wrapping_add(51)));

        // Flower meadows: noise blobs where flowers bloom in dense two-tone
        // drifts. Each ~9 m patch keeps one palette.
        let meadow = fractal_noise_2d(
            world_x as f32 * 0.02 + 4700.0,
            world_z as f32 * 0.02,
            3,
            seed.wrapping_add(61),
        );
        let meadow_amount = smoothstep(0.60, 0.72, meadow) * lushness * (1.0 - SEASON * 0.75);
        if meadow_amount > 0.0 && roll < 0.14 * meadow_amount {
            let palette = hash_3d(
                world_x.div_euclid(72),
                905,
                world_z.div_euclid(72),
                seed.wrapping_add(62),
            ) % 4;
            let pick = hash_3d(world_x, 906, world_z, seed.wrapping_add(63));
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
            if pick % 5 < 2 {
                self.set(world_x, column_height + 1, world_z, Voxel::TallGrass);
                self.set(world_x, column_height + 2, world_z, flower);
            } else {
                self.set(world_x, column_height + 1, world_z, flower);
            }
        } else if roll < 0.70 * clump {
            let stalk_roll = hash_to_unit(hash_3d(world_x, 907, world_z, seed.wrapping_add(64)));
            let stalk_height = if stalk_roll < clump * 0.5 {
                3
            } else if stalk_roll < clump * 1.4 {
                2
            } else {
                1
            };
            for step in 1..=stalk_height {
                if self.get(world_x, column_height + step, world_z) == Voxel::Air {
                    self.set(world_x, column_height + step, world_z, Voxel::TallGrass);
                }
            }
        } else if clump > 0.3 && roll > 0.9975 {
            let flower = match hash_3d(world_x, 901, world_z, seed.wrapping_add(52)) % 3 {
                0 => Voxel::FlowerPink,
                1 => Voxel::FlowerWhite,
                _ => Voxel::FlowerYellow,
            };
            self.set(world_x, column_height + 1, world_z, flower);
        }
    }

    /// Grow one tree into the window, dispatching by species. Reproduces the
    /// island's canopy shapes and leaf voxels.
    fn grow_tree(&mut self, tree: &TreeCandidate) {
        let TreeCandidate {
            trunk_x,
            trunk_z,
            ground_height,
            tree_hash,
            species,
            ..
        } = *tree;
        match species {
            Species::Oak => self.grow_oak(trunk_x, ground_height, trunk_z, tree_hash),
            Species::Birch => self.grow_birch(trunk_x, ground_height, trunk_z, tree_hash),
            Species::Pine => self.grow_pine(trunk_x, ground_height, trunk_z, tree_hash),
            Species::Willow => self.grow_willow(trunk_x, ground_height, trunk_z, tree_hash),
        }
    }

    /// Chunky reference-style tree: thick 3×3 trunk and a crown of overlapping
    /// rectangular slabs in two leaf tones.
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

    /// Slender white-barked tree: thin 2×2 trunk, a narrow stack of small leaf
    /// blobs high up.
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

    /// Conifer: stacked shrinking discs with one-voxel gaps between them.
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

    /// Waterline tree: short thick trunk, a wide dome, and leaf strands hanging
    /// from the dome's rim.
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

        let strand_count = 26;
        for strand_index in 0..strand_count {
            let strand_hash = hash_3d(x + strand_index, 850, z - strand_index, tree_hash);
            if hash_to_unit(strand_hash) > 0.80 {
                continue;
            }
            let angle = std::f32::consts::TAU * strand_index as f32 / strand_count as f32;
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

    /// Low leaf blob scattered on grassland — undergrowth between trees.
    fn grow_bush(&mut self, bush: &BushCandidate) {
        let BushCandidate {
            center_x: x,
            center_z: z,
            ground_height,
            bush_hash,
            ..
        } = *bush;
        let half_extent_x = 2 + (bush_hash % 3) as i32;
        let half_extent_z = 2 + ((bush_hash >> 3) % 3) as i32;
        let half_extent_y = 1 + ((bush_hash >> 6) % 2) as i32;

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
}

#[cfg(test)]
mod tests {
    use super::*;
    // The per-column reference implementations the cached fields must match
    // exactly: the brute-force radius-24 water search and the 5×5 slope window,
    // both straight from the terrain generator.
    use crate::terrain_chunk::{slope_at, water_distance_at};

    /// Read a single world column's full voxel stack independently of any
    /// window: a 1×1 window at `(world_x, world_z)` (plus its unavoidable
    /// apron), then the center column pulled out.
    fn column_stack(source: &StreamedSource, world_x: i32, world_z: i32) -> Vec<Voxel> {
        let scratch = source.unpack_chunk(world_x, world_x + 1, world_z, world_z + 1);
        (0..WORLD_SIZE_Y as i32)
            .map(|y| scratch.get(world_x, y, world_z))
            .collect()
    }

    #[test]
    fn column_is_deterministic_across_calls() {
        let source = StreamedSource::new(42);
        for &(world_x, world_z) in &[(0, 0), (63, 64), (500, -500), (-7, 13)] {
            let first = column_stack(&source, world_x, world_z);
            let second = column_stack(&source, world_x, world_z);
            assert_eq!(
                first, second,
                "column ({world_x},{world_z}) is not deterministic"
            );
        }
    }

    #[test]
    fn overlapping_windows_agree_on_shared_columns() {
        // Two windows that overlap in x 30..40, z 30..40. Every shared column,
        // over the full world height, must be voxel-identical — including any
        // column whose covering tree has its trunk outside one of the windows.
        let source = StreamedSource::new(7);
        let left = source.unpack_chunk(0, 40, 0, 40);
        let right = source.unpack_chunk(30, 70, 30, 70);
        for world_z in 30..40 {
            for world_x in 30..40 {
                for y in 0..WORLD_SIZE_Y as i32 {
                    assert_eq!(
                        left.get(world_x, y, world_z),
                        right.get(world_x, y, world_z),
                        "windows disagree at ({world_x},{y},{world_z})"
                    );
                }
            }
        }
    }

    #[test]
    fn a_tree_across_the_border_is_seamless() {
        // Find a leaf voxel that sits inside a window but belongs to a tree
        // whose trunk is OUTSIDE that window, then confirm a second window that
        // *does* contain the trunk reports the identical voxel there. This is
        // the canopy-spill case the whole design turns on.
        let source = StreamedSource::new(3);
        // Search a band of trunk cells for a tree whose canopy reaches left of
        // its trunk, past a chosen border x.
        let border_x: i32 = 200;
        let mut checked_any = false;
        // Derive the cell band from the border so this holds at any tree density:
        // only a trunk in the cell containing the border (or the next one over)
        // can be right of the border yet within reach of it.
        let border_cell_x = border_x.div_euclid(TREE_CELL);
        for cell_z in -20..20 {
            for cell_x in border_cell_x..=(border_cell_x + 1) {
                let Some(tree) = tree_candidate(&source, cell_x, cell_z) else {
                    continue;
                };
                if tree.trunk_x <= border_x || tree.trunk_x - MAX_TREE_REACH >= border_x {
                    continue; // want the trunk to the right of the border, canopy spilling left
                }
                // Window strictly LEFT of the trunk (border on its right edge):
                // it contains no trunk but may catch spilled leaves.
                let left = source.unpack_chunk(
                    border_x - 40,
                    border_x,
                    tree.trunk_z - 20,
                    tree.trunk_z + 20,
                );
                // Window that DOES contain the trunk.
                let right = source.unpack_chunk(
                    border_x,
                    tree.trunk_x + 30,
                    tree.trunk_z - 20,
                    tree.trunk_z + 20,
                );
                for world_z in (tree.trunk_z - 20)..(tree.trunk_z + 20) {
                    for world_x in (border_x - 1)..border_x {
                        for y in 0..WORLD_SIZE_Y as i32 {
                            assert_eq!(
                                left.get(world_x, y, world_z),
                                right.get(world_x, y, world_z),
                                "canopy-spill seam at ({world_x},{y},{world_z})"
                            );
                        }
                    }
                }
                checked_any = true;
            }
        }
        assert!(
            checked_any,
            "test did not find a suitable across-border tree to check"
        );
    }

    #[test]
    fn land_water_trees_and_cover_all_appear() {
        let source = StreamedSource::new(1);
        let mut has_grass = false;
        let mut has_stone = false;
        let mut has_water = false;
        let mut has_leaves = false;
        let mut has_trunk = false;
        let mut has_tall_grass = false;
        // A wide sampled region so hills, water, forests, and meadows all land.
        let scratch = source.unpack_chunk(-48, 96, -48, 96);
        for world_z in -48..96 {
            for world_x in -48..96 {
                for y in 0..WORLD_SIZE_Y as i32 {
                    match scratch.get(world_x, y, world_z) {
                        Voxel::Grass => has_grass = true,
                        Voxel::Stone => has_stone = true,
                        Voxel::Water => has_water = true,
                        Voxel::Leaves
                        | Voxel::LeavesDark
                        | Voxel::LeavesBirch
                        | Voxel::LeavesPine => has_leaves = true,
                        Voxel::Trunk | Voxel::TrunkBirch => has_trunk = true,
                        Voxel::TallGrass => has_tall_grass = true,
                        _ => {}
                    }
                }
            }
        }
        assert!(has_grass, "expected grass");
        assert!(has_stone, "expected stone");
        assert!(has_water, "expected water");
        assert!(has_leaves, "expected tree canopy");
        assert!(has_tall_grass, "expected tall-grass cover");

        // Trees are sparse (see `TREE_CELL`), so a fixed sample window can
        // legitimately contain spilled canopy but no trunk — as this one does.
        // Target a real tree instead: take the first candidate the placement grid
        // yields, then assert its window actually stamps the trunk.
        let mut planted: Option<TreeCandidate> = None;
        'search: for cell_z in -8..8 {
            for cell_x in -8..8 {
                if let Some(tree) = tree_candidate(&source, cell_x, cell_z) {
                    planted = Some(tree);
                    break 'search;
                }
            }
        }
        let tree = planted.expect("the placement grid should yield a tree somewhere near origin");
        let around_tree = source.unpack_chunk(
            tree.trunk_x - 24,
            tree.trunk_x + 24,
            tree.trunk_z - 24,
            tree.trunk_z + 24,
        );
        for y in 0..WORLD_SIZE_Y as i32 {
            if matches!(
                around_tree.get(tree.trunk_x, y, tree.trunk_z),
                Voxel::Trunk | Voxel::TrunkBirch
            ) {
                has_trunk = true;
            }
        }
        assert!(
            has_trunk,
            "a planted tree should stamp a trunk at ({}, {})",
            tree.trunk_x, tree.trunk_z
        );
    }

    #[test]
    fn columns_are_solid_to_bedrock() {
        // Infinite terrain fills from bedrock, so the bottom voxel of every
        // column is solid.
        let source = StreamedSource::new(5);
        let scratch = source.unpack_chunk(0, 32, 0, 32);
        for world_z in 0..32 {
            for world_x in 0..32 {
                assert!(
                    scratch.get(world_x, 0, world_z).is_solid(),
                    "expected bedrock at ({world_x},{world_z})"
                );
            }
        }
    }

    #[test]
    fn color_contexts_are_in_range() {
        let source = StreamedSource::new(9);
        let mut saw_a_tone = false;
        // Step across a moderate region: the range checks hold everywhere, and
        // a coarse grid still catches tree/bush tones without the full-density
        // per-column water-distance cost.
        for world_z in (-48..112).step_by(2) {
            for world_x in (-48..112).step_by(2) {
                let dryness = source.dryness_at(world_x, world_z);
                assert!(
                    (0.0..=1.0).contains(&dryness),
                    "dryness out of range at ({world_x},{world_z}): {dryness}"
                );
                let cover = source.cover_at(world_x, world_z);
                assert!(
                    (0.0..=1.0).contains(&cover),
                    "cover out of range at ({world_x},{world_z}): {cover}"
                );
                let water_distance = source.water_distance_at(world_x, world_z);
                assert!(
                    water_distance >= 0.0,
                    "negative water distance at ({world_x},{world_z})"
                );
                let tone = source.tree_tone_at(world_x, world_z);
                assert!(
                    (0.0..=1.0).contains(&tone),
                    "tree tone out of range at ({world_x},{world_z}): {tone}"
                );
                if tone != 0.5 {
                    saw_a_tone = true;
                }
            }
        }
        assert!(
            saw_a_tone,
            "expected at least one tree/bush tone away from the 0.5 default"
        );
    }

    #[test]
    fn bare_columns_report_the_default_tone() {
        // A column far from any tree cell must report the 0.5 "no tree" tone.
        // Scan for one rather than assuming a fixed coordinate is bare.
        let source = StreamedSource::new(2);
        let mut found_bare = false;
        for world_z in 0..60 {
            for world_x in 0..60 {
                if source.tree_tone_at(world_x, world_z) == 0.5 {
                    found_bare = true;
                }
            }
        }
        assert!(found_bare, "expected some columns with no tree (tone 0.5)");
    }

    #[test]
    fn water_distance_field_matches_the_brute_force_search() {
        // The tiled distance transform replaced a per-column radius-24 search.
        // It must return the *identical* float for every column — these values
        // pick beach sand, tree plantability, and the shoreline tint, so any
        // drift would move the world. Sampled across seeds and across tile
        // borders (including negative coordinates, where the tile indexing
        // floor-divides), with both the "water in reach" and the "nothing in
        // reach" (f32::MAX) branches required to show up.
        let mut saw_water_in_reach = false;
        let mut saw_nothing_in_reach = false;

        // Dense: every column of a whole chunk window plus its apron — the exact
        // set a streamed chunk reads.
        let dense_source = StreamedSource::new(7);
        for world_z in -1..=64 {
            for world_x in -1..=64 {
                let fast = dense_source.water_distance_at(world_x, world_z);
                let reference = water_distance_at(world_x, world_z, dense_source.seed);
                assert_eq!(
                    fast, reference,
                    "water distance drifted at ({world_x},{world_z})"
                );
            }
        }

        // Sampled: other seeds, farther out, and across tile borders in the
        // negative quadrant where the tile index floor-divides.
        for seed in [1_u32, 42] {
            let source = StreamedSource::new(seed);
            for world_z in (-70..134).step_by(3) {
                for world_x in (-70..134).step_by(3) {
                    let fast = source.water_distance_at(world_x, world_z);
                    let reference = water_distance_at(world_x, world_z, seed);
                    assert_eq!(
                        fast, reference,
                        "water distance drifted at ({world_x},{world_z}) seed {seed}"
                    );
                    if reference == f32::MAX {
                        saw_nothing_in_reach = true;
                    } else {
                        saw_water_in_reach = true;
                    }
                }
            }
        }
        assert!(
            saw_water_in_reach,
            "no column found water within the radius"
        );
        assert!(
            saw_nothing_in_reach,
            "no column exercised the radius cap (f32::MAX)"
        );
    }

    #[test]
    fn cached_slope_matches_the_reference_window() {
        // Slope now reads the tile's cached heights instead of re-evaluating the
        // terrain noise 25 times; same window, same min/max, same value —
        // including at tile edges, where the window reaches into the halo.
        let source = StreamedSource::new(13);
        // Dense over a whole chunk window plus apron, so every height index the
        // window touches is checked, not just a sample.
        for world_z in -1..=64 {
            for world_x in -1..=64 {
                assert_eq!(
                    source.slope(world_x, world_z),
                    slope_at(world_x, world_z, source.seed),
                    "slope drifted at ({world_x},{world_z})"
                );
                assert_eq!(
                    source.height(world_x, world_z),
                    terrain_column_height(world_x, world_z, source.seed),
                    "height drifted at ({world_x},{world_z})"
                );
            }
        }
        // Far tiles, including negative coordinates and tile borders.
        for world_z in [-193, -65, -64, -63, 63, 64, 65, 191] {
            for world_x in [-193, -65, -64, -63, 63, 64, 65, 191] {
                assert_eq!(
                    source.slope(world_x, world_z),
                    slope_at(world_x, world_z, source.seed),
                    "slope drifted at ({world_x},{world_z})"
                );
            }
        }
    }

    #[test]
    fn color_contexts_are_order_independent() {
        // Memoization must not let query order (or which instance answers) leak
        // into a value. One source is asked in row-major order, a second in
        // reverse order, a third interleaved with unrelated far-away probes that
        // populate its caches first.
        let region: Vec<(i32, i32)> = (-40..72)
            .step_by(7)
            .flat_map(|world_z| (-40..72).step_by(7).map(move |world_x| (world_x, world_z)))
            .collect();

        let forward = StreamedSource::new(23);
        let reverse = StreamedSource::new(23);
        let interleaved = StreamedSource::new(23);

        let contexts = |source: &StreamedSource, (world_x, world_z): (i32, i32)| {
            (
                source.dryness_at(world_x, world_z),
                source.cover_at(world_x, world_z),
                source.water_distance_at(world_x, world_z),
                source.tree_tone_at(world_x, world_z),
            )
        };

        let forward_values: Vec<_> = region
            .iter()
            .map(|&column| contexts(&forward, column))
            .collect();

        let mut reverse_values: Vec<_> = region
            .iter()
            .rev()
            .map(|&column| contexts(&reverse, column))
            .collect();
        reverse_values.reverse();

        // Warm the third source's caches from elsewhere in the world first, then
        // ask the same region.
        for &(world_x, world_z) in &[(900, -900), (-31, 205), (64, 64)] {
            contexts(&interleaved, (world_x, world_z));
        }
        let interleaved_values: Vec<_> = region
            .iter()
            .map(|&column| contexts(&interleaved, column))
            .collect();

        assert_eq!(
            forward_values, reverse_values,
            "reverse-order queries disagreed with row-major ones"
        );
        assert_eq!(
            forward_values, interleaved_values,
            "a pre-warmed cache changed the answers"
        );
        // Re-reading the first source must also be stable (pure memo, not a
        // mutating accumulator).
        let repeat_values: Vec<_> = region
            .iter()
            .map(|&column| contexts(&forward, column))
            .collect();
        assert_eq!(forward_values, repeat_values, "repeat reads drifted");
    }

    #[test]
    fn world_offset_is_uncentered() {
        assert_eq!(StreamedSource::new(1).world_offset(), (0.0, 0.0));
    }

    /// How many color-context reads the timing probe makes, matching the order
    /// of magnitude the greedy mesher makes per chunk (roughly one per emitted
    /// face, mostly on repeated columns).
    const COLOR_CONTEXT_CALLS: usize = 50_000;

    /// Times one chunk's worth of work the way the mesher actually asks for it:
    /// a single 64×64 window unpack, then tens of thousands of per-column color
    /// reads over that window. Prints the split with `--nocapture`; it asserts
    /// nothing about wall time (that would be flaky on shared machines), it is
    /// the probe used to compare generations of this module.
    #[test]
    fn one_chunk_of_work_is_fast() {
        let source = StreamedSource::new(11);

        let unpack_start = std::time::Instant::now();
        let scratch = source.unpack_chunk(0, 64, 0, 64);
        let unpack_elapsed = unpack_start.elapsed();

        let color_start = std::time::Instant::now();
        let mut checksum = 0.0_f32;
        for call_index in 0..COLOR_CONTEXT_CALLS {
            // Walk the window in a stride that revisits columns, exactly the
            // repeated-column pattern the mesher produces.
            let world_x = (call_index * 7 % 64) as i32;
            let world_z = (call_index * 13 / 64 % 64) as i32;
            checksum += source.dryness_at(world_x, world_z);
            checksum += source.cover_at(world_x, world_z);
            checksum += source.water_distance_at(world_x, world_z).min(1_000.0);
            checksum += source.tree_tone_at(world_x, world_z);
        }
        let color_elapsed = color_start.elapsed();

        println!(
            "one chunk: unpack_chunk {:?}, {COLOR_CONTEXT_CALLS} color reads {:?}, total {:?}",
            unpack_elapsed,
            color_elapsed,
            unpack_elapsed + color_elapsed
        );
        assert!(checksum.is_finite(), "timing probe produced a broken sum");
        assert!(scratch.get(0, 0, 0).is_solid(), "expected bedrock");
    }
}
