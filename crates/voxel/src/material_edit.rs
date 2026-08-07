//! S0 — the material panel: the authoring loop that did not exist.
//!
//! Before this, every one of the 27 rows was tuned by editing Rust and rebuilding,
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
//! offered 27 rows of controls that silently did nothing on 25 of them.
//!
//! ## The two tiers, stated rather than hidden
//!
//! [`voxel_rt::cagi`] bakes albedo and quantised transmittance into its packed cell
//! attributes, and E5b stores packed per-cell emission beside that word. Its shaders never
//! read the material binding. So an albedo or emission edit is instant in direct
//! shading and **stale in the GI bounce** until the attributes are re-packed. Rather than
//! pretend otherwise, the
//! panel labels which fields are in which tier and offers the re-pack explicitly —
//! it is a ~0.5 s rebuild that belongs off-frame on the world thread, not something
//! to run silently on every slider tick.
//!
//! ## Why kind is shown but not editable
//!
//! Kind decides [`voxel_material::material::MaterialFlags`], and through them the
//! character's movement predicate, the editor's notion of emptiness, and whether
//! traversal continues through the voxel. Those CPU predicates read the *compiled*
//! table on purpose (they are sampled per frame and must not depend on renderer
//! state), so a live kind change would desync the physics from the picture. Values
//! within a kind are what tuning actually needs.

use voxel_core::world::Voxel;
use voxel_rt::studio::StudioPose;
use voxel_rt::vox_material::VoxImportRow;

/// The nine quick-access blocks in the in-world test bar. The bar is for fast
/// construction; its "more" picker still exposes every material row.
pub const WORLD_HOTBAR_BLOCKS: [Voxel; 9] = [
    Voxel::Grass,
    Voxel::Dirt,
    Voxel::Sand,
    Voxel::Stone,
    Voxel::Trunk,
    Voxel::Leaves,
    Voxel::Snow,
    Voxel::GlowBlock,
    Voxel::Lava,
];

/// Temporary renderer inspection modes. These never modify the authored
/// material; they only change what the DDA pass displays for debugging.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialDebugView {
    Lit,
    BaseColor,
    Specular,
    AmbientOcclusion,
    Displacement,
    Roughness,
    Normal,
    Emission,
    Lod,
    /// The light volume's luminance at each surface, as a log heat ramp —
    /// black 0.008, red ~1, yellow ~16, white 64 (the storage ceiling).
    GiHeat,
    /// Where the volume's light FLOWS: hue = travel direction, saturation =
    /// anisotropy (banks6 only; isotropic reads white), brightness = magnitude.
    GiFlow,
}

impl Default for MaterialDebugView {
    fn default() -> Self {
        Self::Lit
    }
}

impl MaterialDebugView {
    pub const ALL: [Self; 11] = [
        Self::Lit,
        Self::BaseColor,
        Self::Specular,
        Self::AmbientOcclusion,
        Self::Displacement,
        Self::Roughness,
        Self::Normal,
        Self::Emission,
        Self::Lod,
        Self::GiHeat,
        Self::GiFlow,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Lit => "Lit material",
            Self::BaseColor => "Base color",
            Self::Specular => "Specular",
            Self::AmbientOcclusion => "Ambient occlusion",
            Self::Displacement => "Displacement / height",
            Self::Roughness => "Roughness",
            Self::Normal => "Normal",
            Self::Emission => "Emission",
            Self::Lod => "Pattern LOD",
            Self::GiHeat => "GI heat (light volume)",
            Self::GiFlow => "GI flow (light direction)",
        }
    }

    pub const fn shader_mode(self) -> u32 {
        match self {
            Self::Lit => 0,
            Self::BaseColor => 1,
            Self::Specular => 2,
            Self::AmbientOcclusion => 3,
            Self::Displacement => 4,
            Self::Roughness => 5,
            Self::Normal => 6,
            Self::Emission => 7,
            Self::Lod => 8,
            Self::GiHeat => 9,
            Self::GiFlow => 10,
        }
    }
}

/// The panel's own UI state — what is selected and what it has asked for.
#[derive(Clone, Debug, PartialEq)]
pub struct MaterialPanelState {
    /// Material id currently being edited.
    pub selected: u8,
    /// Temporary renderer inspection switch. Bit 0 is pattern layer 1, bit 1
    /// layer 2, and so on. Four bits are enough for the renderer's fixed stack.
    pub debug_layers_enabled: bool,
    pub debug_layer_mask: u8,
    pub debug_view: MaterialDebugView,
    /// The eyedropper is armed: the next world pick selects that voxel's row
    /// instead of editing the world.
    pub eyedropper_armed: bool,
    /// The user asked for a CAGI attribute re-pack (the second tier above).
    pub repack_gi_requested: bool,
    /// S0b — the `.vox` import panel's state.
    pub import: VoxImportState,
    /// S2 — the user picked a studio pose. A one-shot request rather than a stored
    /// pose, because servicing it means rebuilding the world, which the platform
    /// layer owns; the pose itself lives on [`voxel_rt::studio::StudioScene`].
    pub studio_pose_requested: Option<StudioPose>,
    /// S2 — the user asked for the studio's ground plate to be there or not. Also
    /// one-shot, and for the same reason: the plate is world geometry, so hiding it
    /// is a rebuild rather than a draw flag.
    pub studio_plate_requested: Option<bool>,
    /// S2 — the user picked what the studio's ground plate is made of. Separate from
    /// [`Self::selected`] on purpose: the floor is scenery, the selection is the row
    /// being EDITED, and the studio subject follows the latter.
    pub studio_plate_material_requested: Option<u8>,
}

impl Default for MaterialPanelState {
    fn default() -> Self {
        Self {
            selected: 0,
            debug_layers_enabled: false,
            debug_layer_mask: 0b1111,
            debug_view: MaterialDebugView::Lit,
            eyedropper_armed: false,
            repack_gi_requested: false,
            import: VoxImportState::default(),
            studio_pose_requested: None,
            studio_plate_requested: None,
            studio_plate_material_requested: None,
        }
    }
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
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The re-pack request must be a latch the platform layer clears, not a
    /// fire-and-forget — a ~0.5 s rebuild triggered every frame would be a hitch
    /// machine.
    ///
    /// Kept when the material panel was removed at `1613d75`: the latch itself is still live,
    /// set from two places in `main.rs`. The panel's other four tests went with the panel.
    #[test]
    fn the_repack_request_is_a_one_shot_latch() {
        let mut state = MaterialPanelState::default();
        assert!(!state.repack_gi_requested);
        state.repack_gi_requested = true;
        assert!(std::mem::take(&mut state.repack_gi_requested));
        assert!(!state.repack_gi_requested);
    }
}
