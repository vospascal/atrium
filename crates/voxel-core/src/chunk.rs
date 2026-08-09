//! Chunked, seed-driven world generation.
//!
//! The legacy [`crate::world::VoxelWorld`] is intentionally kept as the small
//! renderer fixture. Large worlds use this module: chunks are addressed by
//! signed coordinates, generation samples global coordinates, and only the
//! requested chunks need to exist in memory.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

use crate::cave::{
    CaveFluid, CaveFluids, CaveProgram, CaveShape, CaveSurface, CellularAutomataField,
    CellularFieldCache, CELLULAR_Y_BAND_SIZE,
};
use crate::noise::{coordinate_roll, fractal_noise_2d_u64};
use crate::scatter::{ScatterProgram, ScatterTally, ScatterTarget};
use crate::stage_report::{GenerationStage, StageReport};
use crate::world::{Voxel, WATER_LEVEL_WORLD, WORLD_VOXELS_Y};

/// Horizontal edge length of one streamed chunk, in world voxels.
pub const CHUNK_SIZE: usize = 16;
/// Default vertical span of a generated chunk.
///
/// The same height the renderer's world lattice uses. Named rather than repeated
/// as a literal, because the two silently disagreeing is a whole class of bug:
/// terrain would generate to one ceiling and the renderer would read another.
pub const CHUNK_HEIGHT: usize = WORLD_VOXELS_Y;

/// Physical biome classification attached to generated coordinates.
///
/// The single home for this distinction. `voxel-rt`'s biome resolver routes on
/// the same type rather than declaring its own two-variant copy — two enums
/// meaning "surface or cave" in crates that already depend on each other would
/// eventually need a mapping function between them.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum BiomeKind {
    #[default]
    Surface,
    Cave,
}

/// Signed horizontal chunk coordinate. Negative coordinates are valid and use
/// Euclidean division, so chunk seams remain stable on both sides of origin.
#[derive(
    Clone, Copy, Debug, Default, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize,
)]
pub struct ChunkCoord {
    pub x: i32,
    pub z: i32,
}

impl ChunkCoord {
    pub const fn new(x: i32, z: i32) -> Self {
        Self { x, z }
    }

    pub const fn origin(self) -> (i32, i32) {
        (self.x * CHUNK_SIZE as i32, self.z * CHUNK_SIZE as i32)
    }

    pub fn from_world(x: i32, z: i32) -> Self {
        Self::new(
            x.div_euclid(CHUNK_SIZE as i32),
            z.div_euclid(CHUNK_SIZE as i32),
        )
    }
}

/// A generated chunk in authoritative one-metre voxels.
#[derive(Clone, Debug, PartialEq)]
pub struct Chunk {
    coord: ChunkCoord,
    min_y: i32,
    height: usize,
    voxels: Vec<Voxel>,
    biomes: Vec<BiomeKind>,
    /// What this chunk's generation produced, for the debug window. Carried on
    /// the chunk so the counts are the real ones rather than a re-simulation.
    stats: GenerationStats,
    /// What each stage did on the way there, so the pipeline is readable as
    /// layers rather than as one finished result.
    report: StageReport,
}

impl Chunk {
    fn new(coord: ChunkCoord, min_y: i32, height: usize) -> Self {
        Self {
            coord,
            min_y,
            height,
            voxels: vec![Voxel::Air; CHUNK_SIZE * height * CHUNK_SIZE],
            biomes: vec![BiomeKind::Surface; CHUNK_SIZE * height * CHUNK_SIZE],
            stats: GenerationStats::new(),
            report: StageReport::new(min_y, height),
        }
    }

    /// What each generation stage did in this chunk.
    pub const fn stage_report(&self) -> &StageReport {
        &self.report
    }

    /// What this chunk's generation produced.
    pub const fn stats(&self) -> &GenerationStats {
        &self.stats
    }

    /// What pass two placed in this chunk.
    pub const fn scatter_tally(&self) -> &ScatterTally {
        &self.stats.scatter
    }

    /// Which cave face an open voxel sits against, in chunk-local X/Z.
    ///
    /// Floor beats ceiling beats wall, so a one-voxel-tall crawlspace reads as
    /// floor: something standing on the ground is the more useful answer than
    /// something hanging from a roof one voxel above it.
    pub fn cave_surface(&self, local_x: i32, world_y: i32, local_z: i32) -> CaveSurface {
        if !self.get(local_x, world_y, local_z).is_open_space() {
            return CaveSurface::Interior;
        }
        if self.get(local_x, world_y - 1, local_z).is_solid() {
            return CaveSurface::Floor;
        }
        if self.get(local_x, world_y + 1, local_z).is_solid() {
            return CaveSurface::Ceiling;
        }
        let touches_wall = [(-1, 0), (1, 0), (0, -1), (0, 1)]
            .iter()
            .any(|(dx, dz)| self.get(local_x + dx, world_y, local_z + dz).is_solid());
        if touches_wall {
            CaveSurface::Wall
        } else {
            CaveSurface::Interior
        }
    }

    pub const fn coord(&self) -> ChunkCoord {
        self.coord
    }

    pub const fn min_y(&self) -> i32 {
        self.min_y
    }

    pub const fn max_y_exclusive(&self) -> i32 {
        self.min_y + self.height as i32
    }

    pub fn get(&self, x: i32, y: i32, z: i32) -> Voxel {
        if !(0..CHUNK_SIZE as i32).contains(&x)
            || !(self.min_y..self.max_y_exclusive()).contains(&y)
            || !(0..CHUNK_SIZE as i32).contains(&z)
        {
            return Voxel::Air;
        }
        self.voxels[self.index(x as usize, (y - self.min_y) as usize, z as usize)]
    }

    pub fn world_voxel(&self, x: i32, y: i32, z: i32) -> Voxel {
        let (origin_x, origin_z) = self.coord.origin();
        self.get(x - origin_x, y, z - origin_z)
    }

    pub fn biome_at(&self, x: i32, y: i32, z: i32) -> BiomeKind {
        let (origin_x, origin_z) = self.coord.origin();
        let local_x = x - origin_x;
        let local_z = z - origin_z;
        if !(0..CHUNK_SIZE as i32).contains(&local_x)
            || !(self.min_y..self.max_y_exclusive()).contains(&y)
            || !(0..CHUNK_SIZE as i32).contains(&local_z)
        {
            return BiomeKind::Surface;
        }
        self.biomes[self.index(
            local_x as usize,
            (y - self.min_y) as usize,
            local_z as usize,
        )]
    }

    fn set(&mut self, x: usize, y: usize, z: usize, voxel: Voxel, biome: BiomeKind) {
        let index = self.index(x, y, z);
        self.voxels[index] = voxel;
        self.biomes[index] = biome;
    }

    fn index(&self, x: usize, y: usize, z: usize) -> usize {
        (y * CHUNK_SIZE + z) * CHUNK_SIZE + x
    }
}

/// Pure configuration for deterministic chunk generation.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ChunkGenerator {
    pub world_seed: u64,
    pub min_y: i32,
    pub height: usize,
    /// Highest world voxel that open-air water fills. Defaults to the renderer's
    /// [`WATER_LEVEL_WORLD`], because the generator's ocean and the rendered water
    /// plane being the same height is a requirement, not a coincidence.
    pub sea_level: i32,
    /// Number of empty air voxels reserved above the highest terrain surface.
    #[serde(default = "default_surface_headroom")]
    pub surface_headroom: i32,
    /// Terrain height at the middle of the relief range, in world voxels.
    #[serde(default = "default_terrain_base_height")]
    pub terrain_base_height: i32,
    /// Peak-to-trough terrain relief, in world voxels. Half of it sits below
    /// [`Self::terrain_base_height`], which is what puts terrain on both sides of
    /// sea level so oceans and beaches can exist at all.
    #[serde(default = "default_terrain_relief")]
    pub terrain_relief: i32,
    #[serde(default = "default_caves_enabled")]
    pub caves_enabled: bool,
    pub cave_program: CaveProgram,
    /// Underground fluid levels applied to carved voids.
    #[serde(default)]
    pub cave_fluids: CaveFluids,
    /// Pass two: decoration scattered over the finished shape.
    #[serde(default)]
    pub scatter: ScatterProgram,
}

fn default_caves_enabled() -> bool {
    true
}

pub const DEFAULT_SURFACE_HEADROOM: i32 = 10;

fn default_surface_headroom() -> i32 {
    DEFAULT_SURFACE_HEADROOM
}

/// Base height and relief are chosen together so the surface straddles
/// [`WATER_LEVEL_WORLD`]: 8 voxels of terrain sit below the water plane and 12
/// above it. The previous single `40 + (terrain - 0.5) * 16` formula put every
/// surface at or above 32 with sea level also at 32, which made the water branch
/// unreachable — the world could not contain a drop of water.
pub const DEFAULT_TERRAIN_BASE_HEIGHT: i32 = WATER_LEVEL_WORLD + 2;
pub const DEFAULT_TERRAIN_RELIEF: i32 = 20;

fn default_terrain_base_height() -> i32 {
    DEFAULT_TERRAIN_BASE_HEIGHT
}

fn default_terrain_relief() -> i32 {
    DEFAULT_TERRAIN_RELIEF
}

/// A carved, gently rising connection between two cellular cave strata.
///
/// The cellular recipe makes excellent horizontal rooms, but each field is
/// otherwise a vertical extrusion. This override reserves a solid floor under
/// a three-voxel run / one-voxel rise, so walk mode can travel between bands
/// instead of meeting a sheer shelf.
#[derive(Clone, Copy, Debug)]
struct CellularRamp {
    start_y: i32,
    end_y: i32,
    start: (i32, i32),
    end: (i32, i32),
    half_width: i32,
}

impl CellularRamp {
    const HEADROOM: i32 = 14;
    const MIN_RUN_PER_RISE: i32 = 3;

    fn between(
        lower: &CellularAutomataField,
        upper: &CellularAutomataField,
        start_y: i32,
        end_y: i32,
        cell_size: u32,
    ) -> Option<Self> {
        let start = lower.nearest_open_cell(32, 32)?;
        let end = upper.farthest_open_cell_from(start)?;
        let run_squared = (end.0 - start.0).pow(2) + (end.1 - start.1).pow(2);
        let rise = end_y - start_y;
        if rise <= 0 || run_squared < (rise * Self::MIN_RUN_PER_RISE).pow(2) {
            return None;
        }
        Some(Self {
            start_y,
            end_y,
            start,
            end,
            half_width: (cell_size as i32).max(2),
        })
    }

    /// Returns `Some(carved)` inside the ramp footprint. `false` deliberately
    /// keeps the terrain below the air tunnel, creating the staircase floor.
    ///
    /// `ceiling` is the highest voxel this ramp may carve — the cave band's own
    /// top, further limited by the column's surface. Without it the headroom
    /// carved up to `end_y + 13`, which for the top band reached Y 61 while every
    /// surface sits in [32, 48]: the ramp punched an open trench from the cave to
    /// the sky wherever its headroom crossed the terrain cap.
    fn carve_override(self, x: i32, y: i32, z: i32, ceiling: i32) -> Option<bool> {
        if y < self.start_y - 1 || y >= self.end_y + Self::HEADROOM || y > ceiling {
            return None;
        }
        let run_x = self.end.0 - self.start.0;
        let run_z = self.end.1 - self.start.1;
        let run_squared = run_x * run_x + run_z * run_z;
        if run_squared == 0 {
            return None;
        }
        let offset_x = x - self.start.0;
        let offset_z = z - self.start.1;
        let projection = (offset_x * run_x + offset_z * run_z) as f32 / run_squared as f32;
        if !(0.0..=1.0).contains(&projection) {
            return None;
        }
        let closest_x = self.start.0 as f32 + run_x as f32 * projection;
        let closest_z = self.start.1 as f32 + run_z as f32 * projection;
        let lateral_x = x as f32 - closest_x;
        let lateral_z = z as f32 - closest_z;
        if lateral_x * lateral_x + lateral_z * lateral_z
            > (self.half_width * self.half_width) as f32
        {
            return None;
        }
        let floor_y =
            self.start_y + ((self.end_y - self.start_y) as f32 * projection).round() as i32;
        Some((floor_y..floor_y + Self::HEADROOM).contains(&y))
    }
}

impl Default for ChunkGenerator {
    fn default() -> Self {
        Self {
            world_seed: 1,
            min_y: 0,
            height: CHUNK_HEIGHT,
            sea_level: WATER_LEVEL_WORLD,
            surface_headroom: default_surface_headroom(),
            terrain_base_height: default_terrain_base_height(),
            terrain_relief: default_terrain_relief(),
            caves_enabled: true,
            cave_program: CaveProgram::default(),
            cave_fluids: CaveFluids::default(),
            scatter: ScatterProgram::default(),
        }
    }
}

impl ChunkGenerator {
    pub fn validate(&self) -> bool {
        self.height > 0
            && self.height <= i32::MAX as usize
            && self.sea_level >= self.min_y
            && self.sea_level < self.min_y + self.height as i32
            && self.surface_headroom >= 0
            && self.surface_headroom < self.height as i32 - 1
            && self.terrain_relief >= 0
            && self.cave_program.is_valid()
    }

    /// This configuration with every out-of-range value pulled back into range.
    ///
    /// Authored values reach here from node fields whose declared ranges are
    /// deliberately generous (`surface_headroom` allows up to 4096), and
    /// generation must never be the thing that reports a bad number by panicking.
    /// Clamping is the honest response: the author sees terrain built at the
    /// nearest workable value and can read the real limit off the debug window.
    pub fn sanitized(&self) -> Self {
        let height = self.height.clamp(2, i32::MAX as usize);
        let span = height as i32;
        let mut sanitized = Self {
            height,
            // One voxel of terrain and one of headroom are the floor: anything
            // less has nowhere to put a surface.
            surface_headroom: self.surface_headroom.clamp(0, span - 2),
            sea_level: self.sea_level.clamp(self.min_y, self.min_y + span - 1),
            terrain_relief: self.terrain_relief.clamp(0, span),
            terrain_base_height: self
                .terrain_base_height
                .clamp(self.min_y + 1, self.min_y + span - 1),
            ..self.clone()
        };
        if !sanitized.cave_program.is_valid() {
            sanitized.cave_program = CaveProgram::default();
        }
        // Caves and their fluids must stay inside the generated span, or a lake
        // fills voxels no chunk owns.
        sanitized.cave_program.min_y = sanitized.cave_program.min_y.clamp(self.min_y, span - 1);
        sanitized.cave_program.max_y = sanitized
            .cave_program
            .max_y
            .clamp(sanitized.cave_program.min_y, span - 1);
        sanitized.cave_fluids.min_y = sanitized.cave_fluids.min_y.max(self.min_y);
        sanitized.cave_fluids.water_level = sanitized
            .cave_fluids
            .water_level
            .clamp(self.min_y, self.min_y + span - 1);
        sanitized.cave_fluids.lava_level = sanitized
            .cave_fluids
            .lava_level
            .clamp(self.min_y, self.min_y + span - 1);
        sanitized
    }

    /// The highest terrain surface this configuration can produce, and the lowest.
    /// Published because the debug window shows them next to the sea level, which
    /// is how "why is there no water" becomes visible instead of inferred.
    pub fn surface_range(&self) -> (i32, i32) {
        let half_relief = self.terrain_relief / 2;
        let ceiling = self.min_y + self.height as i32 - self.surface_headroom - 1;
        let lowest = (self.terrain_base_height - half_relief).clamp(self.min_y + 1, ceiling);
        let highest = (self.terrain_base_height + half_relief).clamp(lowest, ceiling);
        (lowest, highest)
    }

    /// The terrain surface height for one world column.
    ///
    /// A broad landform field plus a finer detail field, mapped onto the authored
    /// base height and relief. Keeping the two fields separate is what stops the
    /// headroom ceiling flattening everything into a tabletop.
    pub fn surface_height(&self, world_x: i32, world_z: i32) -> i32 {
        let broad = fractal_noise_2d_u64(
            world_x as f32 * 0.004,
            world_z as f32 * 0.004,
            3,
            self.world_seed ^ 0xA17E_0001,
        );
        let detail = fractal_noise_2d_u64(
            world_x as f32 * 0.025,
            world_z as f32 * 0.025,
            4,
            self.world_seed ^ 0xA17E_0002,
        );
        let terrain = broad * 0.75 + detail * 0.25;
        let (lowest, highest) = self.surface_range();
        let height = self.terrain_base_height as f32 + (terrain - 0.5) * self.terrain_relief as f32;
        (height.round() as i32).clamp(lowest, highest)
    }

    /// Generate one chunk. Every noise sample uses global world coordinates,
    /// so generating adjacent chunks in different orders cannot create seams.
    ///
    /// Two passes, in this order and never interleaved:
    ///
    /// 1. **shape** — terrain columns, cave carving, and fluid fill;
    /// 2. **scatter** — decoration read off the finished shape.
    ///
    /// Pass two runs second precisely so it can ask what pass one produced (is
    /// this a cave floor, is that voxel water) instead of re-deriving it.
    pub fn generate(&self, coord: ChunkCoord) -> Chunk {
        self.generate_with_cache(coord, &mut CellularFieldCache::new())
    }

    /// Generate one chunk, reusing region-aligned cellular fields across calls.
    ///
    /// The field is a function of `(region origin, Y band)`, so every chunk inside
    /// one region asks for the identical field. Building a whole world through one
    /// cache turns 256 field builds into 4.
    pub fn generate_with_cache(&self, coord: ChunkCoord, cache: &mut CellularFieldCache) -> Chunk {
        // Sanitised rather than asserted: an authored value outside a workable
        // range must not be able to panic generation.
        let generator = self.sanitized();
        let mut chunk = Chunk::new(coord, generator.min_y, generator.height);
        let (origin_x, origin_z) = coord.origin();
        let max_y = generator.min_y + generator.height as i32;
        let uses_cellular = generator.cave_program.shape == CaveShape::CellularAutomata;
        let bands: Vec<i32> = (generator.min_y..max_y)
            .step_by(CELLULAR_Y_BAND_SIZE as usize)
            .collect();
        let cellular_fields: Option<Vec<CellularAutomataField>> = uses_cellular.then(|| {
            bands
                .iter()
                .map(|&band_y| {
                    cache
                        .field_at(
                            &generator.cave_program,
                            generator.world_seed,
                            band_y,
                            origin_x,
                            origin_z,
                        )
                        .clone()
                })
                .collect()
        });
        let cellular_ramps = cellular_fields.as_ref().map(|fields| {
            fields
                .windows(2)
                .enumerate()
                .filter_map(|(band, fields)| {
                    let end_y = generator.min_y + (band as i32 + 1) * CELLULAR_Y_BAND_SIZE;
                    let start_y = end_y
                        .saturating_sub(CELLULAR_Y_BAND_SIZE)
                        .max(generator.cave_program.min_y);
                    (end_y <= generator.cave_program.max_y).then_some(())?;
                    CellularRamp::between(
                        &fields[0],
                        &fields[1],
                        start_y,
                        end_y,
                        generator.cave_program.cellular_cell_size,
                    )
                })
                .collect::<Vec<_>>()
        });

        // ---- Pass one: shape ------------------------------------------------
        for local_z in 0..CHUNK_SIZE {
            for local_x in 0..CHUNK_SIZE {
                let world_x = origin_x + local_x as i32;
                let world_z = origin_z + local_z as i32;
                let surface_y = generator.surface_height(world_x, world_z);
                chunk.stats.lowest_surface = chunk.stats.lowest_surface.min(surface_y);
                chunk.stats.highest_surface = chunk.stats.highest_surface.max(surface_y);
                // A ramp may carve up to the cave band's top, and never above
                // the terrain surface of its own column: the cave stays a cave.
                let ramp_ceiling = generator
                    .cave_program
                    .max_y
                    .min(surface_y.saturating_sub(1));
                for world_y in generator.min_y..max_y {
                    let terrain_voxel = generator.terrain_voxel(world_y, surface_y);
                    chunk.report.stage_mut(GenerationStage::Terrain).consider();
                    if terrain_voxel != Voxel::Air {
                        chunk.report.stage_mut(GenerationStage::Terrain).change();
                    }
                    // Only solid rock is offered to the carve, so `considered`
                    // reads as "how much rock could have become cave" — the
                    // denominator an author is actually tuning against.
                    if generator.caves_enabled && terrain_voxel.is_solid() {
                        chunk.report.stage_mut(GenerationStage::Carve).consider();
                    }
                    let carved = generator.caves_enabled
                        && terrain_voxel.is_solid()
                        && generator.carves(
                            &cellular_fields,
                            &cellular_ramps,
                            &bands,
                            world_x,
                            world_y,
                            world_z,
                            ramp_ceiling,
                        );
                    let voxel = if carved {
                        chunk.report.stage_mut(GenerationStage::Carve).change();
                        // Cave fluids fill the void this carve just opened. The
                        // level's own floor stays solid because `carves` is only
                        // ever asked about a solid voxel, so a lake always has a
                        // bed of whatever rock lay beneath it.
                        chunk.report.stage_mut(GenerationStage::Fluids).consider();
                        match generator.cave_fluids.fluid_at(world_y) {
                            Some(CaveFluid::Water) => {
                                chunk.report.stage_mut(GenerationStage::Fluids).change();
                                Voxel::Water
                            }
                            Some(CaveFluid::Lava) => {
                                chunk.report.stage_mut(GenerationStage::Fluids).change();
                                Voxel::Lava
                            }
                            None => Voxel::Air,
                        }
                    } else {
                        terrain_voxel
                    };
                    let stage = if carved {
                        if voxel == Voxel::Air {
                            GenerationStage::Carve
                        } else {
                            GenerationStage::Fluids
                        }
                    } else {
                        GenerationStage::Terrain
                    };
                    chunk.report.record_voxel(world_y, voxel, stage);
                    if carved {
                        chunk.stats.cave_voxels += 1;
                        chunk.stats.lowest_cave = chunk.stats.lowest_cave.min(world_y);
                        chunk.stats.highest_cave = chunk.stats.highest_cave.max(world_y);
                        match voxel {
                            Voxel::Water => chunk.stats.cave_water += 1,
                            Voxel::Lava => chunk.stats.cave_lava += 1,
                            _ => {}
                        }
                    } else if voxel.is_solid() {
                        chunk.stats.solid_voxels += 1;
                    } else if voxel == Voxel::Water {
                        chunk.stats.open_air_water += 1;
                    }
                    chunk.set(
                        local_x,
                        (world_y - generator.min_y) as usize,
                        local_z,
                        voxel,
                        if carved {
                            BiomeKind::Cave
                        } else {
                            BiomeKind::Surface
                        },
                    );
                }
            }
        }

        // ---- Pass two: scatter ----------------------------------------------
        generator.scatter_into(&mut chunk);
        // The finished composition, counted once. Taking it here rather than as
        // each stage decides is what keeps a voxel pass two replaced from being
        // counted twice.
        for local_z in 0..CHUNK_SIZE {
            for local_x in 0..CHUNK_SIZE {
                for world_y in generator.min_y..max_y {
                    let voxel = chunk.get(local_x as i32, world_y, local_z as i32);
                    chunk.report.record_material(voxel);
                }
            }
        }
        chunk.stats.chunks = 1;
        chunk.stats.cellular_fields_built = cache.len() as u32;
        chunk
    }

    /// The terrain material for one voxel of a column, before caves.
    fn terrain_voxel(&self, world_y: i32, surface_y: i32) -> Voxel {
        if world_y > surface_y {
            // Open-air water: everything above the surface and at or below the
            // sea level is ocean. Reachable now that terrain straddles the sea
            // level rather than sitting entirely above it.
            return if world_y <= self.sea_level {
                Voxel::Water
            } else {
                Voxel::Air
            };
        }
        if world_y == surface_y {
            // A beach is the shoreline band, not merely "at or below sea level":
            // one voxel of tolerance above the waterline is what makes a shore
            // read as sand rather than grass meeting water at a hard line.
            if surface_y <= self.sea_level + 1 {
                Voxel::Sand
            } else {
                Voxel::Grass
            }
        } else if world_y >= surface_y - 2 {
            if surface_y <= self.sea_level + 1 {
                Voxel::Sediment
            } else {
                Voxel::Dirt
            }
        } else {
            Voxel::Stone
        }
    }

    /// Whether the cave pass carves this voxel.
    #[allow(clippy::too_many_arguments)]
    fn carves(
        &self,
        cellular_fields: &Option<Vec<CellularAutomataField>>,
        cellular_ramps: &Option<Vec<CellularRamp>>,
        bands: &[i32],
        world_x: i32,
        world_y: i32,
        world_z: i32,
        ramp_ceiling: i32,
    ) -> bool {
        // The ramp override is consulted first and short-circuits, so a voxel a
        // ramp already decided about never pays for the Worley/billow sample.
        if let Some(ramps) = cellular_ramps {
            if let Some(carved) = ramps
                .iter()
                .find_map(|ramp| ramp.carve_override(world_x, world_y, world_z, ramp_ceiling))
            {
                return carved;
            }
        }
        match cellular_fields {
            Some(fields) => {
                let band = ((world_y - self.min_y) / CELLULAR_Y_BAND_SIZE)
                    .clamp(0, bands.len().saturating_sub(1) as i32)
                    as usize;
                match fields.get(band) {
                    Some(field) => self.cave_program.carves_with_cellular_field(
                        field,
                        self.world_seed,
                        world_x,
                        world_y,
                        world_z,
                    ),
                    None => false,
                }
            }
            None => self
                .cave_program
                .carves(self.world_seed, world_x, world_y, world_z),
        }
    }

    /// Pass two, in place over a finished chunk.
    fn scatter_into(&self, chunk: &mut Chunk) {
        if !self.scatter.enabled {
            return;
        }
        let (origin_x, origin_z) = chunk.coord().origin();
        let max_y = chunk.max_y_exclusive();
        let mut tally = ScatterTally::default();
        for local_z in 0..CHUNK_SIZE {
            for local_x in 0..CHUNK_SIZE {
                let world_x = origin_x + local_x as i32;
                let world_z = origin_z + local_z as i32;
                for world_y in chunk.min_y()..max_y {
                    // Only empty air can receive cover: never overwrite rock, and
                    // never grow a mushroom underwater or in lava.
                    if chunk.get(local_x as i32, world_y, local_z as i32) != Voxel::Air {
                        continue;
                    }
                    let below = chunk.get(local_x as i32, world_y - 1, local_z as i32);
                    if !below.is_solid() {
                        // Cover needs something under it unless it hangs from a
                        // ceiling, which the cave-ceiling target handles below.
                        if chunk.biome_at(world_x, world_y, world_z) != BiomeKind::Cave {
                            continue;
                        }
                    }
                    let target =
                        self.scatter_target(chunk, local_x, world_y, local_z, world_x, world_z);
                    let Some(target) = target else { continue };
                    chunk.report.stage_mut(GenerationStage::Scatter).consider();
                    let Some(class) =
                        self.scatter
                            .placement(self.world_seed, world_x, world_y, world_z, target)
                    else {
                        continue;
                    };
                    chunk.report.stage_mut(GenerationStage::Scatter).change();
                    let variant_roll =
                        coordinate_roll(world_x, world_y, world_z, self.world_seed, 0x5CA7_FFFF);
                    let placed = class.voxel(variant_roll);
                    chunk
                        .report
                        .record_voxel(world_y, placed, GenerationStage::Scatter);
                    chunk.set(
                        local_x,
                        (world_y - chunk.min_y()) as usize,
                        local_z,
                        class.voxel(variant_roll),
                        chunk.biome_at(world_x, world_y, world_z),
                    );
                    tally.record(class);
                }
            }
        }
        chunk.stats.scatter = tally;
    }

    /// Which scatter target, if any, an empty voxel presents.
    fn scatter_target(
        &self,
        chunk: &Chunk,
        local_x: usize,
        world_y: i32,
        local_z: usize,
        world_x: i32,
        world_z: i32,
    ) -> Option<ScatterTarget> {
        let local_x = local_x as i32;
        let local_z = local_z as i32;
        let below = chunk.get(local_x, world_y - 1, local_z);
        let above = chunk.get(local_x, world_y + 1, local_z);
        if chunk.biome_at(world_x, world_y, world_z) != BiomeKind::Cave {
            // Open sky above and solid ground below is the lit surface. Anything
            // roofed is cave-like even if the carve pass never touched it.
            return (below.is_solid() && above == Voxel::Air).then_some(ScatterTarget::SurfaceTop);
        }
        match chunk.cave_surface(local_x, world_y, local_z) {
            CaveSurface::Floor => Some(ScatterTarget::CaveFloor),
            CaveSurface::Ceiling => Some(ScatterTarget::CaveCeiling),
            CaveSurface::Wall => Some(ScatterTarget::CaveWall),
            CaveSurface::Interior => None,
        }
    }
}

/// What one generation run actually produced.
///
/// Measured while generating rather than re-derived afterwards, so the debug
/// window reports the world that exists instead of a second simulation of it that
/// could disagree. This is the readout that makes "why is there no water" or "why
/// are there no caves down there" answerable without a probe binary — both of
/// which this arc had to write a throwaway probe to answer.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct GenerationStats {
    pub chunks: u32,
    pub solid_voxels: u32,
    pub cave_voxels: u32,
    pub open_air_water: u32,
    pub cave_water: u32,
    pub cave_lava: u32,
    /// Lowest and highest terrain surface actually generated, which is the pair to
    /// read against the sea level.
    pub lowest_surface: i32,
    pub highest_surface: i32,
    /// Deepest and shallowest carved cave voxel, for reading against the band.
    pub lowest_cave: i32,
    pub highest_cave: i32,
    pub scatter: ScatterTally,
    /// Cellular fields built for this run. With the region cache this is a handful
    /// per world; without it, it was one per chunk per band.
    pub cellular_fields_built: u32,
}

impl GenerationStats {
    /// A zeroed set of stats with the extents inverted, so the first recorded
    /// value wins on both sides.
    pub fn new() -> Self {
        Self {
            lowest_surface: i32::MAX,
            highest_surface: i32::MIN,
            lowest_cave: i32::MAX,
            highest_cave: i32::MIN,
            ..Self::default()
        }
    }

    pub fn merge(&mut self, other: &Self) {
        self.chunks += other.chunks;
        self.solid_voxels = self.solid_voxels.saturating_add(other.solid_voxels);
        self.cave_voxels = self.cave_voxels.saturating_add(other.cave_voxels);
        self.open_air_water = self.open_air_water.saturating_add(other.open_air_water);
        self.cave_water = self.cave_water.saturating_add(other.cave_water);
        self.cave_lava = self.cave_lava.saturating_add(other.cave_lava);
        self.lowest_surface = self.lowest_surface.min(other.lowest_surface);
        self.highest_surface = self.highest_surface.max(other.highest_surface);
        self.lowest_cave = self.lowest_cave.min(other.lowest_cave);
        self.highest_cave = self.highest_cave.max(other.highest_cave);
        self.scatter.merge(&other.scatter);
        self.cellular_fields_built = self.cellular_fields_built.max(other.cellular_fields_built);
    }

    /// Whether any cave was carved. Reported because "no caves" and "caves you
    /// have not walked to" look identical from inside the world.
    pub fn has_caves(&self) -> bool {
        self.cave_voxels > 0
    }

    /// The generated surface range, or `None` for a run that produced no terrain.
    pub fn surface_range(&self) -> Option<(i32, i32)> {
        (self.lowest_surface <= self.highest_surface)
            .then_some((self.lowest_surface, self.highest_surface))
    }

    /// The carved cave range, or `None` for a world without caves.
    pub fn cave_range(&self) -> Option<(i32, i32)> {
        (self.lowest_cave <= self.highest_cave).then_some((self.lowest_cave, self.highest_cave))
    }
}

/// In-memory loaded-chunk set. Streaming policy belongs above this type; this
/// store only owns deterministic insertion, lookup, and radius eviction.
#[derive(Clone, Debug, Default)]
pub struct ChunkStore {
    chunks: BTreeMap<ChunkCoord, Chunk>,
}

impl ChunkStore {
    pub fn insert(&mut self, chunk: Chunk) -> Option<Chunk> {
        self.chunks.insert(chunk.coord(), chunk)
    }

    pub fn get(&self, coord: ChunkCoord) -> Option<&Chunk> {
        self.chunks.get(&coord)
    }

    pub fn len(&self) -> usize {
        self.chunks.len()
    }

    pub fn is_empty(&self) -> bool {
        self.chunks.is_empty()
    }

    pub fn retain_radius(&mut self, center: ChunkCoord, radius: i32) {
        let radius = radius.max(0);
        self.chunks.retain(|coord, _| {
            (coord.x - center.x).abs() <= radius && (coord.z - center.z).abs() <= radius
        });
    }
}

/// A small streaming coordinator for the CPU-side world authority. It keeps a
/// square of chunks around a center coordinate and discards chunks outside the
/// radius. GPU upload and background scheduling deliberately sit above this
/// pure type.
#[derive(Clone, Debug)]
pub struct ChunkWorld {
    generator: ChunkGenerator,
    center: ChunkCoord,
    radius: i32,
    store: ChunkStore,
}

impl ChunkWorld {
    pub fn new(generator: ChunkGenerator, center: ChunkCoord, radius: i32) -> Self {
        let mut world = Self {
            generator,
            center,
            radius: radius.max(0),
            store: ChunkStore::default(),
        };
        world.stream_to(center);
        world
    }

    pub const fn center(&self) -> ChunkCoord {
        self.center
    }

    pub const fn radius(&self) -> i32 {
        self.radius
    }

    pub fn loaded_chunks(&self) -> usize {
        self.store.len()
    }

    pub fn stream_to(&mut self, center: ChunkCoord) {
        self.center = center;
        for z in center.z - self.radius..=center.z + self.radius {
            for x in center.x - self.radius..=center.x + self.radius {
                let coord = ChunkCoord::new(x, z);
                if self.store.get(coord).is_none() {
                    self.store.insert(self.generator.generate(coord));
                }
            }
        }
        self.store.retain_radius(center, self.radius);
    }

    pub fn voxel_at(&self, x: i32, y: i32, z: i32) -> Voxel {
        self.store
            .get(ChunkCoord::from_world(x, z))
            .map(|chunk| chunk.world_voxel(x, y, z))
            .unwrap_or(Voxel::Air)
    }

    pub fn biome_at(&self, x: i32, y: i32, z: i32) -> BiomeKind {
        self.store
            .get(ChunkCoord::from_world(x, z))
            .map(|chunk| chunk.biome_at(x, y, z))
            .unwrap_or(BiomeKind::Surface)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn chunk_coordinates_handle_negative_world_positions() {
        assert_eq!(ChunkCoord::from_world(0, 0), ChunkCoord::new(0, 0));
        assert_eq!(ChunkCoord::from_world(15, 15), ChunkCoord::new(0, 0));
        assert_eq!(ChunkCoord::from_world(16, 16), ChunkCoord::new(1, 1));
        assert_eq!(ChunkCoord::from_world(-1, -1), ChunkCoord::new(-1, -1));
        assert_eq!(ChunkCoord::from_world(-16, -16), ChunkCoord::new(-1, -1));
        assert_eq!(ChunkCoord::from_world(-17, -17), ChunkCoord::new(-2, -2));
    }

    #[test]
    fn adjacent_chunks_are_global_coordinate_continuous() {
        let generator = ChunkGenerator::default();
        let left = generator.generate(ChunkCoord::new(0, 0));
        let right = generator.generate(ChunkCoord::new(1, 0));
        for y in 0..32 {
            for z in 0..CHUNK_SIZE as i32 {
                assert_eq!(
                    left.world_voxel(15, y, z),
                    generator
                        .generate(ChunkCoord::new(0, 0))
                        .world_voxel(15, y, z)
                );
                assert_eq!(
                    right.world_voxel(16, y, z),
                    generator
                        .generate(ChunkCoord::new(1, 0))
                        .world_voxel(16, y, z)
                );
            }
        }
    }

    #[test]
    fn same_seed_replays_a_chunk_and_upper_bits_matter() {
        let generator = ChunkGenerator {
            world_seed: 42,
            ..ChunkGenerator::default()
        };
        let same = generator.generate(ChunkCoord::new(-3, 7));
        let replay = generator.generate(ChunkCoord::new(-3, 7));
        assert_eq!(same, replay);
        let different = ChunkGenerator {
            world_seed: 42 | (1_u64 << 48),
            ..generator
        }
        .generate(ChunkCoord::new(-3, 7));
        assert_ne!(same, different);
    }

    #[test]
    fn chunk_store_evicts_outside_the_loaded_radius() {
        let generator = ChunkGenerator::default();
        let mut store = ChunkStore::default();
        for x in -2..=2 {
            store.insert(generator.generate(ChunkCoord::new(x, 0)));
        }
        store.retain_radius(ChunkCoord::new(0, 0), 1);
        assert_eq!(store.len(), 3);
        assert!(store.get(ChunkCoord::new(-1, 0)).is_some());
        assert!(store.get(ChunkCoord::new(2, 0)).is_none());
    }

    #[test]
    fn streaming_world_keeps_a_bounded_square_and_replays_after_moving() {
        let generator = ChunkGenerator::default();
        let mut world = ChunkWorld::new(generator.clone(), ChunkCoord::new(0, 0), 1);
        assert_eq!(world.loaded_chunks(), 9);
        let first = world.voxel_at(0, 20, 0);
        world.stream_to(ChunkCoord::new(4, -3));
        assert_eq!(world.loaded_chunks(), 9);
        assert_eq!(
            world.voxel_at(4 * CHUNK_SIZE as i32, 20, -3 * CHUNK_SIZE as i32),
            generator.generate(ChunkCoord::new(4, -3)).world_voxel(
                4 * CHUNK_SIZE as i32,
                20,
                -3 * CHUNK_SIZE as i32,
            )
        );
        world.stream_to(ChunkCoord::new(0, 0));
        assert_eq!(world.voxel_at(0, 20, 0), first);
    }

    #[test]
    fn generated_cave_coordinates_are_classified_as_cave_biome() {
        let generator = ChunkGenerator {
            cave_program: CaveProgram {
                threshold: 0.5,
                ..CaveProgram::default()
            },
            ..ChunkGenerator::default()
        };
        let mut cave_count = 0;
        for chunk_z in -2..=2 {
            for chunk_x in -2..=2 {
                let chunk = generator.generate(ChunkCoord::new(chunk_x, chunk_z));
                for z in 0..CHUNK_SIZE as i32 {
                    for x in 0..CHUNK_SIZE as i32 {
                        for y in 4..24 {
                            cave_count += usize::from(chunk.biome_at(x, y, z) == BiomeKind::Cave);
                        }
                    }
                }
            }
        }
        assert!(
            cave_count > 0,
            "the generated chunk has no cave-biome samples"
        );
    }

    #[test]
    fn cellular_automata_caves_carve_open_regions_without_special_materials() {
        let generator = ChunkGenerator {
            cave_program: CaveProgram {
                shape: CaveShape::CellularAutomata,
                ..CaveProgram::default()
            },
            ..ChunkGenerator::default()
        };
        let chunk = generator.generate(ChunkCoord::new(0, 0));
        let mut air_inside_band = 0;
        for z in 0..CHUNK_SIZE as i32 {
            for x in 0..CHUNK_SIZE as i32 {
                for y in generator.cave_program.min_y..=generator.cave_program.max_y {
                    if chunk.world_voxel(x, y, z) == Voxel::Air
                        && chunk.biome_at(x, y, z) == BiomeKind::Cave
                    {
                        air_inside_band += 1;
                    }
                }
            }
        }
        assert!(
            air_inside_band > 0,
            "cellular cave produced no open regions"
        );
    }

    #[test]
    fn cellular_ramp_keeps_a_walkable_stair_floor() {
        let ramp = CellularRamp {
            start_y: 8,
            end_y: 24,
            start: (0, 0),
            end: (48, 0),
            half_width: 2,
        };
        // Half way along a three-voxel-run / one-voxel-rise ramp, the floor is
        // at Y=16. It must remain solid, while the headroom immediately above
        // it is carved. A generous ceiling keeps this about the ramp's own shape.
        let ceiling = 4096;
        assert_eq!(ramp.carve_override(24, 15, 0, ceiling), Some(false));
        assert_eq!(ramp.carve_override(24, 16, 0, ceiling), Some(true));
        assert_eq!(ramp.carve_override(24, 29, 0, ceiling), Some(true));
        assert_eq!(ramp.carve_override(24, 30, 0, ceiling), Some(false));
        assert_eq!(ramp.carve_override(24, 16, 3, ceiling), None);
    }

    /// A ramp's headroom must stop at the ceiling it is handed, or its 14 voxels
    /// of clearance carve straight through the terrain surface and open the cave
    /// to the sky. The regression this pins was visible as open trenches in the
    /// top cave band.
    #[test]
    fn a_ramp_never_carves_above_its_ceiling() {
        let ramp = CellularRamp {
            start_y: 8,
            end_y: 24,
            start: (0, 0),
            end: (48, 0),
            half_width: 2,
        };
        // Floor at Y=16, so without a ceiling the headroom would reach Y=29.
        assert_eq!(ramp.carve_override(24, 20, 0, 4096), Some(true));
        assert_eq!(ramp.carve_override(24, 20, 0, 18), None);
        for y in 19..=29 {
            assert_eq!(
                ramp.carve_override(24, y, 0, 18),
                None,
                "ramp carved Y={y} above its ceiling of 18"
            );
        }
    }

    /// Every value a node field can author must produce a world, not a panic.
    /// `surface_headroom` declares a 0..4096 range while the generated span is
    /// 64 voxels, so the out-of-range case is reachable from the UI alone.
    #[test]
    fn out_of_range_authored_values_are_clamped_instead_of_panicking() {
        for headroom in [-50, 0, 10, 62, 63, 64, 4096] {
            let generator = ChunkGenerator {
                surface_headroom: headroom,
                ..ChunkGenerator::default()
            };
            let chunk = generator.generate(ChunkCoord::new(0, 0));
            assert!(
                generator.sanitized().validate(),
                "headroom {headroom} did not sanitize into a valid configuration"
            );
            let has_terrain = (0..CHUNK_SIZE as i32)
                .flat_map(|x| (generator.min_y..chunk.max_y_exclusive()).map(move |y| (x, y)))
                .any(|(x, y)| chunk.world_voxel(x, y, 0).is_solid());
            assert!(has_terrain, "headroom {headroom} generated no terrain");
        }
        // A cave band inverted or pushed outside the span must also survive.
        let generator = ChunkGenerator {
            cave_program: CaveProgram {
                min_y: 900,
                max_y: -20,
                ..CaveProgram::default()
            },
            ..ChunkGenerator::default()
        };
        let _ = generator.generate(ChunkCoord::new(0, 0));
    }

    /// The whole point of the terrain-shape change: a default world must contain
    /// water. The old formula put every surface at or above the sea level, so the
    /// water branch was unreachable and oceans silently did not exist.
    #[test]
    fn a_default_world_contains_open_water_and_a_beach() {
        let generator = ChunkGenerator::default();
        let (lowest, highest) = generator.surface_range();
        assert!(
            lowest < generator.sea_level && highest > generator.sea_level,
            "terrain must straddle the sea level: {lowest}..{highest} around {}",
            generator.sea_level
        );
        let mut water = 0;
        let mut sand = 0;
        for chunk_x in -2..=2 {
            for chunk_z in -2..=2 {
                let chunk = generator.generate(ChunkCoord::new(chunk_x, chunk_z));
                let (origin_x, origin_z) = ChunkCoord::new(chunk_x, chunk_z).origin();
                for local_x in 0..CHUNK_SIZE as i32 {
                    for local_z in 0..CHUNK_SIZE as i32 {
                        for y in generator.min_y..chunk.max_y_exclusive() {
                            match chunk.world_voxel(origin_x + local_x, y, origin_z + local_z) {
                                Voxel::Water => water += 1,
                                Voxel::Sand => sand += 1,
                                _ => {}
                            }
                        }
                    }
                }
            }
        }
        assert!(water > 0, "a default world generated no water");
        assert!(sand > 0, "a default world generated no beach");
    }

    /// Cave fluids fill voids and always keep a bed: a water or lava voxel must
    /// never sit directly above air, or a lake hangs in an open shaft.
    #[test]
    fn cave_fluids_fill_voids_and_keep_a_solid_bed() {
        let generator = ChunkGenerator {
            cave_program: CaveProgram {
                shape: CaveShape::CellularAutomata,
                min_y: 4,
                max_y: 40,
                ..CaveProgram::default()
            },
            cave_fluids: CaveFluids {
                water_level: 28,
                lava_level: 16,
                min_y: 2,
                enabled: true,
            },
            ..ChunkGenerator::default()
        };
        let mut cave_water = 0;
        let mut cave_lava = 0;
        for chunk_x in -1..=1 {
            for chunk_z in -1..=1 {
                let coord = ChunkCoord::new(chunk_x, chunk_z);
                let chunk = generator.generate(coord);
                let (origin_x, origin_z) = coord.origin();
                for local_x in 0..CHUNK_SIZE as i32 {
                    for local_z in 0..CHUNK_SIZE as i32 {
                        for y in generator.min_y..chunk.max_y_exclusive() {
                            let world_x = origin_x + local_x;
                            let world_z = origin_z + local_z;
                            let voxel = chunk.world_voxel(world_x, y, world_z);
                            if chunk.biome_at(world_x, y, world_z) != BiomeKind::Cave {
                                continue;
                            }
                            match voxel {
                                Voxel::Water => {
                                    cave_water += 1;
                                    assert!(y <= 28, "cave water above its authored level at {y}");
                                }
                                Voxel::Lava => {
                                    cave_lava += 1;
                                    assert!(y <= 16, "cave lava above its authored level at {y}");
                                }
                                _ => continue,
                            }
                            assert!(
                                chunk.world_voxel(world_x, y - 1, world_z) != Voxel::Air,
                                "fluid at {y} has no bed under it"
                            );
                        }
                    }
                }
            }
        }
        assert!(cave_water > 0, "no underground water was placed");
        assert!(cave_lava > 0, "no underground lava was placed");
    }

    /// Disabling fluids must leave dry caves, so the toggle is a real authoring
    /// state and not merely a level of zero.
    #[test]
    fn disabled_cave_fluids_leave_dry_caves() {
        let generator = ChunkGenerator {
            cave_program: CaveProgram {
                shape: CaveShape::CellularAutomata,
                ..CaveProgram::default()
            },
            cave_fluids: CaveFluids {
                enabled: false,
                ..CaveFluids::default()
            },
            scatter: ScatterProgram {
                enabled: false,
                ..ScatterProgram::default()
            },
            ..ChunkGenerator::default()
        };
        let coord = ChunkCoord::new(0, 0);
        let chunk = generator.generate(coord);
        let (origin_x, origin_z) = coord.origin();
        for local_x in 0..CHUNK_SIZE as i32 {
            for local_z in 0..CHUNK_SIZE as i32 {
                for y in generator.cave_program.min_y..=generator.cave_program.max_y {
                    let world_x = origin_x + local_x;
                    let world_z = origin_z + local_z;
                    if chunk.biome_at(world_x, y, world_z) == BiomeKind::Cave {
                        assert_ne!(chunk.world_voxel(world_x, y, world_z), Voxel::Lava);
                    }
                }
            }
        }
    }

    /// Pass two must place decoration on cave surfaces and on the lit surface,
    /// and only ever into air.
    #[test]
    fn scatter_decorates_caves_and_the_surface_without_replacing_rock() {
        let generator = ChunkGenerator {
            cave_program: CaveProgram {
                shape: CaveShape::CellularAutomata,
                min_y: 4,
                max_y: 44,
                ..CaveProgram::default()
            },
            ..ChunkGenerator::default()
        };
        let mut cache = CellularFieldCache::new();
        let mut surface_cover = 0;
        let mut cave_cover = 0;
        for chunk_x in -1..=1 {
            for chunk_z in -1..=1 {
                let coord = ChunkCoord::new(chunk_x, chunk_z);
                let chunk = generator.generate_with_cache(coord, &mut cache);
                let (origin_x, origin_z) = coord.origin();
                for local_x in 0..CHUNK_SIZE as i32 {
                    for local_z in 0..CHUNK_SIZE as i32 {
                        for y in generator.min_y..chunk.max_y_exclusive() {
                            let world_x = origin_x + local_x;
                            let world_z = origin_z + local_z;
                            let voxel = chunk.world_voxel(world_x, y, world_z);
                            if !voxel.is_scatter_cover() {
                                continue;
                            }
                            if chunk.biome_at(world_x, y, world_z) == BiomeKind::Cave {
                                cave_cover += 1;
                            } else {
                                surface_cover += 1;
                            }
                        }
                    }
                }
                assert_eq!(
                    chunk.scatter_tally().total() > 0,
                    true,
                    "chunk {chunk_x},{chunk_z} tallied no decoration"
                );
            }
        }
        assert!(surface_cover > 0, "no surface decoration was placed");
        assert!(cave_cover > 0, "no cave decoration was placed");
    }

    /// The pipeline must be readable as stages with real percentages — that is the
    /// whole point of the report. Each stage's hit rate has to be a sensible
    /// fraction of what it was offered, and the layer view has to show vertical
    /// structure rather than one flat total.
    #[test]
    fn the_stage_report_measures_every_stage_and_its_layers() {
        use crate::stage_report::GenerationStage;
        let generator = ChunkGenerator {
            cave_program: CaveProgram {
                shape: CaveShape::CellularAutomata,
                min_y: 4,
                max_y: 44,
                ..CaveProgram::default()
            },
            ..ChunkGenerator::default()
        };
        let chunk = generator.generate(ChunkCoord::new(0, 0));
        let report = chunk.stage_report();

        // Terrain is offered every voxel of the column and fills the part below
        // the surface, so its hit rate is a real fraction rather than 0 or 1.
        let terrain = report.stage(GenerationStage::Terrain);
        assert_eq!(
            terrain.considered,
            (CHUNK_SIZE * CHUNK_SIZE * generator.height) as u32
        );
        assert!(
            terrain.hit_rate() > 0.1 && terrain.hit_rate() < 0.95,
            "terrain filled {:.0}% of the column, which is not a shape",
            terrain.hit_rate() * 100.0
        );

        // Carve is only offered solid rock, and must take some of it but not all:
        // taking none was the Worley-threshold bug, taking all is a hollow world.
        let carve = report.stage(GenerationStage::Carve);
        assert!(carve.considered > 0, "no rock was offered to the carve");
        assert!(
            carve.hit_rate() > 0.0 && carve.hit_rate() < 0.9,
            "carve took {:.0}% of the rock",
            carve.hit_rate() * 100.0
        );

        // Fluids are offered exactly the carved voids.
        let fluids = report.stage(GenerationStage::Fluids);
        assert_eq!(
            fluids.considered, carve.changed,
            "fluids must be offered exactly the voids the carve opened"
        );
        assert!(fluids.changed > 0, "no carved void was filled");

        let scatter = report.stage(GenerationStage::Scatter);
        assert!(scatter.considered > 0 && scatter.changed > 0);
        assert!(
            scatter.hit_rate() < 0.8,
            "decoration covered {:.0}% of every candidate surface",
            scatter.hit_rate() * 100.0
        );

        // Material composition: stone must dominate, and the total must account
        // for every voxel exactly once.
        assert_eq!(
            report.materials.total(),
            (CHUNK_SIZE * CHUNK_SIZE * generator.height) as u32
        );
        assert!(report.materials.share(Voxel::Stone) > 0.1);
        assert!(report.materials.count(Voxel::CaveMoss) > 0);

        // Layers: the vertical structure must vary, not be one repeated row.
        let layers = report.layers();
        assert_eq!(layers.len(), generator.height);
        let carved_per_layer: Vec<u32> = layers.iter().map(|layer| layer.carved).collect();
        assert!(
            carved_per_layer.iter().any(|count| *count > 0),
            "no layer records carved space"
        );
        assert!(
            carved_per_layer
                .iter()
                .collect::<std::collections::BTreeSet<_>>()
                .len()
                > 3,
            "every layer carved the same amount, so the view has no structure"
        );
        // Nothing may be carved outside the authored band.
        for layer in layers {
            if layer.world_y < generator.cave_program.min_y
                || layer.world_y > generator.cave_program.max_y
            {
                assert_eq!(
                    layer.carved + layer.lava,
                    0,
                    "layer Y {} is outside the cave band but records carving",
                    layer.world_y
                );
            }
        }
    }

    /// The region field cache must not change a single voxel: it is a memo of a
    /// pure function, so a cached build and an uncached build must agree exactly.
    #[test]
    fn the_field_cache_does_not_change_generated_output() {
        let generator = ChunkGenerator {
            cave_program: CaveProgram {
                shape: CaveShape::CellularAutomata,
                ..CaveProgram::default()
            },
            ..ChunkGenerator::default()
        };
        let mut cache = CellularFieldCache::new();
        // Chunks 0..=7 on both axes are the 128-voxel region at the origin, which
        // is what makes the redundancy measurable: 64 chunks, one region.
        let mut chunks = 0;
        for chunk_x in 0..8 {
            for chunk_z in 0..8 {
                let coord = ChunkCoord::new(chunk_x, chunk_z);
                assert_eq!(
                    generator.generate(coord),
                    generator.generate_with_cache(coord, &mut cache),
                    "cached generation differs at {coord:?}"
                );
                chunks += 1;
            }
        }
        // Four Y bands across a 64-voxel span, and one region: four fields, not
        // one per chunk per band. This is the 64x redundancy the cache removes.
        assert_eq!(chunks, 64);
        assert_eq!(
            cache.len(),
            4,
            "one region of 64 chunks must share exactly its four band fields"
        );
    }

    #[test]
    fn cellular_strata_receive_a_gradual_connector() {
        let program = CaveProgram {
            shape: CaveShape::CellularAutomata,
            ..CaveProgram::default()
        };
        let lower = program.cellular_field_at(1, 8, 0, 0);
        let upper = program.cellular_field_at(1, 16, 0, 0);
        let ramp = CellularRamp::between(&lower, &upper, 8, 16, program.cellular_cell_size)
            .expect("the connected cellular chambers need a walkable transition");
        assert!(
            (ramp.end.0 - ramp.start.0).pow(2) + (ramp.end.1 - ramp.start.1).pow(2)
                >= (8 * CellularRamp::MIN_RUN_PER_RISE).pow(2)
        );
    }

    #[test]
    fn terrain_reserves_the_authored_surface_headroom() {
        let generator = ChunkGenerator {
            height: 32,
            sea_level: 12,
            surface_headroom: 10,
            ..ChunkGenerator::default()
        };
        let chunk = generator.generate(ChunkCoord::new(0, 0));
        let surfaces: Vec<_> = (0..CHUNK_SIZE as i32)
            .flat_map(|x| (0..CHUNK_SIZE as i32).map(move |z| (x, z)))
            .flat_map(|(x, z)| {
                (generator.min_y..generator.min_y + generator.height as i32).map(move |y| (x, y, z))
            })
            .filter(|&(x, y, z)| chunk.world_voxel(x, y, z).is_solid())
            .map(|(_, y, _)| y)
            .collect();
        let highest_surface = surfaces
            .iter()
            .copied()
            .max()
            .expect("generated chunk has terrain");
        let lowest_surface = surfaces
            .iter()
            .copied()
            .min()
            .expect("generated chunk has terrain");
        assert!(highest_surface <= 21);
        assert!(
            highest_surface - lowest_surface >= 2,
            "terrain should have visible relief"
        );
        assert!(
            generator.min_y + generator.height as i32 - highest_surface - 1
                >= generator.surface_headroom
        );
    }
}
