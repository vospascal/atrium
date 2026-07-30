//! Two-level sparse brickmap over a [`voxel_core::world::VoxelWorld`].
//!
//! Level 0 is a dense grid of brick pointers (one `u32` per 8x8x8-voxel
//! brick, `u32::MAX` = empty), accompanied by two empty-space acceleration
//! grids derived from it: a 1-bit-per-brick occupancy grid (the traversal's
//! cache-resident occupancy test) and a chebyshev skip-distance byte per
//! brick (empty-cube jumps). Level 1 stores, per *occupied* brick, a 512-bit
//! occupancy mask plus one material byte per voxel. All the arrays
//! (`brick_indices`, `occupancy_words`, `material_words`, palette, column
//! heights, bit grid, skip distances) upload straight into GPU storage
//! buffers; [`BrickmapMetadata`] is the matching uniform.
//!
//! Renderer-independence note: this same occupancy grid is the planned
//! acoustic-ray structure (the atrium `VoxelDdaResolver`, Stage 5 of
//! `docs/voxel-rt-plan.md`). Nothing in this module may grow a dependency on
//! wgpu, winit, or any renderer type — keep it pure data + CPU logic.

use voxel_core::world::{Voxel, VoxelWorld, VOXEL_SIZE, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};

/// Edge length of one brick in voxels.
pub const BRICK_SIZE: usize = 8;
/// Voxels per brick (8^3).
pub const VOXELS_PER_BRICK: usize = BRICK_SIZE * BRICK_SIZE * BRICK_SIZE;
/// `u32` words of occupancy bits per occupied brick (512 bits / 32).
pub const OCCUPANCY_WORDS_PER_BRICK: usize = VOXELS_PER_BRICK / 32;
/// `u32` words of material bytes per occupied brick (512 bytes / 4).
pub const MATERIAL_WORDS_PER_BRICK: usize = VOXELS_PER_BRICK / 4;

/// Brick grid dimensions: world size padded up to a multiple of the brick
/// size. 1000/8 = 125 and 256/8 = 32 divide exactly, so no padding bricks
/// actually exist today, but `div_ceil` keeps the math honest if the world
/// constants ever change.
pub const BRICK_GRID_X: usize = WORLD_SIZE_X.div_ceil(BRICK_SIZE);
pub const BRICK_GRID_Y: usize = WORLD_SIZE_Y.div_ceil(BRICK_SIZE);
pub const BRICK_GRID_Z: usize = WORLD_SIZE_Z.div_ceil(BRICK_SIZE);

/// Sentinel pointer marking a brick with no non-air voxels.
pub const EMPTY_BRICK: u32 = u32::MAX;

/// Sentinel for an XZ brick column (or a whole world) with no occupied
/// bricks. Chosen so the WGSL comparison `brick_y > i32(max)` reads it as -1
/// (u32 -> i32 conversion is modular) and every brick Y counts as "above".
pub const EMPTY_COLUMN: u32 = u32::MAX;

/// Number of material ids (== number of `Voxel` variants, Air included).
/// Exercised by the palette test; no renderer code needs the count directly.
#[cfg_attr(not(test), allow(dead_code))]
pub const MATERIAL_COUNT: usize = 24;

/// Dimension metadata for the GPU, bindable as a uniform buffer.
///
/// `#[repr(C)]` layout (48 bytes, 16-byte aligned — matches the WGSL
/// `BrickmapMeta` struct in `shaders/dda.wgsl`):
///
/// | offset | field                | WGSL type    |
/// |--------|----------------------|--------------|
/// | 0      | `brick_grid_size`    | `vec3<u32>`  |
/// | 12     | `occupied_brick_count` | `u32`      |
/// | 16     | `world_size_voxels`  | `vec3<u32>`  |
/// | 28     | `voxel_size_meters`  | `f32`        |
/// | 32     | `max_occupied_brick_y` | `u32`      |
/// | 36     | `_pad`               | 3 x `u32`    |
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct BrickmapMetadata {
    /// Brick grid dimensions (125, 32, 125).
    pub brick_grid_size: [u32; 3],
    /// How many occupied bricks exist (= `occupancy_words.len() / 16`).
    pub occupied_brick_count: u32,
    /// World dimensions in voxels (1000, 256, 1000).
    pub world_size_voxels: [u32; 3],
    /// Edge length of one voxel in meters ([`VOXEL_SIZE`] = 0.125).
    pub voxel_size_meters: f32,
    /// Highest occupied brick Y anywhere in the world ([`EMPTY_COLUMN`] when
    /// no bricks exist). Upward rays above this height can never hit.
    pub max_occupied_brick_y: u32,
    /// Explicit tail padding to the 16-byte uniform stride.
    pub _pad: [u32; 3],
}

// Manual impls instead of derive so we do not depend on bytemuck's `derive`
// feature flag: the struct is `#[repr(C)]`, all fields are u32/f32, and there
// are no padding bytes ([u32; 3] + u32 packs to 16 bytes exactly).
unsafe impl bytemuck::Zeroable for BrickmapMetadata {}
unsafe impl bytemuck::Pod for BrickmapMetadata {}

/// Two-level sparse voxel brickmap, GPU-upload-ready.
pub struct Brickmap {
    /// Dense level-0 grid of brick pointers, one per brick cell.
    ///
    /// Layout is X-major, then Y, then Z (x varies fastest):
    /// `cell = brick_x + brick_y * BRICK_GRID_X + brick_z * BRICK_GRID_X * BRICK_GRID_Y`.
    ///
    /// [`EMPTY_BRICK`] (`u32::MAX`) = the brick contains no non-air voxels;
    /// any other value is an index into the per-brick arrays below.
    pub brick_indices: Vec<u32>,
    /// Occupancy bitmasks, [`OCCUPANCY_WORDS_PER_BRICK`] (16) words per
    /// occupied brick, concatenated in pointer order. Within a brick the bit
    /// index of a voxel is `local_x + local_y * 8 + local_z * 64`; bit `b`
    /// lives in word `b / 32` at bit position `b % 32`. A set bit = non-air
    /// voxel (Water and thin cover included — the renderer decides what to
    /// do with them).
    pub occupancy_words: Vec<u32>,
    /// Material bytes, [`MATERIAL_WORDS_PER_BRICK`] (128) words per occupied
    /// brick, concatenated in pointer order. One byte per voxel, same local
    /// index as the occupancy bits, packed little-endian: voxel `b`'s byte is
    /// `(word[b / 4] >> ((b % 4) * 8)) & 0xff`, i.e. byte 0 occupies bits
    /// 0..8. Byte value = [`material_id`]; 0 (Air) only where the occupancy
    /// bit is clear.
    pub material_words: Vec<u32>,
    /// Per-XZ-brick-column maximum occupied brick Y, one `u32` per column
    /// (`BRICK_GRID_X * BRICK_GRID_Z` entries, x-major then z:
    /// `column = brick_x + brick_z * BRICK_GRID_X`). [`EMPTY_COLUMN`] when
    /// the column has no occupied bricks. Traversal uses this as a
    /// column-height early exit: an upward ray above a column's max can hit
    /// nothing in that column.
    pub column_max_brick_y: Vec<u32>,
    /// One occupancy bit per brick cell (same x-major cell index as
    /// `brick_indices`; bit `cell & 31` of word `cell >> 5`), set when the
    /// brick holds any non-air voxel. The traversal's hot occupancy test:
    /// at 62.5 KB it stays cache-resident where the 2 MB pointer grid does
    /// not — the pointer is only read for bricks this grid marks occupied.
    pub brick_occupancy_bit_words: Vec<u32>,
    /// Chebyshev distance (in bricks) from every brick cell to the nearest
    /// occupied brick — 0 for occupied cells, saturated at 255. One byte per
    /// cell, four per word, little-endian like `material_words`. A cell at
    /// distance d sits centered in a guaranteed-empty cube of half-width
    /// d - 1 bricks, which the traversal jumps in one step (`distance_skip`
    /// in dda.wgsl).
    pub brick_skip_distance_words: Vec<u32>,
    occupied_brick_count: u32,
    max_occupied_brick_y: u32,
}

/// `Voxel` -> material id, in enum declaration order with `Air = 0`
/// (crates/voxel-core/src/world.rs, the 24-variant `Voxel` enum).
pub fn material_id(voxel: Voxel) -> u8 {
    match voxel {
        Voxel::Air => 0,
        Voxel::Grass => 1,
        Voxel::TallGrass => 2,
        Voxel::Dirt => 3,
        Voxel::Sand => 4,
        Voxel::Sediment => 5,
        Voxel::Stone => 6,
        Voxel::Water => 7,
        Voxel::Trunk => 8,
        Voxel::TrunkBirch => 9,
        Voxel::Leaves => 10,
        Voxel::LeavesDark => 11,
        Voxel::LeavesBirch => 12,
        Voxel::LeavesPine => 13,
        Voxel::FlowerPink => 14,
        Voxel::FlowerWhite => 15,
        Voxel::FlowerYellow => 16,
        Voxel::FlowerBlue => 17,
        Voxel::WaterWeed => 18,
        Voxel::LilyPad => 19,
        Voxel::LilyBloom => 20,
        Voxel::Reed => 21,
        Voxel::CattailHead => 22,
        Voxel::Snow => 23,
    }
}

/// One representative sRGB color per material id, GPU-upload-ready as an
/// `array<vec4<f32>>` storage buffer ([r, g, b, a], a = 1.0 except water).
///
/// Colors are lifted from the `voxel_color` match in
/// `crates/voxel-sandbox/src/mesh.rs` (lines ~1253-1358). That function
/// blends per-position (dryness/lushness/season/tree-tone/depth); Stage 1
/// takes ONE representative value per type — positional variation comes
/// later. The exact picks:
///
/// - Grass: the grassy patch endpoint `[0.41, 0.52, 0.29]` of the
///   dirt->grass patchiness lerp (summer, mid-biome).
/// - TallGrass: base blade `[0.28, 0.45, 0.23]`.
/// - Leaves: midpoint of the summer oak tone lerp
///   `[0.30,0.47,0.22]..[0.46,0.54,0.25]` -> `[0.38, 0.505, 0.235]`.
/// - LeavesDark: Leaves * 0.74 (mesh.rs darkening factor).
/// - LeavesBirch: midpoint of `[0.47,0.56,0.26]..[0.55,0.60,0.30]`.
/// - LeavesPine: midpoint of `[0.18,0.32,0.23]..[0.24,0.37,0.25]`.
/// - Sand: dry above-water sand `[0.86, 0.77, 0.55]`.
/// - TrunkBirch: the dominant paper-bark branch `[0.80, 0.78, 0.72]`
///   (dark flecks are per-voxel jitter, skipped here).
/// - Reed: summer stalk `[0.55, 0.56, 0.31]`.
/// - Water: midpoint of the shallow->deep lerp
///   `[0.30,0.72,0.82]..[0.08,0.32,0.60]` -> `[0.19, 0.52, 0.71]`,
///   alpha 0.7 (mid-depth opacity; Stage 1 may render it opaque).
/// - Dirt/Sediment/Stone/Trunk/flowers/WaterWeed/LilyPad/LilyBloom/
///   CattailHead/Snow: taken verbatim from their single-value arms.
///
/// Values are sRGB-encoded, exactly as authored in mesh.rs. The Stage 1
/// shader writes them (shaded) to an `rgba8unorm` target without extra
/// gamma encoding.
pub fn palette() -> Vec<[f32; 4]> {
    vec![
        [0.0, 0.0, 0.0, 0.0],       // 0  Air (never sampled on a hit)
        [0.41, 0.52, 0.29, 1.0],    // 1  Grass
        [0.28, 0.45, 0.23, 1.0],    // 2  TallGrass
        [0.44, 0.32, 0.22, 1.0],    // 3  Dirt
        [0.86, 0.77, 0.55, 1.0],    // 4  Sand
        [0.17, 0.16, 0.11, 1.0],    // 5  Sediment
        [0.52, 0.52, 0.55, 1.0],    // 6  Stone
        [0.19, 0.52, 0.71, 0.7],    // 7  Water
        [0.45, 0.31, 0.19, 1.0],    // 8  Trunk
        [0.80, 0.78, 0.72, 1.0],    // 9  TrunkBirch
        [0.38, 0.505, 0.235, 1.0],  // 10 Leaves
        [0.281, 0.374, 0.174, 1.0], // 11 LeavesDark
        [0.51, 0.58, 0.28, 1.0],    // 12 LeavesBirch
        [0.21, 0.345, 0.24, 1.0],   // 13 LeavesPine
        [0.93, 0.55, 0.75, 1.0],    // 14 FlowerPink
        [0.96, 0.95, 0.90, 1.0],    // 15 FlowerWhite
        [0.95, 0.83, 0.35, 1.0],    // 16 FlowerYellow
        [0.45, 0.52, 0.92, 1.0],    // 17 FlowerBlue
        [0.15, 0.30, 0.19, 1.0],    // 18 WaterWeed
        [0.26, 0.50, 0.24, 1.0],    // 19 LilyPad
        [0.95, 0.92, 0.85, 1.0],    // 20 LilyBloom
        [0.55, 0.56, 0.31, 1.0],    // 21 Reed
        [0.32, 0.18, 0.08, 1.0],    // 22 CattailHead
        [0.92, 0.93, 0.96, 1.0],    // 23 Snow
    ]
}

impl Brickmap {
    /// Build the brickmap from a finished world.
    ///
    /// Sweeps every column through [`VoxelWorld::column_runs`] (the cheap RLE
    /// path — air runs, i.e. the vast majority of the volume, are skipped
    /// without touching a single voxel) and only allocates level-1 data for
    /// bricks that contain at least one non-air voxel.
    ///
    /// Occupancy deliberately includes EVERYTHING non-air — Water and thin
    /// cover (flowers, reeds, lily pads, tall grass) too, even though
    /// `Voxel::is_solid()` excludes them. Stage 1 wants all of it visible;
    /// later stages (and the acoustic resolver) can derive stricter masks
    /// from the material bytes.
    pub fn build(world: &VoxelWorld) -> Brickmap {
        let mut brick_indices = vec![EMPTY_BRICK; BRICK_GRID_X * BRICK_GRID_Y * BRICK_GRID_Z];
        let mut occupancy_words: Vec<u32> = Vec::new();
        let mut material_words: Vec<u32> = Vec::new();
        let mut column_max_brick_y = vec![EMPTY_COLUMN; BRICK_GRID_X * BRICK_GRID_Z];
        let mut max_occupied_brick_y = EMPTY_COLUMN;

        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                let brick_x = x as usize / BRICK_SIZE;
                let local_x = x as usize % BRICK_SIZE;
                let brick_z = z as usize / BRICK_SIZE;
                let local_z = z as usize % BRICK_SIZE;
                let column_cell_base = brick_x + brick_z * BRICK_GRID_X * BRICK_GRID_Y;
                let column_bit_base = local_x + local_z * 64;
                let column = brick_x + brick_z * BRICK_GRID_X;

                for (voxel, y_start, length) in world.column_runs(x, z) {
                    if voxel == Voxel::Air {
                        continue;
                    }
                    let id = material_id(voxel) as u32;
                    for y in y_start..y_start + length {
                        let brick_y = y as usize / BRICK_SIZE;
                        let local_y = y as usize % BRICK_SIZE;
                        let cell = column_cell_base + brick_y * BRICK_GRID_X;
                        let mut pointer = brick_indices[cell];
                        if pointer == EMPTY_BRICK {
                            pointer = (occupancy_words.len() / OCCUPANCY_WORDS_PER_BRICK) as u32;
                            brick_indices[cell] = pointer;
                            occupancy_words
                                .resize(occupancy_words.len() + OCCUPANCY_WORDS_PER_BRICK, 0);
                            material_words
                                .resize(material_words.len() + MATERIAL_WORDS_PER_BRICK, 0);
                            // Every occupied brick passes through this
                            // allocation branch exactly once — track the
                            // column and world height maxima here. The
                            // `EMPTY_COLUMN` sentinel is u32::MAX, so it
                            // must be replaced explicitly, never `max`ed.
                            let brick_y = brick_y as u32;
                            if column_max_brick_y[column] == EMPTY_COLUMN
                                || column_max_brick_y[column] < brick_y
                            {
                                column_max_brick_y[column] = brick_y;
                            }
                            if max_occupied_brick_y == EMPTY_COLUMN
                                || max_occupied_brick_y < brick_y
                            {
                                max_occupied_brick_y = brick_y;
                            }
                        }
                        let bit = column_bit_base + local_y * 8;
                        occupancy_words
                            [pointer as usize * OCCUPANCY_WORDS_PER_BRICK + (bit >> 5)] |=
                            1 << (bit & 31);
                        material_words[pointer as usize * MATERIAL_WORDS_PER_BRICK + (bit >> 2)] |=
                            id << ((bit & 3) * 8);
                    }
                }
            }
        }

        let occupied_brick_count = (occupancy_words.len() / OCCUPANCY_WORDS_PER_BRICK) as u32;
        let brick_occupancy_bit_words = pack_occupancy_bits(&brick_indices);
        let brick_skip_distance_words = pack_bytes_little_endian(&chebyshev_skip_distances(
            &brick_indices,
            BRICK_GRID_X,
            BRICK_GRID_Y,
            BRICK_GRID_Z,
        ));
        Brickmap {
            brick_indices,
            occupancy_words,
            material_words,
            column_max_brick_y,
            brick_occupancy_bit_words,
            brick_skip_distance_words,
            occupied_brick_count,
            max_occupied_brick_y,
        }
    }

    /// CPU-side material lookup: the material id at a world voxel coordinate,
    /// 0 for air and for anything out of bounds. Material bytes are only ever
    /// non-zero where the occupancy bit is set, so this is exactly the
    /// GPU-visible value. Only the round-trip test calls this today.
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn get(&self, x: i32, y: i32, z: i32) -> u8 {
        if x < 0
            || y < 0
            || z < 0
            || x >= WORLD_SIZE_X as i32
            || y >= WORLD_SIZE_Y as i32
            || z >= WORLD_SIZE_Z as i32
        {
            return 0;
        }
        let cell = x as usize / BRICK_SIZE
            + (y as usize / BRICK_SIZE) * BRICK_GRID_X
            + (z as usize / BRICK_SIZE) * BRICK_GRID_X * BRICK_GRID_Y;
        let pointer = self.brick_indices[cell];
        if pointer == EMPTY_BRICK {
            return 0;
        }
        let bit = x as usize % BRICK_SIZE
            + (y as usize % BRICK_SIZE) * 8
            + (z as usize % BRICK_SIZE) * 64;
        let word = self.material_words[pointer as usize * MATERIAL_WORDS_PER_BRICK + (bit >> 2)];
        ((word >> ((bit & 3) * 8)) & 0xff) as u8
    }

    /// Whether the occupancy bit is set at a world voxel coordinate (false
    /// out of bounds). Equivalent to `get(...) != 0` by construction; kept as
    /// the future entry point for the acoustic DDA resolver (Stage 5).
    #[cfg_attr(not(test), allow(dead_code))]
    pub fn is_occupied(&self, x: i32, y: i32, z: i32) -> bool {
        if x < 0
            || y < 0
            || z < 0
            || x >= WORLD_SIZE_X as i32
            || y >= WORLD_SIZE_Y as i32
            || z >= WORLD_SIZE_Z as i32
        {
            return false;
        }
        let cell = x as usize / BRICK_SIZE
            + (y as usize / BRICK_SIZE) * BRICK_GRID_X
            + (z as usize / BRICK_SIZE) * BRICK_GRID_X * BRICK_GRID_Y;
        let pointer = self.brick_indices[cell];
        if pointer == EMPTY_BRICK {
            return false;
        }
        let bit = x as usize % BRICK_SIZE
            + (y as usize % BRICK_SIZE) * 8
            + (z as usize % BRICK_SIZE) * 64;
        let word = self.occupancy_words[pointer as usize * OCCUPANCY_WORDS_PER_BRICK + (bit >> 5)];
        (word >> (bit & 31)) & 1 == 1
    }

    /// The GPU uniform describing this brickmap's dimensions.
    pub fn metadata(&self) -> BrickmapMetadata {
        BrickmapMetadata {
            brick_grid_size: [
                BRICK_GRID_X as u32,
                BRICK_GRID_Y as u32,
                BRICK_GRID_Z as u32,
            ],
            occupied_brick_count: self.occupied_brick_count,
            world_size_voxels: [
                WORLD_SIZE_X as u32,
                WORLD_SIZE_Y as u32,
                WORLD_SIZE_Z as u32,
            ],
            voxel_size_meters: VOXEL_SIZE,
            max_occupied_brick_y: self.max_occupied_brick_y,
            _pad: [0; 3],
        }
    }

    /// Number of occupied bricks (bricks with at least one non-air voxel).
    pub fn occupied_brick_count(&self) -> u32 {
        self.occupied_brick_count
    }
}

/// One bit per brick cell (bit `cell & 31` of word `cell >> 5`), set when
/// the cell's pointer is not [`EMPTY_BRICK`].
fn pack_occupancy_bits(brick_indices: &[u32]) -> Vec<u32> {
    let mut words = vec![0_u32; brick_indices.len().div_ceil(32)];
    for (cell, &pointer) in brick_indices.iter().enumerate() {
        if pointer != EMPTY_BRICK {
            words[cell >> 5] |= 1 << (cell & 31);
        }
    }
    words
}

/// Bytes packed four per `u32`, little-endian (byte 0 = bits 0..8) — the
/// same scheme as `material_words`.
fn pack_bytes_little_endian(bytes: &[u8]) -> Vec<u32> {
    let mut words = vec![0_u32; bytes.len().div_ceil(4)];
    for (index, &byte) in bytes.iter().enumerate() {
        words[index >> 2] |= u32::from(byte) << ((index & 3) * 8);
    }
    words
}

/// Exact chebyshev (L-infinity) distance transform over the brick grid: per
/// cell, the distance in cells to the nearest occupied cell — 0 for occupied
/// cells, saturated at 255 (`u8::MAX`).
///
/// Two chamfer sweeps over the 26-neighborhood with unit weights are EXACT
/// for the chebyshev metric (any chebyshev-d path decomposes into d king
/// moves, and each sweep direction covers one half-space of moves), so the
/// whole transform is O(cells x 26) — ~13M relaxations for 125x32x125.
/// Exactness is pinned by the brute-force test below.
fn chebyshev_skip_distances(
    brick_indices: &[u32],
    grid_size_x: usize,
    grid_size_y: usize,
    grid_size_z: usize,
) -> Vec<u8> {
    assert_eq!(brick_indices.len(), grid_size_x * grid_size_y * grid_size_z);
    let mut distances: Vec<u8> = brick_indices
        .iter()
        .map(|&pointer| if pointer == EMPTY_BRICK { u8::MAX } else { 0 })
        .collect();

    // Relax one cell from the half-space of neighbors the current sweep has
    // already finalized: lexicographically (dz, dy, dx) < 0 on the forward
    // (ascending) sweep, > 0 on the backward (descending) sweep.
    fn relax_cell(
        distances: &mut [u8],
        grid_size: (i32, i32, i32),
        x: i32,
        y: i32,
        z: i32,
        forward: bool,
    ) {
        let (grid_size_x, grid_size_y, grid_size_z) = grid_size;
        let cell = (x + y * grid_size_x + z * grid_size_x * grid_size_y) as usize;
        let mut best = distances[cell];
        if best == 0 {
            return;
        }
        for dz in -1_i32..=1 {
            for dy in -1_i32..=1 {
                for dx in -1_i32..=1 {
                    let visited = if forward {
                        (dz, dy, dx) < (0, 0, 0)
                    } else {
                        (dz, dy, dx) > (0, 0, 0)
                    };
                    if !visited {
                        continue;
                    }
                    let (nx, ny, nz) = (x + dx, y + dy, z + dz);
                    if nx < 0
                        || ny < 0
                        || nz < 0
                        || nx >= grid_size_x
                        || ny >= grid_size_y
                        || nz >= grid_size_z
                    {
                        continue;
                    }
                    let neighbor =
                        (nx + ny * grid_size_x + nz * grid_size_x * grid_size_y) as usize;
                    best = best.min(distances[neighbor].saturating_add(1));
                }
            }
        }
        distances[cell] = best;
    }

    let grid_size = (grid_size_x as i32, grid_size_y as i32, grid_size_z as i32);
    for z in 0..grid_size.2 {
        for y in 0..grid_size.1 {
            for x in 0..grid_size.0 {
                relax_cell(&mut distances, grid_size, x, y, z, true);
            }
        }
    }
    for z in (0..grid_size.2).rev() {
        for y in (0..grid_size.1).rev() {
            for x in (0..grid_size.0).rev() {
                relax_cell(&mut distances, grid_size, x, y, z, false);
            }
        }
    }
    distances
}

#[cfg(test)]
mod tests {
    use super::*;

    /// x/z sample coordinates: brick-boundary triples (7, 8, 9 style) across
    /// the whole axis plus the extreme edges.
    const AXIS_SAMPLES_XZ: &[i32] = &[
        0, 1, 6, 7, 8, 9, 15, 16, 17, 63, 64, 65, 127, 128, 129, 255, 256, 257, 449, 450, 499, 500,
        501, 503, 504, 505, 599, 600, 601, 807, 808, 809, 991, 992, 993, 997, 998, 999,
    ];
    /// y samples: brick boundaries plus the interesting terrain bands
    /// (PLATEAU_FLOOR = 58, WATER_LEVEL = 84, tree crowns ~96-160).
    const AXIS_SAMPLES_Y: &[i32] = &[
        0, 1, 6, 7, 8, 9, 15, 16, 17, 55, 56, 57, 63, 64, 65, 83, 84, 85, 95, 96, 97, 127, 128,
        129, 159, 160, 161, 254, 255,
    ];

    /// Round trip: `Brickmap::get` must agree with `VoxelWorld::get` mapped
    /// through `material_id`, over a deterministic sample grid (~42k
    /// coordinates) heavy on brick boundaries, plus out-of-bounds probes.
    /// NOTE: generates the full 1000x256x1000 world — slow in debug builds,
    /// run with `--release` when iterating.
    #[test]
    fn brickmap_round_trips_generated_world() {
        let world = VoxelWorld::generate(1234, 0.0);
        let brickmap = Brickmap::build(&world);
        assert!(
            brickmap.occupied_brick_count() > 0,
            "island produced no occupied bricks"
        );

        let mut non_air_seen = 0_u64;
        for &z in AXIS_SAMPLES_XZ {
            for &y in AXIS_SAMPLES_Y {
                for &x in AXIS_SAMPLES_XZ {
                    let expected = material_id(world.get(x, y, z));
                    let actual = brickmap.get(x, y, z);
                    assert_eq!(
                        actual, expected,
                        "material mismatch at ({x}, {y}, {z}): brickmap {actual}, world {expected}"
                    );
                    assert_eq!(
                        brickmap.is_occupied(x, y, z),
                        expected != 0,
                        "occupancy mismatch at ({x}, {y}, {z})"
                    );
                    if expected != 0 {
                        non_air_seen += 1;
                    }
                }
            }
        }
        assert!(
            non_air_seen > 1000,
            "sample grid barely touched the island ({non_air_seen} non-air hits) — samples are wrong"
        );

        let out_of_bounds = [
            (-1, 0, 0),
            (WORLD_SIZE_X as i32, 0, 0),
            (0, -1, 0),
            (0, WORLD_SIZE_Y as i32, 0),
            (0, 0, -1),
            (0, 0, WORLD_SIZE_Z as i32),
            (i32::MIN, 10, 10),
            (10, i32::MAX, 10),
            (500, 100, i32::MIN),
        ];
        for (x, y, z) in out_of_bounds {
            assert_eq!(
                brickmap.get(x, y, z),
                0,
                "out-of-bounds ({x}, {y}, {z}) must be air"
            );
            assert!(!brickmap.is_occupied(x, y, z));
        }
    }

    #[test]
    fn palette_covers_every_material_id() {
        assert_eq!(palette().len(), MATERIAL_COUNT);
        // Air is the miss sentinel: fully transparent black.
        assert_eq!(palette()[0], [0.0, 0.0, 0.0, 0.0]);
    }

    #[test]
    fn metadata_layout_is_gpu_ready() {
        assert_eq!(std::mem::size_of::<BrickmapMetadata>(), 48);
        assert_eq!(std::mem::align_of::<BrickmapMetadata>(), 4);
    }

    /// The column-height grid must agree with a brute-force sweep of
    /// `brick_indices`: per XZ column, the max brick Y holding a pointer
    /// (`EMPTY_COLUMN` when none), and the global max over all columns.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn column_heights_match_brick_indices() {
        let world = VoxelWorld::generate(1234, 0.0);
        let brickmap = Brickmap::build(&world);
        assert_eq!(
            brickmap.column_max_brick_y.len(),
            BRICK_GRID_X * BRICK_GRID_Z
        );

        let mut expected_global_max = EMPTY_COLUMN;
        let mut occupied_columns = 0_usize;
        for brick_z in 0..BRICK_GRID_Z {
            for brick_x in 0..BRICK_GRID_X {
                let mut expected_column_max = EMPTY_COLUMN;
                for brick_y in 0..BRICK_GRID_Y {
                    let cell =
                        brick_x + brick_y * BRICK_GRID_X + brick_z * BRICK_GRID_X * BRICK_GRID_Y;
                    if brickmap.brick_indices[cell] != EMPTY_BRICK {
                        expected_column_max = brick_y as u32;
                    }
                }
                let actual = brickmap.column_max_brick_y[brick_x + brick_z * BRICK_GRID_X];
                assert_eq!(
                    actual, expected_column_max,
                    "column max mismatch at brick column ({brick_x}, {brick_z})"
                );
                if expected_column_max != EMPTY_COLUMN {
                    occupied_columns += 1;
                    if expected_global_max == EMPTY_COLUMN
                        || expected_global_max < expected_column_max
                    {
                        expected_global_max = expected_column_max;
                    }
                }
            }
        }
        assert_eq!(
            brickmap.metadata().max_occupied_brick_y,
            expected_global_max
        );
        assert!(
            occupied_columns > 1000,
            "island barely produced occupied columns ({occupied_columns}) — world constants changed?"
        );
        assert!(
            expected_global_max != EMPTY_COLUMN && (expected_global_max as usize) < BRICK_GRID_Y,
            "global max brick Y {expected_global_max} out of range"
        );
    }

    /// The chamfer transform must equal brute-force chebyshev distance on a
    /// small deterministic random grid — this is the exactness proof the
    /// shader's empty-cube skip relies on (an overestimate would tunnel
    /// through geometry; an underestimate only costs speed).
    #[test]
    fn chebyshev_distances_match_brute_force_on_synthetic_grid() {
        let (grid_size_x, grid_size_y, grid_size_z) = (13_usize, 9_usize, 11_usize);
        let cell_count = grid_size_x * grid_size_y * grid_size_z;
        let mut brick_indices = vec![EMPTY_BRICK; cell_count];
        // Deterministic LCG sprinkle, ~1/16 of cells occupied.
        let mut lcg_state = 0x1234_5678_u32;
        for pointer in brick_indices.iter_mut() {
            lcg_state = lcg_state
                .wrapping_mul(1_664_525)
                .wrapping_add(1_013_904_223);
            if lcg_state >> 28 == 0 {
                *pointer = 1;
            }
        }
        let occupied_cells: Vec<(i32, i32, i32)> = (0..cell_count)
            .filter(|&cell| brick_indices[cell] != EMPTY_BRICK)
            .map(|cell| {
                (
                    (cell % grid_size_x) as i32,
                    ((cell / grid_size_x) % grid_size_y) as i32,
                    (cell / (grid_size_x * grid_size_y)) as i32,
                )
            })
            .collect();
        assert!(
            occupied_cells.len() > 20,
            "LCG sprinkle produced too few occupied cells ({})",
            occupied_cells.len()
        );

        let distances =
            chebyshev_skip_distances(&brick_indices, grid_size_x, grid_size_y, grid_size_z);
        for (cell, &actual_distance) in distances.iter().enumerate() {
            let x = (cell % grid_size_x) as i32;
            let y = ((cell / grid_size_x) % grid_size_y) as i32;
            let z = (cell / (grid_size_x * grid_size_y)) as i32;
            let expected = occupied_cells
                .iter()
                .map(|&(ox, oy, oz)| (x - ox).abs().max((y - oy).abs()).max((z - oz).abs()) as u32)
                .min()
                .expect("occupied set is non-empty")
                .min(255) as u8;
            assert_eq!(
                actual_distance, expected,
                "chebyshev distance mismatch at ({x}, {y}, {z})"
            );
        }
    }

    /// An all-empty grid must saturate everywhere (the shader then skips at
    /// maximum stride and the trace terminates on the world bounds).
    #[test]
    fn chebyshev_distances_saturate_on_empty_grid() {
        let distances = chebyshev_skip_distances(&vec![EMPTY_BRICK; 4 * 3 * 5], 4, 3, 5);
        assert!(distances.iter().all(|&distance| distance == u8::MAX));
    }

    /// The GPU-side empty-space grids must agree with the pointer grid on
    /// the real generated world: bit set ⟺ pointer present ⟺ distance 0,
    /// and (sampled) every distance-d cell really is centered in an
    /// all-empty cube of half-width d - 1 — the exact property the
    /// traversal's `distance_skip` jump assumes.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn empty_space_grids_match_brick_indices() {
        let world = VoxelWorld::generate(1234, 0.0);
        let brickmap = Brickmap::build(&world);
        let cell_count = BRICK_GRID_X * BRICK_GRID_Y * BRICK_GRID_Z;
        assert_eq!(
            brickmap.brick_occupancy_bit_words.len(),
            cell_count.div_ceil(32)
        );
        assert_eq!(
            brickmap.brick_skip_distance_words.len(),
            cell_count.div_ceil(4)
        );

        let occupancy_bit =
            |cell: usize| (brickmap.brick_occupancy_bit_words[cell >> 5] >> (cell & 31)) & 1 == 1;
        let skip_distance = |cell: usize| {
            (brickmap.brick_skip_distance_words[cell >> 2] >> ((cell & 3) * 8)) & 0xff
        };

        for cell in 0..cell_count {
            let occupied = brickmap.brick_indices[cell] != EMPTY_BRICK;
            assert_eq!(
                occupancy_bit(cell),
                occupied,
                "occupancy bit mismatch at cell {cell}"
            );
            assert_eq!(
                skip_distance(cell) == 0,
                occupied,
                "skip distance 0 must mean occupied at cell {cell}"
            );
        }

        let mut cubes_checked = 0_usize;
        for cell in (0..cell_count).step_by(17) {
            let distance = skip_distance(cell) as i32;
            if distance < 2 {
                continue;
            }
            let x = (cell % BRICK_GRID_X) as i32;
            let y = ((cell / BRICK_GRID_X) % BRICK_GRID_Y) as i32;
            let z = (cell / (BRICK_GRID_X * BRICK_GRID_Y)) as i32;
            let half_width = distance - 1;
            for nz in (z - half_width).max(0)..=(z + half_width).min(BRICK_GRID_Z as i32 - 1) {
                for ny in (y - half_width).max(0)..=(y + half_width).min(BRICK_GRID_Y as i32 - 1) {
                    for nx in
                        (x - half_width).max(0)..=(x + half_width).min(BRICK_GRID_X as i32 - 1)
                    {
                        let neighbor = nx as usize
                            + ny as usize * BRICK_GRID_X
                            + nz as usize * BRICK_GRID_X * BRICK_GRID_Y;
                        assert_eq!(
                            brickmap.brick_indices[neighbor], EMPTY_BRICK,
                            "distance {distance} at ({x}, {y}, {z}) but occupied brick \
                             at ({nx}, {ny}, {nz}) inside the guaranteed-empty cube"
                        );
                    }
                }
            }
            cubes_checked += 1;
        }
        assert!(
            cubes_checked > 1000,
            "skip-safety sweep barely sampled any skippable cells ({cubes_checked})"
        );
    }
}
