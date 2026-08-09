//! Deterministic cave carving for the greenfield world generator.
//!
//! A cave program is sampled in authoritative one-metre coordinates. It only
//! returns a mask; terrain ownership, fluids, materials and biome selection
//! remain separate passes. That makes the expensive field cacheable per chunk
//! and leaves room for the later aquifer pass without changing the cave API.

use serde::{Deserialize, Serialize};

use std::collections::{BTreeMap, VecDeque};

use crate::noise::{
    billow_noise_3d, hash_3d_u64, hash_to_unit_u64, value_noise_3d, worley_noise_3d,
};

const CELLULAR_REGION_SIZE: i32 = 64;
/// Vertical coherence band for the 3D cellular field. Reusing one 2D room map
/// over several Y layers keeps deep worlds affordable while the band seed
/// still changes the layout as the player descends.
pub const CELLULAR_Y_BAND_SIZE: i32 = 16;

/// The first family of cave shapes supported by the greenfield generator.
///
/// These names mirror the useful split in modern voxel generators: broad
/// chambers (`Cheese`), winding tunnels (`Spaghetti`), and thin connectors
/// (`Noodle`). They are deliberately a shape choice, not a biome choice.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CaveShape {
    #[default]
    Cheese,
    Spaghetti,
    Noodle,
    CellularAutomata,
}

/// A deterministic, serialisable cave-carving program.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaveProgram {
    pub shape: CaveShape,
    pub min_y: i32,
    pub max_y: i32,
    /// Horizontal lattice size in world voxels.
    pub horizontal_scale: f32,
    /// Vertical lattice size in world voxels.
    pub vertical_scale: f32,
    /// Density above which a solid terrain voxel is carved to air.
    pub threshold: f32,
    /// Stable per-program salt. The world seed is never replaced by this value.
    pub salt: u64,
    /// Initial wall percentage for the cellular-automata cave recipe.
    #[serde(default = "default_cellular_fill")]
    pub cellular_fill: f32,
    /// Number of cellular-automata smoothing passes.
    #[serde(default = "default_cellular_iterations")]
    pub cellular_iterations: u32,
    /// Final 3x3 cleanup passes using only the R1 rule. This mirrors the
    /// room-forming/cleanup split used by the classic cave recipe.
    #[serde(default = "default_cellular_cleanup_iterations")]
    pub cellular_cleanup_iterations: u32,
    /// The 3x3 wall-count cutoff in the 4–5 rule.
    #[serde(default = "default_cellular_r1_cutoff")]
    pub cellular_r1_cutoff: u32,
    /// The 5x5 cleanup cutoff; negative disables the second-radius rule.
    #[serde(default = "default_cellular_r2_cutoff")]
    pub cellular_r2_cutoff: i32,
    /// World-voxel size represented by one cellular-automata cell. Larger
    /// cells produce rooms instead of one-voxel trenches.
    #[serde(default = "default_cellular_cell_size")]
    pub cellular_cell_size: u32,
    /// World-space frequency for the 3D Worley chamber field.
    #[serde(default = "default_cellular_worley_scale")]
    pub cellular_worley_scale: f32,
    /// World-space frequency for the 3D Billow ceiling/floor detail field.
    #[serde(default = "default_cellular_billow_scale")]
    pub cellular_billow_scale: f32,
    /// Amount of Billow detail subtracted from the Worley volume.
    #[serde(default = "default_cellular_billow_strength")]
    pub cellular_billow_strength: f32,
    /// Number of Billow detail octaves.
    #[serde(default = "default_cellular_billow_octaves")]
    pub cellular_billow_octaves: u32,
    /// How much of a CA-open column actually opens, in `[0, 1]`.
    ///
    /// The cellular recipe's own knob, deliberately NOT [`Self::threshold`].
    /// `threshold` cuts a value-noise density whose mean is 0.5, which is why 0.70
    /// is a sensible cut for `Cheese`; the cellular branch's 3D component is a
    /// Worley *chamber* field with a different distribution entirely, and reusing
    /// the same number there meant the 3D gate almost never opened — measured at
    /// zero hits per band, so cave shape came down to rare spikes and whole depth
    /// bands generated no caves at all.
    #[serde(default = "default_cellular_openness")]
    pub cellular_openness: f32,
    /// Smallest open region, in CA cells, that earns a corridor back to the main
    /// chamber. Regions below it are left as sealed pockets and removed.
    ///
    /// Authored by `biome.cellular_connect`. It was a hardcoded 12 inside the
    /// field builder, which meant the one control that decides whether a cave
    /// system is connected or a field of unreachable holes was not authorable.
    #[serde(default = "default_cellular_min_room_cells")]
    pub cellular_min_room_cells: u32,
    /// Half-width of a carved corridor, in CA cells. 1 gives the classic
    /// three-cell tunnel; 0 gives a one-cell crawl.
    #[serde(default = "default_cellular_corridor_radius")]
    pub cellular_corridor_radius: u32,
}

fn default_cellular_fill() -> f32 {
    0.40
}

fn default_cellular_iterations() -> u32 {
    4
}

fn default_cellular_cleanup_iterations() -> u32 {
    3
}

fn default_cellular_r1_cutoff() -> u32 {
    5
}

fn default_cellular_r2_cutoff() -> i32 {
    2
}

fn default_cellular_cell_size() -> u32 {
    2
}

fn default_cellular_worley_scale() -> f32 {
    0.05
}

fn default_cellular_billow_scale() -> f32 {
    0.08
}

fn default_cellular_billow_strength() -> f32 {
    0.40
}

fn default_cellular_billow_octaves() -> u32 {
    3
}

fn default_cellular_openness() -> f32 {
    0.72
}

fn default_cellular_min_room_cells() -> u32 {
    12
}

fn default_cellular_corridor_radius() -> u32 {
    1
}

impl Default for CaveProgram {
    fn default() -> Self {
        Self {
            shape: CaveShape::Cheese,
            min_y: 8,
            max_y: 56,
            horizontal_scale: 40.0,
            vertical_scale: 20.0,
            threshold: 0.70,
            salt: 0xCAFE_0001,
            cellular_fill: default_cellular_fill(),
            cellular_iterations: default_cellular_iterations(),
            cellular_cleanup_iterations: default_cellular_cleanup_iterations(),
            cellular_r1_cutoff: default_cellular_r1_cutoff(),
            cellular_r2_cutoff: default_cellular_r2_cutoff(),
            cellular_cell_size: default_cellular_cell_size(),
            cellular_worley_scale: default_cellular_worley_scale(),
            cellular_billow_scale: default_cellular_billow_scale(),
            cellular_billow_strength: default_cellular_billow_strength(),
            cellular_billow_octaves: default_cellular_billow_octaves(),
            cellular_openness: default_cellular_openness(),
            cellular_min_room_cells: default_cellular_min_room_cells(),
            cellular_corridor_radius: default_cellular_corridor_radius(),
        }
    }
}

impl CaveProgram {
    /// Returns whether this program can be evaluated safely.
    pub fn is_valid(&self) -> bool {
        self.min_y <= self.max_y
            && self.horizontal_scale.is_finite()
            && self.horizontal_scale > 0.0
            && self.vertical_scale.is_finite()
            && self.vertical_scale > 0.0
            && self.threshold.is_finite()
            && (0.0..=1.0).contains(&self.threshold)
            && self.cellular_fill.is_finite()
            && (0.0..=1.0).contains(&self.cellular_fill)
            && self.cellular_iterations <= 32
            && self.cellular_cleanup_iterations <= 32
            && self.cellular_r1_cutoff <= 9
            && self.cellular_r2_cutoff <= 25
            && (1..=8).contains(&self.cellular_cell_size)
            && self.cellular_worley_scale.is_finite()
            && self.cellular_worley_scale > 0.0
            && self.cellular_billow_scale.is_finite()
            && self.cellular_billow_scale > 0.0
            && self.cellular_billow_strength.is_finite()
            && (0.0..=1.0).contains(&self.cellular_billow_strength)
            && self.cellular_billow_octaves <= 8
            && self.cellular_openness.is_finite()
            && (0.0..=1.0).contains(&self.cellular_openness)
            && self.cellular_min_room_cells <= 4096
            && self.cellular_corridor_radius <= 8
    }

    /// The density cut this program's shape uses.
    ///
    /// `Cheese`, `Spaghetti` and `Noodle` share one value-noise density and cut it
    /// at [`Self::threshold`]. The cellular branch cuts its chamber field at
    /// `1 - cellular_openness` instead, so its authored control reads as "how open
    /// are the rooms" rather than as a threshold on a different distribution.
    pub fn density_cut(&self) -> f32 {
        match self.shape {
            CaveShape::CellularAutomata => 1.0 - self.cellular_openness,
            _ => self.threshold,
        }
    }

    /// Samples the normalised cave density at one world-voxel coordinate.
    /// Values outside the configured vertical band are always zero.
    pub fn sample(&self, world_seed: u64, x: i32, y: i32, z: i32) -> f32 {
        if !self.is_valid() || y < self.min_y || y > self.max_y {
            return 0.0;
        }
        let seed = world_seed.wrapping_add(self.salt);
        let horizontal = self.horizontal_scale;
        let vertical = self.vertical_scale;
        match self.shape {
            CaveShape::Cheese => value_noise_3d(
                x as f32 / horizontal,
                y as f32 / vertical,
                z as f32 / horizontal,
                seed,
            ),
            CaveShape::Spaghetti => {
                let broad = value_noise_3d(
                    x as f32 / (horizontal * 0.75),
                    y as f32 / vertical,
                    z as f32 / (horizontal * 0.75),
                    seed,
                );
                let strand = value_noise_3d(
                    x as f32 / (horizontal * 0.35),
                    y as f32 / (vertical * 1.6),
                    z as f32 / (horizontal * 0.35),
                    seed ^ 0x9E37_79B9_7F4A_7C15,
                );
                (1.0 - (broad - 0.5).abs() * 2.0).max(0.0) * strand
            }
            CaveShape::Noodle => {
                let path = value_noise_3d(
                    x as f32 / (horizontal * 0.45),
                    y as f32 / (vertical * 2.4),
                    z as f32 / (horizontal * 0.45),
                    seed,
                );
                let thickness = value_noise_3d(
                    x as f32 / (horizontal * 0.18),
                    y as f32 / (vertical * 0.8),
                    z as f32 / (horizontal * 0.18),
                    seed ^ 0xBF58_476D_1CE4_E5B9,
                );
                (1.0 - (path - 0.5).abs() * 2.0).max(0.0) * thickness
            }
            // The cellular field supplies the broad room topology. This
            // scalar component supplies rounded chambers and uneven ceilings
            // and floors before the field mask is intersected below.
            CaveShape::CellularAutomata => {
                // `worley_noise_3d` returns DISTANCE to the nearest feature point,
                // so it is smallest inside a chamber. Inverting it makes the value
                // read as openness — high in a chamber's middle, low out at its
                // rim — which is what the billow texture then eats into to give
                // uneven floors, ceilings and arches.
                let chamber = 1.0
                    - worley_noise_3d(
                        x as f32 * self.cellular_worley_scale,
                        y as f32 * self.cellular_worley_scale,
                        z as f32 * self.cellular_worley_scale,
                        seed ^ 0xD6E8_FEB8_6659_FD93,
                    );
                let texture = billow_noise_3d(
                    x as f32 * self.cellular_billow_scale,
                    y as f32 * self.cellular_billow_scale,
                    z as f32 * self.cellular_billow_scale,
                    self.cellular_billow_octaves,
                    seed ^ 0xA5A3_564D_4F3B_1D29,
                );
                // Keep the result in the existing `[0, 1]` density contract.
                (chamber - texture * self.cellular_billow_strength).clamp(0.0, 1.0)
            }
        }
    }

    /// Whether a solid voxel should be changed to air by this program.
    pub fn carves(&self, world_seed: u64, x: i32, y: i32, z: i32) -> bool {
        self.sample(world_seed, x, y, z) >= self.density_cut()
    }

    pub fn carves_with_cellular_field(
        &self,
        field: &CellularAutomataField,
        world_seed: u64,
        x: i32,
        y: i32,
        z: i32,
    ) -> bool {
        if y < self.min_y || y > self.max_y {
            return false;
        }
        match self.shape {
            CaveShape::CellularAutomata => {
                // The CA field supplies the broad room topology, while the
                // scalar 3D field supplies floors, ceilings, and arches. The
                // old mask-only branch extruded each 2D room unchanged
                // through the complete vertical band, which is the source of
                // the tall pillars in the preview.
                !field.is_wall(x, z) && self.carves(world_seed, x, y, z)
            }
            _ => self.carves(world_seed, x, y, z),
        }
    }

    /// Build one deterministic, globally aligned cellular-automata region.
    /// Aligning regions to a fixed world lattice makes neighbouring chunks see
    /// the same smoothed cells instead of introducing chunk-edge seams.
    pub fn cellular_field(&self, world_seed: u64, x: i32, z: i32) -> CellularAutomataField {
        self.cellular_field_at(world_seed, 0, x, z)
    }

    /// Build a depth-layered field. Four adjacent Y layers share a map so
    /// chambers have vertical continuity without becoming one straight shaft.
    pub fn cellular_field_at(
        &self,
        world_seed: u64,
        y: i32,
        x: i32,
        z: i32,
    ) -> CellularAutomataField {
        let cell_size = self.cellular_cell_size as i32;
        let region_world_size = CELLULAR_REGION_SIZE * cell_size;
        let world_origin_x = x.div_euclid(region_world_size) * region_world_size;
        let world_origin_z = z.div_euclid(region_world_size) * region_world_size;
        let origin_x = world_origin_x.div_euclid(cell_size);
        let origin_z = world_origin_z.div_euclid(cell_size);
        let size = CELLULAR_REGION_SIZE as usize;
        let y_band = y.div_euclid(CELLULAR_Y_BAND_SIZE) as i64 as u64;
        let seed = world_seed
            .wrapping_add(self.salt)
            .wrapping_add(y_band.wrapping_mul(0x9E37_79B9_7F4A_7C15));
        let mut current = vec![true; size * size];
        for local_z in 1..size - 1 {
            for local_x in 1..size - 1 {
                let hash = hash_3d_u64(
                    origin_x + local_x as i32,
                    0,
                    origin_z + local_z as i32,
                    seed,
                );
                current[local_z * size + local_x] = hash_to_unit_u64(hash) < self.cellular_fill;
            }
        }
        let mut next = current.clone();
        for _ in 0..self.cellular_iterations {
            for local_z in 1..size - 1 {
                for local_x in 1..size - 1 {
                    let wall_at = |x: i32, z: i32| {
                        if x < 0 || z < 0 || x >= CELLULAR_REGION_SIZE || z >= CELLULAR_REGION_SIZE
                        {
                            true
                        } else {
                            current[z as usize * size + x as usize]
                        }
                    };
                    let mut r1 = 0_u32;
                    for dz in -1..=1 {
                        for dx in -1..=1 {
                            if wall_at(local_x as i32 + dx, local_z as i32 + dz) {
                                r1 += 1;
                            }
                        }
                    }
                    let mut r2 = 0_u32;
                    for dz in -2_i32..=2 {
                        for dx in -2_i32..=2 {
                            if dz.abs() == 2 && dx.abs() == 2 {
                                continue;
                            }
                            if wall_at(local_x as i32 + dx, local_z as i32 + dz) {
                                r2 += 1;
                            }
                        }
                    }
                    next[local_z * size + local_x] = r1 >= self.cellular_r1_cutoff
                        || (self.cellular_r2_cutoff >= 0 && r2 <= self.cellular_r2_cutoff as u32);
                }
            }
            std::mem::swap(&mut current, &mut next);
        }
        // A short strict-R1 cleanup removes isolated wall pixels without
        // reopening the broad rooms made by the R2 rule above. This is the
        // characteristic second pass in the Sebastian Lague-style recipe.
        for _ in 0..self.cellular_cleanup_iterations {
            for local_z in 1..size - 1 {
                for local_x in 1..size - 1 {
                    let wall_at = |x: i32, z: i32| {
                        if x < 0 || z < 0 || x >= CELLULAR_REGION_SIZE || z >= CELLULAR_REGION_SIZE
                        {
                            true
                        } else {
                            current[z as usize * size + x as usize]
                        }
                    };
                    let mut r1 = 0_u32;
                    for dz in -1..=1 {
                        for dx in -1..=1 {
                            if wall_at(local_x as i32 + dx, local_z as i32 + dz) {
                                r1 += 1;
                            }
                        }
                    }
                    next[local_z * size + local_x] = r1 >= self.cellular_r1_cutoff;
                }
            }
            std::mem::swap(&mut current, &mut next);
        }
        // Analyse open regions before cleanup. The largest region is the
        // anchor; sizeable secondary regions are joined to it with short
        // tunnels. This is the useful part of the classic cavern recipe: a
        // smoothed mask gives the rooms, region analysis makes them playable,
        // and explicit links prevent a world full of unreachable pockets.
        let regions = open_regions(&current, size);

        let largest_region = regions
            .iter()
            .enumerate()
            .max_by_key(|(_, region)| region.len())
            .map(|(index, _)| index);
        let mut corridors = Vec::new();
        if let Some(largest_index) = largest_region {
            // Build a closest-room connection tree rather than connecting
            // every room directly to the largest chamber. The tree produces
            // the branching layout seen in the reference maps and avoids a
            // telltale starburst of long tunnels.
            let meaningful = regions
                .iter()
                .map(|region| region.len() >= self.cellular_min_room_cells as usize)
                .collect::<Vec<_>>();
            let mut connected = vec![false; regions.len()];
            connected[largest_index] = true;
            loop {
                let mut best: Option<(u32, usize, usize, usize)> = None;
                for (connected_index, connected_region) in regions.iter().enumerate() {
                    if !connected[connected_index] {
                        continue;
                    }
                    for (region_index, region) in regions.iter().enumerate() {
                        if connected[region_index] || !meaningful[region_index] {
                            continue;
                        }
                        let start_index = region[region.len() / 2];
                        let start_x = start_index % size;
                        let start_z = start_index / size;
                        let connected_point = connected_region
                            .iter()
                            .min_by_key(|&&index| {
                                (index % size).abs_diff(start_x) + (index / size).abs_diff(start_z)
                            })
                            .copied()
                            .unwrap_or_else(|| connected_region[connected_region.len() / 2]);
                        let connected_x = connected_point % size;
                        let connected_z = connected_point / size;
                        let distance = (connected_x as i32 - start_x as i32).unsigned_abs()
                            + (connected_z as i32 - start_z as i32).unsigned_abs();
                        if best.is_none_or(|candidate| distance < candidate.0) {
                            best = Some((distance, region_index, connected_point, start_index));
                        }
                    }
                }
                let Some((_, region_index, target_index, start_index)) = best else {
                    break;
                };
                connected[region_index] = true;
                let target = (target_index % size, target_index / size);
                let start = (start_index % size, start_index / size);
                corridors.push((start, target));
                let steps = (target.0 as i32 - start.0 as i32)
                    .unsigned_abs()
                    .max((target.1 as i32 - start.1 as i32).unsigned_abs());
                for step in 0..=steps {
                    let along = if steps == 0 {
                        0.0
                    } else {
                        step as f32 / steps as f32
                    };
                    let center_x = (start.0 as f32 + (target.0 as f32 - start.0 as f32) * along)
                        .round() as i32;
                    let center_z = (start.1 as f32 + (target.1 as f32 - start.1 as f32) * along)
                        .round() as i32;
                    let radius = self.cellular_corridor_radius as i32;
                    for dz in -radius..=radius {
                        for dx in -radius..=radius {
                            let carve_x = center_x + dx;
                            let carve_z = center_z + dz;
                            if carve_x > 0
                                && carve_z > 0
                                && carve_x < CELLULAR_REGION_SIZE - 1
                                && carve_z < CELLULAR_REGION_SIZE - 1
                            {
                                current[carve_z as usize * size + carve_x as usize] = false;
                            }
                        }
                    }
                }
            }
        }

        // Keep the connected network and remove isolated specks. Re-running
        // the flood fill after corridor carving makes the invariant explicit.
        let mut keep = vec![false; current.len()];
        let largest = open_regions(&current, size)
            .into_iter()
            .max_by_key(|region| region.len())
            .unwrap_or_default();
        for index in largest {
            keep[index] = true;
        }
        for (index, cell) in current.iter_mut().enumerate() {
            if !*cell && !keep[index] {
                *cell = true;
            }
        }
        CellularAutomataField {
            origin_x,
            origin_z,
            cell_size,
            size,
            walls: current,
            corridors,
        }
    }
}

/// Every connected run of open cells in a CA grid, as index lists.
///
/// One function rather than the two hand-inlined breadth-first searches this
/// module used to carry: they were the same walk, and the second one's copy of
/// the neighbour list had its entries in a different order, which is exactly the
/// kind of difference that is invisible until one of them is edited.
///
/// The one-cell border is deliberately excluded, so a region can never touch the
/// region edge and neighbouring regions always meet in rock.
fn open_regions(walls: &[bool], size: usize) -> Vec<Vec<usize>> {
    let mut visited = vec![false; walls.len()];
    let mut regions = Vec::new();
    for start_z in 1..size - 1 {
        for start_x in 1..size - 1 {
            let start = start_z * size + start_x;
            if walls[start] || visited[start] {
                continue;
            }
            visited[start] = true;
            let mut queue = VecDeque::from([(start_x, start_z)]);
            let mut region = Vec::new();
            while let Some((cell_x, cell_z)) = queue.pop_front() {
                region.push(cell_z * size + cell_x);
                for (neighbour_x, neighbour_z) in [
                    (cell_x - 1, cell_z),
                    (cell_x + 1, cell_z),
                    (cell_x, cell_z - 1),
                    (cell_x, cell_z + 1),
                ] {
                    // Interior-only walk: the loop starts at 1 and the border is
                    // always wall, so `cell_x - 1` cannot underflow.
                    if neighbour_x == 0
                        || neighbour_z == 0
                        || neighbour_x >= size - 1
                        || neighbour_z >= size - 1
                    {
                        continue;
                    }
                    let index = neighbour_z * size + neighbour_x;
                    if !walls[index] && !visited[index] {
                        visited[index] = true;
                        queue.push_back((neighbour_x, neighbour_z));
                    }
                }
            }
            regions.push(region);
        }
    }
    regions
}

/// One room-to-room corridor, as the pair of CA-cell coordinates it joins.
///
/// Named because three signatures across two crates were restating the nested
/// tuple, and a reader cannot tell `((usize, usize), (usize, usize))` apart from
/// any other pair of pairs.
pub type CorridorLink = ((usize, usize), (usize, usize));

/// Which face of a cave an open voxel sits against.
///
/// Published by the carve pass so the scatter pass can ask "is this a floor"
/// without re-deriving the cave mask: mushrooms want floors, vines want
/// ceilings, and crystals want either. `Interior` is open air touching nothing.
#[derive(Clone, Copy, Debug, Default, Eq, PartialEq)]
pub enum CaveSurface {
    #[default]
    Interior,
    Floor,
    Ceiling,
    Wall,
}

/// Underground fluid levels, in world-voxel Y.
///
/// A carved void at or below a level fills with that fluid. Lava wins where the
/// two overlap, because a lava level below the water table is the interesting
/// arrangement and the alternative (water quenching it) needs a reaction the
/// generator does not model.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct CaveFluids {
    /// Highest world voxel that underground water fills. Below [`Self::min_y`]
    /// nothing fills, which is what lets a dry cave system exist.
    pub water_level: i32,
    /// Highest world voxel that lava fills.
    pub lava_level: i32,
    /// Lowest world voxel any fluid may occupy, so the deepest strata stay dry
    /// and a lake always has rock beneath it.
    pub min_y: i32,
    pub enabled: bool,
}

impl Default for CaveFluids {
    fn default() -> Self {
        Self {
            water_level: 20,
            lava_level: 12,
            min_y: 2,
            enabled: true,
        }
    }
}

impl CaveFluids {
    /// The fluid that fills a carved void at this height, if any.
    pub fn fluid_at(&self, y: i32) -> Option<CaveFluid> {
        if !self.enabled || y < self.min_y {
            return None;
        }
        if y <= self.lava_level {
            Some(CaveFluid::Lava)
        } else if y <= self.water_level {
            Some(CaveFluid::Water)
        } else {
            None
        }
    }

    /// The highest voxel any fluid can reach, for the generator's bed check.
    pub fn highest_level(&self) -> i32 {
        self.water_level.max(self.lava_level)
    }
}

/// Which fluid fills a void. Deliberately not `Voxel`: `voxel-core`'s cave
/// module describes cave physics, and the material choice belongs to the pass
/// that writes voxels.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CaveFluid {
    Water,
    Lava,
}

/// Memoised cellular fields, keyed by the region and band they belong to.
///
/// [`CaveProgram::cellular_field_at`] snaps its coordinates to a region lattice,
/// so every chunk inside one region asks for a byte-identical field. Building it
/// per chunk meant the shipped world computed 256 fields where 4 exist — CA
/// iterations, two flood fills, and an O(regions²) corridor search each time.
#[derive(Clone, Debug, Default)]
pub struct CellularFieldCache {
    fields: BTreeMap<(i32, i32, i32), CellularAutomataField>,
}

impl CellularFieldCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// The field covering this coordinate and band, building it on first ask.
    pub fn field_at(
        &mut self,
        program: &CaveProgram,
        world_seed: u64,
        y: i32,
        x: i32,
        z: i32,
    ) -> &CellularAutomataField {
        let cell_size = program.cellular_cell_size.max(1) as i32;
        let region_world_size = CELLULAR_REGION_SIZE * cell_size;
        let key = (
            x.div_euclid(region_world_size),
            z.div_euclid(region_world_size),
            y.div_euclid(CELLULAR_Y_BAND_SIZE),
        );
        self.fields
            .entry(key)
            .or_insert_with(|| program.cellular_field_at(world_seed, y, x, z))
    }

    pub fn len(&self) -> usize {
        self.fields.len()
    }

    pub fn is_empty(&self) -> bool {
        self.fields.is_empty()
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct CellularAutomataField {
    origin_x: i32,
    origin_z: i32,
    cell_size: i32,
    size: usize,
    walls: Vec<bool>,
    corridors: Vec<CorridorLink>,
}

impl CellularAutomataField {
    pub fn is_wall(&self, x: i32, z: i32) -> bool {
        let local_x = x.div_euclid(self.cell_size) - self.origin_x;
        let local_z = z.div_euclid(self.cell_size) - self.origin_z;
        if local_x < 0
            || local_z < 0
            || local_x as usize >= self.size
            || local_z as usize >= self.size
        {
            true
        } else {
            self.walls[local_z as usize * self.size + local_x as usize]
        }
    }

    /// Room-to-room links carved into this field, expressed in local CA-cell
    /// coordinates for previews and generation diagnostics.
    pub fn corridor_links(&self) -> &[CorridorLink] {
        &self.corridors
    }

    /// World-space centre of the open cell nearest a local CA-grid coordinate.
    /// The generator uses this to attach a vertical route to a real chamber,
    /// rather than tunnelling toward an arbitrary point that might be solid.
    pub(crate) fn nearest_open_cell(
        &self,
        preferred_x: usize,
        preferred_z: usize,
    ) -> Option<(i32, i32)> {
        let preferred_x = preferred_x.min(self.size.saturating_sub(1));
        let preferred_z = preferred_z.min(self.size.saturating_sub(1));
        self.walls
            .iter()
            .enumerate()
            .filter(|(_, wall)| !**wall)
            .min_by_key(|(index, _)| {
                let x = index % self.size;
                let z = index / self.size;
                x.abs_diff(preferred_x).pow(2) + z.abs_diff(preferred_z).pow(2)
            })
            .map(|(index, _)| {
                let x = index % self.size;
                let z = index / self.size;
                (
                    (self.origin_x + x as i32) * self.cell_size + self.cell_size / 2,
                    (self.origin_z + z as i32) * self.cell_size + self.cell_size / 2,
                )
            })
    }

    /// World-space centre of the open cell farthest from `origin`.
    pub(crate) fn farthest_open_cell_from(&self, origin: (i32, i32)) -> Option<(i32, i32)> {
        self.walls
            .iter()
            .enumerate()
            .filter(|(_, wall)| !**wall)
            .max_by_key(|(index, _)| {
                let x = (self.origin_x + (index % self.size) as i32) * self.cell_size
                    + self.cell_size / 2;
                let z = (self.origin_z + (index / self.size) as i32) * self.cell_size
                    + self.cell_size / 2;
                (x - origin.0).pow(2) + (z - origin.1).pow(2)
            })
            .map(|(index, _)| {
                let x = index % self.size;
                let z = index / self.size;
                (
                    (self.origin_x + x as i32) * self.cell_size + self.cell_size / 2,
                    (self.origin_z + z as i32) * self.cell_size + self.cell_size / 2,
                )
            })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn cave_program_replays_exactly_for_the_same_seed() {
        let program = CaveProgram::default();
        let samples = [(0, 8, 0), (17, 12, -9), (101, 20, 44)];
        let first: Vec<_> = samples
            .iter()
            .map(|&(x, y, z)| program.sample(0x0123_4567_89AB_CDEF, x, y, z))
            .collect();
        let second: Vec<_> = samples
            .iter()
            .map(|&(x, y, z)| program.sample(0x0123_4567_89AB_CDEF, x, y, z))
            .collect();
        assert_eq!(first, second);
    }

    #[test]
    fn upper_seed_bits_change_the_cave_field() {
        let program = CaveProgram::default();
        let a = program.sample(7, 12, 12, 12);
        let b = program.sample(7 | (1_u64 << 48), 12, 12, 12);
        assert_ne!(a, b);
    }

    #[test]
    fn vertical_band_and_chunk_edges_are_stable() {
        let program = CaveProgram::default();
        assert_eq!(program.sample(42, 16, program.min_y - 1, 16), 0.0);
        assert_eq!(program.sample(42, 16, program.max_y + 1, 16), 0.0);
        let left_chunk_edge = program.sample(42, 15, 12, 16);
        let right_chunk_edge = program.sample(42, 16, 12, 16);
        assert!(left_chunk_edge.is_finite() && right_chunk_edge.is_finite());
        assert_eq!(
            program.carves(42, 15, 12, 16),
            left_chunk_edge >= program.threshold
        );
        assert_eq!(
            program.carves(42, 16, 12, 16),
            right_chunk_edge >= program.threshold
        );
    }

    #[test]
    fn cellular_field_is_deterministic_and_keeps_region_edges_walled() {
        let program = CaveProgram {
            shape: CaveShape::CellularAutomata,
            ..CaveProgram::default()
        };
        let first = program.cellular_field(42, 0, 0);
        let replay = program.cellular_field(42, 0, 0);
        assert_eq!(first, replay);
        assert!(first.is_wall(0, 0));
        assert!(first.is_wall(127, 127));
        let floor_count = (1..63)
            .flat_map(|z| (1..63).map(move |x| (x, z)))
            .filter(|&(x, z)| !first.is_wall(x, z))
            .count();
        assert!(
            floor_count > 0,
            "cellular recipe produced no open cave cells"
        );
    }

    #[test]
    fn cellular_rooms_are_modulated_by_height() {
        let program = CaveProgram {
            shape: CaveShape::CellularAutomata,
            ..CaveProgram::default()
        };
        let field = program.cellular_field_at(42, 8, 0, 0);
        let has_vertical_variation = (1..63).any(|z| {
            (1..63).any(|x| {
                if field.is_wall(x, z) {
                    return false;
                }
                let low = program.sample(42, x, 8, z);
                let high = program.sample(42, x, 15, z);
                (low - high).abs() > 0.001
            })
        });
        assert!(
            has_vertical_variation,
            "open cellular rooms must be shaped by the 3D density field"
        );
    }
}
