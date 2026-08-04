//! S0b — provenance: which rows came from a `.vox`, and what you have changed by
//! hand since.
//!
//! ## The problem this solves
//!
//! An import writes into the live table, so without a record of what it wrote,
//! re-importing has to choose between two bad options: clobber everything (losing
//! the tuning that is the whole point of the panel) or skip everything (making a
//! redraw in the external editor pointless). Neither is acceptable, because the
//! external tool is exactly where iteration happens — you *will* redraw a shape
//! after seeing it lit in this engine's lighting.
//!
//! So this remembers, per row, **the row as the import left it**. A field whose
//! current value still equals that is untouched and safe to refresh from the file;
//! a field that differs was hand-tuned and is kept. Re-import becomes a merge
//! rather than a choice.
//!
//! Storing the whole post-import row rather than a set of dirty flags is the
//! cheaper and more honest design: a `Material` is small, flags would have to be
//! maintained by every widget that can touch a value (and one that forgot would
//! silently lose tuning), and a value-comparison cannot drift out of sync with the
//! values it describes.
//!
//! ## The stable-key problem
//!
//! A record keys on the palette's **1-based file index**, not its position in the
//! loaded array, because MagicaVoxel rewrites palette positions when entries are
//! dragged around — that is what the `IMAP` chunk exists to record, and what
//! `voxel_core::vox` resolves. But a file index is not proof of identity either:
//! the artist may simply have repainted that slot. So a record also keeps the
//! colour and class it imported from, and a re-import where those have changed is
//! reported as a **conflict** rather than silently applied. Silently applying it is
//! how stone's tuning ends up on grass.

use voxel_core::vox::VoxMaterialKind;

use crate::vox_material::ImportedFields;
use voxel_material::material::{Material, MATERIAL_COUNT};

/// Where an imported row came from — the identity a re-import is checked against.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoxSource {
    /// The file the import came from, as typed. Compared as a string rather than a
    /// canonicalised path: two spellings of the same file are a cosmetic mismatch,
    /// and treating them as different sources costs nothing but a re-import.
    pub path: String,
    /// The reorder-stable 1-based file index of the palette entry.
    pub file_index: u8,
    /// The colour that entry had when it was imported — half of the identity check.
    pub rgba: [u8; 4],
    /// The class it had when it was imported — the other half.
    pub kind: VoxMaterialKind,
}

/// Why a re-import is suspicious.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Conflict {
    /// The palette entry at this file index is now a different colour — most likely
    /// the slot was repainted or the palette reordered in a way `IMAP` did not
    /// capture. Applying it would move one material's tuning onto another.
    ColourChanged,
    /// The entry's material class changed (diffuse became emit, say), so the fields
    /// it offers are not the fields that were imported.
    KindChanged,
}

impl Conflict {
    pub fn describe(self) -> &'static str {
        match self {
            Conflict::ColourChanged => {
                "the palette entry at this file index is a different colour than when \
                 it was imported — the slot was repainted, or reordered in a way the \
                 index map did not capture. Applying it could move this row's tuning \
                 onto a different material."
            }
            Conflict::KindChanged => {
                "the palette entry's material class changed since it was imported, so \
                 it no longer offers the same fields."
            }
        }
    }
}

/// What a re-import did.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ReimportOutcome {
    /// Fields refreshed from the file, because they were untouched since the import.
    pub refreshed: Vec<&'static str>,
    /// Fields left alone, because they had been hand-tuned since the import.
    pub kept: Vec<&'static str>,
    /// Set when the source entry no longer looks like the one that was imported.
    pub conflict: Option<Conflict>,
}

impl ReimportOutcome {
    pub fn changed_anything(&self) -> bool {
        !self.refreshed.is_empty()
    }

    /// One line for the panel and the log.
    pub fn summary(&self) -> String {
        let mut summary = format!(
            "{} refreshed, {} kept hand-tuned",
            self.refreshed.len(),
            self.kept.len()
        );
        if !self.kept.is_empty() {
            summary.push_str(&format!(" ({})", self.kept.join(", ")));
        }
        if self.conflict.is_some() {
            summary.push_str(" — CONFLICT, nothing applied");
        }
        summary
    }
}

/// One row's import record: where it came from, and the row as the import left it.
#[derive(Clone, Debug, PartialEq)]
pub struct ImportRecord {
    pub source: VoxSource,
    /// The row immediately after the import applied. The baseline every "did you
    /// change this by hand?" question is answered against.
    pub applied: Material,
}

/// Per-row import records for the whole table.
#[derive(Clone, Debug, PartialEq)]
pub struct ProvenanceTable {
    records: Vec<Option<ImportRecord>>,
}

impl Default for ProvenanceTable {
    fn default() -> Self {
        Self {
            records: vec![None; MATERIAL_COUNT],
        }
    }
}

impl ProvenanceTable {
    /// Note that `row_after_apply` is what an import of `source` just produced.
    pub fn record(&mut self, material: u8, source: VoxSource, row_after_apply: Material) {
        if let Some(slot) = self.records.get_mut(material as usize) {
            *slot = Some(ImportRecord {
                source,
                applied: row_after_apply,
            });
        }
    }

    pub fn record_for(&self, material: u8) -> Option<&ImportRecord> {
        self.records.get(material as usize)?.as_ref()
    }

    /// Forget a row's provenance — what a "reset row" must do, or the row would
    /// still claim to have been imported from a file it no longer reflects.
    pub fn forget(&mut self, material: u8) {
        if let Some(slot) = self.records.get_mut(material as usize) {
            *slot = None;
        }
    }

    pub fn forget_all(&mut self) {
        for slot in &mut self.records {
            *slot = None;
        }
    }

    /// Every row imported from `path`, in id order — what "re-import this file"
    /// operates on.
    pub fn rows_from(&self, path: &str) -> Vec<u8> {
        self.records
            .iter()
            .enumerate()
            .filter_map(|(id, record)| {
                record
                    .as_ref()
                    .filter(|record| record.source.path == path)
                    .map(|_| id as u8)
            })
            .collect()
    }

    /// Whether a field of `current` still holds what the import left there.
    ///
    /// The only question this module really answers. A row with no record counts as
    /// hand-tuned throughout, which is the safe default: never overwrite something
    /// whose history is unknown.
    fn untouched(&self, material: u8, current: &Material, field: Field) -> bool {
        match self.record_for(material) {
            Some(record) => field.equal(&record.applied, current),
            None => false,
        }
    }

    /// Re-import `fields` into `current`, refreshing only what has not been
    /// hand-tuned since the last import from the same entry.
    ///
    /// Returns without touching anything when the source entry no longer looks like
    /// the one that was imported — see [`Conflict`]. That is a decision for a human,
    /// not a merge rule.
    pub fn reimport(
        &mut self,
        material: u8,
        current: &mut Material,
        fields: &ImportedFields,
        source: &VoxSource,
    ) -> ReimportOutcome {
        let mut outcome = ReimportOutcome::default();

        if let Some(record) = self.record_for(material) {
            if record.source.file_index == source.file_index {
                if record.source.rgba != source.rgba {
                    outcome.conflict = Some(Conflict::ColourChanged);
                }
                if record.source.kind != source.kind {
                    outcome.conflict = Some(Conflict::KindChanged);
                }
            }
        }
        if outcome.conflict.is_some() {
            return outcome;
        }

        // Split the import into the half that may be refreshed and the half that
        // must be preserved, then apply only the former.
        let mut refreshable = *fields;
        for field in Field::ALL {
            if field.present_in(fields) {
                if self.untouched(material, current, field) {
                    outcome.refreshed.push(field.name());
                } else {
                    outcome.kept.push(field.name());
                    field.clear(&mut refreshable);
                }
            }
        }
        refreshable.apply_to(current);
        self.record(material, source.clone(), *current);
        outcome
    }
}

/// One importable field, as something that can be compared and suppressed.
///
/// An enum rather than a closure table so the set is exhaustive and a new
/// importable field cannot be forgotten here: [`Field::ALL`] is what the merge
/// walks, and adding a variant without extending the matches is a compile error.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Field {
    Albedo,
    Roughness,
    Specular,
    Emission,
    Transmittance,
    IndexOfRefraction,
    Absorption,
    Scattering,
}

impl Field {
    const ALL: [Field; 8] = [
        Field::Albedo,
        Field::Roughness,
        Field::Specular,
        Field::Emission,
        Field::Transmittance,
        Field::IndexOfRefraction,
        Field::Absorption,
        Field::Scattering,
    ];

    fn name(self) -> &'static str {
        match self {
            Field::Albedo => "albedo",
            Field::Roughness => "roughness",
            Field::Specular => "specular",
            Field::Emission => "emission",
            Field::Transmittance => "transmittance",
            Field::IndexOfRefraction => "index of refraction",
            Field::Absorption => "absorption",
            Field::Scattering => "scattering",
        }
    }

    /// Whether the import carries this field at all.
    fn present_in(self, fields: &ImportedFields) -> bool {
        match self {
            Field::Albedo => fields.albedo.is_some(),
            Field::Roughness => fields.roughness.is_some(),
            Field::Specular => fields.specular.is_some(),
            Field::Emission => fields.emission.is_some(),
            Field::Transmittance => fields.transmittance.is_some(),
            Field::IndexOfRefraction => fields.index_of_refraction.is_some(),
            Field::Absorption => fields.absorption_per_meter.is_some(),
            Field::Scattering => fields.scattering_per_meter.is_some(),
        }
    }

    /// Drop this field from an import, so a merge leaves the row's value alone.
    fn clear(self, fields: &mut ImportedFields) {
        match self {
            Field::Albedo => fields.albedo = None,
            Field::Roughness => fields.roughness = None,
            Field::Specular => fields.specular = None,
            Field::Emission => fields.emission = None,
            Field::Transmittance => fields.transmittance = None,
            Field::IndexOfRefraction => fields.index_of_refraction = None,
            Field::Absorption => fields.absorption_per_meter = None,
            Field::Scattering => fields.scattering_per_meter = None,
        }
    }

    /// Whether two rows agree on this field.
    ///
    /// Reads the DERIVED accessors rather than the union payloads, so a field that
    /// lives in different places for different kinds still compares correctly, and
    /// a row whose kind cannot hold the field trivially agrees.
    fn equal(self, left: &Material, right: &Material) -> bool {
        match self {
            Field::Albedo => left.albedo == right.albedo,
            Field::Roughness => left.roughness == right.roughness,
            Field::Specular => left.specular == right.specular,
            Field::Emission => left.emitted_radiance() == right.emitted_radiance(),
            Field::Transmittance => left.transmittance() == right.transmittance(),
            Field::IndexOfRefraction => left.index_of_refraction() == right.index_of_refraction(),
            Field::Absorption => left.absorption_per_meter() == right.absorption_per_meter(),
            Field::Scattering => left.scattering_per_meter() == right.scattering_per_meter(),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_core::world::Voxel;
    use voxel_material::material::{material_id, MaterialKind, MATERIALS};

    fn source(file_index: u8, rgba: [u8; 4]) -> VoxSource {
        VoxSource {
            path: "sheet.vox".to_string(),
            file_index,
            rgba,
            kind: VoxMaterialKind::Diffuse,
        }
    }

    fn fields(roughness: f32, specular: f32, albedo: [f32; 3]) -> ImportedFields {
        ImportedFields {
            albedo: Some(albedo),
            roughness: Some(roughness),
            specular: Some(specular),
            ..ImportedFields::default()
        }
    }

    /// A row with no record is hand-tuned throughout: never overwrite something
    /// whose history is unknown.
    #[test]
    fn a_row_with_no_record_is_never_refreshed() {
        let mut provenance = ProvenanceTable::default();
        let stone = material_id(Voxel::Stone);
        let mut row = MATERIALS[stone as usize];
        let before = row;

        let outcome = provenance.reimport(
            stone,
            &mut row,
            &fields(0.1, 0.2, [1.0, 0.0, 0.0]),
            &source(1, [255, 0, 0, 255]),
        );
        assert!(outcome.refreshed.is_empty());
        assert_eq!(outcome.kept.len(), 3);
        assert_eq!(row, before, "nothing may be overwritten without a record");
    }

    /// **The property this module exists for.** Import, hand-tune ONE field,
    /// re-import: the tuned field survives and the others refresh.
    #[test]
    fn a_reimport_keeps_hand_tuning_and_refreshes_the_rest() {
        let mut provenance = ProvenanceTable::default();
        let stone = material_id(Voxel::Stone);
        let mut row = MATERIALS[stone as usize];

        // First import.
        let first = fields(0.80, 0.05, [0.5, 0.5, 0.5]);
        first.apply_to(&mut row);
        provenance.record(stone, source(1, [128, 128, 128, 255]), row);

        // Hand-tune roughness only.
        row.roughness = 0.33;

        // The file changed its colour and roughness; re-import.
        let second = fields(0.95, 0.05, [0.2, 0.4, 0.6]);
        let outcome =
            provenance.reimport(stone, &mut row, &second, &source(1, [128, 128, 128, 255]));

        assert_eq!(outcome.kept, vec!["roughness"]);
        assert!(outcome.refreshed.contains(&"albedo"));
        assert!(outcome.refreshed.contains(&"specular"));
        assert_eq!(row.roughness, 0.33, "hand tuning was clobbered");
        assert_eq!(
            row.albedo,
            [0.2, 0.4, 0.6],
            "the file's colour did not arrive"
        );
        assert!(outcome.conflict.is_none());
        assert!(outcome.changed_anything());
    }

    /// After a re-import the baseline moves, so a field refreshed once is still
    /// refreshable next time — otherwise the merge would freeze after one round.
    #[test]
    fn the_baseline_advances_after_each_reimport() {
        let mut provenance = ProvenanceTable::default();
        let stone = material_id(Voxel::Stone);
        let mut row = MATERIALS[stone as usize];
        let key = source(1, [10, 20, 30, 255]);

        let mut import = fields(0.5, 0.1, [0.1, 0.1, 0.1]);
        import.apply_to(&mut row);
        provenance.record(stone, key.clone(), row);

        for step in 1..4 {
            let value = 0.5 + step as f32 * 0.1;
            import = fields(value, 0.1, [0.1, 0.1, 0.1]);
            let outcome = provenance.reimport(stone, &mut row, &import, &key);
            assert!(
                outcome.refreshed.contains(&"roughness"),
                "round {step} stopped refreshing"
            );
            assert_eq!(row.roughness, value);
            assert!(outcome.kept.is_empty());
        }
    }

    /// A repainted palette slot must be a CONFLICT, not a merge: this is how one
    /// material's tuning ends up on another.
    #[test]
    fn a_repainted_palette_slot_is_a_conflict() {
        let mut provenance = ProvenanceTable::default();
        let stone = material_id(Voxel::Stone);
        let mut row = MATERIALS[stone as usize];

        let first = fields(0.8, 0.04, [0.5, 0.5, 0.5]);
        first.apply_to(&mut row);
        provenance.record(stone, source(7, [128, 128, 128, 255]), row);
        let after_import = row;

        // Same file index, entirely different colour.
        let outcome = provenance.reimport(
            stone,
            &mut row,
            &fields(0.2, 0.9, [0.0, 1.0, 0.0]),
            &source(7, [0, 255, 0, 255]),
        );
        assert_eq!(outcome.conflict, Some(Conflict::ColourChanged));
        assert!(outcome.refreshed.is_empty());
        assert_eq!(row, after_import, "a conflict must apply nothing");
        assert!(!outcome.conflict.unwrap().describe().is_empty());
        assert!(outcome.summary().contains("CONFLICT"));
    }

    /// A class change is the other half of the identity check.
    #[test]
    fn a_changed_material_class_is_a_conflict() {
        let mut provenance = ProvenanceTable::default();
        let stone = material_id(Voxel::Stone);
        let mut row = MATERIALS[stone as usize];
        let mut key = source(3, [90, 90, 90, 255]);

        fields(0.8, 0.04, [0.35, 0.35, 0.35]).apply_to(&mut row);
        provenance.record(stone, key.clone(), row);

        key.kind = VoxMaterialKind::Emit;
        let outcome =
            provenance.reimport(stone, &mut row, &fields(0.1, 0.1, [1.0, 1.0, 0.5]), &key);
        assert_eq!(outcome.conflict, Some(Conflict::KindChanged));
    }

    /// A DIFFERENT file index is not a conflict — it is a deliberate rebind, and
    /// the identity check must not fire on it.
    #[test]
    fn rebinding_to_another_palette_entry_is_not_a_conflict() {
        let mut provenance = ProvenanceTable::default();
        let stone = material_id(Voxel::Stone);
        let mut row = MATERIALS[stone as usize];

        fields(0.8, 0.04, [0.5, 0.5, 0.5]).apply_to(&mut row);
        provenance.record(stone, source(1, [128, 128, 128, 255]), row);

        let outcome = provenance.reimport(
            stone,
            &mut row,
            &fields(0.3, 0.2, [0.9, 0.1, 0.1]),
            &source(9, [230, 26, 26, 255]),
        );
        assert!(outcome.conflict.is_none());
        assert_eq!(row.albedo, [0.9, 0.1, 0.1]);
        // ...and the record now points at the new entry.
        assert_eq!(provenance.record_for(stone).unwrap().source.file_index, 9);
    }

    /// Provenance must be forgettable, or a reset row would still claim a source it
    /// no longer reflects — and would then be silently overwritten by a re-import.
    #[test]
    fn forgetting_provenance_makes_a_row_untouchable_again() {
        let mut provenance = ProvenanceTable::default();
        let stone = material_id(Voxel::Stone);
        let mut row = MATERIALS[stone as usize];
        let key = source(1, [128, 128, 128, 255]);

        fields(0.8, 0.04, [0.5, 0.5, 0.5]).apply_to(&mut row);
        provenance.record(stone, key.clone(), row);
        assert!(provenance.record_for(stone).is_some());

        provenance.forget(stone);
        assert!(provenance.record_for(stone).is_none());
        let before = row;
        let outcome = provenance.reimport(stone, &mut row, &fields(0.1, 0.1, [0.0; 3]), &key);
        assert!(outcome.refreshed.is_empty());
        assert_eq!(row, before);
    }

    /// "Re-import this file" needs to know which rows came from it, and must not
    /// claim rows imported from somewhere else.
    #[test]
    fn rows_are_tracked_per_source_file() {
        let mut provenance = ProvenanceTable::default();
        let stone = material_id(Voxel::Stone);
        let sand = material_id(Voxel::Sand);
        provenance.record(stone, source(1, [1, 1, 1, 255]), MATERIALS[stone as usize]);
        provenance.record(
            sand,
            VoxSource {
                path: "other.vox".to_string(),
                ..source(1, [2, 2, 2, 255])
            },
            MATERIALS[sand as usize],
        );

        assert_eq!(provenance.rows_from("sheet.vox"), vec![stone]);
        assert_eq!(provenance.rows_from("other.vox"), vec![sand]);
        assert!(provenance.rows_from("missing.vox").is_empty());

        provenance.forget_all();
        assert!(provenance.rows_from("sheet.vox").is_empty());
    }

    /// The merge must work on a medium's own fields too, which live inside the union
    /// payload rather than on the row directly.
    #[test]
    fn medium_fields_merge_through_the_union() {
        let mut provenance = ProvenanceTable::default();
        let water = material_id(Voxel::Water);
        let mut row = MATERIALS[water as usize];
        let key = source(5, [48, 132, 178, 255]);

        let first = ImportedFields {
            index_of_refraction: Some(1.45),
            absorption_per_meter: Some([0.2; 3]),
            ..ImportedFields::default()
        };
        first.apply_to(&mut row);
        provenance.record(water, key.clone(), row);
        assert_eq!(row.index_of_refraction(), 1.45);

        // Hand-tune the index of refraction; leave absorption alone.
        if let MaterialKind::Medium(medium) = &mut row.kind {
            medium.index_of_refraction = 1.61;
        }

        let second = ImportedFields {
            index_of_refraction: Some(1.33),
            absorption_per_meter: Some([0.4; 3]),
            ..ImportedFields::default()
        };
        let outcome = provenance.reimport(water, &mut row, &second, &key);
        assert_eq!(outcome.kept, vec!["index of refraction"]);
        assert!(outcome.refreshed.contains(&"absorption"));
        assert_eq!(row.index_of_refraction(), 1.61, "hand tuning was clobbered");
        assert_eq!(row.absorption_per_meter(), [0.4; 3]);
    }

    /// Every importable field must be covered by the merge walk. If a field is
    /// added to `ImportedFields` without a `Field` variant it would silently stop
    /// being protected from a re-import, which is the exact failure this module is
    /// meant to prevent.
    #[test]
    fn the_merge_walk_covers_every_importable_field() {
        // A fully-populated import: every field the merge could ever see.
        let all = ImportedFields {
            albedo: Some([0.1, 0.2, 0.3]),
            roughness: Some(0.4),
            specular: Some(0.5),
            emission: Some([1.0, 1.0, 1.0]),
            transmittance: Some(0.6),
            index_of_refraction: Some(1.7),
            absorption_per_meter: Some([0.8; 3]),
            scattering_per_meter: Some([0.9; 3]),
            specular_is_from_metalness: false,
            source_kind: VoxMaterialKind::Glass,
        };
        for field in Field::ALL {
            assert!(
                field.present_in(&all),
                "{} is not seen by the merge walk",
                field.name()
            );
            let mut cleared = all;
            field.clear(&mut cleared);
            assert!(
                !field.present_in(&cleared),
                "{} cannot be suppressed",
                field.name()
            );
        }
        // And clearing every field must leave an import that changes nothing.
        let mut empty = all;
        for field in Field::ALL {
            field.clear(&mut empty);
        }
        let mut row = MATERIALS[material_id(Voxel::Water) as usize];
        let before = row;
        assert!(!empty.apply_to(&mut row));
        assert_eq!(row, before);
    }
}
