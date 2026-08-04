//! Staged world-profile authoring and compilation.
//!
//! This module is the composition root between otherwise independent domains:
//! biomes classify environment data, surface profiles assign semantic material
//! roles, palettes bind roles to intrinsic materials, feature sets request
//! physical generation, and presentation/runtime profiles add modifiers,
//! animation, and sound. Materials never depend on concrete biome IDs.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::biome::{
    AnimationProfileId, AudioProfileId, BiomeDefinition, BiomeError, BiomeId, BiomeRegistry,
    BiomeSample, FeatureId, FeatureSetId, MaterialPaletteId, MaterialRole, ModifierId, RuleId,
    SurfaceProfileId,
};
use crate::environment::{
    EnvironmentChannel, EnvironmentContext, EvaluationClass, RuntimeEnvironmentState, Season,
    SpatialGranularity, UpdateFrequency, WeatherKind,
};
use crate::graph::GraphKind;
use voxel_graph::{AssetId, STUDIO_ASSET_SCHEMA_VERSION};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SurfaceLayer {
    Surface,
    Subsoil,
    Deep,
    Cliff,
    WaterBed,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MaterialTrait {
    Exposed,
    Porous,
    Organic,
    Mineral,
    Granular,
    Wettable,
    Snowable,
    Emissive,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum ScalarSource {
    Environment(EnvironmentChannel),
    PositionX,
    PositionY,
    PositionZ,
    NormalY,
    SeasonPhase,
    DayPhase,
    WeatherIntensity,
    BiomeWeight(BiomeId),
    StableRandom { salt: u64 },
}

impl ScalarSource {
    fn value(&self, environment: &EnvironmentContext, biome: &BiomeSample) -> f32 {
        match self {
            Self::Environment(channel) => environment.field(channel),
            Self::PositionX => environment.position[0],
            Self::PositionY => environment.position[1],
            Self::PositionZ => environment.position[2],
            Self::NormalY => environment.normal[1],
            Self::SeasonPhase => environment.season_phase,
            Self::DayPhase => environment.day_phase,
            Self::WeatherIntensity => environment.weather_intensity,
            Self::BiomeWeight(id) => biome.weight(id),
            Self::StableRandom { salt } => environment.stable_random(*salt),
        }
    }

    fn evaluation_class(&self) -> EvaluationClass {
        match self {
            Self::SeasonPhase => EvaluationClass {
                spatial: SpatialGranularity::Global,
                update: UpdateFrequency::WorldEvent,
            },
            Self::DayPhase | Self::WeatherIntensity => EvaluationClass::FRAME_UNIFORM,
            Self::Environment(
                EnvironmentChannel::SnowAccumulation
                | EnvironmentChannel::SoilMoisture
                | EnvironmentChannel::Wetness,
            ) => EvaluationClass {
                spatial: SpatialGranularity::PerVoxel,
                update: UpdateFrequency::SimulationTick,
            },
            Self::Environment(_) | Self::BiomeWeight(_) | Self::StableRandom { .. } => {
                EvaluationClass::STATIC_WORLD
            }
            Self::PositionX | Self::PositionY | Self::PositionZ | Self::NormalY => {
                EvaluationClass {
                    spatial: SpatialGranularity::PerSample,
                    update: UpdateFrequency::CompileTime,
                }
            }
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CompareOp {
    Less,
    LessEqual,
    Greater,
    GreaterEqual,
    Equal,
    NotEqual,
}

impl CompareOp {
    fn evaluate(self, left: f32, right: f32) -> bool {
        match self {
            Self::Less => left < right,
            Self::LessEqual => left <= right,
            Self::Greater => left > right,
            Self::GreaterEqual => left >= right,
            Self::Equal => (left - right).abs() <= 0.000_001,
            Self::NotEqual => (left - right).abs() > 0.000_001,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum Condition {
    Always,
    Season(Season),
    Weather(WeatherKind),
    Compare {
        source: ScalarSource,
        operation: CompareOp,
        value: f32,
    },
    All(Vec<Condition>),
    Any(Vec<Condition>),
    Not(Box<Condition>),
}

impl Condition {
    pub fn evaluate(&self, environment: &EnvironmentContext, biome: &BiomeSample) -> bool {
        match self {
            Self::Always => true,
            Self::Season(season) => environment.season == *season,
            Self::Weather(weather) => environment.weather == *weather,
            Self::Compare {
                source,
                operation,
                value,
            } => operation.evaluate(source.value(environment, biome), *value),
            Self::All(conditions) => conditions
                .iter()
                .all(|condition| condition.evaluate(environment, biome)),
            Self::Any(conditions) => conditions
                .iter()
                .any(|condition| condition.evaluate(environment, biome)),
            Self::Not(condition) => !condition.evaluate(environment, biome),
        }
    }

    pub fn evaluation_class(&self) -> EvaluationClass {
        match self {
            Self::Always => EvaluationClass {
                spatial: SpatialGranularity::Global,
                update: UpdateFrequency::CompileTime,
            },
            Self::Season(_) | Self::Weather(_) => EvaluationClass {
                spatial: SpatialGranularity::Global,
                update: UpdateFrequency::WorldEvent,
            },
            Self::Compare { source, .. } => source.evaluation_class(),
            Self::All(conditions) | Self::Any(conditions) => conditions.iter().fold(
                EvaluationClass {
                    spatial: SpatialGranularity::Global,
                    update: UpdateFrequency::CompileTime,
                },
                |class, condition| class.combines(condition.evaluation_class()),
            ),
            Self::Not(condition) => condition.evaluation_class(),
        }
    }

    fn validate(&self) -> bool {
        match self {
            Self::Compare { value, .. } => value.is_finite(),
            Self::All(conditions) | Self::Any(conditions) => {
                !conditions.is_empty() && conditions.iter().all(Self::validate)
            }
            Self::Not(condition) => condition.validate(),
            _ => true,
        }
    }

    fn may_match_runtime(&self, runtime: &RuntimeEnvironmentState) -> bool {
        match self {
            Self::Season(season) => runtime.season == *season,
            Self::Weather(weather) => runtime.weather == *weather,
            Self::All(conditions) => conditions
                .iter()
                .all(|condition| condition.may_match_runtime(runtime)),
            Self::Any(conditions) => conditions
                .iter()
                .any(|condition| condition.may_match_runtime(runtime)),
            // A negated spatial condition may be true somewhere. Only a
            // negated purely runtime leaf can be ruled out globally.
            Self::Not(condition) => match condition.as_ref() {
                Self::Season(_) | Self::Weather(_) => !condition.may_match_runtime(runtime),
                _ => true,
            },
            _ => true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SurfaceAction {
    SetRole {
        layer: SurfaceLayer,
        role: MaterialRole,
    },
    AddWorldVoxelLayer {
        role: MaterialRole,
        thickness_world_voxels: u8,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceRule {
    pub id: RuleId,
    pub priority: i32,
    pub condition: Condition,
    pub action: SurfaceAction,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub enum SurfaceTarget {
    All,
    Layer(SurfaceLayer),
    Role(MaterialRole),
    Trait(MaterialTrait),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceModifierRule {
    pub id: RuleId,
    pub condition: Condition,
    pub target: SurfaceTarget,
    pub modifier: ModifierId,
    pub weight: f32,
    pub parameters: BTreeMap<String, f32>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SurfaceProfile {
    pub id: SurfaceProfileId,
    pub base_layers: BTreeMap<SurfaceLayer, MaterialRole>,
    pub rules: Vec<SurfaceRule>,
    pub modifiers: Vec<SurfaceModifierRule>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialChoice {
    pub material: AssetId,
    pub weight: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialBinding {
    pub choices: Vec<MaterialChoice>,
    pub traits: BTreeSet<MaterialTrait>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MaterialPalette {
    pub id: MaterialPaletteId,
    pub bindings: BTreeMap<MaterialRole, MaterialBinding>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PresentationModifierDefinition {
    pub id: ModifierId,
    pub graph: AssetId,
    pub evaluation: EvaluationClass,
    pub parameter_defaults: BTreeMap<String, f32>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FeatureStage {
    Carving,
    Geology,
    Surface,
    Vegetation,
    Decoration,
    PostProcess,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureRule {
    pub id: FeatureId,
    pub stage: FeatureStage,
    pub generator_graph: AssetId,
    pub condition: Condition,
    pub probability: f32,
    pub salt: u64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FeatureSet {
    pub id: FeatureSetId,
    pub features: Vec<FeatureRule>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioCue {
    pub sound: AssetId,
    pub condition: Condition,
    pub gain: f32,
    pub radius_meters: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AudioProfile {
    pub id: AudioProfileId,
    pub cues: Vec<AudioCue>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationBinding {
    pub graph: AssetId,
    pub condition: Condition,
    pub target: SurfaceTarget,
    pub parameter: String,
    pub amplitude: f32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AnimationProfile {
    pub id: AnimationProfileId,
    pub bindings: Vec<AnimationBinding>,
}

/// Project-level identities available to a world profile compiler. Authored
/// profiles never assume that a non-empty string is executable: every material,
/// graph, and runtime resource must be registered here by the owning project.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct WorldAssetCatalog {
    pub material_slots: BTreeMap<AssetId, u8>,
    pub graph_kinds: BTreeMap<AssetId, GraphKind>,
    pub runtime_assets: BTreeSet<AssetId>,
}

impl WorldAssetCatalog {
    pub fn material_slot(&self, id: &AssetId) -> Option<u8> {
        self.material_slots.get(id).copied()
    }

    pub fn graph_kind(&self, id: &AssetId) -> Option<GraphKind> {
        self.graph_kinds.get(id).copied()
    }

    pub fn contains_runtime_asset(&self, id: &AssetId) -> bool {
        self.runtime_assets.contains(id)
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct WorldProfileAsset {
    pub schema_version: u32,
    pub id: AssetId,
    pub name: String,
    pub biomes: BiomeRegistry,
    pub surface_profiles: Vec<SurfaceProfile>,
    pub material_palettes: Vec<MaterialPalette>,
    pub modifiers: Vec<PresentationModifierDefinition>,
    pub feature_sets: Vec<FeatureSet>,
    pub audio_profiles: Vec<AudioProfile>,
    pub animation_profiles: Vec<AnimationProfile>,
}

impl WorldProfileAsset {
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            schema_version: STUDIO_ASSET_SCHEMA_VERSION,
            id: AssetId::new(),
            name: name.into(),
            biomes: BiomeRegistry::default(),
            surface_profiles: Vec::new(),
            material_palettes: Vec::new(),
            modifiers: Vec::new(),
            feature_sets: Vec::new(),
            audio_profiles: Vec::new(),
            animation_profiles: Vec::new(),
        }
    }

    pub fn compile(
        mut self,
        assets: &WorldAssetCatalog,
    ) -> Result<CompiledWorldProfile, WorldProfileError> {
        validate_world_profile(&self, assets)?;
        for profile in &mut self.surface_profiles {
            profile
                .rules
                .sort_by_key(|rule| (rule.priority, rule.id.clone()));
        }
        CompiledWorldProfile::new(self, assets)
    }
}

#[derive(Clone, Debug)]
pub struct CompiledWorldProfile {
    asset: WorldProfileAsset,
    biome_indices: BTreeMap<BiomeId, usize>,
    surface_indices: BTreeMap<SurfaceProfileId, usize>,
    palette_indices: BTreeMap<MaterialPaletteId, usize>,
    modifier_indices: BTreeMap<ModifierId, usize>,
    feature_set_indices: BTreeMap<FeatureSetId, usize>,
    audio_profile_indices: BTreeMap<AudioProfileId, usize>,
    animation_profile_indices: BTreeMap<AnimationProfileId, usize>,
    material_slots: BTreeMap<AssetId, u8>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AddedVoxelLayer {
    pub role: MaterialRole,
    pub material: AssetId,
    pub material_slot: u8,
    pub thickness_world_voxels: u8,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedMaterialLayer {
    pub layer: SurfaceLayer,
    pub role: MaterialRole,
    pub material: AssetId,
    pub material_slot: u8,
    pub traits: BTreeSet<MaterialTrait>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedModifier {
    pub layer: SurfaceLayer,
    pub modifier: ModifierId,
    pub graph: AssetId,
    pub weight: f32,
    pub parameters: BTreeMap<String, f32>,
    pub evaluation: EvaluationClass,
}

#[derive(Clone, Debug, PartialEq)]
pub struct FeatureRequest {
    pub feature: FeatureId,
    pub stage: FeatureStage,
    pub generator_graph: AssetId,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AudioRequest {
    pub sound: AssetId,
    pub gain: f32,
    pub radius_meters: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct AnimationRequest {
    pub graph: AssetId,
    pub layer: SurfaceLayer,
    pub parameter: String,
    pub amplitude: f32,
}

#[derive(Clone, Debug, PartialEq)]
pub struct GenerationProjection {
    pub material_roles: BTreeMap<SurfaceLayer, MaterialRole>,
    pub added_voxel_layers: Vec<AddedVoxelLayer>,
    pub features: Vec<FeatureRequest>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct PresentationProjection {
    pub materials: Vec<ResolvedMaterialLayer>,
    pub modifiers: Vec<ResolvedModifier>,
    pub animations: Vec<AnimationRequest>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct RuntimeProjection {
    pub audio: Vec<AudioRequest>,
}

#[derive(Clone, Debug, PartialEq)]
pub struct ResolvedWorldSample {
    pub biome: BiomeSample,
    pub generation: GenerationProjection,
    pub presentation: PresentationProjection,
    pub runtime: RuntimeProjection,
    pub evaluation: EvaluationClass,
}

impl CompiledWorldProfile {
    fn new(
        asset: WorldProfileAsset,
        assets: &WorldAssetCatalog,
    ) -> Result<Self, WorldProfileError> {
        Ok(Self {
            biome_indices: index_by(asset.biomes.biomes.iter().map(|value| &value.id)),
            surface_indices: index_by(asset.surface_profiles.iter().map(|value| &value.id)),
            palette_indices: index_by(asset.material_palettes.iter().map(|value| &value.id)),
            modifier_indices: index_by(asset.modifiers.iter().map(|value| &value.id)),
            feature_set_indices: index_by(asset.feature_sets.iter().map(|value| &value.id)),
            audio_profile_indices: index_by(asset.audio_profiles.iter().map(|value| &value.id)),
            animation_profile_indices: index_by(
                asset.animation_profiles.iter().map(|value| &value.id),
            ),
            material_slots: assets.material_slots.clone(),
            asset,
        })
    }

    pub fn asset(&self) -> &WorldProfileAsset {
        &self.asset
    }

    pub fn has_active_voxel_layer_rules(&self, runtime: &RuntimeEnvironmentState) -> bool {
        self.asset.surface_profiles.iter().any(|profile| {
            profile.rules.iter().any(|rule| {
                matches!(rule.action, SurfaceAction::AddWorldVoxelLayer { .. })
                    && rule.condition.may_match_runtime(runtime)
            })
        })
    }

    pub fn resolve(
        &self,
        environment: &EnvironmentContext,
    ) -> Result<ResolvedWorldSample, WorldProfileError> {
        let biome_sample = self.asset.biomes.resolve_validated(environment, 4);
        let primary = self.biome(&biome_sample.primary)?;
        let profile = self.surface_profile(&primary.surface_profile)?;
        let palette = self.palette(&primary.material_palette)?;

        let mut roles = profile.base_layers.clone();
        let mut added_roles = Vec::new();
        let mut evaluation = EvaluationClass {
            spatial: SpatialGranularity::Global,
            update: UpdateFrequency::CompileTime,
        };
        for rule in &profile.rules {
            evaluation = evaluation.combines(rule.condition.evaluation_class());
            if !rule.condition.evaluate(environment, &biome_sample) {
                continue;
            }
            match &rule.action {
                SurfaceAction::SetRole { layer, role } => {
                    roles.insert(*layer, role.clone());
                }
                SurfaceAction::AddWorldVoxelLayer {
                    role,
                    thickness_world_voxels,
                } => added_roles.push((role.clone(), *thickness_world_voxels)),
            }
        }

        let mut materials = Vec::new();
        for (layer, role) in &roles {
            let binding = palette.bindings.get(role).ok_or_else(|| {
                WorldProfileError::new(
                    "missing_material_role",
                    format!("palette `{}` does not bind role `{role}`", palette.id),
                )
            })?;
            let material = select_material(binding, environment, stable_text_hash(&role.0))?;
            materials.push(ResolvedMaterialLayer {
                layer: *layer,
                role: role.clone(),
                material_slot: self.material_slot(&material)?,
                material,
                traits: binding.traits.clone(),
            });
        }
        let mut added_voxel_layers = Vec::new();
        for (role, thickness_world_voxels) in added_roles {
            let binding = palette.bindings.get(&role).ok_or_else(|| {
                WorldProfileError::new(
                    "missing_material_role",
                    format!("palette `{}` does not bind added role `{role}`", palette.id),
                )
            })?;
            let material = select_material(binding, environment, stable_text_hash(&role.0))?;
            added_voxel_layers.push(AddedVoxelLayer {
                material_slot: self.material_slot(&material)?,
                material,
                role,
                thickness_world_voxels,
            });
        }

        let mut modifiers = Vec::new();
        let mut features = Vec::new();
        let mut audio = Vec::new();
        let mut animations = Vec::new();
        for influence in biome_sample
            .influences
            .iter()
            .filter(|influence| influence.weight > 0.0)
        {
            let biome = self.biome(&influence.biome)?;
            let influence_profile = self.surface_profile(&biome.surface_profile)?;
            for rule in &influence_profile.modifiers {
                evaluation = evaluation.combines(rule.condition.evaluation_class());
                if !rule.condition.evaluate(environment, &biome_sample) {
                    continue;
                }
                let definition = self.modifier(&rule.modifier)?;
                evaluation = evaluation.combines(definition.evaluation);
                for material in materials
                    .iter()
                    .filter(|material| target_matches(&rule.target, material))
                {
                    let mut parameters = definition.parameter_defaults.clone();
                    parameters.extend(rule.parameters.clone());
                    modifiers.push(ResolvedModifier {
                        layer: material.layer,
                        modifier: definition.id.clone(),
                        graph: definition.graph.clone(),
                        weight: rule.weight * influence.weight,
                        parameters,
                        evaluation: definition.evaluation,
                    });
                }
            }
            for set_id in &biome.feature_sets {
                let set = self.feature_set(set_id)?;
                for feature in &set.features {
                    evaluation = evaluation.combines(feature.condition.evaluation_class());
                    if feature.condition.evaluate(environment, &biome_sample)
                        && environment.stable_random(feature.salt)
                            < feature.probability * influence.weight
                    {
                        features.push(FeatureRequest {
                            feature: feature.id.clone(),
                            stage: feature.stage,
                            generator_graph: feature.generator_graph.clone(),
                        });
                    }
                }
            }
            if let Some(profile_id) = &biome.audio_profile {
                for cue in &self.audio_profile(profile_id)?.cues {
                    evaluation = evaluation.combines(cue.condition.evaluation_class());
                    if cue.condition.evaluate(environment, &biome_sample) {
                        audio.push(AudioRequest {
                            sound: cue.sound.clone(),
                            gain: cue.gain * influence.weight,
                            radius_meters: cue.radius_meters,
                        });
                    }
                }
            }
            if let Some(profile_id) = &biome.animation_profile {
                for binding in &self.animation_profile(profile_id)?.bindings {
                    evaluation = evaluation.combines(binding.condition.evaluation_class());
                    if !binding.condition.evaluate(environment, &biome_sample) {
                        continue;
                    }
                    for material in materials
                        .iter()
                        .filter(|material| target_matches(&binding.target, material))
                    {
                        animations.push(AnimationRequest {
                            graph: binding.graph.clone(),
                            layer: material.layer,
                            parameter: binding.parameter.clone(),
                            amplitude: binding.amplitude * influence.weight,
                        });
                    }
                }
            }
        }

        Ok(ResolvedWorldSample {
            biome: biome_sample,
            generation: GenerationProjection {
                material_roles: roles,
                added_voxel_layers,
                features,
            },
            presentation: PresentationProjection {
                materials,
                modifiers,
                animations,
            },
            runtime: RuntimeProjection { audio },
            evaluation,
        })
    }

    fn biome(&self, id: &BiomeId) -> Result<&BiomeDefinition, WorldProfileError> {
        self.biome_indices
            .get(id)
            .and_then(|index| self.asset.biomes.biomes.get(*index))
            .ok_or_else(|| {
                WorldProfileError::new("missing_biome", format!("biome `{id}` is unavailable"))
            })
    }

    fn surface_profile(&self, id: &SurfaceProfileId) -> Result<&SurfaceProfile, WorldProfileError> {
        self.surface_indices
            .get(id)
            .and_then(|index| self.asset.surface_profiles.get(*index))
            .ok_or_else(|| WorldProfileError::new("missing_surface_profile", id.to_string()))
    }

    fn palette(&self, id: &MaterialPaletteId) -> Result<&MaterialPalette, WorldProfileError> {
        self.palette_indices
            .get(id)
            .and_then(|index| self.asset.material_palettes.get(*index))
            .ok_or_else(|| WorldProfileError::new("missing_palette", id.to_string()))
    }

    fn modifier(
        &self,
        id: &ModifierId,
    ) -> Result<&PresentationModifierDefinition, WorldProfileError> {
        self.modifier_indices
            .get(id)
            .and_then(|index| self.asset.modifiers.get(*index))
            .ok_or_else(|| WorldProfileError::new("missing_modifier", id.to_string()))
    }

    fn feature_set(&self, id: &FeatureSetId) -> Result<&FeatureSet, WorldProfileError> {
        self.feature_set_indices
            .get(id)
            .and_then(|index| self.asset.feature_sets.get(*index))
            .ok_or_else(|| WorldProfileError::new("missing_feature_set", id.to_string()))
    }

    fn audio_profile(&self, id: &AudioProfileId) -> Result<&AudioProfile, WorldProfileError> {
        self.audio_profile_indices
            .get(id)
            .and_then(|index| self.asset.audio_profiles.get(*index))
            .ok_or_else(|| WorldProfileError::new("missing_audio_profile", id.to_string()))
    }

    fn animation_profile(
        &self,
        id: &AnimationProfileId,
    ) -> Result<&AnimationProfile, WorldProfileError> {
        self.animation_profile_indices
            .get(id)
            .and_then(|index| self.asset.animation_profiles.get(*index))
            .ok_or_else(|| WorldProfileError::new("missing_animation_profile", id.to_string()))
    }

    fn material_slot(&self, id: &AssetId) -> Result<u8, WorldProfileError> {
        self.material_slots.get(id).copied().ok_or_else(|| {
            WorldProfileError::new(
                "missing_material_asset",
                format!("material asset `{id}` is unavailable"),
            )
        })
    }
}

fn index_by<'a, Id>(values: impl Iterator<Item = &'a Id>) -> BTreeMap<Id, usize>
where
    Id: Clone + Ord + 'a,
{
    values
        .enumerate()
        .map(|(index, id)| (id.clone(), index))
        .collect()
}

fn target_matches(target: &SurfaceTarget, material: &ResolvedMaterialLayer) -> bool {
    match target {
        SurfaceTarget::All => true,
        SurfaceTarget::Layer(layer) => material.layer == *layer,
        SurfaceTarget::Role(role) => material.role == *role,
        SurfaceTarget::Trait(material_trait) => material.traits.contains(material_trait),
    }
}

fn select_material(
    binding: &MaterialBinding,
    environment: &EnvironmentContext,
    salt: u64,
) -> Result<AssetId, WorldProfileError> {
    let total = binding
        .choices
        .iter()
        .map(|choice| choice.weight)
        .sum::<f32>();
    if total <= f32::EPSILON {
        return Err(WorldProfileError::new(
            "empty_material_binding",
            "material binding has no positive choices",
        ));
    }
    let mut target = environment.stable_random(salt) * total;
    for choice in &binding.choices {
        target -= choice.weight;
        if target <= 0.0 {
            return Ok(choice.material.clone());
        }
    }
    Ok(binding
        .choices
        .last()
        .expect("validated material bindings are non-empty")
        .material
        .clone())
}

fn stable_text_hash(value: &str) -> u64 {
    value.bytes().fold(0xcbf2_9ce4_8422_2325, |hash, byte| {
        (hash ^ u64::from(byte)).wrapping_mul(0x1000_0000_01b3)
    })
}

fn validate_world_profile(
    asset: &WorldProfileAsset,
    assets: &WorldAssetCatalog,
) -> Result<(), WorldProfileError> {
    if asset.schema_version != STUDIO_ASSET_SCHEMA_VERSION {
        return Err(WorldProfileError::new(
            "schema_version",
            format!(
                "world profile schema {} does not equal {}",
                asset.schema_version, STUDIO_ASSET_SCHEMA_VERSION
            ),
        ));
    }
    asset
        .biomes
        .validate()
        .map_err(WorldProfileError::from_biome)?;
    unique_ids(
        asset.surface_profiles.iter().map(|value| &value.id.0),
        "surface_profile",
    )?;
    unique_ids(
        asset.material_palettes.iter().map(|value| &value.id.0),
        "material_palette",
    )?;
    unique_ids(asset.modifiers.iter().map(|value| &value.id.0), "modifier")?;
    unique_ids(
        asset.feature_sets.iter().map(|value| &value.id.0),
        "feature_set",
    )?;
    unique_ids(
        asset.audio_profiles.iter().map(|value| &value.id.0),
        "audio_profile",
    )?;
    unique_ids(
        asset.animation_profiles.iter().map(|value| &value.id.0),
        "animation_profile",
    )?;

    for biome in &asset.biomes.biomes {
        require_id(
            asset
                .surface_profiles
                .iter()
                .any(|value| value.id == biome.surface_profile),
            "surface_profile",
            &biome.surface_profile.0,
        )?;
        require_id(
            asset
                .material_palettes
                .iter()
                .any(|value| value.id == biome.material_palette),
            "material_palette",
            &biome.material_palette.0,
        )?;
        for id in &biome.feature_sets {
            require_id(
                asset.feature_sets.iter().any(|value| value.id == *id),
                "feature_set",
                &id.0,
            )?;
        }
        if let Some(id) = &biome.audio_profile {
            require_id(
                asset.audio_profiles.iter().any(|value| value.id == *id),
                "audio_profile",
                &id.0,
            )?;
        }
        if let Some(id) = &biome.animation_profile {
            require_id(
                asset.animation_profiles.iter().any(|value| value.id == *id),
                "animation_profile",
                &id.0,
            )?;
        }
        let surface = asset
            .surface_profiles
            .iter()
            .find(|value| value.id == biome.surface_profile)
            .expect("surface reference checked above");
        let palette = asset
            .material_palettes
            .iter()
            .find(|value| value.id == biome.material_palette)
            .expect("palette reference checked above");
        let mut required_roles: BTreeSet<_> = surface.base_layers.values().cloned().collect();
        for rule in &surface.rules {
            match &rule.action {
                SurfaceAction::SetRole { role, .. }
                | SurfaceAction::AddWorldVoxelLayer { role, .. } => {
                    required_roles.insert(role.clone());
                }
            }
        }
        for role in required_roles {
            if !palette.bindings.contains_key(&role) {
                return Err(WorldProfileError::new(
                    "missing_material_role",
                    format!(
                        "biome `{}` pairs surface `{}` with palette `{}`, which does not bind `{role}`",
                        biome.id, surface.id, palette.id
                    ),
                ));
            }
        }
    }
    for palette in &asset.material_palettes {
        for (role, binding) in &palette.bindings {
            if binding.choices.is_empty()
                || binding.choices.iter().any(|choice| {
                    !choice.weight.is_finite()
                        || choice.weight <= 0.0
                        || choice.material.0.is_empty()
                        || assets.material_slot(&choice.material).is_none()
                })
            {
                return Err(WorldProfileError::new(
                    "invalid_material_binding",
                    format!(
                        "palette `{}` has an invalid binding for `{role}`",
                        palette.id
                    ),
                ));
            }
        }
    }
    for profile in &asset.surface_profiles {
        unique_ids(
            profile
                .rules
                .iter()
                .map(|value| &value.id.0)
                .chain(profile.modifiers.iter().map(|value| &value.id.0)),
            "surface_rule",
        )?;
        for rule in &profile.rules {
            if !rule.condition.validate()
                || !condition_references_exist(&rule.condition, &asset.biomes)
            {
                return Err(WorldProfileError::new(
                    "invalid_condition",
                    format!("surface rule `{}` is invalid", rule.id),
                ));
            }
            if let SurfaceAction::AddWorldVoxelLayer {
                thickness_world_voxels,
                ..
            } = rule.action
            {
                if thickness_world_voxels == 0 {
                    return Err(WorldProfileError::new(
                        "invalid_layer_thickness",
                        format!("surface rule `{}` adds an empty layer", rule.id),
                    ));
                }
            }
        }
        for rule in &profile.modifiers {
            require_id(
                asset
                    .modifiers
                    .iter()
                    .any(|value| value.id == rule.modifier),
                "modifier",
                &rule.modifier.0,
            )?;
            if !rule.condition.validate()
                || !condition_references_exist(&rule.condition, &asset.biomes)
                || !rule.weight.is_finite()
                || !(0.0..=1.0).contains(&rule.weight)
                || !rule.parameters.values().all(|value| value.is_finite())
            {
                return Err(WorldProfileError::new(
                    "invalid_modifier_rule",
                    format!("modifier rule `{}` is invalid", rule.id),
                ));
            }
        }
    }
    for modifier in &asset.modifiers {
        if assets.graph_kind(&modifier.graph) != Some(GraphKind::WorldModifier)
            || !modifier
                .parameter_defaults
                .values()
                .all(|value| value.is_finite())
        {
            return Err(WorldProfileError::new(
                "invalid_modifier",
                format!("modifier `{}` is invalid", modifier.id),
            ));
        }
    }
    for set in &asset.feature_sets {
        unique_ids(set.features.iter().map(|value| &value.id.0), "feature")?;
        for feature in &set.features {
            if !feature.condition.validate()
                || !condition_references_exist(&feature.condition, &asset.biomes)
                || !feature.probability.is_finite()
                || !(0.0..=1.0).contains(&feature.probability)
                || assets.graph_kind(&feature.generator_graph) != Some(GraphKind::Feature)
            {
                return Err(WorldProfileError::new(
                    "invalid_feature",
                    format!("feature `{}` is invalid", feature.id),
                ));
            }
        }
    }
    for profile in &asset.audio_profiles {
        for cue in &profile.cues {
            if !assets.contains_runtime_asset(&cue.sound)
                || !cue.condition.validate()
                || !condition_references_exist(&cue.condition, &asset.biomes)
                || !cue.gain.is_finite()
                || cue.gain < 0.0
                || !cue.radius_meters.is_finite()
                || cue.radius_meters <= 0.0
            {
                return Err(WorldProfileError::new(
                    "invalid_audio_cue",
                    format!("audio profile `{}` contains an invalid cue", profile.id),
                ));
            }
        }
    }
    for profile in &asset.animation_profiles {
        for binding in &profile.bindings {
            if assets.graph_kind(&binding.graph) != Some(GraphKind::Animation)
                || binding.parameter.trim().is_empty()
                || !binding.condition.validate()
                || !condition_references_exist(&binding.condition, &asset.biomes)
                || !binding.amplitude.is_finite()
            {
                return Err(WorldProfileError::new(
                    "invalid_animation_binding",
                    format!(
                        "animation profile `{}` contains an invalid binding",
                        profile.id
                    ),
                ));
            }
        }
    }
    Ok(())
}

fn condition_references_exist(condition: &Condition, biomes: &BiomeRegistry) -> bool {
    match condition {
        Condition::Compare {
            source: ScalarSource::BiomeWeight(id),
            ..
        } => biomes.get(id).is_some(),
        Condition::All(conditions) | Condition::Any(conditions) => conditions
            .iter()
            .all(|condition| condition_references_exist(condition, biomes)),
        Condition::Not(condition) => condition_references_exist(condition, biomes),
        _ => true,
    }
}

fn unique_ids<'a>(
    values: impl Iterator<Item = &'a String>,
    kind: &'static str,
) -> Result<(), WorldProfileError> {
    let mut ids = BTreeSet::new();
    for value in values {
        if value.trim().is_empty() || !ids.insert(value) {
            return Err(WorldProfileError::new(
                "duplicate_id",
                format!("{kind} id `{value}` is empty or duplicated"),
            ));
        }
    }
    Ok(())
}

fn require_id(exists: bool, kind: &'static str, id: &str) -> Result<(), WorldProfileError> {
    if exists {
        Ok(())
    } else {
        Err(WorldProfileError::new(
            "missing_reference",
            format!("{kind} `{id}` is not defined"),
        ))
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct WorldProfileError {
    pub code: &'static str,
    pub message: String,
}

impl WorldProfileError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }

    fn from_biome(error: BiomeError) -> Self {
        Self::new("biome", format!("{error:?}"))
    }
}

impl fmt::Display for WorldProfileError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for WorldProfileError {}

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use crate::biome::{BiomeSelector, BiomeTrait, FieldConstraint, FieldRange};
    use crate::environment::{GeneratedEnvironment, RuntimeEnvironmentState};

    fn asset(value: &str) -> AssetId {
        AssetId(value.to_string())
    }

    fn role(value: &str) -> MaterialRole {
        MaterialRole::new(value)
    }

    fn catalog() -> WorldAssetCatalog {
        WorldAssetCatalog {
            material_slots: BTreeMap::from([
                (asset("material-grass"), 1),
                (asset("material-soil"), 3),
                (asset("material-stone"), 6),
                (asset("material-snow"), 23),
            ]),
            graph_kinds: BTreeMap::from([
                (asset("graph-winter-cover"), GraphKind::WorldModifier),
                (asset("graph-crystals"), GraphKind::Feature),
                (asset("graph-pulse"), GraphKind::Animation),
            ]),
            runtime_assets: BTreeSet::from([asset("sound-winter-wind")]),
        }
    }

    fn winter_world() -> WorldProfileAsset {
        let surface_profile = SurfaceProfileId::new("temperate-ground");
        let palette_id = MaterialPaletteId::new("temperate");
        let features = FeatureSetId::new("geology");
        let audio = AudioProfileId::new("winter-forest-audio");
        let animation = AnimationProfileId::new("winter-pulse");
        let modifier = ModifierId::new("winter-cover");
        WorldProfileAsset {
            schema_version: STUDIO_ASSET_SCHEMA_VERSION,
            id: asset("world"),
            name: "Test world".into(),
            biomes: BiomeRegistry {
                biomes: vec![BiomeDefinition {
                    id: BiomeId::new("temperate-forest"),
                    name: "Temperate Forest".into(),
                    selector: BiomeSelector {
                        constraints: vec![FieldConstraint {
                            channel: EnvironmentChannel::Temperature,
                            range: FieldRange {
                                min: -1.0,
                                max: 1.0,
                                falloff: 0.0,
                            },
                            weight: 1.0,
                        }],
                        priority: 0,
                    },
                    traits: BTreeSet::from([BiomeTrait::Forested]),
                    surface_profile: surface_profile.clone(),
                    material_palette: palette_id.clone(),
                    feature_sets: vec![features.clone()],
                    audio_profile: Some(audio.clone()),
                    animation_profile: Some(animation.clone()),
                }],
            },
            surface_profiles: vec![SurfaceProfile {
                id: surface_profile,
                base_layers: BTreeMap::from([
                    (SurfaceLayer::Surface, role("ground.surface")),
                    (SurfaceLayer::Subsoil, role("ground.subsoil")),
                    (SurfaceLayer::Deep, role("ground.deep")),
                ]),
                rules: vec![SurfaceRule {
                    id: RuleId::new("winter-snow-layer"),
                    priority: 10,
                    condition: Condition::All(vec![
                        Condition::Season(Season::Winter),
                        Condition::Compare {
                            source: ScalarSource::NormalY,
                            operation: CompareOp::GreaterEqual,
                            value: 0.65,
                        },
                    ]),
                    action: SurfaceAction::AddWorldVoxelLayer {
                        role: role("cover.snow"),
                        thickness_world_voxels: 1,
                    },
                }],
                modifiers: vec![SurfaceModifierRule {
                    id: RuleId::new("winter-tint"),
                    condition: Condition::Season(Season::Winter),
                    target: SurfaceTarget::Trait(MaterialTrait::Snowable),
                    modifier: modifier.clone(),
                    weight: 1.0,
                    parameters: BTreeMap::from([("coverage".into(), 0.8)]),
                }],
            }],
            material_palettes: vec![MaterialPalette {
                id: palette_id,
                bindings: BTreeMap::from([
                    (
                        role("ground.surface"),
                        MaterialBinding {
                            choices: vec![MaterialChoice {
                                material: asset("material-grass"),
                                weight: 1.0,
                            }],
                            traits: BTreeSet::from([
                                MaterialTrait::Organic,
                                MaterialTrait::Snowable,
                            ]),
                        },
                    ),
                    (
                        role("ground.subsoil"),
                        MaterialBinding {
                            choices: vec![MaterialChoice {
                                material: asset("material-soil"),
                                weight: 1.0,
                            }],
                            traits: BTreeSet::from([MaterialTrait::Porous]),
                        },
                    ),
                    (
                        role("ground.deep"),
                        MaterialBinding {
                            choices: vec![MaterialChoice {
                                material: asset("material-stone"),
                                weight: 1.0,
                            }],
                            traits: BTreeSet::from([MaterialTrait::Mineral]),
                        },
                    ),
                    (
                        role("cover.snow"),
                        MaterialBinding {
                            choices: vec![MaterialChoice {
                                material: asset("material-snow"),
                                weight: 1.0,
                            }],
                            traits: BTreeSet::new(),
                        },
                    ),
                ]),
            }],
            modifiers: vec![PresentationModifierDefinition {
                id: modifier,
                graph: asset("graph-winter-cover"),
                evaluation: EvaluationClass::CHUNK_EVENT,
                parameter_defaults: BTreeMap::from([("coverage".into(), 0.0)]),
            }],
            feature_sets: vec![FeatureSet {
                id: features,
                features: vec![FeatureRule {
                    id: FeatureId::new("deep-crystals"),
                    stage: FeatureStage::Geology,
                    generator_graph: asset("graph-crystals"),
                    condition: Condition::Compare {
                        source: ScalarSource::Environment(EnvironmentChannel::Depth),
                        operation: CompareOp::GreaterEqual,
                        value: 10.0,
                    },
                    probability: 1.0,
                    salt: 77,
                }],
            }],
            audio_profiles: vec![AudioProfile {
                id: audio,
                cues: vec![AudioCue {
                    sound: asset("sound-winter-wind"),
                    condition: Condition::Season(Season::Winter),
                    gain: 0.6,
                    radius_meters: 24.0,
                }],
            }],
            animation_profiles: vec![AnimationProfile {
                id: animation,
                bindings: vec![AnimationBinding {
                    graph: asset("graph-pulse"),
                    condition: Condition::Season(Season::Winter),
                    target: SurfaceTarget::Layer(SurfaceLayer::Deep),
                    parameter: "emission_strength".into(),
                    amplitude: 0.75,
                }],
            }],
        }
    }

    fn context(season: Season, depth: f32) -> EnvironmentContext {
        EnvironmentContext::compose(
            &GeneratedEnvironment {
                world_seed: 91,
                position: [10.0, 40.0, -3.0],
                normal: [0.0, 1.0, 0.0],
                fields: BTreeMap::from([
                    (EnvironmentChannel::Temperature, 0.2),
                    (EnvironmentChannel::Depth, depth),
                ]),
            },
            &RuntimeEnvironmentState {
                season,
                ..RuntimeEnvironmentState::default()
            },
            &BTreeMap::new(),
        )
        .unwrap()
    }

    #[test]
    fn winter_depth_resolves_generation_presentation_audio_and_animation_separately() {
        let compiled = winter_world().compile(&catalog()).unwrap();
        let sample = compiled.resolve(&context(Season::Winter, 14.0)).unwrap();
        assert_eq!(sample.biome.primary, BiomeId::new("temperate-forest"));
        assert_eq!(sample.generation.added_voxel_layers.len(), 1);
        assert_eq!(
            sample.generation.added_voxel_layers[0].material,
            asset("material-snow")
        );
        assert_eq!(
            sample.generation.features[0].feature,
            FeatureId::new("deep-crystals")
        );
        assert_eq!(sample.presentation.modifiers.len(), 1);
        assert_eq!(
            sample.presentation.modifiers[0].graph,
            asset("graph-winter-cover")
        );
        assert_eq!(
            sample.presentation.animations[0].graph,
            asset("graph-pulse")
        );
        assert_eq!(sample.runtime.audio[0].sound, asset("sound-winter-wind"));
        assert!(sample.evaluation.update >= UpdateFrequency::WorldEvent);
    }

    #[test]
    fn summer_keeps_intrinsic_materials_and_omits_winter_outputs() {
        let compiled = winter_world().compile(&catalog()).unwrap();
        let sample = compiled.resolve(&context(Season::Summer, 2.0)).unwrap();
        assert!(sample.generation.added_voxel_layers.is_empty());
        assert!(sample.generation.features.is_empty());
        assert!(sample.presentation.modifiers.is_empty());
        assert!(sample.presentation.animations.is_empty());
        assert!(sample.runtime.audio.is_empty());
        assert_eq!(
            sample
                .presentation
                .materials
                .iter()
                .find(|material| material.layer == SurfaceLayer::Surface)
                .unwrap()
                .material,
            asset("material-grass")
        );
    }

    #[test]
    fn missing_cross_domain_reference_fails_at_compile_time() {
        let mut world = winter_world();
        world.modifiers.clear();
        let error = world.compile(&catalog()).unwrap_err();
        assert_eq!(error.code, "missing_reference");
    }

    #[test]
    fn runtime_effect_definitions_are_rejected_before_evaluation() {
        let mut world = winter_world();
        world.audio_profiles[0].cues[0].radius_meters = 0.0;
        assert_eq!(
            world.compile(&catalog()).unwrap_err().code,
            "invalid_audio_cue"
        );

        let mut world = winter_world();
        world.animation_profiles[0].bindings[0].parameter.clear();
        assert_eq!(
            world.compile(&catalog()).unwrap_err().code,
            "invalid_animation_binding"
        );

        let mut world = winter_world();
        world.modifiers[0]
            .parameter_defaults
            .insert("coverage".into(), f32::NAN);
        assert_eq!(
            world.compile(&catalog()).unwrap_err().code,
            "invalid_modifier"
        );

        let mut world = winter_world();
        world.feature_sets[0].features[0].generator_graph.0.clear();
        assert_eq!(
            world.compile(&catalog()).unwrap_err().code,
            "invalid_feature"
        );
    }

    #[test]
    fn world_profile_round_trips_as_canonical_authored_data() {
        let world = winter_world();
        let json = serde_json::to_string_pretty(&world).unwrap();
        let restored: WorldProfileAsset = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, world);
        restored.compile(&catalog()).unwrap();
    }

    #[test]
    fn compilation_rejects_dangling_assets_and_conditionally_missing_roles() {
        let world = winter_world();
        let mut assets = catalog();
        assets.graph_kinds.remove(&asset("graph-crystals"));
        assert_eq!(
            world.clone().compile(&assets).unwrap_err().code,
            "invalid_feature"
        );

        let mut world = world;
        world.material_palettes[0]
            .bindings
            .remove(&role("cover.snow"));
        assert_eq!(
            world.compile(&catalog()).unwrap_err().code,
            "missing_material_role"
        );
    }
}
