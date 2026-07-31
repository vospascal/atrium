//! S0 — the LIVE material table: the rows the renderer is currently using, as
//! opposed to [`crate::material::MATERIALS`], which is what the binary was
//! compiled with.
//!
//! Why this exists at all. Before S0 the table reached the GPU exactly once, in
//! `WorldBindings::new`, through a buffer created without `COPY_DST` — so a live
//! material edit was not merely unimplemented, it was *impossible*. Every one of
//! the 26 rows had to be tuned by editing Rust and rebuilding, which is why half
//! the columns were authored blind (roughness is a uniform `0.60` across every
//! solid, a value written when nothing read it). Every later stage of this arc —
//! face roles, pattern layers, animation, the re-authored roughness column — is
//! judged by eye, so the cost of *not* having this is paid once per tweak,
//! forever.
//!
//! ## What it is, deliberately
//!
//! A `Vec<Material>` and a dirty flag. That is the whole design:
//!
//! * **The rows are the authored union**, not the flat GPU form, so the panel
//!   edits what a human wrote and [`Material::to_gpu`] stays the one place the
//!   union is expanded.
//! * **Dirty is a single flag, not a per-row set.** The table is
//!   [`crate::material::MATERIAL_TABLE_BYTES`] = 2080 bytes; tracking which rows
//!   changed would cost more code than the write it saves, and a `write_buffer`
//!   of 2 KB is not measurable against a frame.
//! * **`Default` is the compiled table**, which is what makes "reset this row"
//!   and "reset everything" free rather than a second copy of the data.
//!
//! ## What it deliberately does NOT do
//!
//! **It does not let the panel change a row's [`MaterialKind`].** Kind is
//! structural, not cosmetic: it decides [`MaterialFlags`], and through them the
//! character's movement predicate, the editor's notion of emptiness, and whether
//! traversal continues through the voxel. Those CPU predicates read the compiled
//! [`crate::material::MATERIALS`] — deliberately, because they are sampled per
//! frame and must not depend on renderer state — so a live kind change would
//! desync the physics from the picture. Values *within* a kind are safe and are
//! what tuning actually needs. If kind-switching is ever wanted it has to come
//! with a story for those predicates, not just a combo box.

use crate::material::{GpuMaterial, Material, MATERIALS, MATERIAL_COUNT};

/// The live material table plus its upload gate.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialTable {
    rows: Vec<Material>,
    dirty: bool,
}

impl Default for MaterialTable {
    /// The compiled table. Starts CLEAN: `WorldBindings::new` uploads the same
    /// defaults, so there is nothing to re-send until something is edited.
    fn default() -> Self {
        Self {
            rows: MATERIALS.to_vec(),
            dirty: false,
        }
    }
}

impl MaterialTable {
    /// Every row, in material-id order.
    pub fn rows(&self) -> &[Material] {
        &self.rows
    }

    /// One row by material id, or `None` past the table.
    pub fn row(&self, material: u8) -> Option<&Material> {
        self.rows.get(material as usize)
    }

    /// Mutable access to one row, marking the table for re-upload.
    ///
    /// Marks dirty unconditionally rather than comparing before and after: the
    /// caller is an egui slider being dragged, so the common case really is a
    /// change, and a 2 KB upload is cheaper than being clever about it. Returns
    /// `None` past the table rather than panicking, because the id can come from a
    /// picked voxel.
    pub fn row_mut(&mut self, material: u8) -> Option<&mut Material> {
        let row = self.rows.get_mut(material as usize)?;
        self.dirty = true;
        Some(row)
    }

    /// Restore one row to what the binary was compiled with.
    pub fn reset_row(&mut self, material: u8) {
        let Some(default) = MATERIALS.get(material as usize) else {
            return;
        };
        if self.rows[material as usize] != *default {
            self.rows[material as usize] = *default;
            self.dirty = true;
        }
    }

    /// Restore the whole table to the compiled defaults.
    pub fn reset_all(&mut self) {
        if self.rows != MATERIALS {
            self.rows = MATERIALS.to_vec();
            self.dirty = true;
        }
    }

    /// Whether one row differs from the compiled default — what the panel shows
    /// to distinguish "I tuned this" from "this is as shipped".
    pub fn row_is_modified(&self, material: u8) -> bool {
        match (
            self.rows.get(material as usize),
            MATERIALS.get(material as usize),
        ) {
            (Some(row), Some(default)) => row != default,
            _ => false,
        }
    }

    /// Whether any row differs from the compiled defaults.
    pub fn is_modified(&self) -> bool {
        self.rows != MATERIALS
    }

    /// Take the pending upload, if there is one: `Some(rows)` exactly once after
    /// any edit, `None` on every other frame.
    ///
    /// Consuming rather than a plain `is_dirty()` so the frame composer cannot
    /// forget to clear the flag and re-upload the same 2 KB every frame forever.
    pub fn take_dirty(&mut self) -> Option<Vec<GpuMaterial>> {
        if !std::mem::take(&mut self.dirty) {
            return None;
        }
        Some(self.gpu_rows())
    }

    /// The table in upload form. Always [`MATERIAL_COUNT`] rows — the shader
    /// indexes binding 5 by material id, so a short write would leave stale rows.
    pub fn gpu_rows(&self) -> Vec<GpuMaterial> {
        debug_assert_eq!(self.rows.len(), MATERIAL_COUNT);
        self.rows.iter().map(Material::to_gpu).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::{material_id, MaterialKind};
    use voxel_core::world::Voxel;

    /// The default table must be the compiled one AND must not ask for an upload:
    /// `WorldBindings::new` already sent exactly these bytes, so a dirty default
    /// would mean a redundant 2 KB write on the first frame of every run.
    #[test]
    fn the_default_table_is_the_compiled_one_and_is_clean() {
        let mut table = MaterialTable::default();
        assert_eq!(table.rows(), MATERIALS.as_slice());
        assert!(!table.is_modified());
        assert!(table.take_dirty().is_none());
        assert_eq!(
            table.gpu_rows(),
            crate::material::gpu_materials(),
            "the live table must upload what the compiled one does"
        );
    }

    /// An edit must produce exactly ONE upload, and it must carry the edit.
    #[test]
    fn an_edit_yields_one_upload_carrying_the_change() {
        let mut table = MaterialTable::default();
        let stone = material_id(Voxel::Stone);
        table.row_mut(stone).unwrap().albedo = [1.0, 0.0, 0.0];

        let uploaded = table.take_dirty().expect("an edit must request an upload");
        assert_eq!(uploaded.len(), MATERIAL_COUNT);
        assert_eq!(uploaded[stone as usize].albedo, [1.0, 0.0, 0.0]);
        // Every OTHER row must be untouched — the write is wholesale, not a
        // wholesale *rewrite*.
        for (id, row) in uploaded.iter().enumerate() {
            if id as u8 != stone {
                assert_eq!(*row, MATERIALS[id].to_gpu(), "row {id} changed");
            }
        }
        // ...and only one upload.
        assert!(table.take_dirty().is_none());
        assert!(table.is_modified());
        assert!(table.row_is_modified(stone));
        assert!(!table.row_is_modified(material_id(Voxel::Grass)));
    }

    /// Reset must restore the compiled bytes and ask for the upload that undoes
    /// the edit on the GPU.
    #[test]
    fn reset_restores_the_compiled_row_and_uploads() {
        let mut table = MaterialTable::default();
        let stone = material_id(Voxel::Stone);
        table.row_mut(stone).unwrap().roughness = 0.01;
        let _ = table.take_dirty();

        table.reset_row(stone);
        let uploaded = table.take_dirty().expect("reset must request an upload");
        assert_eq!(uploaded[stone as usize], MATERIALS[stone as usize].to_gpu());
        assert!(!table.is_modified());

        // Resetting an already-default row must NOT dirty the table: an idle panel
        // would otherwise upload 2 KB every frame.
        table.reset_row(stone);
        assert!(table.take_dirty().is_none());
        table.reset_all();
        assert!(table.take_dirty().is_none());
    }

    /// Every row must be reachable and out-of-range ids must not panic — the id
    /// can come from a picked voxel.
    #[test]
    fn row_access_covers_the_table_and_tolerates_bad_ids() {
        let mut table = MaterialTable::default();
        for id in 0..MATERIAL_COUNT as u8 {
            assert!(table.row(id).is_some(), "id {id} unreachable");
        }
        assert!(table.row(MATERIAL_COUNT as u8).is_none());
        assert!(table.row_mut(u8::MAX).is_none());
        assert!(!table.row_is_modified(u8::MAX));
        // A rejected id must not have dirtied anything.
        assert!(table.take_dirty().is_none());
        table.reset_row(u8::MAX);
        assert!(table.take_dirty().is_none());
    }

    /// The kind must survive a value edit untouched. The panel is not allowed to
    /// change it (see the module docs: the CPU physics predicates read the
    /// compiled table), so nothing here should ever move it.
    #[test]
    fn editing_values_leaves_the_kind_alone() {
        let mut table = MaterialTable::default();
        let water = material_id(Voxel::Water);
        table.row_mut(water).unwrap().roughness = 0.5;
        let row = table.row(water).unwrap();
        assert!(matches!(row.kind, MaterialKind::Medium(..)));
        assert!(row.is_liquid());
        assert_eq!(row.to_gpu().flags, MATERIALS[water as usize].to_gpu().flags);
    }
}
