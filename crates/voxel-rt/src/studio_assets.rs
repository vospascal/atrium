//! Persistent Studio assets: project manifests, material overrides, and named
//! quality recipes.
//!
//! This is intentionally an asset layer, not renderer state. The editable JSON
//! files are canonical; [`MaterialTable`](crate::material_table::MaterialTable),
//! GPU rows, and pipelines remain rebuildable runtime caches. Keeping the module
//! free of `wgpu`, `winit`, and `egui` also makes every file operation testable
//! without a graphics device.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::path::{Component, Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

use crate::graph::{DiagnosticSeverity, GraphAsset, NodeRegistry};
use crate::material::{
    FaceOverride, FaceRoles, Material, MaterialKind, Medium, MediumPhase, MATERIAL_COUNT,
};
use crate::material_graph::graph_from_material;
use crate::material_table::MaterialTable;
use crate::pattern::{
    PatternBlend, PatternFaces, PatternFrame, PatternGenerator, PatternLayer, PatternStack,
    PatternTarget, MAX_PATTERN_LAYERS,
};
use crate::variants::{
    preset_spec, Lever, LeverRange, LeverValue, QualityPreset, RenderQuality, REGISTRY,
};
use crate::world_profile::{CompiledWorldProfile, WorldAssetCatalog, WorldProfileAsset};

/// The first on-disk Studio asset format. Future readers migrate old documents
/// before resolving them into runtime objects.
pub const STUDIO_ASSET_SCHEMA_VERSION: u32 = 2;

static NEXT_ASSET_ID: AtomicU64 = AtomicU64::new(1);

/// A durable asset identity. It is deliberately opaque: runtime material-table
/// indices are not safe project identities because a project may later reorder
/// or replace its material assignments.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetId(pub String);

impl AssetId {
    /// Create a locally unique, portable text identity without making UUIDs a
    /// rendering dependency. Asset files retain this value forever after first
    /// save, so uniqueness only matters at creation time.
    pub fn new() -> AssetId {
        let sequence = NEXT_ASSET_ID.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        AssetId(format!("vx-{nanos:032x}-{sequence:016x}"))
    }
}

impl Default for AssetId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

/// A project-local reference to one asset file. Paths are always relative to the
/// project directory; [`StudioProjectStore`] rejects absolute and parent paths.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct AssetReference {
    pub id: AssetId,
    pub path: PathBuf,
}

/// Project-level assignments and active selections. It contains references, not
/// copies of assets, so one material graph/asset can be reused later.
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct ProjectManifest {
    pub schema_version: u32,
    pub id: AssetId,
    pub name: String,
    /// Current voxel-material slots keyed as decimal strings. This is explicit
    /// during the fixed-27-row era; the values are still stable asset IDs.
    pub material_assignments: BTreeMap<String, AssetReference>,
    pub quality_recipes: Vec<AssetReference>,
    pub active_quality_recipe: Option<AssetId>,
    /// Reusable node-graph definitions. Graph instances remain owned by the
    /// specific material/geometry/quality backend in later phases.
    #[serde(default)]
    pub graph_assets: Vec<AssetReference>,
    /// Registry-led world composition root. This supersedes the old profile
    /// selection for new projects; graph assets are the canonical authoring
    /// surface for both World and Studio runtime modes.
    #[serde(default)]
    pub active_world_graph: Option<AssetId>,
    /// Reusable world-composition roots. Biomes, palettes, modifiers, features,
    /// audio, and animation are compiled from the active profile.
    pub world_profiles: Vec<AssetReference>,
    pub active_world_profile: Option<AssetId>,
    /// Non-graph runtime resources (currently audio clips) addressable by world
    /// profiles. The manifest owns their identity and path even though decoding
    /// belongs to the eventual audio runtime.
    pub runtime_assets: Vec<AssetReference>,
}

impl ProjectManifest {
    pub fn new(name: impl Into<String>) -> ProjectManifest {
        ProjectManifest {
            schema_version: STUDIO_ASSET_SCHEMA_VERSION,
            id: AssetId::new(),
            name: name.into(),
            material_assignments: BTreeMap::new(),
            quality_recipes: Vec::new(),
            active_quality_recipe: None,
            graph_assets: Vec::new(),
            active_world_graph: None,
            world_profiles: Vec::new(),
            active_world_profile: None,
            runtime_assets: Vec::new(),
        }
    }
}

/// A concrete material identity plus its structural renderer metadata. Visual
/// authoring is owned exclusively by the required canonical graph.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialAsset {
    pub schema_version: u32,
    pub id: AssetId,
    pub name: String,
    pub voxel_material_slot: u8,
    pub material: SavedMaterial,
    pub graph: AssetId,
}

impl MaterialAsset {
    pub fn from_material(slot: u8, material: &Material, graph: AssetId) -> MaterialAsset {
        MaterialAsset {
            schema_version: STUDIO_ASSET_SCHEMA_VERSION,
            id: AssetId::new(),
            name: material.name.to_string(),
            voxel_material_slot: slot,
            material: SavedMaterial::from(material),
            graph,
        }
    }

    /// Apply the saved authoring values to the live table. A material's display
    /// name remains the compiled/static name because the runtime `Material`
    /// currently stores it as `&'static str`; renaming becomes an asset-level
    /// feature when graph/material assets replace fixed table rows.
    pub fn apply_to_table(&self, table: &mut MaterialTable) -> Result<(), AssetError> {
        let Some(base) = table.row(self.voxel_material_slot).copied() else {
            return Err(AssetError::InvalidMaterialSlot(self.voxel_material_slot));
        };
        let restored = self.material.to_material(base)?;
        let target = table
            .row_mut(self.voxel_material_slot)
            .expect("a checked material slot remains valid");
        *target = restored;
        Ok(())
    }
}

/// Serializable material data. It intentionally excludes runtime GPU packing and
/// the static display-name pointer in [`Material`].
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedMaterial {
    pub albedo: [f32; 3],
    pub roughness: f32,
    pub specular: f32,
    pub kind: SavedMaterialKind,
    pub emission: Option<[f32; 3]>,
    pub face_roles: Option<SavedFaceRoles>,
    pub patterns: Vec<SavedPatternLayer>,
    pub acoustic_alpha: [f32; 6],
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedMaterialKind {
    Air,
    Solid,
    Cover { transmittance: f32 },
    Medium(SavedMedium),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedMedium {
    pub phase: SavedMediumPhase,
    pub index_of_refraction: f32,
    pub absorption_per_meter: [f32; 3],
    pub scattering_per_meter: [f32; 3],
    pub opacity: f32,
    pub transmittance: f32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedMediumPhase {
    Liquid,
    Gas,
    Solid,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedFaceRoles {
    pub top: SavedFaceOverride,
    pub side: SavedFaceOverride,
    pub bottom: SavedFaceOverride,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedFaceOverride {
    pub albedo: [f32; 3],
    pub roughness: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedPatternLayer {
    pub generator: SavedPatternGenerator,
    pub frame: SavedPatternFrame,
    pub period_meters: f32,
    pub target: SavedPatternTarget,
    pub blend: SavedPatternBlend,
    pub amount: f32,
    pub target_color: [f32; 3],
    pub faces: SavedPatternFaces,
    pub texels_per_voxel: u32,
    pub vary_per_face: bool,
    pub emission_intensity: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum SavedPatternGenerator {
    Flat,
    Noise { octaves: u32 },
    Speckle { density: f32 },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedPatternFrame {
    World,
    Voxel,
    Face,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedPatternTarget {
    Albedo,
    Roughness,
    Emission,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SavedPatternBlend {
    Multiply,
    MixToColor,
    Add,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SavedPatternFaces {
    pub top: bool,
    pub side: bool,
    pub bottom: bool,
}

impl From<&Material> for SavedMaterial {
    fn from(material: &Material) -> Self {
        SavedMaterial {
            albedo: material.albedo,
            roughness: material.roughness,
            specular: material.specular,
            kind: SavedMaterialKind::from(material.kind),
            emission: material.emission,
            face_roles: material.face_roles.map(SavedFaceRoles::from),
            patterns: material
                .patterns
                .active()
                .map(SavedPatternLayer::from)
                .collect(),
            acoustic_alpha: material.acoustic_alpha,
        }
    }
}

impl SavedMaterial {
    fn to_material(&self, base: Material) -> Result<Material, AssetError> {
        if self.patterns.len() > MAX_PATTERN_LAYERS {
            return Err(AssetError::InvalidMaterial(format!(
                "{} pattern layers exceed the {}-layer limit",
                self.patterns.len(),
                MAX_PATTERN_LAYERS
            )));
        }
        validate_finite("albedo", &self.albedo)?;
        validate_finite("roughness", &[self.roughness])?;
        validate_finite("specular", &[self.specular])?;
        validate_finite("acoustic_alpha", &self.acoustic_alpha)?;
        if let Some(emission) = self.emission {
            validate_finite("emission", &emission)?;
        }

        let mut patterns = PatternStack {
            layers: [None; MAX_PATTERN_LAYERS],
        };
        for saved_layer in &self.patterns {
            assert!(
                patterns.push(saved_layer.to_pattern_layer()?).is_none(),
                "checked layer count leaves a slot"
            );
        }
        Ok(Material {
            name: base.name,
            albedo: self.albedo,
            roughness: self.roughness,
            specular: self.specular,
            kind: self.kind.to_material_kind()?,
            emission: self.emission,
            face_roles: self
                .face_roles
                .as_ref()
                .map(SavedFaceRoles::to_face_roles)
                .transpose()?,
            patterns,
            acoustic_alpha: self.acoustic_alpha,
        })
    }
}

impl From<MaterialKind> for SavedMaterialKind {
    fn from(kind: MaterialKind) -> Self {
        match kind {
            MaterialKind::Air => SavedMaterialKind::Air,
            MaterialKind::Solid => SavedMaterialKind::Solid,
            MaterialKind::Cover { transmittance } => SavedMaterialKind::Cover { transmittance },
            MaterialKind::Medium(medium) => SavedMaterialKind::Medium(SavedMedium::from(medium)),
        }
    }
}

impl SavedMaterialKind {
    fn to_material_kind(&self) -> Result<MaterialKind, AssetError> {
        match self {
            SavedMaterialKind::Air => Ok(MaterialKind::Air),
            SavedMaterialKind::Solid => Ok(MaterialKind::Solid),
            SavedMaterialKind::Cover { transmittance } => {
                validate_finite("cover transmittance", &[*transmittance])?;
                Ok(MaterialKind::Cover {
                    transmittance: *transmittance,
                })
            }
            SavedMaterialKind::Medium(medium) => Ok(MaterialKind::Medium(medium.to_medium()?)),
        }
    }
}

impl From<Medium> for SavedMedium {
    fn from(medium: Medium) -> Self {
        SavedMedium {
            phase: SavedMediumPhase::from(medium.phase),
            index_of_refraction: medium.index_of_refraction,
            absorption_per_meter: medium.absorption_per_meter,
            scattering_per_meter: medium.scattering_per_meter,
            opacity: medium.opacity,
            transmittance: medium.transmittance,
        }
    }
}

impl SavedMedium {
    fn to_medium(&self) -> Result<Medium, AssetError> {
        validate_finite("medium index_of_refraction", &[self.index_of_refraction])?;
        validate_finite("medium absorption_per_meter", &self.absorption_per_meter)?;
        validate_finite("medium scattering_per_meter", &self.scattering_per_meter)?;
        validate_finite("medium opacity", &[self.opacity])?;
        validate_finite("medium transmittance", &[self.transmittance])?;
        Ok(Medium {
            phase: self.phase.into(),
            index_of_refraction: self.index_of_refraction,
            absorption_per_meter: self.absorption_per_meter,
            scattering_per_meter: self.scattering_per_meter,
            opacity: self.opacity,
            transmittance: self.transmittance,
        })
    }
}

impl From<MediumPhase> for SavedMediumPhase {
    fn from(phase: MediumPhase) -> Self {
        match phase {
            MediumPhase::Liquid => SavedMediumPhase::Liquid,
            MediumPhase::Gas => SavedMediumPhase::Gas,
            MediumPhase::Solid => SavedMediumPhase::Solid,
        }
    }
}

impl From<SavedMediumPhase> for MediumPhase {
    fn from(phase: SavedMediumPhase) -> Self {
        match phase {
            SavedMediumPhase::Liquid => MediumPhase::Liquid,
            SavedMediumPhase::Gas => MediumPhase::Gas,
            SavedMediumPhase::Solid => MediumPhase::Solid,
        }
    }
}

impl From<FaceRoles> for SavedFaceRoles {
    fn from(roles: FaceRoles) -> Self {
        SavedFaceRoles {
            top: SavedFaceOverride::from(roles.top),
            side: SavedFaceOverride::from(roles.side),
            bottom: SavedFaceOverride::from(roles.bottom),
        }
    }
}

impl From<FaceOverride> for SavedFaceOverride {
    fn from(override_value: FaceOverride) -> Self {
        SavedFaceOverride {
            albedo: override_value.albedo,
            roughness: override_value.roughness,
        }
    }
}

impl SavedFaceRoles {
    fn to_face_roles(&self) -> Result<FaceRoles, AssetError> {
        Ok(FaceRoles {
            top: self.top.to_face_override("top")?,
            side: self.side.to_face_override("side")?,
            bottom: self.bottom.to_face_override("bottom")?,
        })
    }
}

impl SavedFaceOverride {
    fn to_face_override(&self, name: &str) -> Result<FaceOverride, AssetError> {
        validate_finite(&format!("{name} face albedo"), &self.albedo)?;
        validate_finite(&format!("{name} face roughness"), &[self.roughness])?;
        Ok(FaceOverride {
            albedo: self.albedo,
            roughness: self.roughness,
        })
    }
}

impl From<&PatternLayer> for SavedPatternLayer {
    fn from(layer: &PatternLayer) -> Self {
        SavedPatternLayer {
            generator: SavedPatternGenerator::from(layer.generator),
            frame: layer.frame.into(),
            period_meters: layer.period_meters,
            target: layer.target.into(),
            blend: layer.blend.into(),
            amount: layer.amount,
            target_color: layer.target_color,
            faces: SavedPatternFaces::from(layer.faces),
            texels_per_voxel: layer.texels_per_voxel,
            vary_per_face: layer.vary_per_face,
            emission_intensity: layer.emission_intensity,
        }
    }
}

impl SavedPatternLayer {
    fn to_pattern_layer(&self) -> Result<PatternLayer, AssetError> {
        validate_finite("pattern period_meters", &[self.period_meters])?;
        validate_finite("pattern amount", &[self.amount])?;
        validate_finite("pattern target_color", &self.target_color)?;
        validate_finite("pattern emission_intensity", &[self.emission_intensity])?;
        if let SavedPatternGenerator::Speckle { density } = self.generator {
            validate_finite("pattern speckle density", &[density])?;
        }
        Ok(PatternLayer {
            generator: self.generator.clone().into(),
            frame: self.frame.into(),
            period_meters: self.period_meters,
            target: self.target.into(),
            blend: self.blend.into(),
            amount: self.amount,
            target_color: self.target_color,
            faces: self.faces.clone().into(),
            texels_per_voxel: self.texels_per_voxel,
            vary_per_face: self.vary_per_face,
            emission_intensity: self.emission_intensity,
        })
    }
}

impl From<PatternGenerator> for SavedPatternGenerator {
    fn from(generator: PatternGenerator) -> Self {
        match generator {
            PatternGenerator::Flat => SavedPatternGenerator::Flat,
            PatternGenerator::Noise { octaves } => SavedPatternGenerator::Noise { octaves },
            PatternGenerator::Speckle { density } => SavedPatternGenerator::Speckle { density },
        }
    }
}

impl From<SavedPatternGenerator> for PatternGenerator {
    fn from(generator: SavedPatternGenerator) -> Self {
        match generator {
            SavedPatternGenerator::Flat => PatternGenerator::Flat,
            SavedPatternGenerator::Noise { octaves } => PatternGenerator::Noise { octaves },
            SavedPatternGenerator::Speckle { density } => PatternGenerator::Speckle { density },
        }
    }
}

macro_rules! persisted_enum {
    ($saved:ident, $runtime:ident, { $($variant:ident),+ $(,)? }) => {
        impl From<$runtime> for $saved {
            fn from(value: $runtime) -> Self {
                match value { $( $runtime::$variant => $saved::$variant, )+ }
            }
        }
        impl From<$saved> for $runtime {
            fn from(value: $saved) -> Self {
                match value { $( $saved::$variant => $runtime::$variant, )+ }
            }
        }
    };
}

persisted_enum!(SavedPatternFrame, PatternFrame, { World, Voxel, Face });
persisted_enum!(SavedPatternTarget, PatternTarget, { Albedo, Roughness, Emission });
persisted_enum!(SavedPatternBlend, PatternBlend, { Multiply, MixToColor, Add });

impl From<PatternFaces> for SavedPatternFaces {
    fn from(faces: PatternFaces) -> Self {
        SavedPatternFaces {
            top: faces.top,
            side: faces.side,
            bottom: faces.bottom,
        }
    }
}

impl From<SavedPatternFaces> for PatternFaces {
    fn from(faces: SavedPatternFaces) -> Self {
        PatternFaces {
            top: faces.top,
            side: faces.side,
            bottom: faces.bottom,
        }
    }
}

/// A named persisted snapshot of every registered quality lever. The exact
/// current preset is included for display, but all values are saved so a custom
/// recipe is independent of later preset-table changes.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct QualityRecipeAsset {
    pub schema_version: u32,
    pub id: AssetId,
    pub name: String,
    pub preset: String,
    pub values: BTreeMap<String, SavedLeverValue>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum SavedLeverValue {
    Flag(bool),
    Mode(u32),
    Count(u32),
    VoxelDistance(u32),
    Scalar(f32),
}

impl From<LeverValue> for SavedLeverValue {
    fn from(value: LeverValue) -> Self {
        match value {
            LeverValue::Flag(value) => SavedLeverValue::Flag(value),
            LeverValue::Mode(value) => SavedLeverValue::Mode(value),
            LeverValue::Count(value) => SavedLeverValue::Count(value),
            LeverValue::VoxelDistance(value) => SavedLeverValue::VoxelDistance(value),
            LeverValue::Scalar(value) => SavedLeverValue::Scalar(value),
        }
    }
}

impl From<SavedLeverValue> for LeverValue {
    fn from(value: SavedLeverValue) -> Self {
        match value {
            SavedLeverValue::Flag(value) => LeverValue::Flag(value),
            SavedLeverValue::Mode(value) => LeverValue::Mode(value),
            SavedLeverValue::Count(value) => LeverValue::Count(value),
            SavedLeverValue::VoxelDistance(value) => LeverValue::VoxelDistance(value),
            SavedLeverValue::Scalar(value) => LeverValue::Scalar(value),
        }
    }
}

impl QualityRecipeAsset {
    pub fn from_quality(name: impl Into<String>, quality: &RenderQuality) -> QualityRecipeAsset {
        let values = REGISTRY
            .iter()
            .map(|lever| {
                (
                    lever_key(lever),
                    SavedLeverValue::from(lever.id.read(quality)),
                )
            })
            .collect();
        QualityRecipeAsset {
            schema_version: STUDIO_ASSET_SCHEMA_VERSION,
            id: AssetId::new(),
            name: name.into(),
            preset: format!("{:?}", quality.preset),
            values,
        }
    }

    /// Restore known levers and leave forward-compatible unknown entries alone.
    /// The returned names are diagnostics suitable for a Studio asset panel.
    pub fn apply_to_quality(&self, quality: &mut RenderQuality) -> Result<Vec<String>, AssetError> {
        *quality = preset_from_name(&self.preset)
            .map(|preset| preset_spec(preset).resolve())
            .unwrap_or_else(RenderQuality::baseline);
        let mut warnings = Vec::new();
        for (key, saved_value) in &self.values {
            let Some(lever) = REGISTRY.iter().find(|lever| lever_key(lever) == *key) else {
                warnings.push(format!("unknown saved quality lever `{key}` was ignored"));
                continue;
            };
            let value: LeverValue = (*saved_value).into();
            validate_lever_value(lever, value)?;
            lever.id.apply(quality, value);
        }
        quality.preset = match preset_from_name(&self.preset) {
            Some(preset) if !quality.knobs_differ(&preset_spec(preset).resolve()) => preset,
            _ => QualityPreset::Custom,
        };
        Ok(warnings)
    }
}

/// Root-bound asset I/O. All writes are deterministic JSON written to a flushed
/// adjacent temporary file and renamed only after success.
#[derive(Clone, Debug)]
pub struct StudioProjectStore {
    root: PathBuf,
}

impl StudioProjectStore {
    pub fn new(root: impl Into<PathBuf>) -> StudioProjectStore {
        StudioProjectStore { root: root.into() }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn create_layout(&self) -> Result<(), AssetError> {
        fs::create_dir_all(self.root.join("materials"))?;
        fs::create_dir_all(self.root.join("quality"))?;
        fs::create_dir_all(self.root.join("graphs"))?;
        fs::create_dir_all(self.root.join("world"))?;
        fs::create_dir_all(self.root.join(".autosave"))?;
        Ok(())
    }

    pub fn save_manifest(&self, manifest: &ProjectManifest) -> Result<(), AssetError> {
        self.write_json(Path::new("project.vxproject.json"), manifest)
    }

    pub fn load_manifest(&self) -> Result<ProjectManifest, AssetError> {
        let manifest: ProjectManifest = self.read_json(Path::new("project.vxproject.json"))?;
        validate_schema(manifest.schema_version)?;
        Ok(manifest)
    }

    pub fn save_material(&self, path: &Path, material: &MaterialAsset) -> Result<(), AssetError> {
        self.write_json(path, material)
    }

    pub fn load_material(&self, path: &Path) -> Result<MaterialAsset, AssetError> {
        let material: MaterialAsset = self.read_json(path)?;
        validate_schema(material.schema_version)?;
        Ok(material)
    }

    pub fn save_quality(
        &self,
        path: &Path,
        quality: &QualityRecipeAsset,
    ) -> Result<(), AssetError> {
        self.write_json(path, quality)
    }

    pub fn load_quality(&self, path: &Path) -> Result<QualityRecipeAsset, AssetError> {
        let quality: QualityRecipeAsset = self.read_json(path)?;
        validate_schema(quality.schema_version)?;
        Ok(quality)
    }

    pub fn save_graph(&self, path: &Path, graph: &GraphAsset) -> Result<(), AssetError> {
        self.write_json(path, graph)
    }

    pub fn load_graph(&self, path: &Path) -> Result<GraphAsset, AssetError> {
        let graph: GraphAsset = self.read_json(path)?;
        validate_schema(graph.schema_version)?;
        Ok(graph)
    }

    pub fn save_world_profile(
        &self,
        path: &Path,
        profile: &WorldProfileAsset,
    ) -> Result<(), AssetError> {
        self.write_json(path, profile)
    }

    pub fn load_world_profile(&self, path: &Path) -> Result<WorldProfileAsset, AssetError> {
        let profile: WorldProfileAsset = self.read_json(path)?;
        validate_schema(profile.schema_version)?;
        Ok(profile)
    }

    /// A complete pre-commit image written before a multi-file project save.
    /// It is deliberately separate from normal assets: if the process stops
    /// between asset files, the next launch can restore one coherent state.
    pub fn save_recovery(&self, snapshot: &RecoverySnapshot) -> Result<(), AssetError> {
        self.write_json(Path::new(".autosave/recovery.vxrecovery.json"), snapshot)
    }

    pub fn load_recovery(&self) -> Result<Option<RecoverySnapshot>, AssetError> {
        match self.read_json::<RecoverySnapshot>(Path::new(".autosave/recovery.vxrecovery.json")) {
            Ok(snapshot) => {
                validate_schema(snapshot.schema_version)?;
                Ok(Some(snapshot))
            }
            Err(AssetError::Io(error)) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }

    pub fn clear_recovery(&self) -> Result<(), AssetError> {
        let path = self.resolve(Path::new(".autosave/recovery.vxrecovery.json"))?;
        match fs::remove_file(path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(AssetError::Io(error)),
        }
    }

    fn write_json<T: Serialize>(&self, relative_path: &Path, value: &T) -> Result<(), AssetError> {
        let path = self.resolve(relative_path)?;
        let bytes = serde_json::to_vec_pretty(value)?;
        atomic_write(&path, &bytes)?;
        Ok(())
    }

    fn read_json<T: for<'de> Deserialize<'de>>(
        &self,
        relative_path: &Path,
    ) -> Result<T, AssetError> {
        let path = self.resolve(relative_path)?;
        let bytes = fs::read(path)?;
        Ok(serde_json::from_slice(&bytes)?)
    }

    fn resolve(&self, relative_path: &Path) -> Result<PathBuf, AssetError> {
        if relative_path.as_os_str().is_empty()
            || relative_path.is_absolute()
            || relative_path.components().any(|component| {
                matches!(
                    component,
                    Component::ParentDir | Component::RootDir | Component::Prefix(_)
                )
            })
        {
            return Err(AssetError::UnsafePath(relative_path.to_path_buf()));
        }
        Ok(self.root.join(relative_path))
    }
}

/// The in-memory project record that owns stable asset IDs while the Studio is
/// running. It is small and renderer-independent, so the app can save/load it
/// around the live material table without making GPU objects serializable.
#[derive(Clone, Debug)]
pub struct StudioProject {
    pub manifest: ProjectManifest,
}

/// The all-or-nothing record used only while committing a project save. It is
/// also the exact payload offered by the recovery UI after an interrupted save.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct RecoverySnapshot {
    pub schema_version: u32,
    pub manifest: ProjectManifest,
    pub materials: Vec<MaterialAsset>,
    pub quality: QualityRecipeAsset,
}

/// The small UI-to-platform request bridge for saving and loading. Keeping it
/// as plain data lets the overlay request file work without performing I/O on a
/// frame-render closure.
#[derive(Clone, Debug)]
pub struct StudioAssetPanelState {
    pub project_path: String,
    pub quality_name: String,
    pub status: String,
    pub save_requested: bool,
    pub load_requested: bool,
    pub autosave_enabled: bool,
    pub restore_recovery_requested: bool,
    pub discard_recovery_requested: bool,
    pub recovery_available: bool,
}

impl StudioAssetPanelState {
    pub fn new(project_path: impl Into<String>) -> StudioAssetPanelState {
        StudioAssetPanelState {
            project_path: project_path.into(),
            quality_name: "Active quality".to_string(),
            status: "Unsaved project".to_string(),
            save_requested: false,
            load_requested: false,
            autosave_enabled: false,
            restore_recovery_requested: false,
            discard_recovery_requested: false,
            recovery_available: false,
        }
    }
}

impl StudioProject {
    pub fn new(name: impl Into<String>) -> StudioProject {
        StudioProject {
            manifest: ProjectManifest::new(name),
        }
    }

    /// Save or update one reusable graph definition and register it in the
    /// project manifest. The graph file is independent of material instances,
    /// so several slots can reference the same definition later.
    pub fn save_graph_asset(
        &mut self,
        store: &StudioProjectStore,
        path: impl Into<PathBuf>,
        graph: &GraphAsset,
    ) -> Result<(), AssetError> {
        store.create_layout()?;
        let path = path.into();
        store.save_graph(&path, graph)?;
        self.manifest
            .graph_assets
            .retain(|reference| reference.id != graph.id && reference.path != path);
        self.manifest.graph_assets.push(AssetReference {
            id: graph.id.clone(),
            path,
        });
        store.save_manifest(&self.manifest)
    }

    /// Load all graph definitions registered by this project, rejecting a
    /// mismatched identity before any caller can activate a compiled program.
    pub fn load_graph_assets(
        &self,
        store: &StudioProjectStore,
    ) -> Result<BTreeMap<AssetId, GraphAsset>, AssetError> {
        let mut graphs = BTreeMap::new();
        for reference in &self.manifest.graph_assets {
            let graph = store.load_graph(&reference.path)?;
            if graph.id != reference.id {
                return Err(AssetError::InvalidGraph(format!(
                    "graph asset `{}` does not match its manifest identity",
                    reference.path.display()
                )));
            }
            graphs.insert(graph.id.clone(), graph);
        }
        Ok(graphs)
    }

    /// Resolve the project's one canonical world composition graph. Unlike the
    /// retired profile path this is an ordinary registered graph asset, so the
    /// same identity/validation rules apply to materials and worlds.
    pub fn load_active_world_graph(
        &self,
        store: &StudioProjectStore,
    ) -> Result<Option<GraphAsset>, AssetError> {
        let Some(active) = &self.manifest.active_world_graph else {
            return Ok(None);
        };
        let reference = self
            .manifest
            .graph_assets
            .iter()
            .find(|reference| &reference.id == active)
            .ok_or_else(|| {
                AssetError::InvalidGraph(format!(
                    "active world graph `{active}` is absent from the manifest"
                ))
            })?;
        let graph = store.load_graph(&reference.path)?;
        if graph.id != reference.id {
            return Err(AssetError::InvalidGraph(format!(
                "world graph `{}` does not match its manifest identity",
                reference.path.display()
            )));
        }
        if graph.kind != crate::graph::GraphKind::World {
            return Err(AssetError::InvalidGraph(format!(
                "active graph `{active}` is not a world graph"
            )));
        }
        Ok(Some(graph))
    }

    /// Resolve every project-local identity required by world compilation.
    /// This is deliberately project-owned: a world profile cannot prove that
    /// its material, graph, or sound IDs exist without the manifest and store.
    pub fn world_asset_catalog(
        &self,
        store: &StudioProjectStore,
    ) -> Result<WorldAssetCatalog, AssetError> {
        let mut material_slots = BTreeMap::new();
        for (slot_key, reference) in &self.manifest.material_assignments {
            let slot = slot_key.parse::<u8>().map_err(|_| {
                AssetError::InvalidMaterial(format!(
                    "material assignment key `{slot_key}` is not a u8"
                ))
            })?;
            let material = store.load_material(&reference.path)?;
            if material.id != reference.id || material.voxel_material_slot != slot {
                return Err(AssetError::InvalidMaterial(format!(
                    "material asset `{}` does not match assignment {slot}",
                    reference.path.display()
                )));
            }
            if material_slots.insert(reference.id.clone(), slot).is_some() {
                return Err(AssetError::InvalidMaterial(format!(
                    "material asset `{}` is assigned more than once",
                    reference.id
                )));
            }
        }

        let registry = NodeRegistry::builtin();
        let graph_kinds: BTreeMap<AssetId, crate::graph::GraphKind> = BTreeMap::new();
        for (id, graph) in self.load_graph_assets(store)? {
            let errors: Vec<_> = graph
                .resolve(&registry)
                .diagnostics
                .into_iter()
                .filter(|diagnostic| diagnostic.severity == DiagnosticSeverity::Error)
                .collect();
            if !errors.is_empty() {
                return Err(AssetError::InvalidGraph(format!(
                    "graph `{id}` has {} schema error(s)",
                    errors.len()
                )));
            }
            // A world profile catalog contains executable handles, not merely
            // parseable graph files. Material graphs are compiled by the
            // material service and are never world-effect handles. The other
            // backend compilers are intentionally not advertised until their
            // execution modules exist; profiles referencing them fail closed.
            // Registration point for typed world-effect backend compilers.
            // Keeping raw graph IDs out is safer than claiming they can run.
            let _unregistered_world_graph = (id, graph.kind);
        }

        let mut runtime_assets = BTreeSet::new();
        for reference in &self.manifest.runtime_assets {
            let path = store.resolve(&reference.path)?;
            if !path.is_file() {
                return Err(AssetError::Io(io::Error::new(
                    io::ErrorKind::NotFound,
                    format!("runtime asset `{}` does not exist", path.display()),
                )));
            }
            if !runtime_assets.insert(reference.id.clone()) {
                return Err(AssetError::InvalidGraph(format!(
                    "runtime asset `{}` is registered more than once",
                    reference.id
                )));
            }
        }
        Ok(WorldAssetCatalog {
            material_slots,
            graph_kinds,
            runtime_assets,
        })
    }

    /// Save and activate one complete world-composition profile. Referenced
    /// material and graph assets remain independent and reusable.
    pub fn save_world_profile_asset(
        &mut self,
        store: &StudioProjectStore,
        path: impl Into<PathBuf>,
        profile: &WorldProfileAsset,
    ) -> Result<(), AssetError> {
        // Reject broken cross-domain references before an authored profile can
        // become the project's active composition root.
        let catalog = self.world_asset_catalog(store)?;
        profile.clone().compile(&catalog).map_err(|error| {
            AssetError::InvalidGraph(format!("world profile failed validation: {error}"))
        })?;
        store.create_layout()?;
        let path = path.into();
        store.save_world_profile(&path, profile)?;
        self.manifest
            .world_profiles
            .retain(|reference| reference.id != profile.id && reference.path != path);
        self.manifest.world_profiles.push(AssetReference {
            id: profile.id.clone(),
            path,
        });
        self.manifest.active_world_profile = Some(profile.id.clone());
        store.save_manifest(&self.manifest)
    }

    pub fn load_active_world_profile(
        &self,
        store: &StudioProjectStore,
    ) -> Result<Option<WorldProfileAsset>, AssetError> {
        let Some(active) = &self.manifest.active_world_profile else {
            return Ok(None);
        };
        let reference = self
            .manifest
            .world_profiles
            .iter()
            .find(|reference| &reference.id == active)
            .ok_or_else(|| {
                AssetError::InvalidGraph(format!(
                    "active world profile `{active}` is absent from the manifest"
                ))
            })?;
        let profile = store.load_world_profile(&reference.path)?;
        if profile.id != reference.id {
            return Err(AssetError::InvalidGraph(format!(
                "world profile `{}` does not match its manifest identity",
                reference.path.display()
            )));
        }
        Ok(Some(profile))
    }

    pub fn compile_active_world_profile(
        &self,
        store: &StudioProjectStore,
    ) -> Result<Option<CompiledWorldProfile>, AssetError> {
        let Some(profile) = self.load_active_world_profile(store)? else {
            return Ok(None);
        };
        let catalog = self.world_asset_catalog(store)?;
        profile.compile(&catalog).map(Some).map_err(|error| {
            AssetError::InvalidGraph(format!("world profile failed validation: {error}"))
        })
    }

    /// Save every current material row and one named quality recipe. Existing
    /// asset IDs are retained, so repeated saves update files instead of creating
    /// a new identity every time. The manifest is committed last.
    pub fn save_live_state(
        &mut self,
        store: &StudioProjectStore,
        quality_name: impl Into<String>,
        table: &MaterialTable,
        quality: &RenderQuality,
    ) -> Result<(), AssetError> {
        store.create_layout()?;
        let mut manifest = self.manifest.clone();
        let mut materials = Vec::with_capacity(MATERIAL_COUNT);
        for slot in 0..MATERIAL_COUNT {
            let slot = slot as u8;
            let row = table
                .row(slot)
                .expect("MATERIAL_COUNT and the live table have the same rows");
            let path = material_asset_path(slot, row.name);
            let id = manifest
                .material_assignments
                .get(&slot.to_string())
                .map(|reference| reference.id.clone())
                .unwrap_or_default();
            let graph = match manifest.material_assignments.get(&slot.to_string()) {
                Some(reference) => store.load_material(&reference.path)?.graph,
                None => {
                    let graph = graph_from_material(row);
                    let path = PathBuf::from(format!("graphs/material-{slot:02}.vgraph.json"));
                    // New projects bootstrap canonical graphs before the recovery
                    // journal. A failure here leaves at most an unreferenced file;
                    // no committed manifest or material can point at half a graph.
                    store.save_graph(&path, &graph)?;
                    manifest.graph_assets.push(AssetReference {
                        id: graph.id.clone(),
                        path,
                    });
                    graph.id
                }
            };
            let mut material = MaterialAsset::from_material(slot, row, graph);
            material.id = id.clone();
            materials.push(material);
            manifest
                .material_assignments
                .insert(slot.to_string(), AssetReference { id, path });
        }

        let quality_path = PathBuf::from("quality/active.vquality.json");
        let id = manifest
            .active_quality_recipe
            .as_ref()
            .and_then(|active_id| {
                self.manifest
                    .quality_recipes
                    .iter()
                    .find(|reference| &reference.id == active_id)
            })
            .map(|reference| reference.id.clone())
            .unwrap_or_default();
        let mut recipe = QualityRecipeAsset::from_quality(quality_name, quality);
        recipe.id = id.clone();
        manifest.quality_recipes = vec![AssetReference {
            id: id.clone(),
            path: quality_path,
        }];
        manifest.active_quality_recipe = Some(id);
        let recovery = RecoverySnapshot {
            schema_version: STUDIO_ASSET_SCHEMA_VERSION,
            manifest: manifest.clone(),
            materials,
            quality: recipe,
        };

        // Write the complete candidate before touching normal project files.
        // Any later error leaves this journal behind for a coherent recovery.
        store.save_recovery(&recovery)?;
        for material in &recovery.materials {
            let reference = manifest
                .material_assignments
                .get(&material.voxel_material_slot.to_string())
                .expect("every saved material has an assignment");
            store.save_material(&reference.path, material)?;
        }
        let quality_reference = manifest
            .quality_recipes
            .first()
            .expect("a live-state save always has one quality recipe");
        store.save_quality(&quality_reference.path, &recovery.quality)?;
        store.save_manifest(&manifest)?;
        self.manifest = manifest;
        // The normal project is already committed. If cleanup fails, retain the
        // safe recovery journal but do not lose the stable IDs just committed.
        store.clear_recovery()
    }

    /// Load saved materials into the live table, then restore the active quality
    /// recipe. Missing optional material files are reported instead of crashing
    /// the renderer; invalid assets fail before changing the affected row.
    pub fn load_live_state(
        store: &StudioProjectStore,
        table: &mut MaterialTable,
        quality: &mut RenderQuality,
    ) -> Result<(StudioProject, Vec<String>), AssetError> {
        let manifest = store.load_manifest()?;
        let mut warnings = Vec::new();
        for (slot_key, reference) in &manifest.material_assignments {
            let slot = slot_key.parse::<u8>().map_err(|_| {
                AssetError::InvalidMaterial(format!(
                    "material assignment key `{slot_key}` is not a u8"
                ))
            })?;
            let material = store.load_material(&reference.path)?;
            if material.id != reference.id {
                return Err(AssetError::InvalidMaterial(format!(
                    "material asset `{}` does not match its manifest identity",
                    reference.path.display()
                )));
            }
            if material.voxel_material_slot != slot {
                return Err(AssetError::InvalidMaterial(format!(
                    "material asset `{}` is assigned to slot {slot}, but stores slot {}",
                    reference.path.display(),
                    material.voxel_material_slot
                )));
            }
            material.apply_to_table(table)?;
        }
        if let Some(active_id) = &manifest.active_quality_recipe {
            let Some(reference) = manifest
                .quality_recipes
                .iter()
                .find(|reference| &reference.id == active_id)
            else {
                return Err(AssetError::InvalidQuality(
                    "active quality recipe is missing from the project manifest".to_string(),
                ));
            };
            let recipe = store.load_quality(&reference.path)?;
            if recipe.id != reference.id {
                return Err(AssetError::InvalidQuality(format!(
                    "quality asset `{}` does not match its manifest identity",
                    reference.path.display()
                )));
            }
            warnings.extend(recipe.apply_to_quality(quality)?);
        } else if !manifest.quality_recipes.is_empty() {
            warnings.push("project has saved quality recipes but no active recipe".to_string());
        }
        Ok((StudioProject { manifest }, warnings))
    }
}

impl RecoverySnapshot {
    /// Apply this coherent snapshot to copies of live state before the caller
    /// commits them, preserving the renderer's last-known-good state on error.
    pub fn apply_to_live(
        &self,
        table: &mut MaterialTable,
        quality: &mut RenderQuality,
    ) -> Result<Vec<String>, AssetError> {
        validate_schema(self.schema_version)?;
        for material in &self.materials {
            let Some(reference) = self
                .manifest
                .material_assignments
                .get(&material.voxel_material_slot.to_string())
            else {
                return Err(AssetError::InvalidMaterial(format!(
                    "recovery material slot {} has no manifest assignment",
                    material.voxel_material_slot
                )));
            };
            if reference.id != material.id {
                return Err(AssetError::InvalidMaterial(format!(
                    "recovery material slot {} does not match its manifest identity",
                    material.voxel_material_slot
                )));
            }
            material.apply_to_table(table)?;
        }
        self.quality.apply_to_quality(quality)
    }
}

/// Stable content fingerprint for debounce-based autosave. Asset IDs and names
/// are deliberately excluded: this answers only whether authored live values
/// have changed since the last successful save.
pub fn live_state_fingerprint(
    table: &MaterialTable,
    quality: &RenderQuality,
) -> Result<u64, AssetError> {
    #[derive(Serialize)]
    struct Fingerprint {
        materials: Vec<SavedMaterial>,
        quality: BTreeMap<String, SavedLeverValue>,
        preset: String,
    }
    let mut values = BTreeMap::new();
    for lever in REGISTRY {
        values.insert(lever_key(lever), lever.id.read(quality).into());
    }
    let state = Fingerprint {
        materials: table.rows().iter().map(SavedMaterial::from).collect(),
        quality: values,
        preset: format!("{:?}", quality.preset),
    };
    let bytes = serde_json::to_vec(&state)?;
    Ok(bytes.iter().fold(0xcbf29ce484222325_u64, |hash, byte| {
        (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
    }))
}

#[derive(Debug)]
pub enum AssetError {
    Io(io::Error),
    Json(serde_json::Error),
    UnsafePath(PathBuf),
    UnsupportedSchema { found: u32, supported: u32 },
    InvalidMaterialSlot(u8),
    InvalidMaterial(String),
    InvalidQuality(String),
    InvalidGraph(String),
}

impl fmt::Display for AssetError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            AssetError::Io(error) => write!(formatter, "asset I/O failed: {error}"),
            AssetError::Json(error) => write!(formatter, "asset JSON is invalid: {error}"),
            AssetError::UnsafePath(path) => {
                write!(formatter, "asset path is unsafe: {}", path.display())
            }
            AssetError::UnsupportedSchema { found, supported } => {
                write!(
                    formatter,
                    "asset schema {found} is newer than supported schema {supported}"
                )
            }
            AssetError::InvalidMaterialSlot(slot) => {
                write!(formatter, "material slot {slot} is invalid")
            }
            AssetError::InvalidMaterial(message) => {
                write!(formatter, "material asset is invalid: {message}")
            }
            AssetError::InvalidQuality(message) => {
                write!(formatter, "quality asset is invalid: {message}")
            }
            AssetError::InvalidGraph(message) => {
                write!(formatter, "graph asset is invalid: {message}")
            }
        }
    }
}

impl std::error::Error for AssetError {}

impl From<io::Error> for AssetError {
    fn from(error: io::Error) -> Self {
        AssetError::Io(error)
    }
}

impl From<serde_json::Error> for AssetError {
    fn from(error: serde_json::Error) -> Self {
        AssetError::Json(error)
    }
}

fn atomic_write(path: &Path, contents: &[u8]) -> Result<(), AssetError> {
    let parent = path
        .parent()
        .ok_or_else(|| AssetError::UnsafePath(path.to_path_buf()))?;
    fs::create_dir_all(parent)?;
    let sequence = NEXT_ASSET_ID.fetch_add(1, Ordering::Relaxed);
    let temporary_path = parent.join(format!(
        ".{}.{}.tmp",
        path.file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("asset"),
        sequence
    ));
    let result = (|| -> Result<(), io::Error> {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temporary_path)?;
        file.write_all(contents)?;
        file.sync_all()?;
        fs::rename(&temporary_path, path)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary_path);
    }
    result.map_err(AssetError::Io)
}

fn validate_schema(found: u32) -> Result<(), AssetError> {
    if found > STUDIO_ASSET_SCHEMA_VERSION {
        return Err(AssetError::UnsupportedSchema {
            found,
            supported: STUDIO_ASSET_SCHEMA_VERSION,
        });
    }
    Ok(())
}

fn material_asset_path(slot: u8, name: &str) -> PathBuf {
    let slug: String = name
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() {
                character.to_ascii_lowercase()
            } else {
                '-'
            }
        })
        .collect();
    let slug = slug.trim_matches('-');
    PathBuf::from(format!(
        "materials/{slot:02}-{}.vmat.json",
        if slug.is_empty() { "material" } else { slug }
    ))
}

fn lever_key(lever: &Lever) -> String {
    // `LeverId` is exhaustive inside variants.rs. The key has an asset-schema
    // migration boundary, so an enum rename is intentional and test-visible.
    format!("{:?}", lever.id)
}

fn preset_from_name(name: &str) -> Option<QualityPreset> {
    match name {
        "Potato" => Some(QualityPreset::Potato),
        "Quest" => Some(QualityPreset::Quest),
        "Balanced" => Some(QualityPreset::Balanced),
        "Beautiful" => Some(QualityPreset::Beautiful),
        "Custom" => Some(QualityPreset::Custom),
        _ => None,
    }
}

fn validate_lever_value(lever: &Lever, value: LeverValue) -> Result<(), AssetError> {
    if std::mem::discriminant(&lever.default_value) != std::mem::discriminant(&value) {
        return Err(AssetError::InvalidQuality(format!(
            "lever `{}` has the wrong saved value type",
            lever_key(lever)
        )));
    }
    match (lever.range, value) {
        (
            LeverRange::Continuous {
                minimum, maximum, ..
            },
            LeverValue::Scalar(value),
        ) if !value.is_finite() || value < minimum || value > maximum => {
            Err(AssetError::InvalidQuality(format!(
                "lever `{}` value {value} is outside {minimum}..={maximum}",
                lever_key(lever)
            )))
        }
        (LeverRange::Meters { minimum, maximum }, LeverValue::VoxelDistance(value))
            if (value as f32) / crate::variants::VOXELS_PER_METER < minimum
                || (value as f32) / crate::variants::VOXELS_PER_METER > maximum =>
        {
            Err(AssetError::InvalidQuality(format!(
                "lever `{}` voxel distance {value} is outside its saved range",
                lever_key(lever)
            )))
        }
        (LeverRange::Rungs(rungs), LeverValue::Count(value)) if !rungs.contains(&value) => {
            Err(AssetError::InvalidQuality(format!(
                "lever `{}` count {value} is not an allowed rung",
                lever_key(lever)
            )))
        }
        (_, LeverValue::Mode(value))
            if !lever.mode_options.is_empty()
                && !lever
                    .mode_options
                    .iter()
                    .any(|option| option.value == value) =>
        {
            Err(AssetError::InvalidQuality(format!(
                "lever `{}` mode {value} is not supported",
                lever_key(lever)
            )))
        }
        _ => Ok(()),
    }
}

fn validate_finite(label: &str, values: &[f32]) -> Result<(), AssetError> {
    if values.iter().all(|value| value.is_finite()) {
        Ok(())
    } else {
        Err(AssetError::InvalidMaterial(format!(
            "{label} contains a non-finite number"
        )))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeSet;

    use super::*;
    use crate::biome::{
        BiomeDefinition, BiomeId, BiomeRegistry, BiomeSelector, MaterialPaletteId, MaterialRole,
        SurfaceProfileId,
    };
    use crate::graph::GraphKind;
    use crate::material::{material_id, MATERIALS};
    use crate::variants::{LeverId, LeverValue};
    use crate::world_profile::{
        MaterialBinding, MaterialChoice, MaterialPalette, SurfaceLayer, SurfaceProfile,
    };

    fn temporary_project_root(label: &str) -> PathBuf {
        let root = std::env::temp_dir().join(format!("voxel-rt-{label}-{}", AssetId::new()));
        fs::create_dir_all(&root).unwrap();
        root
    }

    fn minimal_world_profile() -> WorldProfileAsset {
        let profile_id = SurfaceProfileId::new("ground");
        let palette_id = MaterialPaletteId::new("default");
        let role = MaterialRole::new("ground.surface");
        WorldProfileAsset {
            schema_version: STUDIO_ASSET_SCHEMA_VERSION,
            id: AssetId("world-profile".into()),
            name: "World".into(),
            biomes: BiomeRegistry {
                biomes: vec![BiomeDefinition {
                    id: BiomeId::new("world"),
                    name: "World".into(),
                    selector: BiomeSelector {
                        constraints: Vec::new(),
                        priority: 0,
                    },
                    traits: BTreeSet::new(),
                    surface_profile: profile_id.clone(),
                    material_palette: palette_id.clone(),
                    feature_sets: Vec::new(),
                    audio_profile: None,
                    animation_profile: None,
                }],
            },
            surface_profiles: vec![SurfaceProfile {
                id: profile_id,
                base_layers: BTreeMap::from([(SurfaceLayer::Surface, role.clone())]),
                rules: Vec::new(),
                modifiers: Vec::new(),
            }],
            material_palettes: vec![MaterialPalette {
                id: palette_id,
                bindings: BTreeMap::from([(
                    role,
                    MaterialBinding {
                        choices: vec![MaterialChoice {
                            material: AssetId("material".into()),
                            weight: 1.0,
                        }],
                        traits: BTreeSet::new(),
                    },
                )]),
            }],
            modifiers: Vec::new(),
            feature_sets: Vec::new(),
            audio_profiles: Vec::new(),
            animation_profiles: Vec::new(),
        }
    }

    #[test]
    fn material_asset_round_trips_every_authored_field() {
        let lava_slot = material_id(voxel_core::world::Voxel::Lava);
        let source = MaterialAsset::from_material(
            lava_slot,
            &MATERIALS[lava_slot as usize],
            AssetId("graph-lava".into()),
        );
        let encoded = serde_json::to_vec_pretty(&source).unwrap();
        let decoded: MaterialAsset = serde_json::from_slice(&encoded).unwrap();
        assert_eq!(decoded, source);

        let mut table = MaterialTable::default();
        decoded.apply_to_table(&mut table).unwrap();
        assert_eq!(
            table.row(lava_slot).unwrap(),
            &MATERIALS[lava_slot as usize]
        );
    }

    #[test]
    fn material_asset_rejects_non_finite_values_before_touching_the_table() {
        let stone_slot = material_id(voxel_core::world::Voxel::Stone);
        let mut asset = MaterialAsset::from_material(
            stone_slot,
            &MATERIALS[stone_slot as usize],
            AssetId("graph-stone".into()),
        );
        asset.material.roughness = f32::NAN;
        let mut table = MaterialTable::default();
        let before = table.row(stone_slot).copied().unwrap();
        assert!(matches!(
            asset.apply_to_table(&mut table),
            Err(AssetError::InvalidMaterial(_))
        ));
        assert_eq!(table.row(stone_slot), Some(&before));
        assert!(table.take_dirty().is_none());
    }

    #[test]
    fn quality_recipe_round_trips_every_registered_lever() {
        let mut original = RenderQuality::default();
        LeverId::AoStrength.apply(&mut original, LeverValue::Scalar(0.42));
        LeverId::RenderScale.apply(&mut original, LeverValue::Scalar(0.8));
        original.preset = QualityPreset::Custom;
        let asset = QualityRecipeAsset::from_quality("desktop tuned", &original);
        assert_eq!(asset.values.len(), REGISTRY.len());

        let mut restored = RenderQuality::default();
        let warnings = asset.apply_to_quality(&mut restored).unwrap();
        assert!(warnings.is_empty());
        assert_eq!(restored, original);
    }

    #[test]
    fn unknown_future_quality_levers_survive_loading_as_a_warning() {
        let mut asset = QualityRecipeAsset::from_quality("future", &RenderQuality::default());
        asset.values.insert(
            "FutureAmazingLever".to_string(),
            SavedLeverValue::Flag(true),
        );
        let mut quality = RenderQuality::default();
        let warnings = asset.apply_to_quality(&mut quality).unwrap();
        assert_eq!(
            warnings,
            ["unknown saved quality lever `FutureAmazingLever` was ignored"]
        );
    }

    #[test]
    fn store_round_trips_project_material_and_quality_assets() {
        let root = temporary_project_root("assets");
        let store = StudioProjectStore::new(&root);
        store.create_layout().unwrap();

        let stone_slot = material_id(voxel_core::world::Voxel::Stone);
        let material = MaterialAsset::from_material(
            stone_slot,
            &MATERIALS[stone_slot as usize],
            AssetId("graph-stone".into()),
        );
        let quality = QualityRecipeAsset::from_quality("balanced", &RenderQuality::default());
        let mut manifest = ProjectManifest::new("test world");
        manifest.material_assignments.insert(
            stone_slot.to_string(),
            AssetReference {
                id: material.id.clone(),
                path: PathBuf::from("materials/stone.vmat.json"),
            },
        );
        manifest.quality_recipes.push(AssetReference {
            id: quality.id.clone(),
            path: PathBuf::from("quality/balanced.vquality.json"),
        });
        manifest.active_quality_recipe = Some(quality.id.clone());

        store
            .save_material(Path::new("materials/stone.vmat.json"), &material)
            .unwrap();
        store
            .save_quality(Path::new("quality/balanced.vquality.json"), &quality)
            .unwrap();
        store.save_manifest(&manifest).unwrap();

        assert_eq!(
            store
                .load_material(Path::new("materials/stone.vmat.json"))
                .unwrap(),
            material
        );
        assert_eq!(
            store
                .load_quality(Path::new("quality/balanced.vquality.json"))
                .unwrap(),
            quality
        );
        assert_eq!(store.load_manifest().unwrap(), manifest);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn store_round_trips_graph_assets_under_the_project_root() {
        let root = temporary_project_root("graph-assets");
        let store = StudioProjectStore::new(&root);
        let graph = GraphAsset::new("weathered stone", GraphKind::Material);
        store
            .save_graph(
                Path::new("graphs/materials/weathered-stone.vgraph.json"),
                &graph,
            )
            .unwrap();
        assert_eq!(
            store
                .load_graph(Path::new("graphs/materials/weathered-stone.vgraph.json"))
                .unwrap(),
            graph
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_manifest_registers_and_loads_graph_assets() {
        let root = temporary_project_root("graph-manifest");
        let store = StudioProjectStore::new(&root);
        let mut project = StudioProject::new("graph project");
        let graph = GraphAsset::new("material graph", GraphKind::Material);
        project
            .save_graph_asset(&store, "graphs/material.vgraph.json", &graph)
            .unwrap();
        let loaded = project.load_graph_assets(&store).unwrap();
        assert_eq!(loaded.get(&graph.id), Some(&graph));
        assert_eq!(project.manifest.graph_assets.len(), 1);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_persists_validates_and_loads_the_active_world_profile() {
        let root = temporary_project_root("world-profile");
        let store = StudioProjectStore::new(&root);
        let mut project = StudioProject::new("world project");
        let slot = material_id(voxel_core::world::Voxel::Grass);
        let mut material = MaterialAsset::from_material(
            slot,
            &MATERIALS[slot as usize],
            AssetId("graph-grass".into()),
        );
        material.id = AssetId("material".into());
        let material_path = PathBuf::from("materials/grass.vmat.json");
        store.create_layout().unwrap();
        store.save_material(&material_path, &material).unwrap();
        project.manifest.material_assignments.insert(
            slot.to_string(),
            AssetReference {
                id: material.id.clone(),
                path: material_path,
            },
        );
        let profile = minimal_world_profile();
        project
            .save_world_profile_asset(&store, "world/active.vworld.json", &profile)
            .unwrap();
        assert_eq!(
            project.manifest.active_world_profile,
            Some(profile.id.clone())
        );
        assert_eq!(
            project.load_active_world_profile(&store).unwrap(),
            Some(profile)
        );
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn checked_in_project_and_world_profile_are_current_and_compilable() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../studio-project");
        let store = StudioProjectStore::new(root);
        let project = StudioProject {
            manifest: store.load_manifest().unwrap(),
        };
        project
            .compile_active_world_profile(&store)
            .unwrap()
            .expect("checked-in project has a compilable active world profile");
    }

    #[test]
    fn saving_live_materials_keeps_existing_graph_references() {
        let root = temporary_project_root("graph-reference-preservation");
        let store = StudioProjectStore::new(&root);
        let mut project = StudioProject::new("graph reference project");
        let stone_slot = material_id(voxel_core::world::Voxel::Stone);
        let graph = GraphAsset::new("stone graph", GraphKind::Material);
        let material_path = PathBuf::from("materials/06-stone.vmat.json");
        let material = MaterialAsset::from_material(
            stone_slot,
            &MATERIALS[stone_slot as usize],
            graph.id.clone(),
        );
        let mut referenced = material.clone();
        referenced.graph = graph.id.clone();
        project.manifest.material_assignments.insert(
            stone_slot.to_string(),
            AssetReference {
                id: material.id.clone(),
                path: material_path.clone(),
            },
        );
        store.create_layout().unwrap();
        store.save_material(&material_path, &referenced).unwrap();
        project
            .save_graph_asset(&store, "graphs/stone.vgraph.json", &graph)
            .unwrap();

        project
            .save_live_state(
                &store,
                "active",
                &MaterialTable::default(),
                &RenderQuality::default(),
            )
            .unwrap();

        let saved = store.load_material(&material_path).unwrap();
        assert_eq!(saved.graph, graph.id);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn project_snapshot_restores_live_materials_and_quality_with_stable_ids() {
        let root = temporary_project_root("live-state");
        let store = StudioProjectStore::new(&root);
        let mut project = StudioProject::new("saved studio");
        let stone_slot = material_id(voxel_core::world::Voxel::Stone);
        let mut table = MaterialTable::default();
        table.row_mut(stone_slot).unwrap().roughness = 0.17;
        let mut quality = RenderQuality::default();
        LeverId::RenderScale.apply(&mut quality, LeverValue::Scalar(0.8));
        quality.preset = QualityPreset::Custom;

        project
            .save_live_state(&store, "my tuned quality", &table, &quality)
            .unwrap();
        let initial_material_id = project
            .manifest
            .material_assignments
            .get(&stone_slot.to_string())
            .unwrap()
            .id
            .clone();

        table.row_mut(stone_slot).unwrap().roughness = 0.91;
        project
            .save_live_state(&store, "my tuned quality", &table, &quality)
            .unwrap();
        assert_eq!(
            project
                .manifest
                .material_assignments
                .get(&stone_slot.to_string())
                .unwrap()
                .id,
            initial_material_id
        );

        let mut restored_table = MaterialTable::default();
        let mut restored_quality = RenderQuality::default();
        let (loaded, warnings) =
            StudioProject::load_live_state(&store, &mut restored_table, &mut restored_quality)
                .unwrap();
        assert!(warnings.is_empty());
        assert_eq!(restored_table.row(stone_slot).unwrap().roughness, 0.91);
        assert_eq!(restored_quality, quality);
        assert_eq!(loaded.manifest, project.manifest);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn interrupted_project_save_leaves_a_complete_recovery_snapshot() {
        let root = temporary_project_root("recovery");
        let store = StudioProjectStore::new(&root);
        let mut project = StudioProject::new("interrupted studio");
        let stone_slot = material_id(voxel_core::world::Voxel::Stone);
        let mut table = MaterialTable::default();
        table.row_mut(stone_slot).unwrap().roughness = 0.31;
        let mut quality = RenderQuality::default();
        LeverId::RenderScale.apply(&mut quality, LeverValue::Scalar(0.8));
        quality.preset = QualityPreset::Custom;

        // A directory at the manifest path makes the final commit fail after
        // the journal and asset files have been written.
        store.create_layout().unwrap();
        fs::create_dir(root.join("project.vxproject.json")).unwrap();
        assert!(project
            .save_live_state(&store, "interrupted", &table, &quality)
            .is_err());

        let snapshot = store.load_recovery().unwrap().expect("recovery journal");
        let mut restored_table = MaterialTable::default();
        let mut restored_quality = RenderQuality::default();
        assert!(snapshot
            .apply_to_live(&mut restored_table, &mut restored_quality)
            .unwrap()
            .is_empty());
        assert_eq!(restored_table.row(stone_slot).unwrap().roughness, 0.31);
        assert_eq!(restored_quality, quality);
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn successful_project_save_clears_the_recovery_journal() {
        let root = temporary_project_root("recovery-clear");
        let store = StudioProjectStore::new(&root);
        let mut project = StudioProject::new("saved studio");
        project
            .save_live_state(
                &store,
                "quality",
                &MaterialTable::default(),
                &RenderQuality::default(),
            )
            .unwrap();
        assert!(store.load_recovery().unwrap().is_none());
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn store_rejects_paths_that_escape_the_project() {
        let root = temporary_project_root("path");
        let store = StudioProjectStore::new(&root);
        let result = store.save_manifest(&ProjectManifest::new("unsafe"));
        assert!(result.is_ok());
        let quality = QualityRecipeAsset::from_quality("quality", &RenderQuality::default());
        assert!(matches!(
            store.save_quality(Path::new("../outside.json"), &quality),
            Err(AssetError::UnsafePath(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }
}
