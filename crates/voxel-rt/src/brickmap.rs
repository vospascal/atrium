//! Two-level sparse brickmap over a [`voxel_core::world::VoxelWorld`].
//!
//! Level 0 is a dense grid of brick pointers (one `u32` per 8x8x8-voxel
//! brick, `u32::MAX` = empty), accompanied by two empty-space acceleration
//! grids derived from it: a 1-bit-per-brick occupancy grid (the traversal's
//! cache-resident occupancy test) and a chebyshev skip-distance byte per
//! brick (empty-cube jumps). Level 1 stores, per *occupied* brick, a 512-bit
//! occupancy mask plus one material byte per voxel. All the arrays
//! (`brick_indices`, `occupancy_words`, `material_words`, column
//! heights, bit grid, skip distances) upload straight into GPU storage
//! buffers; [`BrickmapMetadata`] is the matching uniform.
//!
//! Renderer-independence note: this same occupancy grid is the planned
//! acoustic-ray structure (the atrium `VoxelDdaResolver`, E8 of
//! `docs/voxel-rt-plan.md`). Nothing in this module may grow a dependency on
//! wgpu, winit, or any renderer type — keep it pure data + CPU logic.
//!
//! E2 added the EDIT half ([`Brickmap::set_voxel`]): one call patches the
//! occupancy bits, the material bytes, the level-0 pointer (allocating or
//! freeing a brick slot), the 1-bit brick grid, the chebyshev clearance field,
//! the per-column and global brick heights, and reports every touched word
//! range as a [`BrickmapEdit`] so the caller can upload deltas instead of the
//! whole 41 MB. The edit path knows nothing about the GPU either — it names its
//! own arrays ([`BrickmapArray`]) and lets the uploader map those to bindings.

use std::ops::Range;

use voxel_core::world::{Voxel, VoxelWorld, VOXEL_SIZE, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};

use crate::material::material_id;

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

// ---- Level-0 pointer tagging -------------------------------------------------
//
// A `brick_indices` word is TAGGED in its top two bits, so the coarse grid can
// answer "what is in this metre" without ever fetching level-1 data. The idea
// (and the measured case for it) comes from NAADF — Ulschmid et al., *Globally
// Illuminated Voxel Worlds Accelerated with Nested Axis-Aligned Distance
// Fields*, Computer Graphics Forum 2026, MIT-licensed at
// <https://github.com/cg-tuwien/NAADF> — whose node word carries the same
// EMPTY / UNIFORM / has-children distinction in its top bits.
//
// The bit ASSIGNMENT below is deliberately not theirs. NAADF spends tag 0 on
// EMPTY; we spend it on UNIQUE and leave the `u32::MAX` empty sentinel intact,
// so every pointer that exists today keeps its exact current value and meaning.
// A bare slot index IS a valid tagged word, which is what makes this change
// additive rather than a re-encoding of the whole grid.
//
// Measured on the shipped island (`cargo run --example brick_census -p
// voxel-core`): of 71,966 occupied bricks, 58.6% are a single material filling
// all 512 cells. Those are the ones this tag collapses.

/// Bit position of the two-bit level-0 tag.
pub const BRICK_TAG_SHIFT: u32 = 30;

/// Payload mask — the 30 bits below the tag. 2^30 slots is ~1000x more level-1
/// bricks than a full world can hold, so nothing is lost by spending two bits.
pub const BRICK_PAYLOAD_MASK: u32 = (1 << BRICK_TAG_SHIFT) - 1;

/// Tag `0b00`: the payload is a level-1 slot index. This is the untagged
/// encoding every pointer used before tagging existed.
pub const BRICK_TAG_UNIQUE: u32 = 0;

/// Tag `0b01`: the payload is a material id, and the brick is that material in
/// all 512 cells. No level-1 storage, and a ray hits it at the brick face.
pub const BRICK_TAG_UNIFORM: u32 = 1;

/// Tag `0b10`: RESERVED for a shared template — the payload will be an index
/// into a deduplicated brick palette. Nothing emits this yet; it is spelled out
/// so the tag space is allocated before the shaders learn to branch on it.
pub const BRICK_TAG_TEMPLATE: u32 = 2;

/// Tag `0b11`: no non-air voxels. [`EMPTY_BRICK`] is `u32::MAX`, which carries
/// this tag already — the sentinel and the tag agree by construction.
pub const BRICK_TAG_EMPTY: u32 = 3;

/// The tag of a level-0 pointer.
#[inline]
pub const fn brick_tag(pointer: u32) -> u32 {
    pointer >> BRICK_TAG_SHIFT
}

/// The level-1 slot a UNIQUE pointer addresses. Masking is a no-op for a
/// genuinely untagged pointer, so call sites can use it unconditionally.
#[inline]
pub const fn brick_slot(pointer: u32) -> u32 {
    pointer & BRICK_PAYLOAD_MASK
}

/// Whether this brick is one material through and through.
#[inline]
pub const fn brick_is_uniform(pointer: u32) -> bool {
    brick_tag(pointer) == BRICK_TAG_UNIFORM
}

/// Whether this brick has level-1 data of its own to descend into.
#[inline]
pub const fn brick_is_unique(pointer: u32) -> bool {
    brick_tag(pointer) == BRICK_TAG_UNIQUE
}

/// The material filling a UNIFORM brick. Meaningless for any other tag.
#[inline]
pub const fn brick_uniform_material(pointer: u32) -> u8 {
    (pointer & 0xff) as u8
}

/// A UNIFORM pointer for `material`, which must be non-air.
#[inline]
pub const fn uniform_brick(material: u8) -> u32 {
    (BRICK_TAG_UNIFORM << BRICK_TAG_SHIFT) | material as u32
}

/// Sentinel for an XZ brick column (or a whole world) with no occupied
/// bricks. Chosen so the WGSL comparison `brick_y > i32(max)` reads it as -1
/// (u32 -> i32 conversion is modular) and every brick Y counts as "above".
pub const EMPTY_COLUMN: u32 = u32::MAX;

/// Spare brick slots the level-1 arrays (and therefore the GPU buffers) carry
/// past the built world, so an edit that materializes a brick patches words
/// that already exist instead of reallocating 41 MB of buffers.
///
/// 4096 slots = 4096 x (16 + 128) words = **2.36 MB** of headroom, and 4096
/// newly materialized bricks is 2 M voxels of construction in one session — the
/// growth path exists ([`BrickmapEdit::arrays_grew`]) and is measured, but it is
/// not the common case. Freed slots are reused first, so a build/dig loop in one
/// place never consumes headroom at all.
pub const EDIT_BRICK_HEADROOM: usize = 4096;

/// Gap (in words) that two dirty word ranges may leave between them and still be
/// uploaded as ONE range. The clearance field's dirty region is a box in the
/// brick grid, i.e. one short range per (y, z) row; 64 words of slack collapses
/// all rows of one z slice into a single upload (the y stride is
/// `BRICK_GRID_X / 4` = 31 words) while keeping the z slices apart (the z stride
/// is `BRICK_GRID_X * BRICK_GRID_Y / 4` = 1000 words). Trading a few unchanged
/// words for an order of magnitude fewer `write_buffer` calls.
pub const DIRTY_RANGE_GAP_WORDS: usize = 64;

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
///
/// `Clone` is a DEEP copy of ~45 MB of arrays and exists for two reasons, both
/// E2: the bench needs an independent world per variant without regenerating it,
/// and the cost of this clone IS the measured price of the plan's original
/// "publish an `Arc<Brickmap>` snapshot per edit" sketch — which is why the
/// shipped authority publishes 576-byte deltas instead (see
/// [`crate::world_host`]).
#[derive(Clone)]
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
    /// AADF (binding 11): six 5-bit directional bounds per brick cell, packed one
    /// `u32` per cell in [`BOUND_DIRECTIONS`] order. Built by
    /// [`directional_bounds`]; 2 MB for the 125x32x125 grid.
    ///
    /// Carried ALONGSIDE the chebyshev bytes, not instead of them: chebyshev
    /// answers "how big is the empty cube here", which the soft-shadow penumbra
    /// term needs and a directional bound cannot express.
    pub brick_bound_words: Vec<u32>,
    /// Whether [`Brickmap::brick_bound_words`] still describes this brickmap.
    ///
    /// Set false the first time an edit makes a previously empty brick occupied,
    /// at which point the field is FLATTENED to zeros — see
    /// [`Brickmap::invalidate_bounds`] for why that is the correct response
    /// rather than an incremental repair.
    bounds_valid: bool,
    /// Level-1 slots ever handed out (the high-water mark): the prefix of the
    /// level-1 arrays that has held real data. Live bricks =
    /// `allocated_brick_slots - free_brick_slots.len()`.
    allocated_brick_slots: u32,
    /// Slots the level-1 arrays have room for (`allocated_brick_slots` plus the
    /// remaining [`EDIT_BRICK_HEADROOM`]).
    brick_capacity: u32,
    /// Allocated-but-unused slots, from freed bricks, popped LIFO.
    ///
    /// FRAGMENTATION: there is none to manage. Every slot is exactly
    /// [`OCCUPANCY_WORDS_PER_BRICK`] + [`MATERIAL_WORDS_PER_BRICK`] words, so a
    /// freed slot fits any future brick exactly and no compaction, coalescing or
    /// best-fit search can ever help. The only waste is slack at the END of the
    /// arrays (freed slots that are never reused), bounded by the number of
    /// bricks that were ever occupied simultaneously — i.e. a session that digs a
    /// hill away and rebuilds it elsewhere reuses every slot.
    free_brick_slots: Vec<u32>,
    max_occupied_brick_y: u32,
}

/// One of the brickmap's own upload-ready arrays, by name. The brickmap must not
/// know what a GPU buffer is (module invariant), so a [`DirtyWords`] range names
/// an array and the uploader maps that to a binding.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BrickmapArray {
    BrickIndices,
    OccupancyWords,
    MaterialWords,
    ColumnMaxBrickY,
    BrickOccupancyBits,
    BrickSkipDistances,
    BrickBounds,
}

impl BrickmapArray {
    /// Every array, for the uploader's exhaustive mapping and the tests.
    pub const ALL: [BrickmapArray; 7] = [
        BrickmapArray::BrickIndices,
        BrickmapArray::OccupancyWords,
        BrickmapArray::MaterialWords,
        BrickmapArray::ColumnMaxBrickY,
        BrickmapArray::BrickOccupancyBits,
        BrickmapArray::BrickSkipDistances,
        BrickmapArray::BrickBounds,
    ];
}

/// A contiguous run of `u32` words of one array that an edit changed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct DirtyWords {
    pub array: BrickmapArray,
    pub first_word: usize,
    pub word_count: usize,
}

impl DirtyWords {
    pub fn bytes(&self) -> usize {
        self.word_count * 4
    }
}

/// How a removal that EMPTIES a brick updates the chebyshev clearance field.
///
/// The asymmetry this enum exists for: adding solid can only *shrink* clearance,
/// and the new field is exactly `min(old, chebyshev_to_the_new_brick)` — a
/// bounded, exact, cheap local update with no strategy choice to make. Removing
/// solid can *grow* clearance arbitrarily far away (a lone brick in open air is
/// the nearest occupied brick for a huge region), so the update is either
/// bounded-and-conservative or a full rebuild. Measured in the bench doc's E2
/// section.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearanceUpdate {
    /// Recompute the exact transform inside a box of `radius_cells` bricks
    /// around the freed brick, seeded from the ring just outside it.
    ///
    /// WHY THIS IS SAFE AT ANY RADIUS: the seeds are the *old* distances, which
    /// after a removal are ≤ the new exact distances, and the chamfer sweep can
    /// only produce `min(seed + path, distance to occupied inside the box)`. So
    /// every value written is ≤ the exact new value — an UNDERESTIMATE, which
    /// the traversal tolerates by construction (a cell at distance d claims a
    /// guaranteed-empty cube of half-width d-1; a smaller d claims less and only
    /// costs steps). An overestimate would tunnel through geometry and is
    /// impossible here. Outside the box the field simply stays stale-low.
    ///
    /// AND HOW WRONG IT CAN BE, exactly: writing `D` for the freed brick's own
    /// new clearance (its chebyshev distance to the nearest SURVIVING brick),
    /// every cell satisfies `old ≤ local ≤ exact_new ≤ old + D`, so the deficit
    /// is at most `D` everywhere — *independent of the radius*. Proof sketch: a
    /// cell whose nearest brick was not the freed one is unchanged; for any other
    /// cell p, `exact_new(p) ≤ cheb(p, freed) + D = old(p) + D`, and the chamfer's
    /// candidates are all ≥ `old(p)`. Consequence for the lever: `D = 1` for any
    /// edit into terrain (the freed brick still has neighbours), so the bounded
    /// update is exact there; a large `D` only happens for an isolated brick in
    /// open air, where a one-cell-too-small clearance costs one extra DDA step.
    /// The radius therefore buys *how many cells become exact*, not safety.
    LocalBox { radius_cells: u32 },
    /// Recompute the whole field. Exact everywhere, and the honest baseline the
    /// local update is judged against.
    FullRebuild,
}

/// What one [`Brickmap::set_voxel`] changed — the delta the GPU uploader
/// consumes and the numbers the bench reports.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct BrickmapEdit {
    pub voxel: [i32; 3],
    pub previous_material: u8,
    pub material: u8,
    /// The edit materialized a brick (level-0 pointer allocated).
    pub brick_allocated: bool,
    /// The edit emptied a brick (level-0 pointer freed to the slot list).
    pub brick_freed: bool,
    /// The level-1 arrays outgrew their headroom: the GPU buffers must be
    /// REALLOCATED and re-uploaded whole, not patched.
    pub arrays_grew: bool,
    /// The brick count or the global max brick Y moved, so the metadata uniform
    /// is stale.
    pub metadata_changed: bool,
    /// Brick cells whose clearance byte was written — the cost of the
    /// distance-field half of this edit.
    pub clearance_cells_written: usize,
    /// Word ranges to upload, coalesced (see [`DIRTY_RANGE_GAP_WORDS`]).
    pub dirty: Vec<DirtyWords>,
}

impl BrickmapEdit {
    /// Bytes an uploader has to move for this edit.
    pub fn dirty_bytes(&self) -> usize {
        self.dirty.iter().map(DirtyWords::bytes).sum()
    }
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
        Brickmap::build_with_collapse(world, true)
    }

    /// [`Brickmap::build`] with the uniform collapse suppressed — a MEASUREMENT
    /// FIXTURE, not a shipping configuration.
    ///
    /// With no collapse there are no [`BRICK_TAG_UNIFORM`] pointers, so
    /// `brick_is_uniform` is false everywhere and the shader's uniform fast path
    /// never fires. That makes this the only valid "uniform tag off" state: the
    /// tag and the fast path are one data format (see
    /// [`collapse_uniform_bricks`]), so they can only be disabled together, and
    /// only by building different DATA rather than by flipping a shader const.
    /// `bench_dda --no-collapse` uses this to put a frame-time number on a change
    /// that is otherwise memory-measured only.
    pub fn build_uncollapsed(world: &VoxelWorld) -> Brickmap {
        Brickmap::build_with_collapse(world, false)
    }

    fn build_with_collapse(world: &VoxelWorld, collapse: bool) -> Brickmap {
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

        if collapse {
            collapse_uniform_bricks(
                &mut brick_indices,
                &mut occupancy_words,
                &mut material_words,
            );
        }

        Brickmap::finish(
            brick_indices,
            occupancy_words,
            material_words,
            column_max_brick_y,
            max_occupied_brick_y,
        )
    }

    /// An all-air brickmap, sized for the world but holding nothing.
    ///
    /// The entry point for scenes that are *composed* rather than generated — S0's
    /// [`crate::studio`] builds its sample voxel with [`Brickmap::set_voxel`] from
    /// here. There is no `VoxelWorld::empty`, and adding one would mean a
    /// generation-side change to serve a renderer-side need; the brickmap already
    /// owns the incremental edit path (E2), so composing through it reuses the
    /// tested route instead of opening a second one.
    ///
    /// Cheap in wall-clock but NOT in memory: the level-0 grid and the derived
    /// per-cell fields are dense and sized by the world, so this allocates the same
    /// ~7 MB of level-0 arrays a generated world does. Only the level-1 brick data
    /// is empty.
    pub fn empty() -> Brickmap {
        Brickmap::finish(
            vec![EMPTY_BRICK; BRICK_GRID_X * BRICK_GRID_Y * BRICK_GRID_Z],
            Vec::new(),
            Vec::new(),
            vec![EMPTY_COLUMN; BRICK_GRID_X * BRICK_GRID_Z],
            EMPTY_COLUMN,
        )
    }

    /// Add the edit headroom and build every DERIVED field from a finished level-0
    /// grid — the one place that happens, so [`Brickmap::build`] and
    /// [`Brickmap::empty`] cannot disagree about what a consistent brickmap is.
    fn finish(
        brick_indices: Vec<u32>,
        mut occupancy_words: Vec<u32>,
        mut material_words: Vec<u32>,
        column_max_brick_y: Vec<u32>,
        max_occupied_brick_y: u32,
    ) -> Brickmap {
        let allocated_brick_slots = (occupancy_words.len() / OCCUPANCY_WORDS_PER_BRICK) as u32;
        // Edit headroom (E2): the arrays — and therefore the GPU buffers created
        // from them — carry EDIT_BRICK_HEADROOM spare slots, so materializing a
        // brick is a word patch instead of a buffer reallocation.
        let brick_capacity = allocated_brick_slots + EDIT_BRICK_HEADROOM as u32;
        occupancy_words.resize(brick_capacity as usize * OCCUPANCY_WORDS_PER_BRICK, 0);
        material_words.resize(brick_capacity as usize * MATERIAL_WORDS_PER_BRICK, 0);
        let brick_occupancy_bit_words = pack_occupancy_bits(&brick_indices);
        let brick_bound_words =
            directional_bounds(&brick_indices, BRICK_GRID_X, BRICK_GRID_Y, BRICK_GRID_Z);
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
            brick_bound_words,
            bounds_valid: true,
            allocated_brick_slots,
            brick_capacity,
            free_brick_slots: Vec::new(),
            max_occupied_brick_y,
        }
    }

    /// CPU-side material lookup: the material id at a world voxel coordinate,
    /// 0 for air and for anything out of bounds. Material bytes are only ever
    /// non-zero where the occupancy bit is set, so this is exactly the
    /// GPU-visible value.
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
        if brick_is_uniform(pointer) {
            return brick_uniform_material(pointer);
        }
        let bit = x as usize % BRICK_SIZE
            + (y as usize % BRICK_SIZE) * 8
            + (z as usize % BRICK_SIZE) * 64;
        let word = self.material_words
            [brick_slot(pointer) as usize * MATERIAL_WORDS_PER_BRICK + (bit >> 2)];
        ((word >> ((bit & 3) * 8)) & 0xff) as u8
    }

    /// Whether the occupancy bit is set at a world voxel coordinate (false
    /// out of bounds). Equivalent to `get(...) != 0` by construction; this is the
    /// entry point [`crate::voxel_dda`] traverses on, i.e. the one the acoustic
    /// resolver (E8) will reuse.
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
        if brick_is_uniform(pointer) {
            return true;
        }
        let bit = x as usize % BRICK_SIZE
            + (y as usize % BRICK_SIZE) * 8
            + (z as usize % BRICK_SIZE) * 64;
        let word = self.occupancy_words
            [brick_slot(pointer) as usize * OCCUPANCY_WORDS_PER_BRICK + (bit >> 5)];
        (word >> (bit & 31)) & 1 == 1
    }

    /// Highest occupied voxel of one XZ column, or `None` for an empty column.
    ///
    /// Starts from the column's cached max brick Y (GPU binding 8) instead of the
    /// world ceiling, so a scan touches the handful of voxels above the surface
    /// rather than all 256 layers. Note the cache is per BRICK column (an 8x8
    /// footprint), so it is an upper bound for this voxel column, not its answer.
    pub fn column_top_occupied_voxel(&self, x: i32, z: i32) -> Option<i32> {
        if x < 0 || z < 0 || x >= WORLD_SIZE_X as i32 || z >= WORLD_SIZE_Z as i32 {
            return None;
        }
        let column = x as usize / BRICK_SIZE + (z as usize / BRICK_SIZE) * BRICK_GRID_X;
        let max_brick_y = self.column_max_brick_y[column];
        if max_brick_y == EMPTY_COLUMN {
            return None;
        }
        let highest_voxel_y = (max_brick_y as i32 + 1) * BRICK_SIZE as i32 - 1;
        (0..=highest_voxel_y)
            .rev()
            .find(|y| self.is_occupied(x, *y, z))
    }

    /// The GPU uniform describing this brickmap's dimensions.
    pub fn metadata(&self) -> BrickmapMetadata {
        BrickmapMetadata {
            brick_grid_size: [
                BRICK_GRID_X as u32,
                BRICK_GRID_Y as u32,
                BRICK_GRID_Z as u32,
            ],
            occupied_brick_count: self.occupied_brick_count(),
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
        self.allocated_brick_slots - self.free_brick_slots.len() as u32
    }

    /// Level-1 slots the arrays have room for — what the GPU buffers are sized
    /// for, i.e. occupied bricks plus the remaining [`EDIT_BRICK_HEADROOM`].
    pub fn brick_capacity(&self) -> u32 {
        self.brick_capacity
    }

    /// Freed-but-not-yet-reused level-1 slots (the free list's length).
    pub fn free_brick_slot_count(&self) -> usize {
        self.free_brick_slots.len()
    }

    /// Total bytes of the CPU-side arrays — the "CPU memory" column of the E2
    /// verdict, and the size of the mirror a GPU-authoritative world would have
    /// to keep fresh for audio.
    pub fn cpu_bytes(&self) -> usize {
        (self.brick_indices.len()
            + self.occupancy_words.len()
            + self.material_words.len()
            + self.column_max_brick_y.len()
            + self.brick_occupancy_bit_words.len()
            + self.brick_skip_distance_words.len()
            + self.brick_bound_words.len())
            * 4
    }

    /// Chebyshev clearance of a brick cell, in bricks: 0 = the brick is occupied,
    /// otherwise the brick sits centered in a guaranteed-empty cube of half-width
    /// `clearance - 1`. The CPU mirror of `skip_distance_of` in `world.wgsl` —
    /// [`crate::voxel_dda`] uses it as its skip stride, so the CPU and GPU
    /// traversals accelerate on the same data. Out-of-grid bricks read 0 (the
    /// conservative answer: "do not skip").
    pub fn brick_clearance_cells(&self, brick: [i32; 3]) -> u8 {
        if brick[0] < 0
            || brick[1] < 0
            || brick[2] < 0
            || brick[0] >= BRICK_GRID_X as i32
            || brick[1] >= BRICK_GRID_Y as i32
            || brick[2] >= BRICK_GRID_Z as i32
        {
            return 0;
        }
        let cell = brick[0] as usize
            + brick[1] as usize * BRICK_GRID_X
            + brick[2] as usize * BRICK_GRID_X * BRICK_GRID_Y;
        self.skip_distance_at(cell)
    }

    /// One of the upload-ready arrays, by name — how the GPU uploader turns a
    /// [`DirtyWords`] range into bytes without this module knowing about buffers.
    pub fn array_words(&self, array: BrickmapArray) -> &[u32] {
        match array {
            BrickmapArray::BrickIndices => &self.brick_indices,
            BrickmapArray::OccupancyWords => &self.occupancy_words,
            BrickmapArray::MaterialWords => &self.material_words,
            BrickmapArray::ColumnMaxBrickY => &self.column_max_brick_y,
            BrickmapArray::BrickOccupancyBits => &self.brick_occupancy_bit_words,
            BrickmapArray::BrickSkipDistances => &self.brick_skip_distance_words,
            BrickmapArray::BrickBounds => &self.brick_bound_words,
        }
    }

    // ---- Editing (E2) --------------------------------------------------------

    /// Set one voxel and repair EVERY derived structure, returning the word
    /// ranges that changed (`None` when the voxel already held this material, so
    /// a hold-to-repeat click on the same target costs nothing downstream).
    ///
    /// What gets repaired, in the order the code does it:
    ///
    /// 1. the level-1 occupancy bit and material byte;
    /// 2. the level-0 pointer — a brick MATERIALIZES on the first non-air voxel
    ///    (slot from the free list, else the headroom, else the arrays grow) and
    ///    is FREED on the last one (words zeroed, slot pushed back);
    /// 3. the 1-bit brick occupancy grid (binding 9);
    /// 4. the chebyshev clearance field (binding 10) — see [`ClearanceUpdate`]
    ///    for the add/remove asymmetry;
    /// 5. the per-XZ-column max brick Y (binding 8) and the global max in the
    ///    metadata uniform.
    ///
    /// Steps 2-5 only run when the brick's *occupancy* flipped, which is the
    /// reason a typical edit is 576 bytes: carving another voxel out of solid
    /// ground touches two words and nothing else.
    pub fn set_voxel(
        &mut self,
        x: i32,
        y: i32,
        z: i32,
        voxel: Voxel,
        clearance: ClearanceUpdate,
    ) -> Option<BrickmapEdit> {
        if x < 0
            || y < 0
            || z < 0
            || x >= WORLD_SIZE_X as i32
            || y >= WORLD_SIZE_Y as i32
            || z >= WORLD_SIZE_Z as i32
        {
            return None;
        }
        let material = material_id(voxel);
        let previous_material = self.get(x, y, z);
        if material == previous_material {
            return None;
        }

        let brick = [
            x as usize / BRICK_SIZE,
            y as usize / BRICK_SIZE,
            z as usize / BRICK_SIZE,
        ];
        let cell = brick[0] + brick[1] * BRICK_GRID_X + brick[2] * BRICK_GRID_X * BRICK_GRID_Y;
        let bit = x as usize % BRICK_SIZE
            + (y as usize % BRICK_SIZE) * 8
            + (z as usize % BRICK_SIZE) * 64;
        let column = brick[0] + brick[2] * BRICK_GRID_X;

        let mut edit = BrickmapEdit {
            voxel: [x, y, z],
            previous_material,
            material,
            brick_allocated: false,
            brick_freed: false,
            arrays_grew: false,
            metadata_changed: false,
            clearance_cells_written: 0,
            dirty: Vec::new(),
        };
        let mut ranges = DirtyRanges::default();

        // COPY-ON-WRITE. A uniform brick has no level-1 data to patch, so any
        // edit inside one has to give it private storage first — including a
        // dig, which is the common case (carving a tunnel through solid stone
        // hits nothing but uniform bricks). After this the cell is UNIQUE and
        // the code below is the original untagged path unchanged.
        if brick_is_uniform(self.brick_indices[cell]) {
            self.materialize_uniform_brick(cell, &mut edit, &mut ranges);
        }

        if material != 0 {
            if self.brick_indices[cell] == EMPTY_BRICK {
                let (slot, arrays_grew) = self.allocate_brick_slot();
                self.brick_indices[cell] = slot;
                edit.brick_allocated = true;
                edit.arrays_grew = arrays_grew;
                edit.metadata_changed = true;
                ranges.push(BrickmapArray::BrickIndices, cell);
                self.brick_occupancy_bit_words[cell >> 5] |= 1 << (cell & 31);
                ranges.push(BrickmapArray::BrickOccupancyBits, cell >> 5);
                // A cell just GAINED occupancy, which is the one direction that
                // can make an AADF bound unsafe.
                self.invalidate_bounds(&mut ranges);
                edit.clearance_cells_written += self.shrink_clearance_around(brick, &mut ranges);
                self.raise_column_and_world_max(column, brick[1] as u32, &mut ranges);
            }
            let pointer = brick_slot(self.brick_indices[cell]) as usize;
            let occupancy_word = pointer * OCCUPANCY_WORDS_PER_BRICK + (bit >> 5);
            self.occupancy_words[occupancy_word] |= 1 << (bit & 31);
            ranges.push(BrickmapArray::OccupancyWords, occupancy_word);
            let material_word = pointer * MATERIAL_WORDS_PER_BRICK + (bit >> 2);
            let shift = (bit & 3) * 8;
            self.material_words[material_word] &= !(0xff << shift);
            self.material_words[material_word] |= u32::from(material) << shift;
            ranges.push(BrickmapArray::MaterialWords, material_word);
        } else {
            let pointer = brick_slot(self.brick_indices[cell]) as usize;
            let occupancy_word = pointer * OCCUPANCY_WORDS_PER_BRICK + (bit >> 5);
            self.occupancy_words[occupancy_word] &= !(1 << (bit & 31));
            ranges.push(BrickmapArray::OccupancyWords, occupancy_word);
            let material_word = pointer * MATERIAL_WORDS_PER_BRICK + (bit >> 2);
            self.material_words[material_word] &= !(0xff << ((bit & 3) * 8));
            ranges.push(BrickmapArray::MaterialWords, material_word);

            let brick_words = &self.occupancy_words
                [pointer * OCCUPANCY_WORDS_PER_BRICK..(pointer + 1) * OCCUPANCY_WORDS_PER_BRICK];
            if brick_words.iter().all(|word| *word == 0) {
                self.free_brick_slot(pointer as u32, &mut ranges);
                self.brick_indices[cell] = EMPTY_BRICK;
                edit.brick_freed = true;
                edit.metadata_changed = true;
                ranges.push(BrickmapArray::BrickIndices, cell);
                self.brick_occupancy_bit_words[cell >> 5] &= !(1 << (cell & 31));
                ranges.push(BrickmapArray::BrickOccupancyBits, cell >> 5);
                edit.clearance_cells_written +=
                    self.grow_clearance_around(brick, clearance, &mut ranges);
                self.lower_column_and_world_max(column, brick[1] as u32, &mut ranges);
            }
        }

        edit.dirty = ranges.finish();
        Some(edit)
    }

    /// Flatten the AADF bound field to zeros, permanently, the first time an
    /// edit makes a previously empty brick occupied.
    ///
    /// WHY FLATTENING RATHER THAN REPAIRING. A bound is only unsafe when it is
    /// too LARGE, so digging is harmless (the field just gets conservative) and
    /// only ADDING geometry can break it. A new brick at cell `c` invalidates
    /// every cell whose box reaches `c` — up to [`BOUND_MAX`] cells away on each
    /// axis, so a repair means re-relaxing a 63^3 box, ~250k cells, tens of
    /// milliseconds PER EDIT. That is not a price worth paying for a lever that
    /// measured slower than the chebyshev cube it replaces (see the
    /// `DirectionalSkip` verdict in `src/variants.rs`).
    ///
    /// Zero is always safe: it claims nothing beyond the cell itself, so the
    /// traversal reads "no skip available" and takes a single step, exactly as it
    /// would with the lever off. The cost is one 2 MB upload, once per session,
    /// and after it the directional skip is inert until the next full rebuild.
    ///
    /// If AADF is ever revived — most likely on Quest, where the cache trade-off
    /// differs — THIS is the function to replace with a local re-relaxation, and
    /// `bounds_valid` is the flag that says whether one is owed.
    fn invalidate_bounds(&mut self, ranges: &mut DirtyRanges) {
        if !self.bounds_valid {
            return; // already flat; nothing can go stale twice
        }
        self.bounds_valid = false;
        self.brick_bound_words.fill(0);
        ranges.push_range(BrickmapArray::BrickBounds, 0..self.brick_bound_words.len());
    }

    /// Whether [`Brickmap::brick_bound_words`] still describes this brickmap. False
    /// once an edit has flattened it — see [`Brickmap::invalidate_bounds`].
    pub fn bounds_are_valid(&self) -> bool {
        self.bounds_valid
    }

    /// Give a UNIFORM cell private level-1 storage, expanding the tag back into
    /// 512 set occupancy bits and 512 copies of its material — the copy half of
    /// copy-on-write.
    ///
    /// The whole brick is marked dirty (16 + 128 words), which is the honest
    /// cost: the GPU has never seen these words. That makes the FIRST edit
    /// inside a uniform brick ~576 bytes instead of the usual ~8, and every
    /// edit after it cheap again, because the cell is UNIQUE from then on.
    fn materialize_uniform_brick(
        &mut self,
        cell: usize,
        edit: &mut BrickmapEdit,
        ranges: &mut DirtyRanges,
    ) {
        let material = brick_uniform_material(self.brick_indices[cell]);
        let (slot, arrays_grew) = self.allocate_brick_slot();
        self.brick_indices[cell] = slot;
        edit.brick_allocated = true;
        edit.arrays_grew |= arrays_grew;
        edit.metadata_changed = true;
        ranges.push(BrickmapArray::BrickIndices, cell);

        let slot = slot as usize;
        let splatted = u32::from_le_bytes([material; 4]);
        for word in 0..OCCUPANCY_WORDS_PER_BRICK {
            let index = slot * OCCUPANCY_WORDS_PER_BRICK + word;
            self.occupancy_words[index] = u32::MAX;
            ranges.push(BrickmapArray::OccupancyWords, index);
        }
        for word in 0..MATERIAL_WORDS_PER_BRICK {
            let index = slot * MATERIAL_WORDS_PER_BRICK + word;
            self.material_words[index] = splatted;
            ranges.push(BrickmapArray::MaterialWords, index);
        }
        // Occupancy of the CELL is unchanged — it was solid as a uniform tag and
        // it is solid as a brick — so the bit grid, the clearance field and the
        // column maxima all stay exactly as they were.
    }

    /// A free level-1 slot: reused first, then the headroom, and only then do the
    /// arrays grow (which forces a whole-buffer reallocation upstream).
    fn allocate_brick_slot(&mut self) -> (u32, bool) {
        if let Some(slot) = self.free_brick_slots.pop() {
            return (slot, false);
        }
        let slot = self.allocated_brick_slots;
        let mut arrays_grew = false;
        if slot >= self.brick_capacity {
            self.brick_capacity += EDIT_BRICK_HEADROOM as u32;
            self.occupancy_words
                .resize(self.brick_capacity as usize * OCCUPANCY_WORDS_PER_BRICK, 0);
            self.material_words
                .resize(self.brick_capacity as usize * MATERIAL_WORDS_PER_BRICK, 0);
            arrays_grew = true;
        }
        self.allocated_brick_slots += 1;
        (slot, arrays_grew)
    }

    /// Return a slot to the free list, ZEROING its words (and marking them dirty)
    /// so the GPU copy stays identical to the CPU copy: a later reuse only
    /// uploads the one word it touches, which would otherwise expose the dead
    /// brick's leftovers.
    fn free_brick_slot(&mut self, slot: u32, ranges: &mut DirtyRanges) {
        let occupancy_base = slot as usize * OCCUPANCY_WORDS_PER_BRICK;
        for word in occupancy_base..occupancy_base + OCCUPANCY_WORDS_PER_BRICK {
            self.occupancy_words[word] = 0;
            ranges.push(BrickmapArray::OccupancyWords, word);
        }
        let material_base = slot as usize * MATERIAL_WORDS_PER_BRICK;
        for word in material_base..material_base + MATERIAL_WORDS_PER_BRICK {
            self.material_words[word] = 0;
            ranges.push(BrickmapArray::MaterialWords, word);
        }
        self.free_brick_slots.push(slot);
    }

    /// Clearance byte of a brick cell (the packed chebyshev distance field).
    fn skip_distance_at(&self, cell: usize) -> u8 {
        ((self.brick_skip_distance_words[cell >> 2] >> ((cell & 3) * 8)) & 0xff) as u8
    }

    fn set_skip_distance(&mut self, cell: usize, distance: u8, ranges: &mut DirtyRanges) {
        let shift = (cell & 3) * 8;
        let word = &mut self.brick_skip_distance_words[cell >> 2];
        *word &= !(0xff << shift);
        *word |= u32::from(distance) << shift;
        ranges.push(BrickmapArray::BrickSkipDistances, cell >> 2);
    }

    /// A brick MATERIALIZED at `brick`: the new field is exactly
    /// `min(old, chebyshev distance to brick)`, so walk outward in chebyshev
    /// shells and stop at the first shell that improves nothing.
    ///
    /// The early-out is exact, not a heuristic: a cell q at shell k+1 can only
    /// improve if `d(q) > k + 1`, and then its neighbour toward `brick` (which
    /// sits at shell k and is in-grid, being between q and the brick) has
    /// `d ≥ d(q) - 1 > k` and would have improved too. No improvement at k
    /// therefore means none beyond it.
    fn shrink_clearance_around(&mut self, brick: [usize; 3], ranges: &mut DirtyRanges) -> usize {
        let center = [brick[0] as i32, brick[1] as i32, brick[2] as i32];
        let cell_of = |x: i32, y: i32, z: i32| {
            x as usize + y as usize * BRICK_GRID_X + z as usize * BRICK_GRID_X * BRICK_GRID_Y
        };
        let mut written = 1;
        self.set_skip_distance(cell_of(center[0], center[1], center[2]), 0, ranges);
        for radius in 1..=u8::MAX as i32 {
            let mut improved = false;
            for z in (center[2] - radius).max(0)..=(center[2] + radius).min(BRICK_GRID_Z as i32 - 1)
            {
                for y in
                    (center[1] - radius).max(0)..=(center[1] + radius).min(BRICK_GRID_Y as i32 - 1)
                {
                    for x in (center[0] - radius).max(0)
                        ..=(center[0] + radius).min(BRICK_GRID_X as i32 - 1)
                    {
                        let chebyshev = (x - center[0])
                            .abs()
                            .max((y - center[1]).abs())
                            .max((z - center[2]).abs());
                        if chebyshev != radius {
                            continue;
                        }
                        let cell = cell_of(x, y, z);
                        if i32::from(self.skip_distance_at(cell)) > radius {
                            self.set_skip_distance(cell, radius as u8, ranges);
                            written += 1;
                            improved = true;
                        }
                    }
                }
            }
            if !improved {
                break;
            }
        }
        written
    }

    /// A brick was FREED at `brick`: clearance can grow, so either recompute a
    /// bounded box (conservative outside it — see [`ClearanceUpdate::LocalBox`])
    /// or rebuild the whole field.
    fn grow_clearance_around(
        &mut self,
        brick: [usize; 3],
        clearance: ClearanceUpdate,
        ranges: &mut DirtyRanges,
    ) -> usize {
        match clearance {
            ClearanceUpdate::FullRebuild => {
                self.brick_skip_distance_words =
                    pack_bytes_little_endian(&chebyshev_skip_distances(
                        &self.brick_indices,
                        BRICK_GRID_X,
                        BRICK_GRID_Y,
                        BRICK_GRID_Z,
                    ));
                ranges.push_range(
                    BrickmapArray::BrickSkipDistances,
                    0..self.brick_skip_distance_words.len(),
                );
                BRICK_GRID_X * BRICK_GRID_Y * BRICK_GRID_Z
            }
            ClearanceUpdate::LocalBox { radius_cells } => {
                self.recompute_clearance_box(brick, radius_cells as i32, ranges)
            }
        }
    }

    /// The bounded local recompute: an exact chamfer transform over the box of
    /// half-width `radius` around `brick`, seeded with the surviving distances of
    /// the ring one cell outside it.
    fn recompute_clearance_box(
        &mut self,
        brick: [usize; 3],
        radius: i32,
        ranges: &mut DirtyRanges,
    ) -> usize {
        let center = [brick[0] as i32, brick[1] as i32, brick[2] as i32];
        let grid = [
            BRICK_GRID_X as i32,
            BRICK_GRID_Y as i32,
            BRICK_GRID_Z as i32,
        ];
        // The sub-grid spans the interior box plus one seed ring; the ring is
        // clipped away at the world bounds, where there is nothing to seed from.
        let low: Vec<i32> = (0..3)
            .map(|axis| (center[axis] - radius - 1).max(0))
            .collect();
        let high: Vec<i32> = (0..3)
            .map(|axis| (center[axis] + radius + 1).min(grid[axis] - 1))
            .collect();
        let size: Vec<i32> = (0..3).map(|axis| high[axis] - low[axis] + 1).collect();
        let sub_index = |x: i32, y: i32, z: i32| {
            ((x - low[0]) + (y - low[1]) * size[0] + (z - low[2]) * size[0] * size[1]) as usize
        };
        let interior = |x: i32, y: i32, z: i32| {
            (x - center[0])
                .abs()
                .max((y - center[1]).abs())
                .max((z - center[2]).abs())
                <= radius
        };

        let mut distances = vec![u8::MAX; (size[0] * size[1] * size[2]) as usize];
        for z in low[2]..=high[2] {
            for y in low[1]..=high[1] {
                for x in low[0]..=high[0] {
                    let cell = x as usize
                        + y as usize * BRICK_GRID_X
                        + z as usize * BRICK_GRID_X * BRICK_GRID_Y;
                    distances[sub_index(x, y, z)] = if self.brick_indices[cell] != EMPTY_BRICK {
                        0
                    } else if interior(x, y, z) {
                        u8::MAX
                    } else {
                        // Seed ring: the old value, which after a removal is at
                        // most the new exact value.
                        self.skip_distance_at(cell)
                    };
                }
            }
        }
        chamfer_sweeps(&mut distances, size[0], size[1], size[2]);

        let mut written = 0;
        for z in low[2]..=high[2] {
            for y in low[1]..=high[1] {
                for x in low[0]..=high[0] {
                    if !interior(x, y, z) {
                        continue;
                    }
                    let cell = x as usize
                        + y as usize * BRICK_GRID_X
                        + z as usize * BRICK_GRID_X * BRICK_GRID_Y;
                    let distance = distances[sub_index(x, y, z)];
                    if distance != self.skip_distance_at(cell) {
                        self.set_skip_distance(cell, distance, ranges);
                        written += 1;
                    }
                }
            }
        }
        written
    }

    /// A brick materialized at `brick_y`: both maxima can only rise, so this is
    /// two compares.
    fn raise_column_and_world_max(
        &mut self,
        column: usize,
        brick_y: u32,
        ranges: &mut DirtyRanges,
    ) {
        if self.column_max_brick_y[column] == EMPTY_COLUMN
            || self.column_max_brick_y[column] < brick_y
        {
            self.column_max_brick_y[column] = brick_y;
            ranges.push(BrickmapArray::ColumnMaxBrickY, column);
        }
        if self.max_occupied_brick_y == EMPTY_COLUMN || self.max_occupied_brick_y < brick_y {
            self.max_occupied_brick_y = brick_y;
        }
    }

    /// A brick was freed at `brick_y`: the column's max may drop (rescan its 32
    /// cells), and if that was the world's tallest brick the global max is
    /// rescanned from the column grid (15 625 reads — microseconds, and only when
    /// the very top of the world comes off).
    fn lower_column_and_world_max(
        &mut self,
        column: usize,
        brick_y: u32,
        ranges: &mut DirtyRanges,
    ) {
        if self.column_max_brick_y[column] != brick_y {
            return;
        }
        let brick_x = column % BRICK_GRID_X;
        let brick_z = column / BRICK_GRID_X;
        let mut column_max = EMPTY_COLUMN;
        for candidate_y in 0..BRICK_GRID_Y {
            let cell = brick_x + candidate_y * BRICK_GRID_X + brick_z * BRICK_GRID_X * BRICK_GRID_Y;
            if self.brick_indices[cell] != EMPTY_BRICK {
                column_max = candidate_y as u32;
            }
        }
        self.column_max_brick_y[column] = column_max;
        ranges.push(BrickmapArray::ColumnMaxBrickY, column);
        if self.max_occupied_brick_y == brick_y {
            self.max_occupied_brick_y = self
                .column_max_brick_y
                .iter()
                .filter(|max| **max != EMPTY_COLUMN)
                .max()
                .copied()
                .unwrap_or(EMPTY_COLUMN);
        }
    }
}

/// Dirty word ranges being accumulated by one edit, coalesced on [`Self::finish`].
#[derive(Default)]
struct DirtyRanges {
    ranges: Vec<(BrickmapArray, Range<usize>)>,
}

impl DirtyRanges {
    fn push(&mut self, array: BrickmapArray, word: usize) {
        self.push_range(array, word..word + 1);
    }

    fn push_range(&mut self, array: BrickmapArray, words: Range<usize>) {
        self.ranges.push((array, words));
    }

    /// Sort per array and merge ranges separated by at most
    /// [`DIRTY_RANGE_GAP_WORDS`] unchanged words.
    fn finish(self) -> Vec<DirtyWords> {
        coalesce_dirty_words(
            self.ranges
                .into_iter()
                .map(|(array, words)| DirtyWords {
                    array,
                    first_word: words.start,
                    word_count: words.len(),
                })
                .collect(),
        )
    }
}

/// Sort word ranges per array and merge the ones separated by at most
/// [`DIRTY_RANGE_GAP_WORDS`] unchanged words — one `write_buffer` per survivor.
///
/// Public because a BULK edit
/// ([`crate::world_edit::apply_bulk`]) coalesces ACROSS thousands of
/// [`Brickmap::set_voxel`] calls: without that, a pool carve would publish tens
/// of thousands of one-word uploads for words that are mostly neighbours.
pub fn coalesce_dirty_words(mut ranges: Vec<DirtyWords>) -> Vec<DirtyWords> {
    ranges.sort_by_key(|range| {
        (
            BrickmapArray::ALL
                .iter()
                .position(|candidate| *candidate == range.array)
                .expect("every array is in BrickmapArray::ALL"),
            range.first_word,
        )
    });
    let mut merged: Vec<DirtyWords> = Vec::new();
    for range in ranges {
        match merged.last_mut() {
            Some(last)
                if last.array == range.array
                    && range.first_word
                        <= last.first_word + last.word_count + DIRTY_RANGE_GAP_WORDS =>
            {
                let end =
                    (last.first_word + last.word_count).max(range.first_word + range.word_count);
                last.word_count = end - last.first_word;
            }
            _ => merged.push(range),
        }
    }
    merged
}

/// Retag every fully-solid single-material brick as [`BRICK_TAG_UNIFORM`] and
/// compact the level-1 arrays so the collapsed bricks cost nothing at all.
///
/// "Uniform" is strict: all 512 occupancy bits set AND all 512 material bytes
/// equal. A half-air brick does not qualify — a ray through it must still find
/// the surface, so there is nothing to skip.
///
/// MEASURED on the shipped island. Memory: 40,531 of 69,977 occupied bricks
/// collapse (57.9%), taking the whole brickmap from 45.2 MB to 21.9 MB. Frame
/// time, via `bench_dda --no-collapse` (minimum of three runs each, because the
/// uncollapsed build is the noisier one and noise only adds): scenario A 4.744 ms
/// collapsed against 5.069 uncollapsed (6.4% faster), scenario C 4.402 against
/// 4.899 (10.1% faster).
///
/// WHY THERE IS NO LEVER: a collapsed brick has no level-1 slot at all, so a
/// shader compiled without the fast path would read its material id as a slot
/// index and fetch an unrelated brick. The tag and the fast path are one
/// format, not a toggle. (A first attempt did ship them as a lever; the bench's
/// "no-uniform-bricks" column was measuring garbage.)
///
/// Adapted from NAADF — Ulschmid et al., CGF 2026, MIT-licensed at
/// <https://github.com/cg-tuwien/NAADF> — which tags nodes UNIFORM the same way.
///
/// Runs once at build time. Edits go the other way, through
/// [`Brickmap::materialize_uniform_brick`].
fn collapse_uniform_bricks(
    brick_indices: &mut [u32],
    occupancy_words: &mut Vec<u32>,
    material_words: &mut Vec<u32>,
) {
    let slot_count = occupancy_words.len() / OCCUPANCY_WORDS_PER_BRICK;

    // Pass 1: classify every allocated slot. `None` = stays a real brick,
    // `Some(material)` = collapses to a uniform tag.
    let mut collapse_to: Vec<Option<u8>> = Vec::with_capacity(slot_count);
    for slot in 0..slot_count {
        let occupancy = &occupancy_words
            [slot * OCCUPANCY_WORDS_PER_BRICK..(slot + 1) * OCCUPANCY_WORDS_PER_BRICK];
        if occupancy.iter().any(|word| *word != u32::MAX) {
            collapse_to.push(None);
            continue;
        }
        let materials =
            &material_words[slot * MATERIAL_WORDS_PER_BRICK..(slot + 1) * MATERIAL_WORDS_PER_BRICK];
        // Every byte of every word equal is the same as every word equal to
        // the first AND that word's four bytes equal to each other.
        let first = materials[0];
        let byte = (first & 0xff) as u8;
        let splatted = u32::from_le_bytes([byte, byte, byte, byte]);
        if first == splatted && materials.iter().all(|word| *word == splatted) {
            collapse_to.push(Some(byte));
        } else {
            collapse_to.push(None);
        }
    }

    // Pass 2: assign surviving slots their new, compacted index.
    let mut remap: Vec<u32> = Vec::with_capacity(slot_count);
    let mut surviving = 0_u32;
    for verdict in &collapse_to {
        if verdict.is_some() {
            remap.push(u32::MAX); // never dereferenced — the cell gets a tag
        } else {
            remap.push(surviving);
            surviving += 1;
        }
    }

    // Pass 3: repoint the grid. Empty cells are untouched; uniform cells lose
    // their pointer entirely; the rest slide down to their compacted slot.
    for pointer in brick_indices.iter_mut() {
        if *pointer == EMPTY_BRICK {
            continue;
        }
        let slot = brick_slot(*pointer) as usize;
        *pointer = match collapse_to[slot] {
            Some(material) => uniform_brick(material),
            None => remap[slot],
        };
    }

    // Pass 4: slide the level-1 payload down. Sources are always at or ahead of
    // destinations, so a single forward sweep over the same buffers is safe.
    for slot in 0..slot_count {
        if collapse_to[slot].is_some() {
            continue;
        }
        let destination = remap[slot] as usize;
        if destination == slot {
            continue;
        }
        occupancy_words.copy_within(
            slot * OCCUPANCY_WORDS_PER_BRICK..(slot + 1) * OCCUPANCY_WORDS_PER_BRICK,
            destination * OCCUPANCY_WORDS_PER_BRICK,
        );
        material_words.copy_within(
            slot * MATERIAL_WORDS_PER_BRICK..(slot + 1) * MATERIAL_WORDS_PER_BRICK,
            destination * MATERIAL_WORDS_PER_BRICK,
        );
    }
    occupancy_words.truncate(surviving as usize * OCCUPANCY_WORDS_PER_BRICK);
    material_words.truncate(surviving as usize * MATERIAL_WORDS_PER_BRICK);
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

// ---- Directional bounds (AADF) ------------------------------------------------

/// Bits per directional bound, so six fit one `u32` (6 x 5 = 30).
pub const BOUND_BITS: u32 = 5;

/// Largest value a bound can hold — 31 cells, i.e. a 31 m skip in one step.
/// The world is 125 bricks across, so a maximal free run costs four steps
/// instead of one; that is a far cheaper compromise than a second word per cell.
pub const BOUND_MAX: u32 = (1 << BOUND_BITS) - 1;

/// Field order, matching NAADF's `checkMatchingBounds`: -x, +x, -y, +y, -z, +z
/// at shifts 0, 5, 10, 15, 20, 25. Each entry is the axis and the step sign.
pub const BOUND_DIRECTIONS: [(usize, i32); 6] = [(0, -1), (0, 1), (1, -1), (1, 1), (2, -1), (2, 1)];

/// One directional bound out of a packed word.
#[inline]
pub const fn bound_of(packed: u32, direction: usize) -> u32 {
    (packed >> (direction as u32 * BOUND_BITS)) & BOUND_MAX
}

/// Per cell, how many FURTHER cells the ray may cross in each of the six axis
/// directions such that the whole box spanned by all six bounds is empty.
///
/// This is the AADF of NAADF (Ulschmid et al., CGF 2026, MIT) — see
/// `Content/shaders/world/data/boundsCommon.fxh`. It replaces nothing: the
/// chebyshev field stays, because the two answer different questions.
///
/// WHY IT BEATS AN ISOTROPIC FIELD. Chebyshev gives the half-width of the
/// largest empty CUBE, so it is capped by the nearest obstacle in ANY direction.
/// A ray skimming one cell above the terrain gets a cube of half-width 1 — the
/// ground is right there — even though it could fly a hundred metres forward.
/// Directional bounds describe a BOX, which is free to be long and thin: that
/// cell's `-y` bound is 0, its `+x` bound is however far the corridor runs. Low
/// sun shadows and grazing water reflections are made of exactly those rays.
///
/// THE GROWTH RULE, and why the box claim is sound. `bound_d(c)` may grow from
/// `k` to `k+1` only if the neighbour `n = c + d` is empty AND `bound_d'(n) >=
/// bound_d'(c)` for every direction except `-d` (the opposite one, which is the
/// only bound that cannot matter). Then the newly claimed slab sits at offset
/// `k` from `n`, inside `Box(n)`, and spans perpendicular extents no larger than
/// `n`'s — so `Box(c)` stays empty by induction. Dropping the OPPOSITE direction
/// from the test rather than the direction being grown is the whole trick; test
/// the growth direction too or the induction has nothing to stand on.
///
/// Relaxation runs to a fixed point, which takes one round per unit of the
/// largest bound (<= [`BOUND_MAX`] + 1 rounds). Updating in place is safe: every
/// value ever read is itself a valid bound, so the condition still implies
/// safety. The result is a conservative fixed point, not a unique maximum —
/// under-approximating costs steps and never correctness.
///
/// Costs 63 ms on the island (it was 1.16 s before the sweep order below; the
/// naive round-per-cell version needed ~32 rounds where this needs a handful).
/// Note 58% of empty cells saturate `+x` at [`BOUND_MAX`], so 5 bits — not the
/// geometry — is what caps most bounds; a maximal run costs four steps instead
/// of one. Widening would need a second word, which is not obviously worth it.
///
/// EDITS still rebuild the whole field. At 63 ms that is fine for a rebuild but
/// far too slow per edit; a localized re-relaxation (the way the chebyshev field
/// has `shrink_clearance_around` / `grow_clearance_around`) is still owed.
fn directional_bounds(
    brick_indices: &[u32],
    grid_x: usize,
    grid_y: usize,
    grid_z: usize,
) -> Vec<u32> {
    let mut bounds = vec![0_u32; brick_indices.len()];
    let index_of = |x: usize, y: usize, z: usize| x + y * grid_x + z * grid_x * grid_y;
    let extent = [grid_x, grid_y, grid_z];

    loop {
        let mut grew = false;
        for (direction, &(axis, step)) in BOUND_DIRECTIONS.iter().enumerate() {
            let opposite = direction ^ 1;
            let shift = direction as u32 * BOUND_BITS;
            // Sweep AGAINST the growth direction, so the neighbour a cell
            // consults has already been updated this pass. A whole corridor then
            // resolves in ONE sweep instead of advancing one cell per round —
            // that is the difference between ~32 rounds and a handful.
            let order = |axis_index: usize, length: usize| -> Vec<usize> {
                if axis_index == axis && step > 0 {
                    (0..length).rev().collect()
                } else {
                    (0..length).collect()
                }
            };
            let (order_x, order_y, order_z) =
                (order(0, grid_x), order(1, grid_y), order(2, grid_z));

            for &z in &order_z {
                for &y in &order_y {
                    for &x in &order_x {
                        let cell = index_of(x, y, z);
                        if brick_indices[cell] != EMPTY_BRICK {
                            continue; // occupied: every bound stays 0
                        }
                        let mut position = [x, y, z];
                        let moved = position[axis] as i32 + step;
                        if moved < 0 || moved >= extent[axis] as i32 {
                            continue; // the grid edge bounds the box
                        }
                        position[axis] = moved as usize;
                        let neighbour = index_of(position[0], position[1], position[2]);
                        if brick_indices[neighbour] != EMPTY_BRICK {
                            continue;
                        }
                        let neighbour_bounds = bounds[neighbour];
                        // Grow as far as this neighbour allows in one visit. The
                        // ceiling is `bound_d(neighbour) + 1`, because the test
                        // below includes direction `d` itself and fails once we
                        // pass the neighbour's own reach.
                        while bound_of(bounds[cell], direction) < BOUND_MAX {
                            let mine = bounds[cell];
                            // Every direction but the OPPOSITE one must be at
                            // least as roomy at the neighbour as it is here.
                            let matched = (0..6).all(|other| {
                                other == opposite
                                    || bound_of(neighbour_bounds, other) >= bound_of(mine, other)
                            });
                            if !matched {
                                break;
                            }
                            bounds[cell] += 1 << shift;
                            grew = true;
                        }
                    }
                }
            }
        }
        if !grew {
            return bounds;
        }
    }
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
    chamfer_sweeps(
        &mut distances,
        grid_size_x as i32,
        grid_size_y as i32,
        grid_size_z as i32,
    );
    distances
}

/// The two chamfer sweeps themselves, over an arbitrary dense grid of distance
/// bytes — seeds (0 for occupied, a boundary value, `u8::MAX` for unknown) in,
/// exact chebyshev distances out. Shared by the full transform above and by E2's
/// bounded local recompute, so the edit path and the build path cannot disagree
/// about the metric.
fn chamfer_sweeps(distances: &mut [u8], grid_size_x: i32, grid_size_y: i32, grid_size_z: i32) {
    assert_eq!(
        distances.len(),
        (grid_size_x * grid_size_y * grid_size_z) as usize
    );

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

    let grid_size = (grid_size_x, grid_size_y, grid_size_z);
    for z in 0..grid_size.2 {
        for y in 0..grid_size.1 {
            for x in 0..grid_size.0 {
                relax_cell(distances, grid_size, x, y, z, true);
            }
        }
    }
    for z in (0..grid_size.2).rev() {
        for y in (0..grid_size.1).rev() {
            for x in (0..grid_size.0).rev() {
                relax_cell(distances, grid_size, x, y, z, false);
            }
        }
    }
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

    // ---- E2: the edit API ----

    /// Brute-force reference for every derived structure, recomputed from the
    /// level-0 pointer grid — the same shape the build path uses, so an edit is
    /// judged against "what a rebuild would have produced".
    fn assert_derived_structures_are_consistent(brickmap: &Brickmap, exact_clearance: bool) {
        let cell_count = BRICK_GRID_X * BRICK_GRID_Y * BRICK_GRID_Z;
        for cell in 0..cell_count {
            let occupied = brickmap.brick_indices[cell] != EMPTY_BRICK;
            let bit = (brickmap.brick_occupancy_bit_words[cell >> 5] >> (cell & 31)) & 1 == 1;
            assert_eq!(bit, occupied, "occupancy bit mismatch at cell {cell}");
            assert_eq!(
                brickmap.skip_distance_at(cell) == 0,
                occupied,
                "clearance 0 must mean occupied at cell {cell}"
            );
        }

        let mut expected_global_max = EMPTY_COLUMN;
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
                assert_eq!(
                    brickmap.column_max_brick_y[brick_x + brick_z * BRICK_GRID_X],
                    expected_column_max,
                    "column max mismatch at ({brick_x}, {brick_z})"
                );
                if expected_column_max != EMPTY_COLUMN
                    && (expected_global_max == EMPTY_COLUMN
                        || expected_global_max < expected_column_max)
                {
                    expected_global_max = expected_column_max;
                }
            }
        }
        assert_eq!(
            brickmap.metadata().max_occupied_brick_y,
            expected_global_max,
            "global max brick Y mismatch"
        );

        // The clearance field: exact for adds and for FullRebuild removals,
        // never an OVERESTIMATE (the safety invariant) for local removals.
        let exact = chebyshev_skip_distances(
            &brickmap.brick_indices,
            BRICK_GRID_X,
            BRICK_GRID_Y,
            BRICK_GRID_Z,
        );
        let mut underestimates = 0_usize;
        for (cell, exact_distance) in exact.iter().enumerate().take(cell_count) {
            let actual = brickmap.skip_distance_at(cell);
            assert!(
                actual <= *exact_distance,
                "clearance OVERESTIMATE at cell {cell}: {actual} > exact {exact_distance}"
            );
            if actual < *exact_distance {
                underestimates += 1;
            }
        }
        if exact_clearance {
            assert_eq!(
                underestimates, 0,
                "{underestimates} cells carry a stale-low clearance where the update \
                 should have been exact"
            );
        }
    }

    /// Editing the real island: carve voxels out of the terrain, place voxels in
    /// open air, and empty whole bricks — then check every derived structure
    /// against a brute-force recompute. Dense-terrain edits must leave the
    /// clearance field EXACT even with the bounded local update.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn edits_keep_every_derived_structure_consistent() {
        let world = VoxelWorld::generate(1234, 0.0);
        let mut brickmap = Brickmap::build(&world);
        let clearance = ClearanceUpdate::LocalBox { radius_cells: 8 };

        // Find a surface column near the island center: the highest occupied
        // voxel of a few columns gives us real terrain to dig into.
        let mut edits_applied = 0_usize;
        let mut bricks_freed = 0_usize;
        for (x, z) in [(500, 500), (496, 504), (512, 488), (480, 520)] {
            let surface_y = (0..WORLD_SIZE_Y as i32)
                .rev()
                .find(|y| brickmap.is_occupied(x, *y, z))
                .expect("island column has occupied voxels");
            // Carve a whole 8x8x8 brick away — the only removal that touches the
            // level-0 pointer, the bit grid and the clearance field.
            let brick_base = [
                x - x % BRICK_SIZE as i32,
                surface_y - surface_y % BRICK_SIZE as i32,
                z - z % BRICK_SIZE as i32,
            ];
            for local_z in 0..BRICK_SIZE as i32 {
                for local_y in 0..BRICK_SIZE as i32 {
                    for local_x in 0..BRICK_SIZE as i32 {
                        let edit = brickmap.set_voxel(
                            brick_base[0] + local_x,
                            brick_base[1] + local_y,
                            brick_base[2] + local_z,
                            Voxel::Air,
                            clearance,
                        );
                        if let Some(edit) = edit {
                            edits_applied += 1;
                            if edit.brick_freed {
                                bricks_freed += 1;
                            }
                        }
                    }
                }
            }
            // ...and place a stone block back on the surface.
            for offset in 0..4 {
                brickmap.set_voxel(x, surface_y + 1 + offset, z, Voxel::Stone, clearance);
                edits_applied += 1;
            }
        }
        assert!(
            edits_applied > 100 && bricks_freed == 4,
            "{edits_applied} edits, {bricks_freed} bricks freed — the test lost its grip \
             on the world"
        );
        assert_derived_structures_are_consistent(&brickmap, true);

        // Material round-trip through the edited words.
        assert_eq!(
            brickmap.get(500, 0, 500),
            material_id(world.get(500, 0, 500))
        );
    }

    /// A brick materialized in open air and then removed again: the slot must be
    /// recycled (no fragmentation to manage — every slot is the same size), its
    /// words must be zeroed, and the clearance field must stay SAFE even though
    /// the freed brick was isolated, which is exactly the case the bounded box
    /// cannot cover exactly.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn an_isolated_brick_recycles_its_slot_and_leaves_a_safe_clearance_field() {
        let world = VoxelWorld::generate(1234, 0.0);
        let mut brickmap = Brickmap::build(&world);
        let radius_cells = 4;
        let clearance = ClearanceUpdate::LocalBox { radius_cells };
        let before_capacity = brickmap.brick_capacity();
        let before_count = brickmap.occupied_brick_count();

        // High above the island, guaranteed empty air.
        let voxel = [200, 250, 200];
        assert!(!brickmap.is_occupied(voxel[0], voxel[1], voxel[2]));
        let placed = brickmap
            .set_voxel(voxel[0], voxel[1], voxel[2], Voxel::Stone, clearance)
            .expect("placing into air changes something");
        assert!(placed.brick_allocated && !placed.arrays_grew);
        assert_eq!(brickmap.occupied_brick_count(), before_count + 1);
        assert_eq!(brickmap.brick_capacity(), before_capacity);
        assert_eq!(brickmap.metadata().max_occupied_brick_y, 250 / 8);
        assert_derived_structures_are_consistent(&brickmap, true);

        let removed = brickmap
            .set_voxel(voxel[0], voxel[1], voxel[2], Voxel::Air, clearance)
            .expect("removing it changes something");
        assert!(removed.brick_freed);
        assert_eq!(brickmap.free_brick_slot_count(), 1);
        assert_eq!(brickmap.occupied_brick_count(), before_count);
        // Safety only: an isolated removal grows clearance far beyond the box, so
        // the field outside it stays stale-low ON PURPOSE.
        assert_derived_structures_are_consistent(&brickmap, false);
        let stale_cells = {
            let exact = chebyshev_skip_distances(
                &brickmap.brick_indices,
                BRICK_GRID_X,
                BRICK_GRID_Y,
                BRICK_GRID_Z,
            );
            (0..exact.len())
                .filter(|cell| brickmap.skip_distance_at(*cell) < exact[*cell])
                .collect::<Vec<usize>>()
        };
        assert!(
            !stale_cells.is_empty(),
            "an isolated brick removal must leave stale-low cells — otherwise this test \
             proves nothing about the bound"
        );
        // ...and the deficit must respect the documented bound: at most the freed
        // brick's own new clearance D, everywhere, independent of the radius.
        let freed_cell = voxel[0] as usize / BRICK_SIZE
            + (voxel[1] as usize / BRICK_SIZE) * BRICK_GRID_X
            + (voxel[2] as usize / BRICK_SIZE) * BRICK_GRID_X * BRICK_GRID_Y;
        let freed_clearance = brickmap.skip_distance_at(freed_cell);
        let exact = chebyshev_skip_distances(
            &brickmap.brick_indices,
            BRICK_GRID_X,
            BRICK_GRID_Y,
            BRICK_GRID_Z,
        );
        let worst_deficit = stale_cells
            .iter()
            .map(|cell| exact[*cell] - brickmap.skip_distance_at(*cell))
            .max()
            .expect("stale_cells is non-empty");
        assert!(
            worst_deficit <= freed_clearance,
            "deficit {worst_deficit} exceeds the freed brick's own clearance \
             {freed_clearance} over {} stale cells",
            stale_cells.len()
        );

        // The slot comes back, and the recycled words must be clean.
        let replaced = brickmap
            .set_voxel(voxel[0] + 1, voxel[1], voxel[2], Voxel::Sand, clearance)
            .expect("placing again changes something");
        assert!(replaced.brick_allocated && !replaced.arrays_grew);
        assert_eq!(brickmap.free_brick_slot_count(), 0);
        assert_eq!(brickmap.brick_capacity(), before_capacity);
        assert_eq!(
            brickmap.get(voxel[0] + 1, voxel[1], voxel[2]),
            material_id(Voxel::Sand)
        );
        assert!(!brickmap.is_occupied(voxel[0], voxel[1], voxel[2]));
        // The FullRebuild strategy repairs everything the bounded box left stale.
        brickmap.set_voxel(
            voxel[0] + 1,
            voxel[1],
            voxel[2],
            Voxel::Air,
            ClearanceUpdate::FullRebuild,
        );
        assert_derived_structures_are_consistent(&brickmap, true);
    }

    /// THE delta-upload gate: every word an edit changes must be inside one of
    /// the reported dirty ranges, or the GPU copy silently diverges from the CPU
    /// copy. Compares full array snapshots around each edit.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn dirty_ranges_cover_every_changed_word() {
        let world = VoxelWorld::generate(1234, 0.0);
        let mut brickmap = Brickmap::build(&world);
        let clearance = ClearanceUpdate::LocalBox { radius_cells: 6 };

        let surface_y = (0..WORLD_SIZE_Y as i32)
            .rev()
            .find(|y| brickmap.is_occupied(500, *y, 500))
            .expect("island column has occupied voxels");
        let mut edits: Vec<(i32, i32, i32, Voxel)> = vec![
            // Carve into solid ground (no brick flip: the cheap common case).
            (500, surface_y, 500, Voxel::Air),
            (501, surface_y, 500, Voxel::Air),
            // Overwrite a material in place.
            (502, surface_y, 500, Voxel::Snow),
            // Place into open air far away (brick materializes).
            (300, 200, 300, Voxel::Stone),
            (301, 200, 300, Voxel::Stone),
            // ...and empty it again (brick freed, clearance grows).
            (300, 200, 300, Voxel::Air),
            (301, 200, 300, Voxel::Air),
        ];
        // Empty one whole surface brick, so a free + column-max drop is covered.
        let brick_base = [496, surface_y - surface_y % BRICK_SIZE as i32, 496];
        for local_z in 0..BRICK_SIZE as i32 {
            for local_y in 0..BRICK_SIZE as i32 {
                for local_x in 0..BRICK_SIZE as i32 {
                    edits.push((
                        brick_base[0] + local_x,
                        brick_base[1] + local_y,
                        brick_base[2] + local_z,
                        Voxel::Air,
                    ));
                }
            }
        }

        let mut edits_checked = 0_usize;
        let mut bricks_flipped = 0_usize;
        for (x, y, z, voxel) in edits {
            let before: Vec<Vec<u32>> = BrickmapArray::ALL
                .iter()
                .map(|array| brickmap.array_words(*array).to_vec())
                .collect();
            let Some(edit) = brickmap.set_voxel(x, y, z, voxel, clearance) else {
                continue;
            };
            edits_checked += 1;
            if edit.brick_allocated || edit.brick_freed {
                bricks_flipped += 1;
            }
            for (array_index, array) in BrickmapArray::ALL.iter().enumerate() {
                let after = brickmap.array_words(*array);
                assert_eq!(
                    after.len(),
                    before[array_index].len(),
                    "{array:?} changed length without arrays_grew"
                );
                for word in 0..after.len() {
                    if after[word] == before[array_index][word] {
                        continue;
                    }
                    assert!(
                        edit.dirty.iter().any(|range| range.array == *array
                            && word >= range.first_word
                            && word < range.first_word + range.word_count),
                        "edit at ({x}, {y}, {z}) changed {array:?} word {word} but did not \
                         report it dirty"
                    );
                }
            }
        }
        assert!(
            edits_checked > 300 && bricks_flipped >= 3,
            "{edits_checked} edits / {bricks_flipped} brick flips checked — the fixture drifted"
        );
    }

    /// A no-op edit reports nothing: hold-to-repeat aims at the same voxel for
    /// several frames, and the pipeline must not pay for that.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn a_no_op_edit_returns_none() {
        let world = VoxelWorld::generate(1234, 0.0);
        let mut brickmap = Brickmap::build(&world);
        let clearance = ClearanceUpdate::LocalBox { radius_cells: 8 };
        // Air over air, and the material already in place.
        assert!(brickmap
            .set_voxel(200, 250, 200, Voxel::Air, clearance)
            .is_none());
        let surface_y = (0..WORLD_SIZE_Y as i32)
            .rev()
            .find(|y| brickmap.is_occupied(500, *y, 500))
            .expect("island column has occupied voxels");
        let existing = brickmap.get(500, surface_y, 500);
        let same_voxel = (0..crate::material::MATERIAL_COUNT as u8)
            .find(|id| *id == existing)
            .expect("the material id exists");
        assert_eq!(same_voxel, existing);
        assert!(brickmap
            .set_voxel(500, surface_y, 500, voxel_of_material(existing), clearance)
            .is_none());
        // Out of bounds is silently ignored, never a panic.
        for (x, y, z) in [(-1, 0, 0), (0, -1, 0), (WORLD_SIZE_X as i32, 5, 5)] {
            assert!(brickmap
                .set_voxel(x, y, z, Voxel::Stone, clearance)
                .is_none());
        }
    }

    /// Test-only inverse of [`material_id`].
    fn voxel_of_material(material: u8) -> Voxel {
        [
            Voxel::Air,
            Voxel::Grass,
            Voxel::TallGrass,
            Voxel::Dirt,
            Voxel::Sand,
            Voxel::Sediment,
            Voxel::Stone,
            Voxel::Water,
            Voxel::Trunk,
            Voxel::TrunkBirch,
            Voxel::Leaves,
            Voxel::LeavesDark,
            Voxel::LeavesBirch,
            Voxel::LeavesPine,
            Voxel::FlowerPink,
            Voxel::FlowerWhite,
            Voxel::FlowerYellow,
            Voxel::FlowerBlue,
            Voxel::WaterWeed,
            Voxel::LilyPad,
            Voxel::LilyBloom,
            Voxel::Reed,
            Voxel::CattailHead,
            Voxel::Snow,
        ][material as usize]
    }

    /// Range coalescing: adjacent and near-adjacent words merge, distant ones do
    /// not, and arrays never mix.
    #[test]
    fn dirty_ranges_coalesce_within_the_gap_tolerance() {
        let mut ranges = DirtyRanges::default();
        ranges.push(BrickmapArray::OccupancyWords, 10);
        ranges.push(BrickmapArray::OccupancyWords, 11);
        ranges.push(BrickmapArray::OccupancyWords, 11 + DIRTY_RANGE_GAP_WORDS);
        ranges.push(
            BrickmapArray::OccupancyWords,
            200 + 2 * DIRTY_RANGE_GAP_WORDS,
        );
        ranges.push(BrickmapArray::MaterialWords, 5);
        let merged = ranges.finish();
        assert_eq!(
            merged,
            vec![
                DirtyWords {
                    array: BrickmapArray::OccupancyWords,
                    first_word: 10,
                    word_count: 2 + DIRTY_RANGE_GAP_WORDS,
                },
                DirtyWords {
                    array: BrickmapArray::OccupancyWords,
                    first_word: 200 + 2 * DIRTY_RANGE_GAP_WORDS,
                    word_count: 1,
                },
                DirtyWords {
                    array: BrickmapArray::MaterialWords,
                    first_word: 5,
                    word_count: 1,
                },
            ]
        );
        assert_eq!(
            merged.iter().map(DirtyWords::bytes).sum::<usize>(),
            (3 + DIRTY_RANGE_GAP_WORDS) * 4 + 4
        );
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

    /// The uniform collapse must actually fire on the shipped island, and every
    /// surviving level-1 brick must genuinely NOT be uniform — otherwise the
    /// pass is leaving payload on the table (or, worse, has collapsed something
    /// with internal structure).
    #[test]
    fn uniform_bricks_collapse_and_the_survivors_are_all_sculpted() {
        let world = VoxelWorld::generate(1234, 0.0);
        let brickmap = Brickmap::build(&world);

        let mut uniform = 0_usize;
        let mut unique = 0_usize;
        for &pointer in &brickmap.brick_indices {
            if pointer == EMPTY_BRICK {
                continue;
            }
            if brick_is_uniform(pointer) {
                uniform += 1;
                assert_ne!(
                    brick_uniform_material(pointer),
                    0,
                    "a uniform brick of AIR should have stayed EMPTY_BRICK"
                );
            } else {
                assert!(
                    brick_is_unique(pointer),
                    "unexpected tag on {pointer:#010x}"
                );
                unique += 1;
            }
        }
        // Measured at 58.6% of occupied bricks; assert the ORDER, not the exact
        // figure, so terrain tuning does not fail the build.
        assert!(
            uniform > unique / 2,
            "collapse barely fired: {uniform} uniform vs {unique} unique"
        );

        // No survivor may be fully solid AND single-material — that is the exact
        // predicate the collapse claims to have removed.
        for slot in 0..brickmap.allocated_brick_slots as usize {
            let occupancy = &brickmap.occupancy_words
                [slot * OCCUPANCY_WORDS_PER_BRICK..(slot + 1) * OCCUPANCY_WORDS_PER_BRICK];
            if occupancy.iter().any(|word| *word != u32::MAX) {
                continue;
            }
            let materials = &brickmap.material_words
                [slot * MATERIAL_WORDS_PER_BRICK..(slot + 1) * MATERIAL_WORDS_PER_BRICK];
            let first = materials[0];
            let byte = (first & 0xff) as u8;
            let splatted = u32::from_le_bytes([byte; 4]);
            assert!(
                !(first == splatted && materials.iter().all(|word| *word == splatted)),
                "slot {slot} is fully solid and single-material but survived the collapse"
            );
        }
    }

    /// Copy-on-write: editing one voxel of a uniform brick must materialize it
    /// WITHOUT disturbing the other 511. This is the failure mode that a
    /// coordinate round-trip on a freshly built map cannot catch, because the
    /// map under test has to be edited first.
    #[test]
    fn editing_a_uniform_brick_preserves_its_other_voxels() {
        let world = VoxelWorld::generate(1234, 0.0);
        let mut brickmap = Brickmap::build(&world);

        // Find a uniform brick — deep ground, so one is guaranteed nearby.
        let (cell, brick) = brickmap
            .brick_indices
            .iter()
            .enumerate()
            .find(|(_, &pointer)| brick_is_uniform(pointer))
            .map(|(cell, _)| {
                let brick_x = cell % BRICK_GRID_X;
                let brick_y = (cell / BRICK_GRID_X) % BRICK_GRID_Y;
                let brick_z = cell / (BRICK_GRID_X * BRICK_GRID_Y);
                (cell, [brick_x, brick_y, brick_z])
            })
            .expect("the island must contain at least one uniform brick");
        let filling = brick_uniform_material(brickmap.brick_indices[cell]);

        let base = [
            (brick[0] * BRICK_SIZE) as i32,
            (brick[1] * BRICK_SIZE) as i32,
            (brick[2] * BRICK_SIZE) as i32,
        ];
        // Carve one voxel out of the middle, where it touches no brick face.
        let target = [base[0] + 3, base[1] + 4, base[2] + 5];
        let edit = brickmap
            .set_voxel(
                target[0],
                target[1],
                target[2],
                Voxel::Air,
                ClearanceUpdate::FullRebuild,
            )
            .expect("carving a solid voxel must produce an edit");
        assert!(
            edit.brick_allocated,
            "the uniform brick must have materialized"
        );

        assert!(
            brick_is_unique(brickmap.brick_indices[cell]),
            "the edited cell must be UNIQUE afterwards"
        );
        assert_eq!(
            brickmap.get(target[0], target[1], target[2]),
            0,
            "the carve"
        );
        for local_z in 0..BRICK_SIZE as i32 {
            for local_y in 0..BRICK_SIZE as i32 {
                for local_x in 0..BRICK_SIZE as i32 {
                    let at = [base[0] + local_x, base[1] + local_y, base[2] + local_z];
                    if at == target {
                        continue;
                    }
                    assert_eq!(
                        brickmap.get(at[0], at[1], at[2]),
                        filling,
                        "expanding the uniform tag lost the voxel at {at:?}"
                    );
                }
            }
        }
    }

    /// THE safety gate for AADF. Every cell's claimed box — spanned by all six
    /// of its bounds at once — must be entirely empty. A false bound here would
    /// let a ray jump THROUGH geometry, which is the one traversal bug that
    /// cannot be shrugged off as a few steps of lost performance.
    ///
    /// Checked on a synthetic grid rather than the island so the occupancy
    /// pattern is adversarial (isolated cells, walls, thin corridors) and the
    /// whole grid can be verified exhaustively instead of sampled.
    #[test]
    fn directional_bounds_claim_only_empty_boxes() {
        const GRID: usize = 24;
        let mut brick_indices = vec![EMPTY_BRICK; GRID * GRID * GRID];
        let index_of = |x: usize, y: usize, z: usize| x + y * GRID + z * GRID * GRID;

        // A floor, a wall with a gap, a couple of isolated blocks and some
        // scattered noise — the thin-corridor cases are the ones that separate a
        // box claim from an axis-line claim.
        for z in 0..GRID {
            for x in 0..GRID {
                brick_indices[index_of(x, 0, z)] = 0; // floor
                if x == 12 && z != 7 {
                    for y in 0..GRID {
                        brick_indices[index_of(x, y, z)] = 0; // wall, gap at z=7
                    }
                }
            }
        }
        brick_indices[index_of(4, 5, 4)] = 0;
        brick_indices[index_of(18, 9, 3)] = 0;
        for step in 0..GRID {
            brick_indices[index_of(
                (step * 7) % GRID,
                1 + (step * 5) % (GRID - 1),
                (step * 11) % GRID,
            )] = 0;
        }

        let bounds = directional_bounds(&brick_indices, GRID, GRID, GRID);

        let mut boxes_checked = 0_usize;
        let mut widest = 0_u32;
        for z in 0..GRID {
            for y in 0..GRID {
                for x in 0..GRID {
                    let cell = index_of(x, y, z);
                    if brick_indices[cell] != EMPTY_BRICK {
                        assert_eq!(
                            bounds[cell], 0,
                            "occupied cell ({x}, {y}, {z}) claims a box"
                        );
                        continue;
                    }
                    let packed = bounds[cell];
                    let low = [
                        x - bound_of(packed, 0) as usize,
                        y - bound_of(packed, 2) as usize,
                        z - bound_of(packed, 4) as usize,
                    ];
                    let high = [
                        x + bound_of(packed, 1) as usize,
                        y + bound_of(packed, 3) as usize,
                        z + bound_of(packed, 5) as usize,
                    ];
                    widest = widest.max(bound_of(packed, 1));
                    for box_z in low[2]..=high[2] {
                        for box_y in low[1]..=high[1] {
                            for box_x in low[0]..=high[0] {
                                assert_eq!(
                                    brick_indices[index_of(box_x, box_y, box_z)],
                                    EMPTY_BRICK,
                                    "cell ({x}, {y}, {z}) claims a box containing the OCCUPIED \
                                     cell ({box_x}, {box_y}, {box_z}) — bounds {packed:#010x}"
                                );
                            }
                        }
                    }
                    boxes_checked += 1;
                }
            }
        }
        assert!(
            boxes_checked > 10_000,
            "barely checked anything: {boxes_checked}"
        );
        assert!(
            widest > 1,
            "bounds never grew past 1 — the relaxation did nothing"
        );
    }

    /// The point of AADF over chebyshev: a cell in a thin horizontal corridor
    /// must claim a LONG box even though its vertical clearance is nil. This is
    /// the grazing-ray case (low sun shadows, water reflections) and the only
    /// reason to carry a second field at all.
    #[test]
    fn a_thin_corridor_gets_a_long_box_where_chebyshev_gets_one_cell() {
        const GRID: usize = 32;
        let mut brick_indices = vec![EMPTY_BRICK; GRID * GRID * GRID];
        let index_of = |x: usize, y: usize, z: usize| x + y * GRID + z * GRID * GRID;
        // Floor at y=0 and ceiling at y=2: everything at y=1 is a one-cell-tall
        // corridor running the full length of x and z.
        for z in 0..GRID {
            for x in 0..GRID {
                brick_indices[index_of(x, 0, z)] = 0;
                brick_indices[index_of(x, 2, z)] = 0;
            }
        }

        let bounds = directional_bounds(&brick_indices, GRID, GRID, GRID);
        let chebyshev = chebyshev_skip_distances(&brick_indices, GRID, GRID, GRID);

        let cell = index_of(GRID / 2, 1, GRID / 2);
        let packed = bounds[cell];
        assert_eq!(bound_of(packed, 2), 0, "the floor is one cell below");
        assert_eq!(bound_of(packed, 3), 0, "the ceiling is one cell above");
        // The corridor runs to the grid edge, which bounds the box before
        // BOUND_MAX does: from x = GRID/2 there are GRID/2 - 1 cells left.
        let corridor_run = (GRID / 2 - 1) as u32;
        assert!(
            corridor_run < BOUND_MAX,
            "pick a grid that the bound can span"
        );
        assert_eq!(
            bound_of(packed, 1),
            corridor_run,
            "+x should run the whole corridor to the grid edge"
        );
        // Chebyshev cannot express this: its cube is capped by the floor.
        assert_eq!(
            chebyshev[cell], 1,
            "chebyshev should see only a 1-cell cube here"
        );
    }

    /// AADF bounds must never survive an edit that ADDS geometry. A stale-high
    /// bound is the one traversal bug that tunnels a ray through solid matter,
    /// and the field has no incremental repair — so the invariant this test pins
    /// is "after such an edit the field is flat, and the flattening was
    /// published to the GPU".
    #[test]
    fn adding_a_brick_flattens_the_directional_bounds() {
        let world = VoxelWorld::generate(1234, 0.0);
        let mut brickmap = Brickmap::build(&world);
        assert!(
            brickmap.bounds_are_valid(),
            "a freshly built map has valid bounds"
        );
        assert!(
            brickmap.brick_bound_words.iter().any(|&word| word != 0),
            "the built field should describe some free space"
        );

        // Find an EMPTY brick with an occupied neighbour below, so placing one
        // voxel makes a previously empty cell occupied.
        let target = (0..BRICK_GRID_Z)
            .flat_map(|z| (0..BRICK_GRID_Y).map(move |y| (y, z)))
            .flat_map(|(y, z)| (0..BRICK_GRID_X).map(move |x| (x, y, z)))
            .find(|&(x, y, z)| {
                let cell = x + y * BRICK_GRID_X + z * BRICK_GRID_X * BRICK_GRID_Y;
                brickmap.brick_indices[cell] == EMPTY_BRICK
            })
            .expect("the island must contain an empty brick");

        let edit = brickmap
            .set_voxel(
                (target.0 * BRICK_SIZE) as i32,
                (target.1 * BRICK_SIZE) as i32,
                (target.2 * BRICK_SIZE) as i32,
                Voxel::Stone,
                ClearanceUpdate::FullRebuild,
            )
            .expect("placing into empty space must produce an edit");
        assert!(
            edit.brick_allocated,
            "the empty brick must have materialized"
        );

        assert!(
            !brickmap.bounds_are_valid(),
            "bounds must be marked invalid"
        );
        assert!(
            brickmap.brick_bound_words.iter().all(|&word| word == 0),
            "a surviving non-zero bound could tunnel a ray through the new brick"
        );
        assert!(
            edit.dirty
                .iter()
                .any(|dirty| dirty.array == BrickmapArray::BrickBounds
                    && dirty.first_word == 0
                    && dirty.word_count == brickmap.brick_bound_words.len()),
            "the flattening must be published to the GPU, or only the CPU copy is safe"
        );

        // Flattening happens at most once: a second edit must not re-publish 2 MB.
        let second = brickmap.set_voxel(
            (target.0 * BRICK_SIZE) as i32 + 1,
            (target.1 * BRICK_SIZE) as i32,
            (target.2 * BRICK_SIZE) as i32,
            Voxel::Stone,
            ClearanceUpdate::FullRebuild,
        );
        if let Some(second) = second {
            assert!(
                !second
                    .dirty
                    .iter()
                    .any(|dirty| dirty.array == BrickmapArray::BrickBounds),
                "the bound field was already flat; re-uploading it is pure waste"
            );
        }
    }
}
