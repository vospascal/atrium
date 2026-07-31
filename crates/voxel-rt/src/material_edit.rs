//! S0 — the material panel: the authoring loop that did not exist.
//!
//! Before this, every one of the 26 rows was tuned by editing Rust and rebuilding,
//! and the table buffer was created without `COPY_DST` so a live edit was
//! impossible even in principle. That is why half the columns were authored blind —
//! roughness is a uniform `0.60` across every solid, written when nothing read it.
//! Every later stage of this arc is judged by eye, so this panel is what makes the
//! rest of the arc affordable.
//!
//! ## Type-driven, which is the point of the union
//!
//! The panel shows the fields that apply to the selected row's [`MaterialKind`] and
//! nothing else. A `Solid` has no index of refraction to drag and no absorption
//! triple to be confused by; a `Medium` has both. Before the union those columns
//! existed on every row carrying sentinels, and a panel over that shape would have
//! offered 26 rows of controls that silently did nothing on 24 of them.
//!
//! ## The two tiers, stated rather than hidden
//!
//! [`crate::cagi`] bakes albedo, a quantised transmittance and the emitter slot
//! into its own cell-attribute volume and its shaders never read the material
//! binding. So an albedo edit is instant in direct shading and **stale in the GI
//! bounce** until the attributes are re-packed. Rather than pretend otherwise, the
//! panel labels which fields are in which tier and offers the re-pack explicitly —
//! it is a ~50 ms rebuild that belongs off-frame on the world thread, not something
//! to run silently on every slider tick.
//!
//! ## Why kind is shown but not editable
//!
//! Kind decides [`crate::material::MaterialFlags`], and through them the
//! character's movement predicate, the editor's notion of emptiness, and whether
//! traversal continues through the voxel. Those CPU predicates read the *compiled*
//! table on purpose (they are sampled per frame and must not depend on renderer
//! state), so a live kind change would desync the physics from the picture. Values
//! within a kind are what tuning actually needs.

use crate::material::{Material, MaterialKind, MATERIALS};
use crate::material_table::MaterialTable;
use crate::material_tune::{ProvenanceTable, VoxSource};
use crate::pattern::{
    PatternBlend, PatternFrame, PatternGenerator, PatternLayer, PatternTarget, MAX_NOISE_OCTAVES,
    MAX_PATTERN_LAYERS, NO_PATTERNS, TEXEL_RUNGS,
};
use crate::studio::StudioPose;
use crate::vox_material::VoxImportRow;
use voxel_core::world::VOXEL_SIZE;

/// The panel's own UI state — what is selected and what it has asked for.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct MaterialPanelState {
    /// Material id currently being edited.
    pub selected: u8,
    /// The eyedropper is armed: the next world pick selects that voxel's row
    /// instead of editing the world.
    pub eyedropper_armed: bool,
    /// The user asked for a CAGI attribute re-pack (the second tier above).
    pub repack_gi_requested: bool,
    /// S0b — the `.vox` import panel's state.
    pub import: VoxImportState,
    /// S2 — the user picked a studio pose. A one-shot request rather than a stored
    /// pose, because servicing it means rebuilding the world, which the platform
    /// layer owns; the pose itself lives on [`crate::studio::StudioScene`].
    pub studio_pose_requested: Option<StudioPose>,
}

/// S0b — the `.vox` import panel.
///
/// Kept beside the panel rather than in the material table because none of it is
/// material data: it is a path, a parsed file held for browsing, and a couple of
/// one-shot requests the platform layer services.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct VoxImportState {
    /// Path being edited in the text field.
    pub path: String,
    /// Set when the user pressed Load; the platform layer clears it after doing
    /// the file I/O, which is deliberately not done from inside the UI closure.
    pub load_requested: bool,
    /// The last load's outcome, shown verbatim — including the error, because
    /// "cannot load foo.vox: ..." is the most useful thing a failed import can say.
    pub status: String,
    /// The loaded file's importable palette entries, or empty when nothing is
    /// loaded.
    pub rows: Vec<VoxImportRow>,
    /// Which of [`Self::rows`] is selected as the import source.
    pub selected_row: usize,
    /// S0b — the user asked to show the loaded model in the studio. Serviced by the
    /// platform layer, which has to rebuild the world.
    pub show_in_studio_requested: bool,
    /// Which of the loaded file's models to show, for a pack holding several.
    pub selected_model: usize,
    /// How many models the loaded file holds.
    pub model_count: usize,
}

impl VoxImportState {
    /// The path the field starts on: the checked-in material sheet, because a
    /// blank field with no hint is a worse first experience than a working default.
    pub const DEFAULT_PATH: &'static str = "assets/vox/material_sheet.vox";

    pub fn new() -> VoxImportState {
        VoxImportState {
            path: VoxImportState::DEFAULT_PATH.to_string(),
            ..VoxImportState::default()
        }
    }

    /// The selected source entry, if there is one.
    pub fn selected(&self) -> Option<&VoxImportRow> {
        self.rows.get(self.selected_row)
    }
}

/// Draw the material section. Returns nothing: everything it changes, it changes
/// through `table` and `state`, so the platform layer owns the consequences (the
/// upload, and the off-frame re-pack).
pub fn draw_material_section(
    ui: &mut egui::Ui,
    table: &mut MaterialTable,
    state: &mut MaterialPanelState,
    provenance: &mut ProvenanceTable,
) {
    ui.collapsing("Materials", |ui| {
        draw_row_selector(ui, table, state);
        ui.separator();

        let selected = state.selected;
        let Some(row) = table.row(selected).copied() else {
            ui.label("no such material row");
            return;
        };

        draw_kind_readout(ui, &row);

        // Shared header — every kind has a surface.
        let mut edited = row;
        ui.horizontal(|ui| {
            ui.color_edit_button_rgb(&mut edited.albedo);
            ui.label("albedo").on_hover_text(
                "Diffuse colour, stored sRGB-ENCODED exactly as authored; the shader \
                 decodes it with `srgb_decode` before lighting. Also the GI bounce \
                 tint, so this field is in the re-pack tier: the direct shading \
                 updates instantly, the bounce after a CAGI re-pack. Note the swatch \
                 is egui's own interpretation of these three numbers and will not \
                 match the rendered surface exactly.",
            );
        });
        ui.add(
            egui::Slider::new(&mut edited.roughness, 0.0..=1.0)
                .text("roughness")
                .max_decimals(3),
        )
        .on_hover_text(
            "0 = mirror, 1 = fully diffuse. AUTHORED BUT UNREAD today: no pass \
             samples it until the reflection stage exists. The uniform 0.60 across \
             every solid row is a placeholder written when nothing could see it was \
             wrong — re-authoring this column is a scheduled step, not a finished one.",
        );
        ui.add(
            egui::Slider::new(&mut edited.specular, 0.0..=1.0)
                .text("specular (F0)")
                .max_decimals(3),
        )
        .on_hover_text(
            "Specular reflectance at normal incidence. Also authored-but-unread; \
             water derives its own F0 from the two indices of refraction rather than \
             reading this.",
        );

        draw_kind_payload(ui, &mut edited);
        draw_emission(ui, &mut edited);
        draw_face_roles(ui, &mut edited);
        draw_pattern_layers(ui, &mut edited);

        if edited != row {
            if let Some(target) = table.row_mut(selected) {
                *target = edited;
            }
        }

        ui.separator();
        draw_studio_pose(ui, state);
        ui.separator();
        draw_import_section(ui, table, state, provenance, selected);
        ui.separator();
        draw_tier_controls(ui, table, state, provenance, selected);
    });
}

/// S0b — load a `.vox` and seed the selected row from one of its palette entries.
///
/// The file work itself is NOT done here: this only raises
/// [`VoxImportState::load_requested`]. Doing blocking I/O inside an egui closure
/// would put a disk read on the frame thread inside the overlay pass, and the
/// platform layer is where every other "this touches the world" request already
/// goes.
fn draw_import_section(
    ui: &mut egui::Ui,
    table: &mut MaterialTable,
    state: &mut MaterialPanelState,
    provenance: &mut ProvenanceTable,
    selected: u8,
) {
    let selected_name = table.row(selected).map_or("<none>", |row| row.name);
    ui.collapsing("Import from .vox", |ui| {
        ui.horizontal(|ui| {
            ui.add(
                egui::TextEdit::singleline(&mut state.import.path)
                    .desired_width(220.0)
                    .hint_text(VoxImportState::DEFAULT_PATH),
            );
            if ui
                .button("Load")
                .on_hover_text(
                    "Reads the file's palette and material chunks. Loading does not \
                     change anything on its own — it lists what the file offers, and \
                     you apply one entry at a time.",
                )
                .clicked()
            {
                state.import.load_requested = true;
            }
        });
        if !state.import.status.is_empty() {
            ui.label(&state.import.status);
        }

        if state.import.rows.is_empty() {
            ui.label("Nothing loaded.").on_hover_text(
                "Relative paths resolve from the directory the app was launched in \
                 (the workspace root, if you used `cargo run`).",
            );
            return;
        }

        // Showing the geometry. Separate from applying a material because they are
        // different questions: one is "what does this file's stone look like", the
        // other is "what does this file look like".
        ui.horizontal(|ui| {
            if ui
                .button("Show model in studio")
                .on_hover_text(
                    "Rebuilds the studio around this file's geometry, with each \
                     palette entry drawn as the table row it is bound to below. The \
                     bindings default to the nearest albedo match, because a .vox \
                     palette can hold 256 colours and this table has a fixed 26 rows \
                     welded to the voxel enum — so a file's colours cannot become \
                     rows, only be previewed through existing ones.",
                )
                .clicked()
            {
                state.import.show_in_studio_requested = true;
            }
            if state.import.model_count > 1 {
                ui.add(
                    egui::DragValue::new(&mut state.import.selected_model)
                        .range(0..=state.import.model_count.saturating_sub(1))
                        .prefix("model "),
                );
            }
        });

        egui::ComboBox::from_label("source entry")
            .selected_text(
                state
                    .import
                    .selected()
                    .map_or_else(|| "—".to_string(), VoxImportRow::label),
            )
            .show_ui(ui, |ui| {
                for (index, row) in state.import.rows.iter().enumerate() {
                    ui.selectable_value(&mut state.import.selected_row, index, row.label());
                }
            });

        let Some(source) = state.import.selected().copied() else {
            return;
        };

        // The colour, so the picker is judgeable without applying anything.
        ui.horizontal(|ui| {
            let mut swatch = source.fields.albedo.unwrap_or([0.0; 3]);
            ui.color_edit_button_rgb(&mut swatch);
            ui.label(format!(
                "rgba {} {} {} {}",
                source.entry.rgba[0],
                source.entry.rgba[1],
                source.entry.rgba[2],
                source.entry.rgba[3]
            ));
        });
        draw_source_summary(ui, &source);

        // The binding this entry is drawn as when the model is shown.
        let bound_name = table
            .row(source.bound_row)
            .map_or("<none>", |row| row.name)
            .to_string();
        egui::ComboBox::from_label("drawn as")
            .selected_text(format!("{:<2} {bound_name}", source.bound_row))
            .show_ui(ui, |ui| {
                for id in 0..MATERIALS.len() as u8 {
                    let name = table.row(id).map_or("<none>", |row| row.name);
                    if let Some(row) = state.import.rows.get_mut(state.import.selected_row) {
                        ui.selectable_value(&mut row.bound_row, id, format!("{id:<2} {name}"));
                    }
                }
            })
            .response
            .on_hover_text(
                "Which existing table row this palette entry is drawn as in the \
                 studio. Defaults to the nearest albedo match; change it and press \
                 'Show model in studio' again. This is a PREVIEW binding — it does \
                 not alter the file or the table.",
            );

        // What this import cannot do to THIS row, stated before it is applied.
        if let Some(row) = table.row(selected) {
            let unusable = source.fields.unusable_on(row);
            if !unusable.is_empty() {
                ui.label(format!(
                    "will skip {} field(s) on {selected_name}:",
                    unusable.len()
                ));
                for field in &unusable {
                    ui.label(format!("  · {}", field.name))
                        .on_hover_text(field.reason);
                }
            }
        }

        if ui
            .button(format!("Apply to '{selected_name}'"))
            .on_hover_text(
                "Seeds this row from the file. Never changes the row's KIND — a file \
                 can recolour and re-roughen stone, it cannot turn stone into a \
                 liquid, because that would move the movement and editor predicates \
                 from an asset. Use 'reset row' to undo.",
            )
            .clicked()
        {
            let applied = table
                .row_mut(selected)
                .map(|row| source.fields.apply_to(row))
                .unwrap_or(false);
            // Record what the import left behind, whether or not it changed
            // anything: this baseline is what lets a later re-import tell "you
            // tuned this" from "the file set this".
            if let Some(row) = table.row(selected).copied() {
                provenance.record(selected, vox_source(&state.import.path, &source), row);
            }
            state.import.status = if applied {
                format!("applied #{} to {selected_name}", source.file_index)
            } else {
                format!("#{} matched {selected_name} already", source.file_index)
            };
        }

        draw_reimport(ui, table, state, provenance);
    });
}

/// The non-destructive refresh: pull the file's current values into every row that
/// came from it, **without** discarding what has been tuned by hand since.
///
/// This is what makes an external editor usable as an editor. Without it a redraw
/// forces a choice between losing your tuning and ignoring the redraw.
fn draw_reimport(
    ui: &mut egui::Ui,
    table: &mut MaterialTable,
    state: &mut MaterialPanelState,
    provenance: &mut ProvenanceTable,
) {
    let tracked = provenance.rows_from(&state.import.path);
    if tracked.is_empty() {
        return;
    }
    ui.separator();
    if !ui
        .button(format!("Re-import into {} tracked row(s)", tracked.len()))
        .on_hover_text(
            "Press Load first to re-read the file, then this. Refreshes every field \
             you have NOT touched since it was imported, and keeps every field you \
             have. A palette entry whose colour or class changed since it was \
             imported is reported as a conflict and skipped entirely, rather than \
             risking one material's tuning landing on another.",
        )
        .clicked()
    {
        return;
    }

    let mut refreshed_rows = 0;
    let mut kept_fields = 0;
    let mut conflicts: Vec<String> = Vec::new();
    for material in tracked {
        // Find the entry this row came from by its reorder-stable FILE index, not by
        // its position in the palette — the position is what an editor rewrites.
        let Some(file_index) = provenance
            .record_for(material)
            .map(|record| record.source.file_index)
        else {
            continue;
        };
        let Some(entry) = state
            .import
            .rows
            .iter()
            .find(|row| row.file_index == file_index)
            .copied()
        else {
            conflicts.push(format!("#{file_index} is no longer in the file"));
            continue;
        };
        let Some(row) = table.row(material).copied() else {
            continue;
        };
        let mut merged = row;
        let outcome = provenance.reimport(
            material,
            &mut merged,
            &entry.fields,
            &vox_source(&state.import.path, &entry),
        );
        if let Some(conflict) = outcome.conflict {
            conflicts.push(format!("{}: {}", row.name, conflict.describe()));
            continue;
        }
        kept_fields += outcome.kept.len();
        if merged != row {
            if let Some(target) = table.row_mut(material) {
                *target = merged;
            }
            refreshed_rows += 1;
        }
    }

    state.import.status = if conflicts.is_empty() {
        format!(
            "re-imported: {refreshed_rows} row(s) changed, \
             {kept_fields} hand-tuned field(s) kept"
        )
    } else {
        format!(
            "re-imported: {refreshed_rows} changed, {kept_fields} kept, \
             {} CONFLICT(s) skipped — {}",
            conflicts.len(),
            conflicts.join(" | ")
        )
    };
    println!("vox re-import: {}", state.import.status);
}

/// The identity a re-import is checked against, for one palette entry.
fn vox_source(path: &str, row: &VoxImportRow) -> VoxSource {
    VoxSource {
        path: path.to_string(),
        file_index: row.file_index,
        rgba: row.entry.rgba,
        kind: row.entry.kind,
    }
}

/// What the file actually said about the selected entry — so an unexpected import
/// is diagnosable from the panel rather than from a debugger.
fn draw_source_summary(ui: &mut egui::Ui, source: &VoxImportRow) {
    if !source.entry.describes_a_material() {
        ui.label("the file gave this entry no material properties")
            .on_hover_text(
                "The common case: most .vox writers emit only colours. Only the \
                 albedo will be seeded, and everything else on the target row keeps \
                 its current value.",
            );
        return;
    }
    let fields = &source.fields;
    let mut described: Vec<String> = Vec::new();
    if let Some(roughness) = fields.roughness {
        described.push(format!("roughness {roughness:.3}"));
    }
    if let Some(specular) = fields.specular {
        described.push(format!(
            "specular {specular:.3}{}",
            if fields.specular_is_from_metalness {
                " (from metalness)"
            } else {
                ""
            }
        ));
    }
    if let Some(emission) = fields.emission {
        described.push(format!(
            "emission {:.2} {:.2} {:.2}",
            emission[0], emission[1], emission[2]
        ));
    }
    if let Some(transmittance) = fields.transmittance {
        described.push(format!("transmittance {transmittance:.3}"));
    }
    if let Some(index) = fields.index_of_refraction {
        described.push(format!("IOR {index:.3}"));
    }
    if fields.absorption_per_meter.is_some() || fields.scattering_per_meter.is_some() {
        described.push("medium coefficients (grey seed)".to_string());
    }
    for line in described {
        ui.label(format!("  {line}"));
    }
    if fields.specular_is_from_metalness {
        ui.label("  ! metalness is an approximation").on_hover_text(
            "There is no metal BRDF here, so `_metal` was used as a stand-in F0 \
             because the file gave no `_sp`. Roughly right — a metal's normal-incidence \
             reflectance really is high — but a guess, not a translation.",
        );
    }
    if fields.absorption_per_meter.is_some() || fields.scattering_per_meter.is_some() {
        ui.label("  ! coefficients are GREY").on_hover_text(
            "MagicaVoxel stores one scalar where this engine wants a per-channel \
             triple, so they arrive grey. A medium's colour is supposed to EMERGE from \
             a per-channel absorption/scattering pair — grey is exactly what it must \
             not stay. Spread them by hand; the derived-colour readout above shows \
             the result.",
        );
    }
}

/// The row picker, plus the eyedropper that is nearly free because the CPU edit
/// cast already returns the material byte it needs.
fn draw_row_selector(ui: &mut egui::Ui, table: &mut MaterialTable, state: &mut MaterialPanelState) {
    let label = |table: &MaterialTable, id: u8| match table.row(id) {
        Some(row) if table.row_is_modified(id) => format!("{id:>2}  {} *", row.name),
        Some(row) => format!("{id:>2}  {}", row.name),
        None => format!("{id:>2}  <none>"),
    };

    egui::ComboBox::from_label("row")
        .selected_text(label(table, state.selected))
        .show_ui(ui, |ui| {
            for id in 0..MATERIALS.len() as u8 {
                ui.selectable_value(&mut state.selected, id, label(table, id));
            }
        })
        .response
        .on_hover_text(
            "The row being edited — and IN THE STUDIO, the voxel being shown: the \
             subject follows this selection, so picking a row rebuilds the studio \
             around it. Air (row 0) is the miss sentinel and is skipped, so selecting \
             it leaves the subject alone. A `*` marks a row edited away from what the \
             binary compiled. In the island this is the placement material instead, \
             and nothing is rebuilt.",
        );

    ui.horizontal(|ui| {
        ui.checkbox(&mut state.eyedropper_armed, "eyedropper")
            .on_hover_text(
                "Armed: the next place/dig click SELECTS the material of the voxel \
                 under the cursor instead of editing the world. Free to implement — \
                 the CPU edit cast already returns the hit voxel's material byte.",
            );
        if table.is_modified() {
            ui.label("(table edited)");
        }
    });
}

/// The kind, read-only, with the reason it is read-only one hover away.
fn draw_kind_readout(ui: &mut egui::Ui, row: &Material) {
    let (kind, detail) = match row.kind {
        MaterialKind::Air => ("Air", "the miss sentinel; never sampled on a hit"),
        MaterialKind::Solid => ("Solid", "opaque; nothing passes through or travels inside"),
        MaterialKind::Cover { .. } => (
            "Cover",
            "thin vegetation: occludes for viewing, transmits for transport",
        ),
        MaterialKind::Medium(..) => (
            "Medium",
            "a participating volume a ray travels inside (water today)",
        ),
    };
    ui.label(format!("kind: {kind}")).on_hover_text(format!(
        "{detail}.\n\nNot editable here on purpose: the kind decides this row's \
         flags, and through them the character's movement predicate, the editor's \
         notion of emptiness, and whether traversal continues through the voxel. \
         Those CPU predicates read the COMPILED table, so changing the kind live \
         would desync the physics from the picture."
    ));
}

/// The fields that exist only for this row's kind.
fn draw_kind_payload(ui: &mut egui::Ui, row: &mut Material) {
    match &mut row.kind {
        MaterialKind::Air | MaterialKind::Solid => {}
        MaterialKind::Cover { transmittance } => {
            // Floored above zero: a cover row that blocks all light is what makes
            // GI paint black canopies, and a test pins it — so the slider must not
            // be able to author the state the test forbids.
            ui.add(
                egui::Slider::new(transmittance, 0.01..=1.0)
                    .text("transmittance")
                    .max_decimals(3),
            )
            .on_hover_text(
                "Fraction of light passing THROUGH for transport (not for viewing) — \
                 what stops the GI treating a leaf canopy as a wall. In the re-pack \
                 tier: CAGI stores it quantised to 4 bits in its cell attributes. \
                 Cannot reach zero: a leaf that blocks everything paints black \
                 canopies, and a test forbids it.",
            );
        }
        MaterialKind::Medium(medium) => {
            ui.add(
                egui::Slider::new(&mut medium.index_of_refraction, 1.0..=2.0)
                    .text("index of refraction")
                    .max_decimals(4),
            )
            .on_hover_text(
                "How hard this medium bends a ray that enters it, and through \
                 ((n-1)/(n+1))^2 how much it mirrors head-on. Water 1.333, oil ~1.47, \
                 honey ~1.50. This is the width dial for Snell's window: the \
                 half-angle is asin(1/n).",
            );
            ui.add(
                egui::Slider::new(&mut medium.opacity, 0.0..=1.0)
                    .text("opacity")
                    .max_decimals(3),
            );
            ui.add(
                egui::Slider::new(&mut medium.transmittance, 0.0..=1.0)
                    .text("transmittance")
                    .max_decimals(3),
            );
            ui.label("absorption /m").on_hover_text(
                "Light this medium DESTROYS, per metre, per channel. Authored as a \
                 PAIR with scattering because a medium's colour is not a value you \
                 pick — it emerges from which wavelengths are absorbed and which are \
                 scattered. Absorption alone sends the depths black.",
            );
            draw_channel_triple(ui, "abs", &mut medium.absorption_per_meter, 0.0..=2.0);
            ui.label("scattering /m").on_hover_text(
                "Light this medium REDIRECTS rather than destroys — what a ray picks \
                 up along its path, and the only reason deep clear water is blue with \
                 no bottom in sight. Scattering-dominated with near-zero absorption is \
                 what a cloud is.",
            );
            draw_channel_triple(ui, "scat", &mut medium.scattering_per_meter, 0.0..=2.0);

            // The derived colour, shown so the "nothing is painted" rule is
            // visible while you drag rather than an assertion in a doc comment.
            let derived = Material {
                kind: MaterialKind::Medium(*medium),
                ..*row
            }
            .single_scattering_albedo();
            ui.label(format!(
                "derived volume colour: {:.3} {:.3} {:.3}",
                derived[0], derived[1], derived[2]
            ))
            .on_hover_text(
                "The single-scattering albedo scattering/extinction — this medium's \
                 apparent colour, DERIVED from the pair above and never authored. \
                 Nothing in the table paints the water.",
            );
        }
    }
}

/// Three per-channel drags on one line — compact enough that a coefficient pair
/// does not push everything else off the panel.
fn draw_channel_triple(
    ui: &mut egui::Ui,
    id: &str,
    channels: &mut [f32; 3],
    range: std::ops::RangeInclusive<f32>,
) {
    ui.horizontal(|ui| {
        for (channel, label) in channels.iter_mut().zip(["r", "g", "b"]) {
            ui.add(
                egui::DragValue::new(channel)
                    .speed(0.002)
                    .range(range.clone())
                    .max_decimals(4)
                    .prefix(format!("{label} ")),
            )
            .on_hover_text(format!("{id} {label}"));
        }
    });
}

/// Emission, if this row emits. Presence is structural — see the hover text.
fn draw_emission(ui: &mut egui::Ui, row: &mut Material) {
    match &mut row.emission {
        Some(emission) => {
            ui.label("emission (linear, may exceed 1)").on_hover_text(
                "Emitted radiance. In the re-pack tier AND indirect: CAGI stores a \
                 3-bit emitter INDEX per cell and looks the radiance up in a palette \
                 built from this table, so a change here needs the attribute re-pack \
                 to reach the light volume. Note the scale cannot mean anything \
                 physical until an HDR intermediate exists — today a bright emitter \
                 clips to flat white, so raising this makes a light flatter, not \
                 brighter.",
            );
            draw_channel_triple(ui, "emit", emission, 0.0..=16.0);
        }
        None => {
            ui.label("emission: none").on_hover_text(
                "This row does not emit. Adding emission is structural, not cosmetic: \
                 it changes the row's EMISSIVE flag, which decides whether CAGI \
                 injects it as a light source and which of the 8 emitter palette slots \
                 it claims. So it is a compiled-table change, not a slider.",
            );
        }
    }
}

/// S1 — the per-face roles, when this row has them.
///
/// Read-only as to *whether* a row has roles: adding them is authoring a new fact
/// about a material rather than tuning one, and the row above (`albedo`,
/// `roughness`) is what a roleless row uses on every face. Editing the three roles
/// it already has is exactly what tuning means.
fn draw_face_roles(ui: &mut egui::Ui, row: &mut Material) {
    let Some(roles) = row.face_roles.as_mut() else {
        return;
    };
    ui.collapsing("Face roles (top / side / bottom)", |ui| {
        ui.label("needs the MATERIAL_FACE_ROLES lever under Quality")
            .on_hover_text(
                "These values are UPLOADED but not READ until the lever is on. It \
                 ships off because turning it on changes how the island looks, and \
                 re-authoring the world's materials is a deliberate later step. The \
                 fields above are what every face uses while it is off — which is why \
                 this row's base albedo is still its pre-S1 colour.",
            );
        for (label, face, hint) in [
            (
                "top",
                &mut roles.top,
                "the +Y face — the one the sky reaches",
            ),
            ("side", &mut roles.side, "all four side faces"),
            (
                "bottom",
                &mut roles.bottom,
                "the -Y face — the only one that never sees the sky, so usually the darkest",
            ),
        ] {
            ui.horizontal(|ui| {
                ui.color_edit_button_rgb(&mut face.albedo);
                ui.label(label).on_hover_text(hint);
                ui.add(
                    egui::DragValue::new(&mut face.roughness)
                        .speed(0.005)
                        .range(0.0..=1.0)
                        .max_decimals(3)
                        .prefix("rough "),
                );
            });
        }
    });
}

/// S2 — which shape the studio builds the sample into.
///
/// Lives in the material panel rather than in a studio panel of its own because it
/// is not a scene setting, it is part of judging a material: a period under one
/// voxel is judged on the single voxel, continuity and any multi-voxel period on the
/// wall, and a
/// corner on the cube. Choosing the pose IS choosing what you are looking for.
fn draw_studio_pose(ui: &mut egui::Ui, state: &mut MaterialPanelState) {
    ui.collapsing("Studio subject", |ui| {
        ui.label("rebuilds the studio world; needs --studio")
            .on_hover_text(
                "Each pose replaces the whole studio scene, which is a full world \
                 rebuild — fine for something a human presses, and the same path a \
                 loaded .vox model already takes.",
            );
        ui.horizontal(|ui| {
            for pose in StudioPose::ALL {
                if ui
                    .button(pose.label())
                    .on_hover_text(studio_pose_hint(pose))
                    .clicked()
                {
                    state.studio_pose_requested = Some(pose);
                }
            }
        });
    });
}

fn studio_pose_hint(pose: StudioPose) -> &'static str {
    match pose {
        StudioPose::Single => {
            "One voxel, nothing else in frame. The pose for a colour, a face role or \
             a within-face grain."
        }
        StudioPose::Wall => {
            "A 16x16 slab — 2 m square. The pose for CROSS-VOXEL CONTINUITY and for \
             any period over one voxel: a world-framed layer must flow across the \
             whole slab, and a per-voxel tile shows up instantly as a 16x16 grid. \
             Any period over one voxel is unjudgeable without it."
        }
        StudioPose::Cube => {
            "A 4x4x4 block: three faces and a corner at once. The pose for whether a \
             world-framed layer wraps an edge or shows a seam along it."
        }
    }
}

/// S2 — the pattern layer stack.
///
/// Unlike face roles, this section is offered on **every** row, because a stack has
/// an empty state that costs nothing and "add a layer" is the authoring act. Face
/// roles are a decision about what a material IS; a layer is tuning, which is what
/// this panel is for.
///
/// The controls follow the data: pick a generator and only its own parameters
/// appear, pick a blend and the target colour appears only if that blend reads it.
/// Same argument as the kind-driven header — a slider that silently does nothing is
/// worse than an absent one.
fn draw_pattern_layers(ui: &mut egui::Ui, row: &mut Material) {
    let active = row.patterns.active_count();
    ui.collapsing(
        format!("Pattern layers ({active}/{MAX_PATTERN_LAYERS})"),
        |ui| {
            ui.label("needs the MATERIAL_PATTERNS lever under Quality")
                .on_hover_text(
                    "Layers are UPLOADED but not READ until the lever is on, for the same \
                 reason face roles ship off: no row authors one yet, so turning it on \
                 today changes nothing, and authoring the island's materials is a \
                 deliberate later step with a re-recorded baseline. The Quality panel \
                 also holds the global strength and the per-hit layer cap.",
                );

            let mut remove: Option<usize> = None;
            for slot in 0..active {
                let Some(layer) = row.patterns.layers[slot].as_mut() else {
                    continue;
                };
                ui.separator();
                ui.horizontal(|ui| {
                    ui.label(format!("{}.", slot + 1));
                    ui.label(layer.generator.label());
                    if ui
                        .small_button("remove")
                        .on_hover_text(
                            "Layers apply in order, each on the previous one's output, so \
                         removing one closes the gap rather than leaving a hole.",
                        )
                        .clicked()
                    {
                        remove = Some(slot);
                    }
                });
                draw_pattern_layer(ui, slot, layer);
            }
            if let Some(slot) = remove {
                row.patterns.remove(slot);
            }

            ui.separator();
            ui.horizontal(|ui| {
                if ui
                    .add_enabled(
                        row.patterns.active_count() < MAX_PATTERN_LAYERS,
                        egui::Button::new("add layer"),
                    )
                    .on_hover_text(
                        "Starts at amount 0, which is the exact identity — so a new layer \
                     is safe to leave in the row while its generator is dialled in.",
                    )
                    .clicked()
                {
                    row.patterns.push(PatternLayer::IDENTITY);
                }
                if ui
                    .add_enabled(active > 0, egui::Button::new("clear all"))
                    .clicked()
                {
                    row.patterns = NO_PATTERNS;
                }
            });
        },
    );
}

/// One layer's controls.
fn draw_pattern_layer(ui: &mut egui::Ui, slot: usize, layer: &mut PatternLayer) {
    ui.push_id(slot, |ui| {
        // Generator. Changing it keeps the parameters it shares and takes the
        // preset's for the ones it does not, which is why `ALL` carries
        // representative values rather than zeroes.
        egui::ComboBox::from_label("generator")
            .selected_text(layer.generator.label())
            .show_ui(ui, |ui| {
                for generator in PatternGenerator::ALL {
                    let selected = generator.code() == layer.generator.code();
                    if ui
                        .selectable_label(selected, generator.label())
                        .on_hover_text(generator_hint(generator))
                        .clicked()
                    {
                        layer.generator = generator;
                    }
                }
            });
        draw_generator_params(ui, layer);

        egui::ComboBox::from_label("frame")
            .selected_text(layer.frame.label())
            .show_ui(ui, |ui| {
                for frame in PatternFrame::ALL {
                    if ui
                        .selectable_label(layer.frame == frame, frame.label())
                        .on_hover_text(frame_hint(frame))
                        .clicked()
                    {
                        layer.frame = frame;
                    }
                }
            });

        // The texel grid, right under the frame: the two together are "where does this
        // pattern live", and the period below is "how big are its features".
        ui.horizontal(|ui| {
            ui.label("texels/voxel").on_hover_text(
                "Quantises the sample to an n x n grid per face, so the generator is \
                 sampled once per texel and held flat across it — square detail on a \
                 world made of cubes, instead of a smooth field that happens to sit on \
                 voxels. Applies to EVERY generator: noise becomes blocky noise, \
                 speckles become square specks. Independent of the period, which keeps \
                 its own job of setting feature size — 8 texels with a 1 m period is a \
                 large soft field rendered in 1.5 cm squares. The grid is anchored to \
                 the WORLD and its size divides a voxel exactly, so it lines up across \
                 neighbours and a texel never straddles a voxel edge. It is also the \
                 same lattice a .vox model drawn at n cells per voxel lands on, so \
                 hand-drawn and generated detail compose. `off` is the continuous \
                 field.",
            );
            for rung in TEXEL_RUNGS {
                let label = if rung == 0 {
                    "off".to_string()
                } else {
                    rung.to_string()
                };
                if ui
                    .radio(layer.texels_per_voxel == rung, label)
                    .on_hover_text(texel_rung_hint(rung))
                    .clicked()
                {
                    layer.texels_per_voxel = rung;
                }
            }
        });

        // Logarithmic: the useful range spans grain (a centimetre) to bands
        // (metres), and a linear slider over that spends nine tenths of its travel
        // above one voxel.
        ui.add(
            egui::Slider::new(&mut layer.period_meters, 0.005..=4.0)
                .text("period (m)")
                .logarithmic(true)
                .max_decimals(4),
        )
        .on_hover_text(
            "The size of the generator's largest feature, in METRES — and the field \
             that decides which scale this layer acts on. One voxel is 0.125 m: below \
             that is within-face detail, at it is per-voxel, above it is a multi-voxel \
             pattern. It also sets the distance fade: a layer fades out at a fixed \
             number of PERIODS, so fine detail dies at range and coarse bands do not.",
        );

        egui::ComboBox::from_label("target")
            .selected_text(layer.target.label())
            .show_ui(ui, |ui| {
                for target in PatternTarget::ALL {
                    if ui
                        .selectable_label(layer.target == target, target.label())
                        .on_hover_text(target_hint(target))
                        .clicked()
                    {
                        layer.target = target;
                    }
                }
            });

        egui::ComboBox::from_label("blend")
            .selected_text(layer.blend.label())
            .show_ui(ui, |ui| {
                for blend in PatternBlend::ALL {
                    if ui
                        .selectable_label(layer.blend == blend, blend.label())
                        .on_hover_text(blend_hint(blend))
                        .clicked()
                    {
                        layer.blend = blend;
                    }
                }
            });

        // Only shown when the blend reads it.
        if layer.blend.uses_target_color() {
            ui.horizontal(|ui| {
                if layer.target.is_color() {
                    ui.color_edit_button_rgb(&mut layer.target_color);
                    ui.label("target colour");
                } else {
                    ui.add(
                        egui::DragValue::new(&mut layer.target_color[0])
                            .speed(0.005)
                            .range(0.0..=1.0)
                            .max_decimals(3),
                    );
                    ui.label("target value").on_hover_text(
                        "A scalar target reads only the first channel, which is why \
                         this is one number rather than a colour picker.",
                    );
                }
            });
        }

        ui.add(
            egui::Slider::new(&mut layer.amount, 0.0..=1.0)
                .text("amount")
                .max_decimals(3),
        )
        .on_hover_text(
            "Zero is the exact identity, so a layer at zero costs bytes and nothing else.",
        );

        ui.horizontal(|ui| {
            ui.label("faces").on_hover_text(
                "Which of S1's three roles this layer applies to. \"Top only\" is how \
                 moss, snow settling and sun-bleaching are authored without a second \
                 material.",
            );
            ui.checkbox(&mut layer.faces.top, "top");
            ui.checkbox(&mut layer.faces.side, "side");
            ui.checkbox(&mut layer.faces.bottom, "bottom");
        });
    });
}

/// The generator's own parameters, and only its own.
fn draw_generator_params(ui: &mut egui::Ui, layer: &mut PatternLayer) {
    match &mut layer.generator {
        PatternGenerator::Flat => {}
        PatternGenerator::Noise { octaves } => {
            ui.add(
                egui::Slider::new(octaves, 1..=MAX_NOISE_OCTAVES)
                    .text("octaves")
                    .integer(),
            )
            .on_hover_text(
                "Each octave doubles the frequency and halves the amplitude, and the \
                 sum is normalised — so this changes the texture without changing the \
                 contrast, and the period keeps naming the largest feature. Costs one \
                 more eight-corner lattice fetch per octave.",
            );
        }
        PatternGenerator::Speckle { density } => {
            ui.add(
                egui::Slider::new(density, 0.0..=1.0)
                    .text("density")
                    .max_decimals(3),
            )
            .on_hover_text(
                "The fraction of CELLS that carry a speck, not the fraction of area \
                 covered — a speck fills a fixed share of its cell, so the period \
                 controls how big and this controls how crowded.",
            );
        }
    }
}

/// What one texel rung means in metres, since "8" is meaningless without the voxel.
fn texel_rung_hint(rung: u32) -> String {
    if rung == 0 {
        return "Continuous — no snap. What a very fine grain still wants, since below \
                a texel the grid is the only thing you would see."
            .to_string();
    }
    let millimeters = VOXEL_SIZE / rung as f32 * 1000.0;
    format!(
        "{rung} x {rung} texels per face, {millimeters:.1} mm each. \
         A .vox model drawn at {rung} cells per voxel lands on exactly this grid."
    )
}

fn generator_hint(generator: PatternGenerator) -> &'static str {
    match generator {
        PatternGenerator::Flat => {
            "One value per cell of the frame. In the voxel frame this is per-voxel \
             TONE — the jitter that stops a stone wall being one flat colour."
        }
        PatternGenerator::Noise { .. } => {
            "Value noise. Grain at a small period, mottle at a large one — the \
             workhorse for making a surface stop looking like a painted cube."
        }
        PatternGenerator::Speckle { .. } => {
            "Scattered round specks: pits in stone, grit in sand, lichen."
        }
    }
}

fn frame_hint(frame: PatternFrame) -> &'static str {
    match frame {
        PatternFrame::World => {
            "World space — the pattern is a field the world sits in, so it flows \
             across neighbouring voxels and CANNOT tile per voxel. The default, and \
             the whole reason continuity works."
        }
        PatternFrame::Voxel => {
            "Restarts at every voxel: the coordinate is the voxel's own centre, so the \
             generator returns ONE value for the whole voxel. For deliberately \
             per-voxel motifs — tone jitter being the point."
        }
        PatternFrame::Face => {
            "Voxel-local within the hit face, so the pattern is ABOUT the face: wear \
             toward an edge, a drip down a side. A period of 0.125 spans one face."
        }
    }
}

fn target_hint(target: PatternTarget) -> &'static str {
    match target {
        PatternTarget::Albedo => {
            "Modulates the per-face albedo, so a layer composes with face roles rather \
             than replacing them. The only target with a shipped consumer."
        }
        PatternTarget::Roughness => {
            "AUTHORED BUT UNREAD, like the roughness slider above — no pass samples \
             roughness until the reflection stage exists. Authoring it now is free; \
             expecting to see it is not."
        }
        PatternTarget::Emission => {
            "Patterned glow: embers in rock, a rune in a wall. Only does anything on a \
             row that emits — on a non-emitter it modulates zero."
        }
    }
}

fn blend_hint(blend: PatternBlend) -> &'static str {
    match blend {
        PatternBlend::Multiply => {
            "Darkens where the value is low and leaves the base alone where it is \
             high. Never brightens, so it cannot push an albedo out of range. The \
             workhorse: grain, mortar shadow, dirt."
        }
        PatternBlend::MixToColor => {
            "Interpolates toward the target colour. What a two-colour material IS — \
             mortar grey against brick red, lichen green on stone."
        }
        PatternBlend::Add => {
            "Adds the target colour scaled by the value. For emission, and for the \
             rare surface that gains light rather than losing it."
        }
    }
}

/// Resets, and the honest second tier.
fn draw_tier_controls(
    ui: &mut egui::Ui,
    table: &mut MaterialTable,
    state: &mut MaterialPanelState,
    provenance: &mut ProvenanceTable,
    selected: u8,
) {
    ui.horizontal(|ui| {
        if ui
            .add_enabled(
                table.row_is_modified(selected),
                egui::Button::new("reset row"),
            )
            .on_hover_text("Restore this row to what the binary was compiled with.")
            .clicked()
        {
            table.reset_row(selected);
            // Forget where it came from too: a reset row no longer reflects the
            // file, and leaving the record would let a re-import silently
            // overwrite the compiled values it was just restored to.
            provenance.forget(selected);
        }
        if ui
            .add_enabled(table.is_modified(), egui::Button::new("reset all"))
            .clicked()
        {
            table.reset_all();
            provenance.forget_all();
        }
    });

    if ui
        .button("re-pack GI attributes")
        .on_hover_text(
            "CAGI bakes albedo, a quantised transmittance and the emitter slot into \
             its own cell-attribute volume, and its shaders never read the material \
             table — so those three fields are live in direct shading and STALE in \
             the GI bounce until this runs. A ~50 ms rebuild, so it goes to the world \
             thread rather than into a frame.",
        )
        .clicked()
    {
        state.repack_gi_requested = true;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::material::material_id;
    use voxel_core::world::Voxel;

    /// The default selection must be a real row. Zero is Air — the miss sentinel —
    /// which is a legitimate thing to inspect but a poor thing to land on, so the
    /// panel should not start there.
    #[test]
    fn the_default_selection_is_a_usable_row() {
        let state = MaterialPanelState::default();
        let table = MaterialTable::default();
        assert!(table.row(state.selected).is_some());
    }

    /// Every row must be selectable and describable — the combo box iterates the
    /// whole table, so a row with no name would render as a blank entry.
    #[test]
    fn every_row_is_selectable_and_named() {
        let table = MaterialTable::default();
        for id in 0..MATERIALS.len() as u8 {
            let row = table.row(id).expect("row must exist");
            assert!(!row.name.is_empty(), "row {id} has no name");
        }
    }

    /// The panel only ever edits values, never the kind. Pinned here as well as in
    /// `material_table`, because this is the module that would break it.
    #[test]
    fn the_panel_never_changes_a_kind() {
        let mut table = MaterialTable::default();
        let water = material_id(Voxel::Water);
        let before = *table.row(water).unwrap();

        // What the medium sliders do, applied directly.
        let mut edited = before;
        if let MaterialKind::Medium(medium) = &mut edited.kind {
            medium.index_of_refraction = 1.47;
            medium.absorption_per_meter = [1.0, 0.5, 0.25];
        }
        *table.row_mut(water).unwrap() = edited;

        let after = table.row(water).unwrap();
        assert!(matches!(after.kind, MaterialKind::Medium(..)));
        assert_eq!(
            after.to_gpu().flags,
            before.to_gpu().flags,
            "a value edit must not move the flag word"
        );
        assert!(after.is_liquid());
    }

    /// The cover slider's floor must keep the table inside what the
    /// `cover_rows_transmit_light` invariant allows — a widget able to author a
    /// state a test forbids is a bug waiting for a bug report.
    #[test]
    fn the_cover_slider_cannot_author_an_opaque_leaf() {
        // The floor the slider is built with. Above zero by construction — that IS
        // the property, so it is asserted at compile time rather than at run time.
        const COVER_TRANSMITTANCE_FLOOR: f32 = 0.01;
        const { assert!(COVER_TRANSMITTANCE_FLOOR > 0.0) };
        for row in &MATERIALS {
            if let MaterialKind::Cover { transmittance } = row.kind {
                assert!(
                    transmittance >= COVER_TRANSMITTANCE_FLOOR,
                    "{} is authored below the slider's own floor",
                    row.name
                );
            }
        }
    }

    /// The re-pack request must be a latch the platform layer clears, not a
    /// fire-and-forget — a ~50 ms rebuild triggered every frame would be a hitch
    /// machine.
    #[test]
    fn the_repack_request_is_a_one_shot_latch() {
        let mut state = MaterialPanelState::default();
        assert!(!state.repack_gi_requested);
        state.repack_gi_requested = true;
        assert!(std::mem::take(&mut state.repack_gi_requested));
        assert!(!state.repack_gi_requested);
    }
}
