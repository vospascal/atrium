//! E2 — the world-edit pipeline's pure-data half: the levers, the edit request,
//! and the GPU delta an applied edit produces. No wgpu, no winit, no threads
//! (those live in [`crate::world_host`] and [`crate::passes::world_bindings`]).
//!
//! THE SHAPE OF THE PIPELINE (the E2 verdict, in code): input produces a
//! [`VoxelEdit`]; the world authority applies it to the CPU [`Brickmap`]
//! ([`Brickmap::set_voxel`]) and turns the resulting [`BrickmapEdit`] into a
//! self-contained [`WorldDelta`] — OWNED word payloads plus the touched CAGI cell
//! attributes — which the render thread drains and writes into the GPU buffers.
//!
//! Owned payloads, deliberately: they are the seam that lets the world authority
//! sit on another thread without the render thread ever locking the brickmap. The
//! whole delta of a typical edit is 576 bytes, so copying it is cheaper than the
//! synchronization that avoiding the copy would need.

use crate::brickmap::{Brickmap, BrickmapArray, BrickmapEdit, ClearanceUpdate};
use crate::cagi::{cell_attribute, CagiGrid};

/// How a removal that empties a brick repairs the chebyshev clearance field —
/// the registry-facing mirror of [`ClearanceUpdate`] (which carries the radius
/// inline).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ClearanceUpdateMode {
    /// Bounded local recompute (shipped).
    LocalBox,
    /// Rebuild the whole distance transform.
    FullRebuild,
}

impl ClearanceUpdateMode {
    pub fn shader_value(self) -> u32 {
        match self {
            ClearanceUpdateMode::LocalBox => 0,
            ClearanceUpdateMode::FullRebuild => 1,
        }
    }

    pub fn from_shader_value(shader_value: u32) -> ClearanceUpdateMode {
        match shader_value {
            0 => ClearanceUpdateMode::LocalBox,
            1 => ClearanceUpdateMode::FullRebuild,
            other => panic!("no clearance update mode {other}"),
        }
    }
}

/// The E2 levers. All RUNTIME (no shader const anywhere in this experiment — an
/// edit changes buffer CONTENTS, never the shader), so every one of them can be
/// flipped mid-session with no pipeline rebuild.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WorldEditSettings {
    /// Apply edits on the world thread (variant B) instead of inline on the
    /// frame's thread (variant A).
    pub world_thread: bool,
    /// Clearance-field repair strategy for brick removals.
    pub clearance_update: ClearanceUpdateMode,
    /// Half-width, in bricks, of [`ClearanceUpdateMode::LocalBox`]'s recompute.
    pub clearance_radius_cells: u32,
    /// Re-flood the CAGI light volume after an edit (E5 replaces this with a
    /// dirty-region flood).
    pub gi_reflood: bool,
}

impl Default for WorldEditSettings {
    /// The shipped configuration — E2's measured winner.
    fn default() -> WorldEditSettings {
        WorldEditSettings {
            world_thread: true,
            clearance_update: ClearanceUpdateMode::LocalBox,
            clearance_radius_cells: 8,
            gi_reflood: true,
        }
    }
}

impl WorldEditSettings {
    /// The brickmap-level clearance strategy these settings mean.
    pub fn clearance(&self) -> ClearanceUpdate {
        match self.clearance_update {
            ClearanceUpdateMode::LocalBox => ClearanceUpdate::LocalBox {
                radius_cells: self.clearance_radius_cells,
            },
            ClearanceUpdateMode::FullRebuild => ClearanceUpdate::FullRebuild,
        }
    }
}

/// One requested voxel change, in world-voxel coordinates.
///
/// `light_grid` is the CAGI volume's current geometry, handed in by the caller
/// because the volume's resolution is a lever: the authority recomputes the
/// touched cell's static attribute (albedo + solid bit) while it holds the
/// brickmap, which is the only place that data can be derived cheaply. `None`
/// when the light volume is off.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxelEdit {
    pub voxel: [i32; 3],
    pub material: voxel_core::world::Voxel,
    pub light_grid: Option<CagiGrid>,
}

/// A run of voxels along Y in one column, inclusive at both ends — the unit a
/// BULK edit is described in.
///
/// Spans, not voxels: the world is column-structured (its generator, its
/// brickmap build and its height caches all are), so any large edit is a handful
/// of runs per column. Describing a 400 000-voxel pool as ~25 000 spans is what
/// keeps the REQUEST small enough to build and send without the frame noticing.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct VoxelSpan {
    pub x: i32,
    pub z: i32,
    pub y_from: i32,
    pub y_to: i32,
    pub material: voxel_core::world::Voxel,
}

/// A change of many voxels that the authority expands ITSELF.
///
/// The seam: the requester describes the change in a few bytes, the world thread
/// turns it into spans against the authoritative brickmap (which is the only
/// place the terrain can be read while it is being written) and publishes ONE
/// coalesced [`WorldDelta`]. So the frame thread pays for a `Box`, not for
/// hundreds of thousands of voxels — E2's whole point, applied to bulk work.
///
/// Today's implementor is [`crate::debug_pool::WaterPool`]; E3's generation, B6's
/// falling-sand fluids and B8's streaming are the same shape.
pub trait BulkEdit: Send {
    /// The spans to write, in application order. Called on the world thread with
    /// the authority's brickmap, before anything has been written.
    fn spans(&self, brickmap: &Brickmap) -> Vec<VoxelSpan>;
    /// What this edit is, for the log line and the overlay readout.
    fn label(&self) -> &'static str;
}

/// A bulk edit request: the shape, plus the light volume's current geometry (the
/// same reason [`VoxelEdit`] carries it).
pub struct BulkEditRequest {
    pub shape: Box<dyn BulkEdit>,
    pub light_grid: Option<CagiGrid>,
}

/// A contiguous word payload for one of the brickmap's GPU buffers.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ArrayWrite {
    pub array: BrickmapArray,
    pub first_word: usize,
    pub words: Vec<u32>,
}

impl ArrayWrite {
    pub fn bytes(&self) -> usize {
        self.words.len() * 4
    }
}

/// Everything the render thread must upload for one applied edit — owned, so it
/// can cross a channel and be applied without touching the brickmap.
#[derive(Clone, Debug, PartialEq)]
pub struct WorldDelta {
    /// The voxel this delta is reported at, and what it became (for the overlay
    /// readout). A bulk delta reports its FIRST written voxel.
    pub voxel: [i32; 3],
    pub material: u8,
    /// How many voxels this delta covers: 1 for a single edit, N for a bulk one,
    /// whose word ranges are coalesced across all of them.
    pub voxels_written: usize,
    /// Word payloads for the brickmap buffers.
    pub writes: Vec<ArrayWrite>,
    /// `(cell index, attribute word)` for the CAGI cells the edit changed, and the
    /// grid those indices are for — the uploader drops them if the light volume has
    /// been reallocated at another resolution since (a lever moved while the edit
    /// was in flight), because a full attribute rebuild is already on its way.
    pub light_cells: Vec<(usize, u32)>,
    pub light_grid: Option<CagiGrid>,
    /// The metadata uniform changed (brick count and/or global max brick Y).
    pub metadata: Option<crate::brickmap::BrickmapMetadata>,
    /// The level-1 arrays outgrew their headroom: the buffers must be
    /// reallocated from the brickmap, not patched.
    pub arrays_grew: bool,
    /// Brick cells whose clearance byte was rewritten (bench diagnostic).
    pub clearance_cells_written: usize,
    /// How long applying the edit to the CPU brickmap took.
    pub apply_micros: f32,
}

impl WorldDelta {
    /// Bytes the uploader moves for this edit (the "upload bytes per edit" number
    /// of the E2 verdict).
    pub fn upload_bytes(&self) -> usize {
        self.writes.iter().map(ArrayWrite::bytes).sum::<usize>()
            + self.light_cells.len() * 4
            + if self.metadata.is_some() {
                std::mem::size_of::<crate::brickmap::BrickmapMetadata>()
            } else {
                0
            }
    }
}

/// Apply one edit to `brickmap` and package the GPU delta. The whole
/// CPU-authoritative pipeline in one function, so variant A (inline) and variant
/// B (world thread) run *exactly* the same code and the only measured difference
/// is which thread it runs on.
pub fn apply(
    brickmap: &mut Brickmap,
    edit: &VoxelEdit,
    settings: &WorldEditSettings,
) -> Option<WorldDelta> {
    let started = std::time::Instant::now();
    let applied = brickmap.set_voxel(
        edit.voxel[0],
        edit.voxel[1],
        edit.voxel[2],
        edit.material,
        settings.clearance(),
    )?;
    let light_cells = light_cell_updates(brickmap, edit, &applied);
    let apply_micros = started.elapsed().as_secs_f32() * 1e6;
    let writes = applied
        .dirty
        .iter()
        .map(|range| ArrayWrite {
            array: range.array,
            first_word: range.first_word,
            words: brickmap.array_words(range.array)
                [range.first_word..range.first_word + range.word_count]
                .to_vec(),
        })
        .collect();
    Some(WorldDelta {
        voxel: applied.voxel,
        material: applied.material,
        voxels_written: 1,
        writes,
        light_cells,
        light_grid: edit.light_grid,
        metadata: applied.metadata_changed.then(|| brickmap.metadata()),
        arrays_grew: applied.arrays_grew,
        clearance_cells_written: applied.clearance_cells_written,
        apply_micros,
    })
}

/// Apply a whole SHAPE to `brickmap` and package it as ONE delta.
///
/// Same per-voxel machinery as [`apply`] — [`Brickmap::set_voxel`], so every
/// derived structure (occupancy, materials, brick allocation, the chebyshev
/// clearance field, the height caches) is repaired exactly as a click would
/// repair it — with three differences that only a bulk edit needs:
///
/// 1. **the dirty ranges are coalesced across every voxel**
///    ([`crate::brickmap::coalesce_dirty_words`]): the 320 000-voxel pool becomes
///    a few hundred `write_buffer` calls instead of a million;
/// 2. **the touched CAGI cells are recomputed once, over the edit's bounding
///    box**, after all writes — recomputing per voxel would redo the same cell
///    dozens of times, and the box is a superset of what changed, which is what
///    correctness needs;
/// 3. **no-ops are free**: a span may cross voxels that already hold the target
///    material (the pool's rim does, by construction), and those cost one `get`.
///
/// `None` when the shape changed nothing.
pub fn apply_bulk(
    brickmap: &mut Brickmap,
    request: &BulkEditRequest,
    settings: &WorldEditSettings,
) -> Option<WorldDelta> {
    let started = std::time::Instant::now();
    let spans = request.shape.spans(brickmap);
    let clearance = settings.clearance();
    let mut dirty = Vec::new();
    let mut first_written: Option<([i32; 3], u8)> = None;
    let mut voxels_written = 0_usize;
    let mut lowest = [i32::MAX; 3];
    let mut highest = [i32::MIN; 3];
    let mut metadata_changed = false;
    let mut arrays_grew = false;
    let mut clearance_cells_written = 0_usize;
    for span in &spans {
        for y in span.y_from..=span.y_to {
            let Some(applied) = brickmap.set_voxel(span.x, y, span.z, span.material, clearance)
            else {
                continue;
            };
            voxels_written += 1;
            first_written = first_written.or(Some((applied.voxel, applied.material)));
            for axis in 0..3 {
                lowest[axis] = lowest[axis].min(applied.voxel[axis]);
                highest[axis] = highest[axis].max(applied.voxel[axis]);
            }
            metadata_changed |= applied.metadata_changed;
            arrays_grew |= applied.arrays_grew;
            clearance_cells_written += applied.clearance_cells_written;
            dirty.extend(applied.dirty);
        }
    }
    let (voxel, material) = first_written?;
    let light_cells = light_cells_in_voxel_box(brickmap, request.light_grid, lowest, highest);
    let apply_micros = started.elapsed().as_secs_f32() * 1e6;
    let writes = crate::brickmap::coalesce_dirty_words(dirty)
        .into_iter()
        .map(|range| ArrayWrite {
            array: range.array,
            first_word: range.first_word,
            words: brickmap.array_words(range.array)
                [range.first_word..range.first_word + range.word_count]
                .to_vec(),
        })
        .collect();
    Some(WorldDelta {
        voxel,
        material,
        voxels_written,
        writes,
        light_cells,
        light_grid: request.light_grid,
        metadata: metadata_changed.then(|| brickmap.metadata()),
        arrays_grew,
        clearance_cells_written,
        apply_micros,
    })
}

/// The CAGI cells an edit invalidates. A cell never straddles two bricks and its
/// attribute is a function of its OWN voxels only (highest occupied voxel's
/// albedo, plus the quarter-fill solid bit), so exactly one cell changes per
/// edited voxel — the E4 attribute build's 48 ms collapses to one cell's worth of
/// work.
fn light_cell_updates(
    brickmap: &Brickmap,
    edit: &VoxelEdit,
    applied: &BrickmapEdit,
) -> Vec<(usize, u32)> {
    let Some(grid) = edit.light_grid else {
        return Vec::new();
    };
    let cell = [
        applied.voxel[0] as u32 / grid.cell_voxels,
        applied.voxel[1] as u32 / grid.cell_voxels,
        applied.voxel[2] as u32 / grid.cell_voxels,
    ];
    if (0..3).any(|axis| cell[axis] >= grid.size[axis]) {
        return Vec::new(); // above the volume's clamped height: nothing to update
    }
    vec![(grid.cell_index(cell), cell_attribute(brickmap, &grid, cell))]
}

/// Every CAGI cell overlapping an inclusive voxel box, with its attribute
/// recomputed from the finished brickmap — the bulk path's counterpart to
/// [`light_cell_updates`].
///
/// Deliberately the cell box rather than the exact set of touched cells: a
/// superset is correct (a cell whose voxels did not change recomputes to the same
/// attribute) and it costs one pass instead of deduplicating hundreds of
/// thousands of cell indices.
fn light_cells_in_voxel_box(
    brickmap: &Brickmap,
    light_grid: Option<CagiGrid>,
    lowest_voxel: [i32; 3],
    highest_voxel: [i32; 3],
) -> Vec<(usize, u32)> {
    let Some(grid) = light_grid else {
        return Vec::new();
    };
    let cell_bounds: Vec<(u32, u32)> = (0..3)
        .map(|axis| {
            let low = lowest_voxel[axis].max(0) as u32 / grid.cell_voxels;
            let high = highest_voxel[axis].max(0) as u32 / grid.cell_voxels;
            (low.min(grid.size[axis]), (high + 1).min(grid.size[axis]))
        })
        .collect();
    let mut cells = Vec::new();
    for z in cell_bounds[2].0..cell_bounds[2].1 {
        for y in cell_bounds[1].0..cell_bounds[1].1 {
            for x in cell_bounds[0].0..cell_bounds[0].1 {
                let cell = [x, y, z];
                cells.push((grid.cell_index(cell), cell_attribute(brickmap, &grid, cell)));
            }
        }
    }
    cells
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::material_id;
    use voxel_core::world::{Voxel, VoxelWorld, WORLD_SIZE_Y};

    fn island() -> Brickmap {
        Brickmap::build(&VoxelWorld::generate(1234, 0.0))
    }

    #[test]
    fn settings_map_onto_the_brickmap_strategy() {
        let local = WorldEditSettings::default();
        assert_eq!(
            local.clearance(),
            ClearanceUpdate::LocalBox { radius_cells: 8 }
        );
        let rebuild = WorldEditSettings {
            clearance_update: ClearanceUpdateMode::FullRebuild,
            ..local
        };
        assert_eq!(rebuild.clearance(), ClearanceUpdate::FullRebuild);
        assert_eq!(
            ClearanceUpdateMode::from_shader_value(local.clearance_update.shader_value()),
            ClearanceUpdateMode::LocalBox
        );
    }

    /// The delta must be self-contained and correct: its word payloads must equal
    /// the brickmap's own words after the edit (that is what makes an off-thread
    /// upload possible), and its byte count must be the small number E2 claims.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn a_delta_carries_exactly_the_edited_words() {
        let mut brickmap = island();
        let settings = WorldEditSettings::default();
        let grid = CagiGrid::for_world(4, brickmap.metadata().max_occupied_brick_y);
        let surface_y = (0..WORLD_SIZE_Y as i32)
            .rev()
            .find(|y| brickmap.is_occupied(500, *y, 500))
            .expect("occupied column");

        // The common case: carve one voxel out of solid ground.
        let delta = apply(
            &mut brickmap,
            &VoxelEdit {
                voxel: [500, surface_y, 500],
                material: Voxel::Air,
                light_grid: Some(grid),
            },
            &settings,
        )
        .expect("carving solid ground changes something");
        assert_eq!(delta.material, 0);
        assert!(!delta.arrays_grew);
        assert_eq!(delta.clearance_cells_written, 0, "no brick flipped");
        assert_eq!(delta.light_cells.len(), 1);
        for write in &delta.writes {
            let words = brickmap.array_words(write.array);
            assert_eq!(
                &words[write.first_word..write.first_word + write.words.len()],
                write.words.as_slice(),
                "{:?} payload does not match the brickmap",
                write.array
            );
        }
        assert!(
            delta.upload_bytes() <= 64,
            "a carve inside an existing brick uploaded {} bytes",
            delta.upload_bytes()
        );

        // Re-applying the same edit is a no-op.
        assert!(apply(
            &mut brickmap,
            &VoxelEdit {
                voxel: [500, surface_y, 500],
                material: Voxel::Air,
                light_grid: Some(grid),
            },
            &settings,
        )
        .is_none());

        // Placing into open air materializes a brick: bigger delta, metadata moves.
        let delta = apply(
            &mut brickmap,
            &VoxelEdit {
                voxel: [200, 250, 200],
                material: Voxel::Stone,
                light_grid: Some(grid),
            },
            &settings,
        )
        .expect("placing into air changes something");
        assert!(delta.metadata.is_some());
        assert!(delta.clearance_cells_written > 0);
        assert_eq!(delta.material, material_id(Voxel::Stone));
    }

    /// The touched CAGI cell attribute must equal what a full rebuild would have
    /// produced for that cell — the delta path and E4's build path cannot drift.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn light_cell_deltas_match_a_full_attribute_rebuild() {
        let mut brickmap = island();
        let settings = WorldEditSettings::default();
        let grid = CagiGrid::for_world(4, brickmap.metadata().max_occupied_brick_y);
        let surface_y = (0..WORLD_SIZE_Y as i32)
            .rev()
            .find(|y| brickmap.is_occupied(500, *y, 500))
            .expect("occupied column");
        let mut deltas = Vec::new();
        for offset in 0..6 {
            if let Some(delta) = apply(
                &mut brickmap,
                &VoxelEdit {
                    voxel: [500 + offset, surface_y, 500],
                    material: Voxel::Air,
                    light_grid: Some(grid),
                },
                &settings,
            ) {
                deltas.extend(delta.light_cells);
            }
        }
        assert!(!deltas.is_empty());
        let rebuilt = crate::cagi::build_cell_attributes(&brickmap, &grid);
        for (cell_index, attribute) in deltas {
            assert_eq!(
                attribute, rebuilt[cell_index],
                "cell {cell_index}'s incremental attribute drifted from the rebuild"
            );
        }
    }
}
