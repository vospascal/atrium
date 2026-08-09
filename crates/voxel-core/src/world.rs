//! One-metre voxel world authority.
//!
//! Gameplay, generation, destruction and construction operate on a
//! `125 × 64 × 125` lattice of indivisible one-metre voxels. Renderers may
//! represent one world voxel with an `8 × 8 × 8` detail tile, but that tile is
//! an implementation detail used only by authored assets and material detail.
//! Procedural terrain never writes individual detail cells.

use crate::cave::CellularFieldCache;
use crate::chunk::{ChunkCoord, ChunkGenerator};
use crate::noise::{fractal_noise_2d, smoothstep};

/// Physical world dimensions and ordinary gameplay-voxel counts.
pub const WORLD_VOXELS_X: usize = 125;
pub const WORLD_VOXELS_Y: usize = 64;
pub const WORLD_VOXELS_Z: usize = 125;
pub const WORLD_VOXEL_SIZE_METERS: f32 = 1.0;

/// Optional detail resolution inside one world voxel.
pub const DETAIL_CELLS_PER_WORLD_VOXEL: usize = 8;
pub const DETAIL_CELL_SIZE_METERS: f32 =
    WORLD_VOXEL_SIZE_METERS / DETAIL_CELLS_PER_WORLD_VOXEL as f32;

/// Renderer detail-grid dimensions. These remain the dimensions consumed by
/// the existing brickmap/DDA implementation: each 8³ brick is one world voxel.
pub const DETAIL_GRID_SIZE_X: usize = WORLD_VOXELS_X * DETAIL_CELLS_PER_WORLD_VOXEL;
pub const DETAIL_GRID_SIZE_Y: usize = WORLD_VOXELS_Y * DETAIL_CELLS_PER_WORLD_VOXEL;
pub const DETAIL_GRID_SIZE_Z: usize = WORLD_VOXELS_Z * DETAIL_CELLS_PER_WORLD_VOXEL;

/// Compatibility names for renderer code that still works in detail-cell
/// coordinates. New world/gameplay APIs must use `WORLD_VOXELS_*` instead.
pub const WORLD_SIZE_X: usize = DETAIL_GRID_SIZE_X;
pub const WORLD_SIZE_Y: usize = DETAIL_GRID_SIZE_Y;
pub const WORLD_SIZE_Z: usize = DETAIL_GRID_SIZE_Z;
pub const VOXEL_SIZE: f32 = DETAIL_CELL_SIZE_METERS;

/// Water occupies complete world voxels through this logical Y coordinate.
pub const WATER_LEVEL_WORLD: i32 = 32;
/// Last detail cell belonging to [`WATER_LEVEL_WORLD`].
pub const WATER_LEVEL: i32 = (WATER_LEVEL_WORLD + 1) * DETAIL_CELLS_PER_WORLD_VOXEL as i32 - 1;

/// The floating island's rim underside in world-voxel coordinates.
pub const PLATEAU_FLOOR_WORLD: i32 = 4;
/// First detail layer at that logical height.
pub const PLATEAU_FLOOR: i32 = PLATEAU_FLOOR_WORLD * DETAIL_CELLS_PER_WORLD_VOXEL as i32;

const NO_LAND: i32 = -1;

/// Coordinate in the authoritative one-metre world lattice.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Hash)]
pub struct WorldVoxelCoord {
    pub x: i32,
    pub y: i32,
    pub z: i32,
}

impl WorldVoxelCoord {
    pub const fn new(x: i32, y: i32, z: i32) -> Self {
        Self { x, y, z }
    }

    pub const fn is_in_bounds(self) -> bool {
        self.x >= 0
            && self.y >= 0
            && self.z >= 0
            && self.x < WORLD_VOXELS_X as i32
            && self.y < WORLD_VOXELS_Y as i32
            && self.z < WORLD_VOXELS_Z as i32
    }

    /// Minimum detail-cell coordinate of this one-metre voxel.
    pub const fn detail_origin(self) -> [i32; 3] {
        let scale = DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        [self.x * scale, self.y * scale, self.z * scale]
    }

    pub const fn from_detail_cell(detail: [i32; 3]) -> Self {
        let scale = DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        Self::new(
            detail[0].div_euclid(scale),
            detail[1].div_euclid(scale),
            detail[2].div_euclid(scale),
        )
    }
}

/// Material/content stored by a world voxel or asset detail cell.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Voxel {
    Air,
    Grass,
    TallGrass,
    Dirt,
    Sand,
    Sediment,
    Stone,
    Water,
    Trunk,
    TrunkBirch,
    Leaves,
    LeavesDark,
    LeavesBirch,
    LeavesPine,
    FlowerPink,
    FlowerWhite,
    FlowerYellow,
    FlowerBlue,
    WaterWeed,
    LilyPad,
    LilyBloom,
    Reed,
    CattailHead,
    Snow,
    GlowBlock,
    GlowBerry,
    Lava,
    /// Cut slate blocks in a running bond. The demonstration material for the
    /// tessellation: it is the only row whose look comes from WHERE the tiles are
    /// rather than from a field sampled across a surface.
    SlateTile,
    // ---- Output-depth test targets, not scenery -------------------------------
    //
    // Three placeable surfaces whose only job is to make the frame buffer's bit
    // depth visible to the eye — see `docs/output-depth.md`. They are ordinary
    // solid voxels so the editor, the physics and the mesher need no special case;
    // what makes them diagnostic is the pattern stack on their material rows, not
    // anything here.
    HdrRed,
    HdrGreen,
    HdrBlue,
    HdrCyan,
    HdrMagenta,
    HdrYellow,
    // ---- Indirect-light test reflectors, not scenery --------------------------
    //
    // Saturated LAMBERTIAN surfaces: high albedo in their own channels, no
    // emission, no patterns. They exist because the `Hdr*` rows above look like
    // they would serve the same purpose and do the opposite — those are pure
    // emitters with ZERO albedo, so a room built from them cannot bounce light
    // at all. The L0 light fixture needs surfaces whose *reflectance* is known
    // and saturated, which is what these are.
    //
    // See `voxel-rt/src/light_fixture.rs`.
    AlbedoRed,
    AlbedoGreen,
    AlbedoBlue,
    AlbedoCyan,
    AlbedoMagenta,
    AlbedoYellow,
    // ---- Cave decoration, scattered by generation pass two -------------------
    //
    // Deliberately crude rows: this arc's work is that the PLACEMENT is authored
    // and deterministic, so the look can be re-authored in Studio later without
    // touching the generator. `CaveCrystal` is the exception that earns its
    // emission — it is what makes a cave readable without a torch.
    CaveMoss,
    CaveMushroom,
    CaveVine,
    CaveCrystal,
    /// The thrown flare's own material. It exists so a flare stops masquerading
    /// as [`Voxel::Lava`]: sharing that row meant the eyedropper picked "lava"
    /// off a flare and any retune of molten rock restyled every flare with it.
    FlareEmber,
}

impl Voxel {
    /// Every voxel kind, in discriminant order.
    ///
    /// Needed by anything that has to present a distribution over voxels — the
    /// generation window's material composition, for one — because a count keyed
    /// by discriminant is otherwise a number nobody can name. A test asserts this
    /// list is complete and in order, so a new variant that is not added here
    /// fails rather than silently vanishing from every readout.
    pub const ALL: [Self; 45] = [
        Self::Air,
        Self::Grass,
        Self::TallGrass,
        Self::Dirt,
        Self::Sand,
        Self::Sediment,
        Self::Stone,
        Self::Water,
        Self::Trunk,
        Self::TrunkBirch,
        Self::Leaves,
        Self::LeavesDark,
        Self::LeavesBirch,
        Self::LeavesPine,
        Self::FlowerPink,
        Self::FlowerWhite,
        Self::FlowerYellow,
        Self::FlowerBlue,
        Self::WaterWeed,
        Self::LilyPad,
        Self::LilyBloom,
        Self::Reed,
        Self::CattailHead,
        Self::Snow,
        Self::GlowBlock,
        Self::GlowBerry,
        Self::Lava,
        Self::SlateTile,
        Self::HdrRed,
        Self::HdrGreen,
        Self::HdrBlue,
        Self::HdrCyan,
        Self::HdrMagenta,
        Self::HdrYellow,
        Self::AlbedoRed,
        Self::AlbedoGreen,
        Self::AlbedoBlue,
        Self::AlbedoCyan,
        Self::AlbedoMagenta,
        Self::AlbedoYellow,
        Self::CaveMoss,
        Self::CaveMushroom,
        Self::CaveVine,
        Self::CaveCrystal,
        Self::FlareEmber,
    ];

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
                | Voxel::GlowBerry
                | Voxel::CaveMoss
                | Voxel::CaveMushroom
                | Voxel::CaveVine
        )
    }

    /// Whether this voxel is decoration placed by generation pass two.
    ///
    /// Pass two must be able to recognise its own output, so re-running it over a
    /// world it already decorated replaces cover rather than stacking on it.
    pub fn is_scatter_cover(self) -> bool {
        matches!(
            self,
            Voxel::TallGrass
                | Voxel::FlowerPink
                | Voxel::FlowerWhite
                | Voxel::FlowerYellow
                | Voxel::FlowerBlue
                | Voxel::CaveMoss
                | Voxel::CaveMushroom
                | Voxel::CaveVine
                | Voxel::CaveCrystal
        )
    }

    /// Whether a ray and a body both pass through this voxel, i.e. it is air or a
    /// liquid. The generator's cave passes use this to decide what counts as an
    /// open void, so water already placed in a cave does not read as rock.
    pub fn is_open_space(self) -> bool {
        matches!(self, Voxel::Air | Voxel::Water | Voxel::Lava)
    }
}

/// One vertical run in renderer detail-cell coordinates.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Run {
    pub voxel: Voxel,
    pub length: u16,
}

/// Compact finished world. The authority is generated at one-metre resolution;
/// runs are expanded to detail-cell lengths only at this renderer boundary.
pub struct VoxelWorld {
    runs: Vec<Run>,
    column_starts: Vec<u32>,
    dryness: Vec<f32>,
    water_distance: Vec<f32>,
}

impl VoxelWorld {
    pub fn generate(seed: u32, _season: f32) -> Self {
        let heights = generated_heightmap(seed);
        Self::from_heightmap(&heights, seed)
    }

    /// Build the renderer-sized region from a complete generator configuration.
    ///
    /// One constructor rather than the three stacked wrappers this replaced. Each
    /// of those existed only to fill in a default the next one down needed, so
    /// every new authored control — `surface_headroom` was the last one — meant
    /// editing three signatures in lockstep. Taking the configuration itself means
    /// the terrain shape, cave, fluid and scatter programs arrive together and
    /// adding the next one changes nothing here.
    pub fn from_chunk_generator(generator: &ChunkGenerator) -> Self {
        Self::from_chunk_generator_reporting(generator).0
    }

    /// Build the region and report what generation produced.
    ///
    /// The stats are measured during generation, so the debug window shows the
    /// world that exists rather than a second pass over it that could disagree.
    pub fn from_chunk_generator_reporting(
        generator: &ChunkGenerator,
    ) -> (
        Self,
        crate::chunk::GenerationStats,
        crate::stage_report::StageReport,
    ) {
        let generator = ChunkGenerator {
            height: WORLD_VOXELS_Y,
            ..generator.clone()
        };
        // One field cache for the whole region: the cellular field is a function
        // of (region, band), so all 64 chunks of this world share four of them.
        let mut cache = CellularFieldCache::new();
        let mut chunks = std::collections::BTreeMap::new();
        let mut blocks = vec![Voxel::Air; WORLD_VOXELS_X * WORLD_VOXELS_Y * WORLD_VOXELS_Z];
        for z in 0..WORLD_VOXELS_Z {
            for x in 0..WORLD_VOXELS_X {
                let coordinate = WorldVoxelCoord::new(x as i32, 0, z as i32);
                let chunk_coord = ChunkCoord::from_world(coordinate.x, coordinate.z);
                let chunk = chunks
                    .entry(chunk_coord)
                    .or_insert_with(|| generator.generate_with_cache(chunk_coord, &mut cache));
                for y in 0..WORLD_VOXELS_Y {
                    blocks[world_index(x, y, z)] =
                        chunk.world_voxel(coordinate.x, y as i32, coordinate.z);
                }
            }
        }
        let mut stats = crate::chunk::GenerationStats::new();
        let mut report = crate::stage_report::StageReport::new(generator.min_y, generator.height);
        for chunk in chunks.values() {
            stats.merge(chunk.stats());
            report.merge(chunk.stage_report());
        }
        (
            Self::from_world_voxels(
                &blocks,
                vec![0.5; WORLD_VOXELS_X * WORLD_VOXELS_Z],
                vec![f32::MAX; WORLD_VOXELS_X * WORLD_VOXELS_Z],
            ),
            stats,
            report,
        )
    }

    /// Build a default-shaped region for one seed, with caves on or off.
    ///
    /// The convenience the tests and the cave-less world path want; anything that
    /// authors terrain shape, fluids or scatter goes through
    /// [`Self::from_chunk_generator`] instead.
    pub fn generate_chunked(world_seed: u64, caves_enabled: bool) -> Self {
        Self::from_chunk_generator(&ChunkGenerator {
            world_seed,
            caves_enabled,
            ..ChunkGenerator::default()
        })
    }

    /// Imported heights are metres relative to the water plane and are sampled
    /// once per one-metre world column. Authored tree points are intentionally
    /// not stamped into terrain: trees are assets in the new model.
    pub fn from_imported(
        terrain: &crate::terrain_import::ImportedTerrain,
        seed: u32,
        _season: f32,
    ) -> Self {
        let mut heights = vec![NO_LAND; WORLD_VOXELS_X * WORLD_VOXELS_Z];
        for z in 0..WORLD_VOXELS_Z {
            for x in 0..WORLD_VOXELS_X {
                let u = x as f32 / (WORLD_VOXELS_X - 1) as f32;
                let v = z as f32 / (WORLD_VOXELS_Z - 1) as f32;
                let height_meters = terrain.sample_height(u, v);
                if height_meters.is_nan() {
                    continue;
                }
                heights[z * WORLD_VOXELS_X + x] =
                    (WATER_LEVEL_WORLD as f32 + height_meters).round() as i32;
            }
        }
        Self::from_heightmap(&heights, seed)
    }

    fn from_heightmap(heights: &[i32], seed: u32) -> Self {
        assert_eq!(heights.len(), WORLD_VOXELS_X * WORLD_VOXELS_Z);
        let slopes = slope_map(heights);
        let water_distance_world = water_distance_map(heights);
        let mut blocks = vec![Voxel::Air; WORLD_VOXELS_X * WORLD_VOXELS_Y * WORLD_VOXELS_Z];
        let mut dryness = vec![0.0; WORLD_VOXELS_X * WORLD_VOXELS_Z];

        for z in 0..WORLD_VOXELS_Z {
            for x in 0..WORLD_VOXELS_X {
                let column = z * WORLD_VOXELS_X + x;
                let top = heights[column];
                let dry_noise = fractal_noise_2d(x as f32 * 0.035, z as f32 * 0.035, 3, seed + 91);
                dryness[column] = ((x as f32 / (WORLD_VOXELS_X - 1) as f32) * 0.7
                    + dry_noise * 0.3)
                    .clamp(0.0, 1.0);
                if top == NO_LAND {
                    continue;
                }

                let centre_x = WORLD_VOXELS_X as f32 * 0.5;
                let centre_z = WORLD_VOXELS_Z as f32 * 0.5;
                let radial = ((x as f32 - centre_x).powi(2) + (z as f32 - centre_z).powi(2)).sqrt()
                    / (WORLD_VOXELS_X.min(WORLD_VOXELS_Z) as f32 * 0.5);
                let underside = (PLATEAU_FLOOR_WORLD as f32 - (1.0 - radial).clamp(0.0, 1.0) * 4.0)
                    .round() as i32;
                let underside = underside.clamp(1, top);
                let altitude = top - WATER_LEVEL_WORLD;
                let cap = if top < WATER_LEVEL_WORLD {
                    if WATER_LEVEL_WORLD - top <= 1 {
                        Voxel::Sand
                    } else {
                        Voxel::Sediment
                    }
                } else if altitude >= 14 && slopes[column] <= 1.0 {
                    Voxel::Snow
                } else if slopes[column] >= 1.25 || altitude >= 11 {
                    Voxel::Stone
                } else if water_distance_world[column] <= 2.0 && altitude <= 1 {
                    Voxel::Sand
                } else {
                    Voxel::Grass
                };
                let subsoil = match cap {
                    Voxel::Grass => Voxel::Dirt,
                    Voxel::Sand => Voxel::Sand,
                    Voxel::Sediment => Voxel::Sediment,
                    _ => Voxel::Stone,
                };

                for y in underside..=top.min(WORLD_VOXELS_Y as i32 - 1) {
                    let material = if y == top {
                        cap
                    } else if y >= top - 2 {
                        subsoil
                    } else {
                        Voxel::Stone
                    };
                    blocks[world_index(x, y as usize, z)] = material;
                }
                for y in (top + 1).max(0)..=WATER_LEVEL_WORLD.min(WORLD_VOXELS_Y as i32 - 1) {
                    blocks[world_index(x, y as usize, z)] = Voxel::Water;
                }
            }
        }

        Self::from_world_voxels(&blocks, dryness, water_distance_world)
    }

    fn from_world_voxels(blocks: &[Voxel], dryness: Vec<f32>, water_distance: Vec<f32>) -> Self {
        assert_eq!(
            blocks.len(),
            WORLD_VOXELS_X * WORLD_VOXELS_Y * WORLD_VOXELS_Z
        );
        let detail_columns = DETAIL_GRID_SIZE_X * DETAIL_GRID_SIZE_Z;
        let mut runs: Vec<Run> = Vec::with_capacity(detail_columns * 6);
        let mut column_starts = Vec::with_capacity(detail_columns + 1);
        let scale = DETAIL_CELLS_PER_WORLD_VOXEL as u16;

        for detail_z in 0..DETAIL_GRID_SIZE_Z {
            let world_z = detail_z / DETAIL_CELLS_PER_WORLD_VOXEL;
            for detail_x in 0..DETAIL_GRID_SIZE_X {
                let world_x = detail_x / DETAIL_CELLS_PER_WORLD_VOXEL;
                let column_start = runs.len();
                column_starts.push(runs.len() as u32);
                for world_y in 0..WORLD_VOXELS_Y {
                    let voxel = blocks[world_index(world_x, world_y, world_z)];
                    if runs.len() > column_start {
                        let previous = runs.last_mut().expect("column has a preceding run");
                        if previous.voxel == voxel {
                            previous.length += scale;
                            continue;
                        }
                    }
                    runs.push(Run {
                        voxel,
                        length: scale,
                    });
                }
            }
        }
        column_starts.push(runs.len() as u32);
        Self {
            runs,
            column_starts,
            dryness,
            water_distance,
        }
    }

    /// Material at one renderer detail cell.
    pub fn get(&self, x: i32, y: i32, z: i32) -> Voxel {
        if x < 0
            || y < 0
            || z < 0
            || x >= DETAIL_GRID_SIZE_X as i32
            || y >= DETAIL_GRID_SIZE_Y as i32
            || z >= DETAIL_GRID_SIZE_Z as i32
        {
            return Voxel::Air;
        }
        self.column_runs(x, z)
            .find_map(|(voxel, start, length)| (y >= start && y < start + length).then_some(voxel))
            .unwrap_or(Voxel::Air)
    }

    /// Material of an authoritative one-metre world voxel.
    pub fn world_voxel(&self, coordinate: WorldVoxelCoord) -> Voxel {
        if !coordinate.is_in_bounds() {
            return Voxel::Air;
        }
        let origin = coordinate.detail_origin();
        self.get(origin[0], origin[1], origin[2])
    }

    pub fn column_runs(&self, x: i32, z: i32) -> impl Iterator<Item = (Voxel, i32, i32)> + '_ {
        let range =
            if x >= 0 && z >= 0 && x < DETAIL_GRID_SIZE_X as i32 && z < DETAIL_GRID_SIZE_Z as i32 {
                let column = z as usize * DETAIL_GRID_SIZE_X + x as usize;
                self.column_starts[column] as usize..self.column_starts[column + 1] as usize
            } else {
                0..0
            };
        self.runs[range].iter().scan(0_i32, |cursor, run| {
            let start = *cursor;
            let length = i32::from(run.length);
            *cursor += length;
            Some((run.voxel, start, length))
        })
    }

    pub fn dryness_at(&self, detail_x: i32, detail_z: i32) -> f32 {
        let Some(column) = world_column_from_detail(detail_x, detail_z) else {
            return 0.0;
        };
        self.dryness[column]
    }

    pub fn cover_at(&self, _detail_x: i32, _detail_z: i32) -> f32 {
        0.0
    }

    pub fn water_distance_at(&self, detail_x: i32, detail_z: i32) -> f32 {
        let Some(column) = world_column_from_detail(detail_x, detail_z) else {
            return f32::MAX;
        };
        self.water_distance[column]
    }

    pub fn tree_tone_at(&self, _detail_x: i32, _detail_z: i32) -> f32 {
        0.5
    }

    pub fn memory_stats(&self) -> (usize, usize) {
        let bytes = self.runs.len() * std::mem::size_of::<Run>()
            + self.column_starts.len() * std::mem::size_of::<u32>();
        (self.runs.len(), bytes)
    }
}

fn world_index(x: usize, y: usize, z: usize) -> usize {
    (y * WORLD_VOXELS_Z + z) * WORLD_VOXELS_X + x
}

fn world_column_from_detail(detail_x: i32, detail_z: i32) -> Option<usize> {
    if detail_x < 0
        || detail_z < 0
        || detail_x >= DETAIL_GRID_SIZE_X as i32
        || detail_z >= DETAIL_GRID_SIZE_Z as i32
    {
        return None;
    }
    let x = detail_x as usize / DETAIL_CELLS_PER_WORLD_VOXEL;
    let z = detail_z as usize / DETAIL_CELLS_PER_WORLD_VOXEL;
    Some(z * WORLD_VOXELS_X + x)
}

fn generated_heightmap(seed: u32) -> Vec<i32> {
    let mut heights = vec![NO_LAND; WORLD_VOXELS_X * WORLD_VOXELS_Z];
    let centre_x = (WORLD_VOXELS_X - 1) as f32 * 0.5;
    let centre_z = (WORLD_VOXELS_Z - 1) as f32 * 0.5;
    for z in 0..WORLD_VOXELS_Z {
        for x in 0..WORLD_VOXELS_X {
            let world_x = x as f32 - centre_x;
            let world_z = z as f32 - centre_z;
            let radial = (world_x * world_x + world_z * world_z).sqrt();
            let rim_noise = fractal_noise_2d(x as f32 * 0.045, z as f32 * 0.045, 3, seed + 17);
            let radius = 45.0 + (rim_noise - 0.5) * 10.0;
            if radial > radius {
                continue;
            }
            let broad = fractal_noise_2d(x as f32 * 0.025, z as f32 * 0.025, 5, seed);
            let detail = fractal_noise_2d(x as f32 * 0.085, z as f32 * 0.085, 3, seed + 41);
            let rim_falloff = smoothstep(radius, radius * 0.72, radial);
            let altitude = 12.0 + broad * 8.0 + detail * 3.0 + rim_falloff * 2.0;
            let mut top = WATER_LEVEL_WORLD + altitude.round() as i32;

            // A block-wide winding river provides water without returning to
            // globally sculpted 0.125 m terrain.
            let river_z = (world_x * 0.10).sin() * 7.0;
            let river_width = 1.25 + fractal_noise_2d(x as f32 * 0.08, 9.0, 2, seed + 73) * 1.5;
            if (world_z - river_z).abs() < river_width && radial < radius * 0.82 {
                top = WATER_LEVEL_WORLD - 2;
            }
            heights[z * WORLD_VOXELS_X + x] =
                top.clamp(PLATEAU_FLOOR_WORLD, WORLD_VOXELS_Y as i32 - 11);
        }
    }
    heights
}

fn slope_map(heights: &[i32]) -> Vec<f32> {
    let mut slopes = vec![0.0; heights.len()];
    for z in 0..WORLD_VOXELS_Z {
        for x in 0..WORLD_VOXELS_X {
            let centre = heights[z * WORLD_VOXELS_X + x];
            if centre == NO_LAND {
                continue;
            }
            let mut rise = 0_i32;
            for (sample_x, sample_z) in [
                (x.saturating_sub(1), z),
                ((x + 1).min(WORLD_VOXELS_X - 1), z),
                (x, z.saturating_sub(1)),
                (x, (z + 1).min(WORLD_VOXELS_Z - 1)),
            ] {
                let neighbor = heights[sample_z * WORLD_VOXELS_X + sample_x];
                if neighbor != NO_LAND {
                    rise = rise.max((centre - neighbor).abs());
                }
            }
            slopes[z * WORLD_VOXELS_X + x] = rise as f32;
        }
    }
    slopes
}

fn water_distance_map(heights: &[i32]) -> Vec<f32> {
    let water_columns: Vec<(i32, i32)> = heights
        .iter()
        .enumerate()
        .filter_map(|(index, height)| {
            (*height != NO_LAND && *height < WATER_LEVEL_WORLD).then_some((
                (index % WORLD_VOXELS_X) as i32,
                (index / WORLD_VOXELS_X) as i32,
            ))
        })
        .collect();
    heights
        .iter()
        .enumerate()
        .map(|(index, height)| {
            if *height == NO_LAND || water_columns.is_empty() {
                return f32::MAX;
            }
            let x = (index % WORLD_VOXELS_X) as i32;
            let z = (index / WORLD_VOXELS_X) as i32;
            water_columns
                .iter()
                .map(|(water_x, water_z)| {
                    let dx = x - water_x;
                    let dz = z - water_z;
                    (dx * dx + dz * dz) as f32
                })
                .fold(f32::MAX, f32::min)
                .sqrt()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn physical_world_is_125_by_64_by_125_metres() {
        assert_eq!(WORLD_VOXELS_X, 125);
        assert_eq!(WORLD_VOXELS_Y, 64);
        assert_eq!(WORLD_VOXELS_Z, 125);
        assert_eq!(WORLD_VOXEL_SIZE_METERS, 1.0);
        assert_eq!(DETAIL_CELLS_PER_WORLD_VOXEL, 8);
        assert_eq!(DETAIL_CELL_SIZE_METERS, 0.125);
    }

    #[test]
    fn generated_terrain_is_uniform_inside_every_world_voxel() {
        let world = VoxelWorld::generate(42, 0.0);
        for world_z in (0..WORLD_VOXELS_Z as i32).step_by(7) {
            for world_y in 0..WORLD_VOXELS_Y as i32 {
                for world_x in (0..WORLD_VOXELS_X as i32).step_by(7) {
                    let coordinate = WorldVoxelCoord::new(world_x, world_y, world_z);
                    let expected = world.world_voxel(coordinate);
                    let origin = coordinate.detail_origin();
                    for local in [[0, 0, 0], [7, 0, 7], [0, 7, 0], [7, 7, 7]] {
                        assert_eq!(
                            world.get(
                                origin[0] + local[0],
                                origin[1] + local[1],
                                origin[2] + local[2]
                            ),
                            expected
                        );
                    }
                }
            }
        }
    }

    /// Carving is a replayable function of the seed, and turning caves off is
    /// the same world minus the carve — the invariant the deleted `with_caves`
    /// bridge used to test, now checked against the real generation path instead
    /// of a second carving implementation that could drift from it.
    #[test]
    fn cave_carving_is_replayable_and_only_removes_solid_voxels() {
        let generator = ChunkGenerator {
            world_seed: 0x0123_4567_89AB_CDEF,
            cave_program: crate::cave::CaveProgram {
                shape: crate::cave::CaveShape::CellularAutomata,
                ..crate::cave::CaveProgram::default()
            },
            // Fluids and decoration off: this test is about the carve alone.
            cave_fluids: crate::cave::CaveFluids {
                enabled: false,
                ..crate::cave::CaveFluids::default()
            },
            scatter: crate::scatter::ScatterProgram {
                enabled: false,
                ..crate::scatter::ScatterProgram::default()
            },
            ..ChunkGenerator::default()
        };
        let solid = VoxelWorld::from_chunk_generator(&ChunkGenerator {
            caves_enabled: false,
            ..generator.clone()
        });
        let first = VoxelWorld::from_chunk_generator(&generator);
        let second = VoxelWorld::from_chunk_generator(&generator);
        let mut carved = 0;
        for z in (0..WORLD_VOXELS_Z as i32).step_by(5) {
            for y in 0..WORLD_VOXELS_Y as i32 {
                for x in (0..WORLD_VOXELS_X as i32).step_by(5) {
                    let coordinate = WorldVoxelCoord::new(x, y, z);
                    assert_eq!(
                        first.world_voxel(coordinate),
                        second.world_voxel(coordinate),
                        "carving must replay exactly for one seed"
                    );
                    let before = solid.world_voxel(coordinate);
                    let after = first.world_voxel(coordinate);
                    if before == after {
                        continue;
                    }
                    // The only difference a carve may make is solid becoming air.
                    assert!(
                        before.is_solid() && after == Voxel::Air,
                        "carving turned {before:?} into {after:?} at {coordinate:?}"
                    );
                    carved += 1;
                }
            }
        }
        assert!(
            carved > 0,
            "the cellular cave program did not carve any sample"
        );
    }

    /// Generation writes terrain, fluids, and the decoration its scatter pass
    /// authors — and nothing that is an ASSET.
    ///
    /// The list shrank when generation pass two landed: tall grass, flowers, and
    /// cave cover are now generated on purpose, and cave lava is a generated
    /// fluid. Trees and water plants stay asset-only, which is the distinction
    /// this test actually defends. It also still pins the original rule that
    /// procedural generation fills whole one-metre voxels rather than writing
    /// individual detail cells.
    #[test]
    fn generation_writes_no_asset_only_materials() {
        let world = VoxelWorld::generate_chunked(7, true);
        let mut solid_world_voxels = 0_usize;
        for z in 0..WORLD_VOXELS_Z as i32 {
            for y in 0..WORLD_VOXELS_Y as i32 {
                for x in 0..WORLD_VOXELS_X as i32 {
                    solid_world_voxels +=
                        usize::from(world.world_voxel(WorldVoxelCoord::new(x, y, z)) != Voxel::Air);
                    assert!(!matches!(
                        world.world_voxel(WorldVoxelCoord::new(x, y, z)),
                        Voxel::Trunk
                            | Voxel::TrunkBirch
                            | Voxel::Leaves
                            | Voxel::LeavesDark
                            | Voxel::LeavesBirch
                            | Voxel::LeavesPine
                            | Voxel::WaterWeed
                            | Voxel::LilyPad
                            | Voxel::LilyBloom
                            | Voxel::Reed
                            | Voxel::CattailHead
                            | Voxel::GlowBlock
                            | Voxel::GlowBerry
                    ));
                }
            }
        }
        assert!(solid_world_voxels > 0, "generation must produce terrain");
        // The scatter pass is on by default, so its own output must be present —
        // otherwise this test would keep passing if pass two silently stopped
        // running.
        let scattered = (0..WORLD_VOXELS_Z as i32)
            .flat_map(|z| (0..WORLD_VOXELS_Y as i32).map(move |y| (y, z)))
            .flat_map(|(y, z)| (0..WORLD_VOXELS_X as i32).map(move |x| (x, y, z)))
            .filter(|&(x, y, z)| {
                world
                    .world_voxel(WorldVoxelCoord::new(x, y, z))
                    .is_scatter_cover()
            })
            .count();
        assert!(scattered > 0, "generation pass two placed no decoration");
    }

    /// `Voxel::ALL` must list every variant exactly once, in discriminant order.
    /// A missing entry would silently drop that material from every distribution
    /// readout, which is the kind of gap that reads as "the world contains none".
    #[test]
    fn the_voxel_list_is_complete_and_in_discriminant_order() {
        for (index, voxel) in Voxel::ALL.iter().enumerate() {
            assert_eq!(
                *voxel as usize, index,
                "Voxel::ALL is out of order at index {index}"
            );
        }
        // The last variant's discriminant pins the length: if a variant is added
        // to the enum and not to ALL, this fails.
        assert_eq!(Voxel::FlareEmber as usize, Voxel::ALL.len() - 1);
    }

    #[test]
    fn coordinate_conversion_is_exact() {
        let coordinate = WorldVoxelCoord::new(124, 31, 0);
        assert_eq!(coordinate.detail_origin(), [992, 248, 0]);
        assert_eq!(WorldVoxelCoord::from_detail_cell([999, 255, 7]), coordinate);
    }
}
