//! S0b — mapping a `.vox` palette entry onto a material row.
//!
//! [`voxel_core::vox`] does the format work and hands back what the file *said*.
//! This module decides what that means in **this engine's** material model, and it
//! is deliberately conservative about it in two ways.
//!
//! ## 1. An import seeds a row; it never changes what the row IS
//!
//! A `.vox` palette holds up to 256 entries; this table has [`MATERIAL_COUNT`]
//! rows, welded 1:1 to `voxel-core`'s `Voxel` enum. So an import cannot create
//! rows — it lands on an existing one, as that row's *imported base*
//! (Pascal, 2026-07-31).
//!
//! Crucially it never touches the row's [`MaterialKind`]. Kind decides
//! [`voxel_material::material::MaterialFlags`], and through them the character's movement
//! predicate, the editor's notion of emptiness, and whether traversal continues
//! through the voxel — and those CPU predicates read the *compiled* table on
//! purpose. A file that could turn stone into a liquid would desync the physics
//! from the picture, from an asset, which is the worst possible place for that
//! decision to live. So: **a file can recolour and re-roughen stone; it cannot
//! change what stone is.**
//!
//! The direct consequence is that [`ImportedFields::apply_to`] silently drops
//! fields the target row cannot accept — an index of refraction landing on a
//! `Solid` has nowhere to go. Dropped rather than an error, because a palette
//! entry authored as glass is a perfectly reasonable thing to sample a *colour*
//! from; [`ImportedFields::unusable_on`] is how the panel can say what was skipped.
//!
//! ## 2. Units do not transfer, so every value is a starting point
//!
//! MagicaVoxel's `_rough`/`_sp`/`_emit` are calibrated to its own path tracer, not
//! to this one. Nothing here pretends otherwise: the import lands a plausible
//! number and the panel is what lands the look. That is the entire reason the
//! tuning layer exists, and it is why every field is an `Option` — "the file said
//! nothing" and "the file said zero" must stay distinguishable, or a re-import
//! would overwrite tuning with defaults.
//!
//! Two mappings are approximations worth naming:
//!
//! * **Metalness has no home.** There is no metal BRDF here. `_metal` is used as a
//!   fallback F0 when `_sp` is absent, which is roughly right (a metal's Fresnel
//!   reflectance at normal incidence really is high and coloured), and is recorded
//!   as approximate rather than presented as a translation.
//! * **`_att`/`_media` are scalars where this engine wants per-channel triples.**
//!   They seed a grey triple. A grey medium is the one thing the E6 model says a
//!   medium is not — its colour must *emerge* from a per-channel absorption and
//!   scattering pair — so this is explicitly a seed to be spread by hand, and the
//!   panel's derived-colour readout is what shows you it is still grey.

use voxel_core::vox::{VoxMaterialKind, VoxPaletteEntry};

use voxel_material::material::{Material, MaterialKind};

/// Everything a `.vox` file can say about a material, in this engine's units and
/// conventions. `None` means the file was silent, which is not the same as zero.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct ImportedFields {
    /// Diffuse colour, **sRGB-encoded** to match how the table stores albedo.
    pub albedo: Option<[f32; 3]>,
    pub roughness: Option<f32>,
    /// Specular reflectance at normal incidence. From `_sp`, or approximated from
    /// `_metal` when the file gave no `_sp`.
    pub specular: Option<f32>,
    /// Emitted radiance, linear and possibly above 1.0.
    pub emission: Option<[f32; 3]>,
    /// Fraction of light passing through — only meaningful on a `Cover` row.
    pub transmittance: Option<f32>,
    /// Only meaningful on a `Medium` row.
    pub index_of_refraction: Option<f32>,
    /// Grey seed from `_att`; only meaningful on a `Medium` row.
    pub absorption_per_meter: Option<[f32; 3]>,
    /// Grey seed from `_media`/`_d`; only meaningful on a `Medium` row.
    pub scattering_per_meter: Option<[f32; 3]>,
    /// Whether `specular` was approximated from `_metal` rather than read from
    /// `_sp` — so the panel can mark it as a guess.
    pub specular_is_from_metalness: bool,
    /// The file's own material class, carried for display. Never applied.
    pub source_kind: VoxMaterialKind,
}

/// A field an import could not apply, and why — the panel's "here is what I
/// skipped" list.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct UnusableField {
    pub name: &'static str,
    pub reason: &'static str,
}

impl ImportedFields {
    /// Read a palette entry into this engine's terms.
    pub(crate) fn from_palette_entry(entry: &VoxPaletteEntry) -> ImportedFields {
        let mut fields = ImportedFields {
            // Always present: even a file with no MATL chunks has colours, which
            // is the whole point of the common case.
            albedo: Some(entry.srgb_rgb()),
            roughness: entry.roughness,
            specular: entry.specular,
            source_kind: entry.kind,
            ..ImportedFields::default()
        };

        // Metalness fallback for F0. Only when the file gave no `_sp`, so an
        // authored specular always wins over an inference.
        if fields.specular.is_none() {
            if let Some(metalness) = entry.metalness {
                fields.specular = Some(metalness.clamp(0.0, 1.0));
                fields.specular_is_from_metalness = true;
            }
        }

        // Emission: the palette colour scaled by `_emit`, with `_flux` as a
        // multiplier. Uncalibrated against this renderer's exposure by definition —
        // and note the scale cannot mean anything physical at all until an HDR
        // intermediate exists, because today a bright emitter clips to flat white.
        if entry.kind == VoxMaterialKind::Emit || entry.emission.is_some() {
            let strength = entry.emission.unwrap_or(1.0) * entry.radiant_flux.unwrap_or(1.0);
            if strength > 0.0 {
                let linear = entry.linear_rgb();
                fields.emission = Some([
                    linear[0] * strength,
                    linear[1] * strength,
                    linear[2] * strength,
                ]);
            }
        }

        // `_trans` is a physical transparency (NOT `_alpha`, which is compositing).
        // It reads as transmittance for a surface and for a medium alike; which of
        // the two the target row is decides whether it can be used.
        fields.transmittance = entry.transparency.map(|value| value.clamp(0.0, 1.0));
        fields.index_of_refraction = entry.index_of_refraction;

        // The scalar-to-grey-triple seeds. Absorption comes from `_att`; scattering
        // from the volumetric density, because a `_media` entry is
        // scattering-dominated — that is what makes it a cloud rather than a filter.
        fields.absorption_per_meter = entry.attenuation.map(grey);
        fields.scattering_per_meter = match entry.kind {
            VoxMaterialKind::Media => entry.media_scattering_seed().map(grey),
            _ => None,
        };
        fields
    }

    /// Apply every field this row's kind can accept, leaving the kind itself and
    /// everything else untouched.
    ///
    /// Returns whether anything changed, so a caller can avoid dirtying the table
    /// for a no-op import.
    pub(crate) fn apply_to(&self, row: &mut Material) -> bool {
        let before = *row;
        if let Some(albedo) = self.albedo {
            row.albedo = albedo;
        }
        if let Some(roughness) = self.roughness {
            row.roughness = roughness.clamp(0.0, 1.0);
        }
        if let Some(specular) = self.specular {
            row.specular = specular.clamp(0.0, 1.0);
        }
        // Emission only lands on a row that ALREADY emits. Adding emission is
        // structural: it flips the EMISSIVE flag, which decides whether CAGI
        // injects the row as a light source and which of its 8 palette slots it
        // claims — not something an asset gets to do.
        if let (Some(emission), Some(existing)) = (self.emission, row.emission.as_mut()) {
            *existing = emission;
        }
        match &mut row.kind {
            MaterialKind::Air | MaterialKind::Solid => {}
            MaterialKind::Cover { transmittance } => {
                if let Some(imported) = self.transmittance {
                    // Floored above zero: a cover row that blocks all light is what
                    // paints black canopies, and a test forbids it.
                    *transmittance = imported.clamp(0.01, 1.0);
                }
            }
            MaterialKind::Medium(medium) => {
                if let Some(index) = self.index_of_refraction {
                    medium.index_of_refraction = index.max(1.0);
                }
                if let Some(transmittance) = self.transmittance {
                    medium.transmittance = transmittance;
                }
                if let Some(absorption) = self.absorption_per_meter {
                    medium.absorption_per_meter = absorption;
                }
                if let Some(scattering) = self.scattering_per_meter {
                    medium.scattering_per_meter = scattering;
                }
            }
        }
        *row != before
    }

    /// Which fields this import carries that the target row cannot accept.
    ///
    /// Not an error path: sampling a colour from an entry authored as glass is
    /// perfectly reasonable. It exists so the panel can say what it skipped instead
    /// of the user wondering why the index of refraction did nothing.
    pub fn unusable_on(&self, row: &Material) -> Vec<UnusableField> {
        let mut unusable = Vec::new();
        let is_medium = matches!(row.kind, MaterialKind::Medium(..));
        let is_cover = matches!(row.kind, MaterialKind::Cover { .. });

        if self.emission.is_some() && row.emission.is_none() {
            unusable.push(UnusableField {
                name: "emission",
                reason: "the target row does not emit, and adding emission is a \
                         compiled-table change: it decides whether the GI injects a \
                         light source and which palette slot it claims",
            });
        }
        if self.transmittance.is_some() && !is_medium && !is_cover {
            unusable.push(UnusableField {
                name: "transmittance",
                reason: "the target row is opaque; only cover and media transmit",
            });
        }
        if self.index_of_refraction.is_some() && !is_medium {
            unusable.push(UnusableField {
                name: "index of refraction",
                reason: "the target row is not a medium, so a ray never enters it \
                         and there is nothing to bend",
            });
        }
        if (self.absorption_per_meter.is_some() || self.scattering_per_meter.is_some())
            && !is_medium
        {
            unusable.push(UnusableField {
                name: "medium coefficients",
                reason: "the target row is not a medium; a ray cannot travel inside it",
            });
        }
        unusable
    }
}

/// A scalar as a grey triple — the honest shape of a single-number import into a
/// per-channel field. Grey is precisely what a medium's coefficients should NOT
/// stay, so this is a seed and the panel is where it gets spread.
fn grey(value: f32) -> [f32; 3] {
    [value.max(0.0); 3]
}

/// The table row whose albedo is closest to an sRGB colour — the default binding
/// when a `.vox` model is shown in the studio.
///
/// Needed because a `.vox` palette holds up to 256 entries and the table has
/// [`voxel_material::material::MATERIAL_COUNT`] rows welded to `voxel-core`'s `Voxel` enum,
/// so a file's colours cannot become rows. Nearest-albedo is a *preview* device:
/// it makes a loaded model recognisable in the studio using materials that already
/// exist, and it is overridable per entry because "closest colour" and "right
/// material" are not the same question.
///
/// Distance is Euclidean in the stored sRGB-encoded space rather than in linear
/// light. That is deliberate: the numbers being compared are the authored ones on
/// both sides, and perceptual closeness tracks the encoded space better than
/// linear does — linear would bunch every dark tone together, which is exactly
/// where a voxel palette puts most of its distinct shades.
///
/// Air is excluded: it is the miss sentinel and matching it would make a cell
/// vanish. Rows that emit are excluded too — binding a colour to a light source
/// because it happens to be pale would silently turn a model into a lamp.
pub(crate) fn nearest_material_row(albedo_srgb: [f32; 3]) -> u8 {
    let mut best = (f32::MAX, 0_u8);
    for (id, row) in voxel_material::material::MATERIALS.iter().enumerate() {
        if matches!(row.kind, MaterialKind::Air) || row.is_emissive() {
            continue;
        }
        let distance: f32 = (0..3)
            .map(|channel| {
                let delta = row.albedo[channel] - albedo_srgb[channel];
                delta * delta
            })
            .sum();
        if distance < best.0 {
            best = (distance, id as u8);
        }
    }
    best.1
}

/// One palette entry worth offering as an import source.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct VoxImportRow {
    /// Position in [`VoxFile::palette`] — what a model's cells hold.
    pub palette_index: usize,
    /// The reorder-stable 1-based file index, the key anything persisted should use.
    pub file_index: u8,
    pub entry: VoxPaletteEntry,
    pub fields: ImportedFields,
    /// How many cells across every model use this entry. Zero for an entry that
    /// only exists in the palette.
    pub used_by_cells: usize,
    /// Which table row this entry is bound to when the model is shown in the
    /// studio. Defaults to [`nearest_material_row`]; overridable per entry.
    pub bound_row: u8,
}

impl VoxImportRow {
    /// A short label for the picker: the file index, the class, and what it is for.
    pub fn label(&self) -> String {
        let kind = match self.entry.kind {
            VoxMaterialKind::Diffuse => "diffuse",
            VoxMaterialKind::Metal => "metal",
            VoxMaterialKind::Emit => "emit",
            VoxMaterialKind::Glass => "glass",
            VoxMaterialKind::Media => "media",
            VoxMaterialKind::Unknown => "unknown",
        };
        let authored = if self.entry.describes_a_material() {
            ""
        } else {
            " (colour only)"
        };
        format!(
            "#{:<3} {kind:<8}{authored}  {} cells",
            self.file_index, self.used_by_cells
        )
    }
}

/// The palette entries worth showing as import sources: those a model actually
/// uses, plus any the file described a material for.
///
/// A `.vox` palette is always 256 entries and most files use a handful, so
/// offering all 256 would bury the six that mean something under 250 unauthored
/// blacks. An entry that describes a material but is unused is still included —
/// a material sheet may well carry a row nothing is painted with yet.
pub fn importable_rows(file: &voxel_core::vox::VoxFile) -> Vec<VoxImportRow> {
    let mut usage = vec![0_usize; file.palette.len()];
    for model in &file.models {
        for cell in model.cells.iter().flatten() {
            if let Some(count) = usage.get_mut(*cell as usize) {
                *count += 1;
            }
        }
    }
    file.palette
        .iter()
        .enumerate()
        .filter(|(index, entry)| usage[*index] > 0 || entry.describes_a_material())
        .map(|(palette_index, entry)| {
            let fields = ImportedFields::from_palette_entry(entry);
            VoxImportRow {
                palette_index,
                file_index: entry.file_index,
                entry: *entry,
                bound_row: nearest_material_row(fields.albedo.unwrap_or([0.0; 3])),
                fields,
                used_by_cells: usage[palette_index],
            }
        })
        .collect()
}

/// A loaded `.vox` model turned into material ids, ready for the studio to build a
/// brickmap from.
///
/// Carries ids rather than palette indices because that is what a brickmap stores —
/// one byte per voxel — so the binding is resolved once here instead of being
/// re-decided wherever the model is used.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct VoxSubject {
    pub size_x: i32,
    pub size_y: i32,
    pub size_z: i32,
    /// Material id per cell, `None` for empty. Same layout as
    /// [`voxel_core::vox::VoxModel::cells`].
    pub cells: Vec<Option<u8>>,
}

impl VoxSubject {
    /// Resolve one model's cells through the current bindings.
    ///
    /// An unbound palette index (one no import row covers, which cannot happen for
    /// a cell that is in use, but is cheap to be safe about) falls back to
    /// nearest-albedo rather than vanishing — a hole in the model would look like a
    /// generation bug rather than a missing binding.
    pub fn from_model(
        model: &voxel_core::vox::VoxModel,
        palette: &[voxel_core::vox::VoxPaletteEntry],
        rows: &[VoxImportRow],
    ) -> VoxSubject {
        let mut binding = [None; 256];
        for row in rows {
            binding[row.palette_index] = Some(row.bound_row);
        }
        VoxSubject {
            size_x: model.size_x,
            size_y: model.size_y,
            size_z: model.size_z,
            cells: model
                .cells
                .iter()
                .map(|cell| {
                    let palette_index = (*cell)? as usize;
                    Some(binding[palette_index].unwrap_or_else(|| {
                        let albedo = palette
                            .get(palette_index)
                            .map_or([0.0; 3], |entry| entry.srgb_rgb());
                        nearest_material_row(albedo)
                    }))
                })
                .collect(),
        }
    }

    pub fn occupied_count(&self) -> usize {
        self.cells.iter().filter(|cell| cell.is_some()).count()
    }
}

/// The scattering seed for a `_media` entry.
trait MediaScattering {
    fn media_scattering_seed(&self) -> Option<f32>;
}

impl MediaScattering for VoxPaletteEntry {
    /// `_d` (density) preferred over `_media` (the class amount), because density is
    /// the one that means "how much stuff per unit length" — which is what a
    /// scattering coefficient is.
    fn media_scattering_seed(&self) -> Option<f32> {
        self.density.or(self.attenuation).or(self.emission)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::{Path, PathBuf};
    use voxel_core::vox::VoxFile;
    use voxel_core::world::Voxel;
    use voxel_material::material::{material_id, MATERIALS};

    fn sheet_path() -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR")).join("../../assets/vox/material_sheet.vox")
    }

    /// Load the checked-in material sheet. It carries seven MATL chunks covering
    /// diffuse / metal / emit / glass / media, plus ONE entry deliberately left
    /// with no chunk at all — a mixed file, which is the case neither an all-MATL
    /// nor a no-MATL fixture can exercise.
    pub(super) fn sheet() -> VoxFile {
        VoxFile::load(&sheet_path()).expect("the material sheet asset must load")
    }

    #[test]
    fn the_sheet_covers_every_material_class() {
        let file = sheet();
        let kinds: Vec<_> = file.palette[..8].iter().map(|entry| entry.kind).collect();
        assert_eq!(
            kinds,
            vec![
                VoxMaterialKind::Diffuse,
                VoxMaterialKind::Metal,
                VoxMaterialKind::Emit,
                VoxMaterialKind::Glass,
                VoxMaterialKind::Glass,
                VoxMaterialKind::Media,
                // Entry 6 has no MATL chunk: it must read as plain diffuse.
                VoxMaterialKind::Diffuse,
                VoxMaterialKind::Diffuse,
            ]
        );
        assert!(!file.palette[6].describes_a_material());
        assert!(file.palette[5].describes_a_material());
        assert_eq!(file.described_material_count(), 7);
    }

    /// Colour is the one field EVERY entry carries, including one with no material
    /// chunk — the common case for external tools.
    #[test]
    fn colour_imports_even_with_no_material_chunk() {
        let file = sheet();
        let fields = ImportedFields::from_palette_entry(&file.palette[6]);
        let albedo = fields.albedo.expect("colour is always importable");
        // sRGB-encoded, matching how the table stores albedo: 150/255, 96/255, 56/255.
        assert!((albedo[0] - 150.0 / 255.0).abs() < 1e-6);
        assert!((albedo[1] - 96.0 / 255.0).abs() < 1e-6);
        assert!(fields.roughness.is_none(), "the file said nothing");
        assert!(fields.specular.is_none());
        assert!(fields.emission.is_none());
    }

    /// `_rough` and `_sp` come straight through when authored.
    #[test]
    fn authored_roughness_and_specular_import_directly() {
        let file = sheet();
        let fields = ImportedFields::from_palette_entry(&file.palette[0]);
        assert_eq!(fields.roughness, Some(0.85));
        assert_eq!(fields.specular, Some(0.04));
        assert!(!fields.specular_is_from_metalness);
    }

    /// Metalness is only a FALLBACK for F0 — an authored `_sp` must win, or an
    /// approximation would quietly override the artist.
    #[test]
    fn metalness_never_overrides_an_authored_specular() {
        let file = sheet();
        // The sheet's metal entry authors BOTH `_metal` 0.9 and `_sp` 0.6.
        let fields = ImportedFields::from_palette_entry(&file.palette[1]);
        assert_eq!(fields.specular, Some(0.6), "the authored _sp must win");
        assert!(!fields.specular_is_from_metalness);

        // With no `_sp`, metalness stands in and is flagged as a guess.
        let mut entry = file.palette[1];
        entry.specular = None;
        let fields = ImportedFields::from_palette_entry(&entry);
        assert_eq!(fields.specular, Some(0.9));
        assert!(fields.specular_is_from_metalness);
    }

    /// The refractive index must arrive with the format's minus-one offset already
    /// removed by the loader: the sheet's water-like entry authors `_ior` 0.333.
    #[test]
    fn the_refractive_index_arrives_physical() {
        let file = sheet();
        let water_like = ImportedFields::from_palette_entry(&file.palette[4]);
        let index = water_like.index_of_refraction.expect("authored");
        assert!(
            (index - 1.333).abs() < 1e-5,
            "expected 1.333, got {index} — the _ior offset was not removed"
        );
        let glass = ImportedFields::from_palette_entry(&file.palette[3]);
        assert!((glass.index_of_refraction.unwrap() - 1.52).abs() < 1e-5);
    }

    /// **The rule that matters most**: an import must never change what a row IS.
    /// A glass entry landing on stone may recolour it and must leave it a `Solid`
    /// with the same flag word — because those flags drive movement, the editor's
    /// emptiness test and traversal.
    #[test]
    fn an_import_never_changes_a_rows_kind_or_flags() {
        let file = sheet();
        for entry in &file.palette[..8] {
            let fields = ImportedFields::from_palette_entry(entry);
            for voxel in [Voxel::Stone, Voxel::Water, Voxel::Leaves, Voxel::GlowBlock] {
                let id = material_id(voxel);
                let original = MATERIALS[id as usize];
                let mut row = original;
                fields.apply_to(&mut row);
                assert_eq!(
                    std::mem::discriminant(&row.kind),
                    std::mem::discriminant(&original.kind),
                    "{} changed kind importing {:?}",
                    original.name,
                    entry.kind
                );
                assert_eq!(
                    row.to_gpu().flags,
                    original.to_gpu().flags,
                    "{} changed its flag word importing {:?}",
                    original.name,
                    entry.kind
                );
                assert_eq!(
                    row.is_liquid(),
                    original.is_liquid(),
                    "{} changed liquidity",
                    original.name
                );
                assert_eq!(row.emission.is_some(), original.emission.is_some());
            }
        }
    }

    /// Fields the target cannot accept are dropped and REPORTED, not applied and
    /// not an error.
    #[test]
    fn fields_the_row_cannot_accept_are_reported() {
        let file = sheet();
        let glass = ImportedFields::from_palette_entry(&file.palette[3]);
        let stone = MATERIALS[material_id(Voxel::Stone) as usize];
        let unusable = glass.unusable_on(&stone);
        let names: Vec<_> = unusable.iter().map(|field| field.name).collect();
        assert!(names.contains(&"index of refraction"), "got {names:?}");
        assert!(names.contains(&"transmittance"), "got {names:?}");
        assert!(names.contains(&"medium coefficients"), "got {names:?}");
        for field in &unusable {
            assert!(!field.reason.is_empty(), "{} has no reason", field.name);
        }

        // The same import onto water, which IS a medium, has nothing to skip.
        let water = MATERIALS[material_id(Voxel::Water) as usize];
        assert!(glass.unusable_on(&water).is_empty());
    }

    /// Emission may only land on a row that already emits, and that restriction
    /// must be reported rather than silent.
    #[test]
    fn emission_only_lands_on_a_row_that_already_emits() {
        let file = sheet();
        let lamp = ImportedFields::from_palette_entry(&file.palette[2]);
        assert!(lamp.emission.is_some(), "the sheet's _emit entry must emit");

        let mut stone = MATERIALS[material_id(Voxel::Stone) as usize];
        lamp.apply_to(&mut stone);
        assert!(stone.emission.is_none(), "stone must not start emitting");
        assert!(lamp
            .unusable_on(&MATERIALS[material_id(Voxel::Stone) as usize])
            .iter()
            .any(|field| field.name == "emission"));

        // On a row that already emits, the radiance is replaced.
        let mut glow = MATERIALS[material_id(Voxel::GlowBlock) as usize];
        let before = glow.emitted_radiance();
        assert!(lamp.apply_to(&mut glow));
        assert!(glow.emission.is_some());
        assert_ne!(glow.emitted_radiance(), before);
    }

    /// A medium import must actually reach the medium's own fields, and the scalar
    /// coefficients must arrive as the grey triple they are — grey being exactly
    /// what the E6 rule says a medium must not stay, hence a seed.
    #[test]
    fn medium_coefficients_import_as_a_grey_seed() {
        let file = sheet();
        let cloud = ImportedFields::from_palette_entry(&file.palette[5]);
        let scattering = cloud
            .scattering_per_meter
            .expect("a media entry must seed scattering");
        assert_eq!(scattering[0], scattering[1]);
        assert_eq!(scattering[1], scattering[2]);
        assert!(scattering[0] > 0.0);

        // The sheet's media entry authors `_d` but no `_att`, so only scattering is
        // seeded. Water's own per-channel ABSORPTION must survive untouched — the
        // whole reason every field is an `Option`: a partial import must not
        // flatten a triple the file never mentioned.
        assert!(
            cloud.absorption_per_meter.is_none(),
            "the sheet's media entry authors no _att"
        );
        let original = MATERIALS[material_id(Voxel::Water) as usize];
        let mut water = original;
        assert!(cloud.apply_to(&mut water));
        let medium = water.medium().expect("still a medium");
        assert_eq!(medium.scattering_per_meter, scattering);
        assert_eq!(
            medium.absorption_per_meter,
            original.absorption_per_meter(),
            "an unmentioned field must not be overwritten"
        );

        // Seeding scattering grey over a coloured absorption still shifts the
        // derived colour, which is the point of the panel's readout: you can SEE
        // that the pair is now half-authored and needs spreading by hand.
        assert_ne!(
            water.single_scattering_albedo(),
            original.single_scattering_albedo()
        );

        // With BOTH halves seeded from scalars the derived colour does go grey —
        // exactly what the E6 rule says a medium must not stay.
        let mut both = original;
        let mut greyed = cloud;
        greyed.absorption_per_meter = Some([0.2; 3]);
        greyed.apply_to(&mut both);
        let derived = both.single_scattering_albedo();
        assert!(
            (derived[0] - derived[2]).abs() < 1e-6,
            "two scalar seeds must produce a grey medium: {derived:?}"
        );
    }

    /// A cover import must not be able to author an opaque leaf, which a test in
    /// `material` forbids outright.
    #[test]
    fn a_cover_import_cannot_author_an_opaque_leaf() {
        let mut entry = sheet().palette[0];
        entry.transparency = Some(0.0);
        let fields = ImportedFields::from_palette_entry(&entry);
        let mut leaves = MATERIALS[material_id(Voxel::Leaves) as usize];
        fields.apply_to(&mut leaves);
        assert!(
            leaves.transmittance() > 0.0,
            "an import drove a cover row opaque"
        );
    }

    /// Applying a no-op import must report no change, so an idle import does not
    /// dirty the table and re-upload every frame.
    #[test]
    fn a_noop_import_reports_no_change() {
        let stone = MATERIALS[material_id(Voxel::Stone) as usize];
        let fields = ImportedFields {
            albedo: Some(stone.albedo),
            roughness: Some(stone.roughness),
            specular: Some(stone.specular),
            ..ImportedFields::default()
        };
        let mut row = stone;
        assert!(!fields.apply_to(&mut row));
        assert_eq!(row, stone);
    }
}

#[cfg(test)]
mod studio_binding_tests {
    use super::*;
    use voxel_core::world::Voxel;
    use voxel_material::material::{material_id, MATERIALS};

    /// The nearest-albedo binding must never pick Air (a cell would vanish) or an
    /// emitter (a model would silently become a lamp).
    #[test]
    fn the_nearest_row_is_never_air_or_an_emitter() {
        // Sweep the colour cube coarsely rather than trusting a few hand-picked
        // colours: the excluded rows must be unreachable from ANY colour.
        for red in 0..=4 {
            for green in 0..=4 {
                for blue in 0..=4 {
                    let colour = [red as f32 / 4.0, green as f32 / 4.0, blue as f32 / 4.0];
                    let id = nearest_material_row(colour);
                    let row = MATERIALS[id as usize];
                    assert!(
                        !matches!(row.kind, MaterialKind::Air),
                        "{colour:?} bound to air"
                    );
                    assert!(
                        !row.is_emissive(),
                        "{colour:?} bound to emitter {}",
                        row.name
                    );
                }
            }
        }
    }

    /// A row's own albedo must bind to that row — the sanity property that makes
    /// nearest-albedo a usable default at all.
    #[test]
    fn a_rows_own_colour_binds_to_itself() {
        for voxel in [
            Voxel::Stone,
            Voxel::Sand,
            Voxel::Water,
            Voxel::Snow,
            Voxel::Dirt,
        ] {
            let id = material_id(voxel);
            assert_eq!(
                nearest_material_row(MATERIALS[id as usize].albedo),
                id,
                "{voxel:?} did not bind to itself"
            );
        }
    }

    /// An explicit binding must win over the nearest-albedo default, or the
    /// override control does nothing.
    #[test]
    fn an_explicit_binding_overrides_the_default() {
        let file = super::tests::sheet();
        let mut rows = importable_rows(&file);
        let target = material_id(Voxel::Snow);
        for row in &mut rows {
            row.bound_row = target;
        }
        let subject = VoxSubject::from_model(&file.models[0], &file.palette, &rows);
        assert!(subject.occupied_count() > 0);
        for material in subject.cells.iter().flatten() {
            assert_eq!(*material, target, "an explicit binding was ignored");
        }
    }
}
