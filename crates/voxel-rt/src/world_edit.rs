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

use crate::brickmap::{Brickmap, BrickmapArray, ClearanceUpdate};
use crate::cagi::{CagiGrid, LightCellUpdate, MaterialAttributes};
use voxel_core::world::{WorldVoxelCoord, DETAIL_CELLS_PER_WORLD_VOXEL};

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

/// One requested change in authoritative one-metre world-voxel coordinates.
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
    /// The material table's CAGI attributes, so an edit's light-cell update describes
    /// the LIVE table rather than the compiled one (S2).
    ///
    /// The reduced 416-byte `Copy` form rather than the rows themselves, precisely so
    /// it can ride in this struct across the thread boundary — see
    /// [`crate::cagi::MaterialAttributes`]. Every edit already carries `light_grid` for
    /// the same reason: the world thread cannot reach the renderer's state, so anything
    /// it needs travels with the request.
    pub material_attributes: MaterialAttributes,
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
    /// readout).
    pub voxel: [i32; 3],
    pub material: u8,
    /// How many one-metre world voxels this delta covers.
    pub voxels_written: usize,
    /// Word payloads for the brickmap buffers.
    pub writes: Vec<ArrayWrite>,
    /// `(cell index, attribute word)` for the CAGI cells the edit changed, and the
    /// grid those indices are for — the uploader drops them if the light volume has
    /// been reallocated at another resolution since (a lever moved while the edit
    /// was in flight), because a full attribute rebuild is already on its way.
    pub light_cells: Vec<LightCellUpdate>,
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
            // What the GPU buffer actually holds per cell, not the CPU-side
            // `LightCellUpdate`: the emission is packed 10:10:10 into one word on
            // the way out, so an f32-triple price here would overstate an edit by
            // 2.5x — and this number IS the E2 verdict.
            + self.light_cells.len() * crate::cagi::CELL_DATA_BYTES
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
    let coordinate = WorldVoxelCoord::new(edit.voxel[0], edit.voxel[1], edit.voxel[2]);
    let applied = brickmap.set_world_voxel(coordinate, edit.material, settings.clearance())?;
    let detail_origin = coordinate.detail_origin();
    let detail_max = detail_origin.map(|value| value + DETAIL_CELLS_PER_WORLD_VOXEL as i32 - 1);
    let light_cells = light_cells_in_voxel_box(
        brickmap,
        edit.light_grid,
        detail_origin,
        detail_max,
        &edit.material_attributes,
    );
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
        voxel: edit.voxel,
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

/// Every CAGI cell overlapping an inclusive detail-cell box, with its attribute
/// recomputed from the finished brickmap.
///
/// Recomputes the touched cell box plus the six face-adjacent cell slabs. A
/// diagonal cell cannot share a voxel face with the edit, so it cannot have its
/// exposed area changed and is intentionally excluded.
fn light_cells_in_voxel_box(
    brickmap: &Brickmap,
    light_grid: Option<CagiGrid>,
    lowest_voxel: [i32; 3],
    highest_voxel: [i32; 3],
    attribute_table: &MaterialAttributes,
) -> Vec<LightCellUpdate> {
    let Some(grid) = light_grid else {
        return Vec::new();
    };
    // Clamping `low` as well as `high` would turn a box that sits ENTIRELY above
    // the volume's clamped height into the topmost cell slab — correct values, but
    // a whole slab plus its six neighbour slabs recomputed for an edit that
    // changed nothing in the volume. Drop those boxes instead.
    let mut cell_bounds = Vec::with_capacity(3);
    for axis in 0..3 {
        let low = lowest_voxel[axis].max(0) as u32 / grid.cell_voxels;
        if low >= grid.size[axis] {
            return Vec::new();
        }
        let high = (highest_voxel[axis].max(0) as u32 / grid.cell_voxels)
            .min(grid.size[axis].saturating_sub(1));
        cell_bounds.push((low, high));
    }
    let mut cells = Vec::new();
    for z in cell_bounds[2].0..=cell_bounds[2].1 {
        for y in cell_bounds[1].0..=cell_bounds[1].1 {
            for x in cell_bounds[0].0..=cell_bounds[0].1 {
                cells.push([x, y, z]);
            }
        }
    }
    // Exposure can also change in the six cell slabs immediately outside the
    // edited box. Diagonal cells cannot share a voxel face with the edit.
    for axis in 0..3 {
        for side in [
            cell_bounds[axis].0.checked_sub(1),
            cell_bounds[axis].1.checked_add(1),
        ] {
            let Some(side) = side.filter(|value| *value < grid.size[axis]) else {
                continue;
            };
            for z in cell_bounds[2].0..=cell_bounds[2].1 {
                for y in cell_bounds[1].0..=cell_bounds[1].1 {
                    for x in cell_bounds[0].0..=cell_bounds[0].1 {
                        let mut cell = [x, y, z];
                        cell[axis] = side;
                        cells.push(cell);
                    }
                }
            }
        }
    }
    cells.sort_unstable();
    cells.dedup();
    cells
        .into_iter()
        .map(|cell| crate::cagi::cell_attribute(brickmap, &grid, cell, attribute_table))
        .collect()
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

        let world_x = 500 / DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        let world_y = surface_y / DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        let world_z = 500 / DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        // The common case: remove one whole one-metre ground voxel.
        let delta = apply(
            &mut brickmap,
            &VoxelEdit {
                voxel: [world_x, world_y, world_z],
                material: Voxel::Air,
                light_grid: Some(grid),
                material_attributes: MaterialAttributes::compiled(),
            },
            &settings,
        )
        .expect("carving solid ground changes something");
        assert_eq!(delta.material, 0);
        assert!(!delta.arrays_grew);
        assert!(
            delta.clearance_cells_written > 0,
            "removing a uniform world brick must repair clearance"
        );
        assert!(!delta.light_cells.is_empty());
        for write in &delta.writes {
            let words = brickmap.array_words(write.array);
            assert_eq!(
                &words[write.first_word..write.first_word + write.words.len()],
                write.words.as_slice(),
                "{:?} payload does not match the brickmap",
                write.array
            );
        }
        // A 1 m edit overlaps multiple CAGI cells, but remains a small patch.
        assert!(
            delta.upload_bytes() <= 1024,
            "a carve inside an existing brick uploaded {} bytes",
            delta.upload_bytes()
        );

        // Re-applying the same edit is a no-op.
        assert!(apply(
            &mut brickmap,
            &VoxelEdit {
                voxel: [world_x, world_y, world_z],
                material: Voxel::Air,
                light_grid: Some(grid),
                material_attributes: MaterialAttributes::compiled(),
            },
            &settings,
        )
        .is_none());

        // Placing into open air materializes a brick: bigger delta, metadata moves.
        let delta = apply(
            &mut brickmap,
            &VoxelEdit {
                voxel: [25, 31, 25],
                material: Voxel::Stone,
                light_grid: Some(grid),
                material_attributes: MaterialAttributes::compiled(),
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
        let world_y = surface_y / DETAIL_CELLS_PER_WORLD_VOXEL as i32;
        let mut deltas = std::collections::HashMap::new();
        for offset in 0..6 {
            if let Some(delta) = apply(
                &mut brickmap,
                &VoxelEdit {
                    voxel: [60 + offset, world_y, 62],
                    material: Voxel::Air,
                    light_grid: Some(grid),
                    material_attributes: MaterialAttributes::compiled(),
                },
                &settings,
            ) {
                for update in delta.light_cells {
                    // Later adjacent edits legitimately supersede an earlier
                    // attribute for the same CAGI cell.
                    deltas.insert(update.index, (update.attribute, update.emission));
                }
            }
        }
        assert!(!deltas.is_empty());
        let (rebuilt, rebuilt_emissions) = crate::cagi::build_cell_attributes_with_emission(
            &brickmap,
            &grid,
            &MaterialAttributes::compiled(),
        );
        for (cell_index, (attribute, emission)) in deltas {
            assert_eq!(
                attribute, rebuilt[cell_index],
                "cell {cell_index}'s incremental attribute drifted from the rebuild"
            );
            assert_eq!(
                &emission, &rebuilt_emissions[cell_index],
                "cell {cell_index}'s incremental emission drifted from the rebuild"
            );
        }
    }
}
