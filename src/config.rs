//! YAML-driven scene configuration.
//!
//! A scene file (`scenes/*.yaml`) wires together separate config files:
//!   - `environments/*.yaml` — virtual acoustic space (dimensions, wall materials)
//!   - `rooms/*.yaml`        — atrium geometry (physical speaker room)
//!   - `sources/*.yaml`      — sound identity (audio file, SPL, directivity)
//!   - `atmospheres/*.yaml`  — atmospheric absorption conditions
//!
//! The scene itself only adds placement (positions, orbits) and mixing
//! parameters (speakers, normalization, distance model).

use std::path::Path;
use std::sync::Arc;

use serde::{Deserialize, Serialize};

use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::decode::decode_file;
use crate::audio::distance::DistanceModel;
use crate::audio::propagation::GroundProperties;
use crate::audio::sound_profile::SoundProfile;
use crate::audio::synth_node::SynthNode;
use crate::audio::test_node::TestNode;
use crate::engine::scene::{AudioScene, InitialSourceState};
use crate::pipeline::{build_all_pipelines, PipelineParams};
use crate::synth::canopy_wind::CanopyWindSource;
use crate::synth::field_wind::FieldWindSource;
use crate::synth::rain::RainSource;
use crate::synth::rain_v2::RainSourceV2;
use crate::synth::river::RiverSource;
use crate::synth::soft_wind::SoftWindSource;
use crate::synth::storm_wind::StormWindSource;
use crate::synth::wave::WaveSource;
use crate::world::room::{BoxRoom, Room};
use crate::world::types::Vec3;
use atrium_core::directivity::DirectivityPattern;
use atrium_core::listener::Listener;
use atrium_core::panner::DistanceModelType;
use atrium_core::speaker::{ChannelMode, RenderMode, SpeakerLayout};

// ── Top-level scene config ──────────────────────────────────────────────────

/// A scene: references environment, atrium, source, processor, and atmosphere
/// files, adds listener placement, speaker layout, and mixing parameters.
#[derive(Deserialize, Serialize, Clone)]
pub struct SceneConfig {
    /// Path to environment definition file (e.g. "environments/riverside.yaml").
    /// The virtual acoustic space where sources live.
    #[serde(alias = "room")]
    pub environment: String,
    /// Path to atrium definition file (e.g. "rooms/atrium_6x4.yaml").
    /// The physical speaker room dimensions. Optional — defaults to environment dims.
    pub atrium: Option<String>,
    pub listener: ListenerConfig,
    #[serde(default = "default_master_gain")]
    pub master_gain: f32,
    #[serde(default)]
    pub distance_model: DistanceModelConfig,
    pub speakers: SpeakerConfig,
    #[serde(default)]
    pub normalization: NormalizationConfig,
    pub sources: Vec<SourceEntry>,
    /// Path to atmosphere definition file (e.g. "atmospheres/default.yaml").
    /// Omit for standard conditions.
    pub atmosphere: Option<String>,
    /// Path to SOFA HRTF file for HRTF rendering (e.g. "assets/hrtf/default.sofa").
    /// Defaults to "assets/hrtf/default.sofa" if omitted.
    #[serde(default = "default_hrtf_path")]
    pub hrtf: String,
}

fn default_master_gain() -> f32 {
    1.0
}

fn default_hrtf_path() -> String {
    "assets/hrtf/default.sofa".into()
}

// ── File-loaded configs (rooms/, processors/, atmospheres/) ─────────────────

/// Environment geometry + acoustic properties (loaded from `environments/*.yaml`).
/// Defines the virtual acoustic space where sources live — dimensions, wall
/// materials, ground surface, and broadband reflectivity.
#[derive(Deserialize)]
pub struct EnvironmentConfig {
    pub width: f32,
    pub depth: f32,
    pub height: f32,
    /// Spawn point — where the atrium center is placed within the environment.
    /// Scene positions (listener, sources, speakers) are atrium-local and get
    /// offset by this point to become world coordinates.
    /// Defaults to the center of the environment: [width/2, depth/2, 0].
    pub spawn: Option<[f32; 3]>,
    /// Ground surface factor for ISO 9613-2 ground effect (0.0 = hard, 1.0 = porous).
    /// Default: 0.0 (hard reflective floor like concrete or tile).
    #[serde(default)]
    pub ground_factor: f32,
    /// Broadband wall reflectivity (energy domain, 0.0–1.0).
    /// Used for image source reflection gain and Sabine RT60.
    /// Default: 0.9 (typical indoor room).
    #[serde(default = "default_wall_reflectivity")]
    pub wall_reflectivity: f32,
    /// Per-wall material names for frequency-dependent absorption.
    #[serde(default)]
    pub walls: WallsConfig,
    /// Explicit late-reverb decay (mid-band RT60, seconds). When set, overrides
    /// the geometry/material-derived Sabine decay in the FDN — lets you dial a
    /// target "atrium tail" (e.g. 3.5 s) that room dimensions alone can't hit.
    #[serde(default)]
    pub rt60_seconds: Option<f32>,
    /// Explicit late-reverb pre-delay (milliseconds). When set, overrides the
    /// mean-free-path pre-delay (e.g. 20 ms for clarity before the wash).
    #[serde(default)]
    pub pre_delay_ms: Option<f32>,
}

fn default_wall_reflectivity() -> f32 {
    0.9
}

/// Per-wall material configuration. Each wall can specify a material name
/// (e.g. "stone", "wood", "open"). Unspecified walls use `default`.
#[derive(Deserialize)]
pub struct WallsConfig {
    /// Fallback material for walls not individually specified.
    #[serde(default = "default_wall_name")]
    pub default: String,
    pub floor: Option<String>,   // -Z
    pub ceiling: Option<String>, // +Z
    pub north: Option<String>,   // +Y
    pub south: Option<String>,   // -Y
    pub east: Option<String>,    // +X
    pub west: Option<String>,    // -X
}

impl Default for WallsConfig {
    fn default() -> Self {
        Self {
            default: default_wall_name(),
            floor: None,
            ceiling: None,
            north: None,
            south: None,
            east: None,
            west: None,
        }
    }
}

fn default_wall_name() -> String {
    "hard_wall".into()
}

/// Atrium geometry (loaded from `rooms/*.yaml` or inline).
/// The physical speaker room — only dimensions, no acoustic properties.
#[derive(Deserialize)]
pub struct AtriumConfig {
    pub width: f32,
    pub depth: f32,
    pub height: f32,
}

/// Map a material name string to a `WallMaterial` preset.
fn parse_wall_material(name: &str) -> crate::pipeline::path::WallMaterial {
    use crate::pipeline::path::WallMaterial;
    match name {
        "hard_wall" => WallMaterial::hard_wall(),
        "stone" => WallMaterial::stone(),
        "wood" => WallMaterial::wood(),
        "glass" => WallMaterial::glass(),
        "carpet" => WallMaterial::carpet(),
        "ceiling_tile" => WallMaterial::ceiling_tile(),
        "grass" => WallMaterial::grass(),
        "open" => WallMaterial::open(),
        other => {
            eprintln!("warning: unknown wall material '{other}', using hard_wall");
            WallMaterial::hard_wall()
        }
    }
}

/// Build the 6-wall material array from an `EnvironmentConfig`.
/// Order: [-X (west), +X (east), -Y (south), +Y (north), -Z (floor), +Z (ceiling)].
fn build_wall_materials(env: &EnvironmentConfig) -> [crate::pipeline::path::WallMaterial; 6] {
    let default = &env.walls.default;
    let get = |wall: &Option<String>| -> crate::pipeline::path::WallMaterial {
        parse_wall_material(wall.as_deref().unwrap_or(default))
    };
    [
        get(&env.walls.west),    // -X
        get(&env.walls.east),    // +X
        get(&env.walls.south),   // -Y
        get(&env.walls.north),   // +Y
        get(&env.walls.floor),   // -Z
        get(&env.walls.ceiling), // +Z
    ]
}

#[derive(Deserialize, Serialize, Clone)]
pub struct ListenerConfig {
    pub position: [f32; 3],
    #[serde(default)]
    pub yaw_degrees: f32,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct DistanceModelConfig {
    #[serde(default = "default_model_type")]
    pub model: String,
    #[serde(default = "default_ref_distance")]
    pub ref_distance: f32,
    #[serde(default = "default_max_distance")]
    pub max_distance: f32,
    #[serde(default = "default_rolloff")]
    pub rolloff: f32,
}

impl Default for DistanceModelConfig {
    fn default() -> Self {
        Self {
            model: "inverse".into(),
            ref_distance: 1.0,
            max_distance: 20.0,
            rolloff: 1.0,
        }
    }
}

fn default_model_type() -> String {
    "inverse".into()
}
fn default_ref_distance() -> f32 {
    1.0
}
fn default_max_distance() -> f32 {
    20.0
}
fn default_rolloff() -> f32 {
    1.0
}

#[derive(Deserialize, Serialize, Clone)]
pub struct SpeakerConfig {
    pub layout: String,
    #[serde(default = "default_render_mode")]
    pub render_mode: String,
    #[serde(default)]
    pub positions: SpeakerPositions,
    /// DBAP rolloff in dB per doubling of distance.
    /// 6.0 = free-field (default), 3–5 for reverberant spaces.
    #[serde(default = "default_dbap_rolloff")]
    pub dbap_rolloff_db: f32,
}

fn default_render_mode() -> String {
    "vbap".into()
}

fn default_dbap_rolloff() -> f32 {
    6.0
}

#[derive(Deserialize, Serialize, Clone, Default)]
pub struct SpeakerPositions {
    pub fl: Option<[f32; 3]>,
    pub fr: Option<[f32; 3]>,
    pub c: Option<[f32; 3]>,
    pub rl: Option<[f32; 3]>,
    pub rr: Option<[f32; 3]>,
    // stereo
    pub l: Option<[f32; 3]>,
    pub r: Option<[f32; 3]>,
}

#[derive(Deserialize, Serialize, Clone)]
pub struct NormalizationConfig {
    #[serde(default = "default_target_rms")]
    pub target_rms: f32,
    /// SPL that maps to 0 dBFS (digital full scale).
    /// gain = 10^((spl - spl_reference) / 20).
    /// IEC 61672 standard: 94 dB (1 Pa RMS calibration tone).
    #[serde(default = "default_spl_reference")]
    pub spl_reference: f32,
    /// SPL hearing threshold in dB — below this level a source is considered inaudible.
    /// Used to compute audible_radius via ISO 9613 free-field propagation:
    ///   d_audible = 10^((reference_spl - spl_threshold) / 20)
    /// Default: 20 dB SPL (quiet room hearing floor).
    #[serde(default = "default_spl_threshold")]
    pub spl_threshold: f32,
}

impl Default for NormalizationConfig {
    fn default() -> Self {
        Self {
            target_rms: 0.5,
            spl_reference: 94.0,
            spl_threshold: 20.0,
        }
    }
}

fn default_target_rms() -> f32 {
    0.5
}
fn default_spl_reference() -> f32 {
    94.0
}
fn default_spl_threshold() -> f32 {
    20.0
}

// ── Source configs (scene entry + file definition) ──────────────────────────

/// Scene entry: places a source in the scene. The sound definition comes either
/// from a referenced `sources/*.yaml` file (`source`) or from inline fields
/// (`audio` + `reference_spl` + …). Inline fields also act as overrides on top
/// of a referenced file — so a live SPL/spread/directivity edit is captured on
/// Save without needing to rewrite the source yaml, and a browsed audio file
/// with no preset yaml round-trips as a fully self-contained entry.
#[derive(Deserialize, Serialize, Clone)]
pub struct SourceEntry {
    /// Path to a source definition YAML file (e.g. "sources/djembe.yaml").
    /// Optional when the entry is defined inline via `audio`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub source: Option<String>,
    /// Path to the audio file (e.g. "assets/frog.mp3"). Overrides the referenced
    /// def's path; required when `source` is absent.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<String>,
    /// Reference SPL in dB at 1 m. Overrides the referenced def.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reference_spl: Option<f32>,
    /// Directivity pattern ("omni", "cardioid", …). Overrides the referenced def.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub directivity: Option<String>,
    /// MDAP spread (0.0 = point, 1.0 = full surround). Overrides the referenced def.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub spread: Option<f32>,
    /// Display name (defaults to filename stem if omitted).
    pub name: Option<String>,
    /// UI color as hex string (e.g. "#ff6b35"). Defaults to palette by index.
    pub color: Option<String>,
    pub position: [f32; 3],
    #[serde(default)]
    pub orbit_radius: f32,
    #[serde(default)]
    pub orbit_speed: f32,
}

/// The sound identity an entry resolved to: a sample file or a synth spec.
enum ResolvedSound {
    Sample { audio_path: String },
    Synth { spec: SynthSpec },
}

/// Resolved intrinsic sound properties for one entry, merging a referenced
/// `SourceDef` (if any) with the entry's inline overrides.
struct ResolvedSource {
    sound: ResolvedSound,
    reference_spl: f32,
    directivity: String,
    spread: f32,
}

impl SourceEntry {
    /// Resolve this entry's sound definition: load the referenced yaml (if any)
    /// and apply inline overrides on top.
    fn resolve(&self) -> Result<ResolvedSource, Box<dyn std::error::Error>> {
        let def: Option<SourceDef> = match &self.source {
            Some(path) => {
                let contents =
                    std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
                Some(serde_yaml::from_str(&contents).map_err(|e| format!("{}: {}", path, e))?)
            }
            None => None,
        };

        // Split the def into its sound identity + intrinsic defaults.
        let (def_sound, def_spl, def_directivity, def_spread) = match def {
            Some(SourceDef::Sample(sample)) => (
                Some(ResolvedSound::Sample {
                    audio_path: sample.path,
                }),
                Some(sample.reference_spl),
                Some(sample.directivity),
                Some(sample.spread),
            ),
            Some(SourceDef::Synth(synth)) => (
                Some(ResolvedSound::Synth { spec: synth.spec }),
                Some(synth.reference_spl),
                Some(synth.directivity),
                Some(synth.spread),
            ),
            None => (None, None, None, None),
        };

        // The entry-level `audio:` override replaces a sample def's file.
        // Combining it with a synth def is contradictory — reject it.
        let sound = match (self.audio.clone(), def_sound) {
            (Some(_), Some(ResolvedSound::Synth { .. })) => {
                return Err("`audio:` override cannot apply to a synth source definition".into())
            }
            (Some(audio_path), _) => ResolvedSound::Sample { audio_path },
            (None, Some(sound)) => sound,
            (None, None) => return Err("source entry needs either `source` or `audio`".into()),
        };

        Ok(ResolvedSource {
            sound,
            reference_spl: self.reference_spl.or(def_spl).unwrap_or(70.0),
            directivity: self
                .directivity
                .clone()
                .or(def_directivity)
                .unwrap_or_else(|| "omni".into()),
            spread: self.spread.or(def_spread).unwrap_or(0.0),
        })
    }

    /// A display name: explicit `name`, else the stem of the referenced yaml or
    /// the audio file.
    fn display_name(&self) -> String {
        if let Some(name) = &self.name {
            return name.clone();
        }
        let stem_of = |p: &str| {
            Path::new(p)
                .file_stem()
                .and_then(|s| s.to_str())
                .map(|s| s.to_string())
        };
        self.source
            .as_deref()
            .and_then(stem_of)
            .or_else(|| self.audio.as_deref().and_then(stem_of))
            .unwrap_or_else(|| "?".to_string())
    }
}

/// Source definition (loaded from `sources/*.yaml`): intrinsic sound identity.
/// Either a sample player (`path:` to an audio file) or a procedural synth
/// generator (`synth:` kind plus generator parameters).
#[derive(Deserialize)]
#[serde(untagged)]
pub enum SourceDef {
    Sample(SampleSourceDef),
    Synth(SynthSourceDef),
}

/// Sample-file source definition.
#[derive(Deserialize)]
pub struct SampleSourceDef {
    pub path: String,
    pub reference_spl: SplValue,
    #[serde(default = "default_directivity")]
    pub directivity: String,
    #[serde(default)]
    pub spread: f32,
}

/// Procedural synth source definition. Generator parameters sit at the same
/// YAML level as the calibration fields (`synth: field_wind` +
/// `min_speed: 1.0` + …).
#[derive(Deserialize)]
pub struct SynthSourceDef {
    #[serde(flatten)]
    pub spec: SynthSpec,
    pub reference_spl: SplValue,
    #[serde(default = "default_directivity")]
    pub directivity: String,
    #[serde(default)]
    pub spread: f32,
}

/// Which procedural generator to run, with its parameters. Tagged by the
/// `synth:` field. Every parameter is optional — unset fields keep the
/// generator's own defaults, so YAML only states what it wants to change.
#[derive(Deserialize, Clone, Debug)]
#[serde(tag = "synth", rename_all = "snake_case")]
pub enum SynthSpec {
    CanopyWind(CanopyWindSynthParams),
    FieldWind(FieldWindSynthParams),
    SoftWind(SoftWindSynthParams),
    StormWind(StormWindSynthParams),
    Rain(RainSynthParams),
    RainV2(RainV2SynthParams),
    River(RiverSynthParams),
    Waves(WaveSynthParams),
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct StormWindSynthParams {
    pub min_speed: Option<f32>,
    pub max_speed: Option<f32>,
    pub change_time_min: Option<f32>,
    pub change_time_max: Option<f32>,
    pub gust_duration_min: Option<f32>,
    pub gust_duration_max: Option<f32>,
    pub turbulence_time_min: Option<f32>,
    pub turbulence_time_max: Option<f32>,
    pub gust_strength: Option<f32>,
    pub rise_bias: Option<f32>,
    pub turbulence_depth: Option<f32>,
    pub gust_brightness: Option<f32>,
    pub turbulence_brightness: Option<f32>,
    pub debris_level: Option<f32>,
    pub structure_level: Option<f32>,
    pub pressure_gain: Option<f32>,
    pub roar_gain: Option<f32>,
    pub shear_gain: Option<f32>,
    pub tear_gain: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct CanopyWindSynthParams {
    pub min_speed: Option<f32>,
    pub max_speed: Option<f32>,
    pub change_time_min: Option<f32>,
    pub change_time_max: Option<f32>,
    pub gust_duration_min: Option<f32>,
    pub gust_duration_max: Option<f32>,
    pub turbulence_time_min: Option<f32>,
    pub turbulence_time_max: Option<f32>,
    pub gust_strength: Option<f32>,
    pub rise_bias: Option<f32>,
    pub turbulence_depth: Option<f32>,
    pub gust_brightness: Option<f32>,
    pub turbulence_brightness: Option<f32>,
    pub foliage_density: Option<f32>,
    pub leaf_dryness: Option<f32>,
    pub branch_level: Option<f32>,
    pub body_gain: Option<f32>,
    pub rustle_gain: Option<f32>,
    pub contact_gain: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct FieldWindSynthParams {
    pub min_speed: Option<f32>,
    pub max_speed: Option<f32>,
    pub change_time_min: Option<f32>,
    pub change_time_max: Option<f32>,
    pub gust_duration_min: Option<f32>,
    pub gust_duration_max: Option<f32>,
    pub turbulence_time_min: Option<f32>,
    pub turbulence_time_max: Option<f32>,
    pub gust_strength: Option<f32>,
    pub rise_bias: Option<f32>,
    pub gust_brightness: Option<f32>,
    pub turbulence_brightness: Option<f32>,
    pub low_gain: Option<f32>,
    pub body_gain: Option<f32>,
    pub mid_gain: Option<f32>,
    pub presence_gain: Option<f32>,
    pub air_gain: Option<f32>,
    pub turbulence_depth: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct SoftWindSynthParams {
    pub min_speed: Option<f32>,
    pub max_speed: Option<f32>,
    pub change_time_min: Option<f32>,
    pub change_time_max: Option<f32>,
    pub gust_duration_min: Option<f32>,
    pub gust_duration_max: Option<f32>,
    pub turbulence_time_min: Option<f32>,
    pub turbulence_time_max: Option<f32>,
    pub gust_strength: Option<f32>,
    pub rise_bias: Option<f32>,
    pub turbulence_depth: Option<f32>,
    pub gust_brightness: Option<f32>,
    pub turbulence_brightness: Option<f32>,
    pub low_gain: Option<f32>,
    pub body_gain: Option<f32>,
    pub mid_gain: Option<f32>,
    pub presence_gain: Option<f32>,
    pub air_gain: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct RainSynthParams {
    pub intensity: Option<f32>,
    pub drop_rate: Option<f32>,
    pub drip_rate: Option<f32>,
    pub hiss_gain: Option<f32>,
    pub brown_gain: Option<f32>,
    pub impact_gain: Option<f32>,
    pub bubble_gain: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct RainV2SynthParams {
    pub intensity: Option<f32>,
    pub drip_rate: Option<f32>,
    pub impact_gain: Option<f32>,
    pub bubble_gain: Option<f32>,
    pub bed_gain: Option<f32>,
    pub texture_gain: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct RiverSynthParams {
    pub min_flow_speed: Option<f32>,
    pub max_flow_speed: Option<f32>,
    pub change_time_min: Option<f32>,
    pub change_time_max: Option<f32>,
    pub eddy_time_min: Option<f32>,
    pub eddy_time_max: Option<f32>,
    pub eddy_depth: Option<f32>,
    pub body_gain: Option<f32>,
    pub current_gain: Option<f32>,
    pub bubble_activity: Option<f32>,
    pub splash_rate: Option<f32>,
    pub splash_gain: Option<f32>,
    pub spray_gain: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

#[derive(Deserialize, Clone, Debug, Default)]
pub struct WaveSynthParams {
    pub period: Option<f32>,
    pub crash_prob: Option<f32>,
    pub roar_level: Option<f32>,
    pub hiss_level: Option<f32>,
    pub crash_gain: Option<f32>,
    pub master_gain: Option<f32>,
    pub seed: Option<u64>,
}

impl SynthSpec {
    pub fn kind_name(&self) -> &'static str {
        match self {
            SynthSpec::CanopyWind(_) => "canopy_wind",
            SynthSpec::FieldWind(_) => "field_wind",
            SynthSpec::SoftWind(_) => "soft_wind",
            SynthSpec::StormWind(_) => "storm_wind",
            SynthSpec::Rain(_) => "rain",
            SynthSpec::RainV2(_) => "rain_v2",
            SynthSpec::River(_) => "river",
            SynthSpec::Waves(_) => "waves",
        }
    }

    /// Construct the generator at `position`. `default_seed` applies when the
    /// params don't pin one — the scene builder passes a slot-stable seed so
    /// reloading a scene sounds statistically identical.
    pub fn build_generator(
        &self,
        position: Vec3,
        default_seed: u64,
    ) -> Box<dyn atrium_core::source::SoundSource> {
        match self {
            SynthSpec::CanopyWind(params) => {
                let mut generator = CanopyWindSource::new(
                    position,
                    params.min_speed.unwrap_or(1.5),
                    params.max_speed.unwrap_or(8.0),
                    params.seed.unwrap_or(default_seed),
                );
                let (default_min, default_max) = generator.change_time_range();
                generator.set_change_time_range(
                    params.change_time_min.unwrap_or(default_min),
                    params.change_time_max.unwrap_or(default_max),
                );
                let (default_min, default_max) = generator.gust_duration_range();
                generator.set_gust_duration_range(
                    params.gust_duration_min.unwrap_or(default_min),
                    params.gust_duration_max.unwrap_or(default_max),
                );
                let (default_min, default_max) = generator.turbulence_time_range();
                generator.set_turbulence_time_range(
                    params.turbulence_time_min.unwrap_or(default_min),
                    params.turbulence_time_max.unwrap_or(default_max),
                );
                if let Some(value) = params.gust_strength {
                    generator.gust_strength = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.rise_bias {
                    generator.rise_bias = value.clamp(-1.0, 1.0);
                }
                if let Some(value) = params.turbulence_depth {
                    generator.turbulence_depth = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.gust_brightness {
                    generator.gust_brightness = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.turbulence_brightness {
                    generator.turbulence_brightness = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.foliage_density {
                    generator.foliage_density = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.leaf_dryness {
                    generator.leaf_dryness = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.branch_level {
                    generator.branch_level = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.body_gain {
                    generator.body_gain = value.max(0.0);
                }
                if let Some(value) = params.rustle_gain {
                    generator.rustle_gain = value.max(0.0);
                }
                if let Some(value) = params.contact_gain {
                    generator.contact_gain = value.max(0.0);
                }
                if let Some(value) = params.master_gain {
                    generator.master_gain = value.clamp(0.0, 2.0);
                }
                Box::new(generator)
            }
            SynthSpec::FieldWind(params) => {
                let mut generator = FieldWindSource::new(
                    position,
                    params.min_speed.unwrap_or(1.0),
                    params.max_speed.unwrap_or(8.0),
                    params.seed.unwrap_or(default_seed),
                );
                let (default_min, default_max) = generator.change_time_range();
                generator.set_change_time_range(
                    params.change_time_min.unwrap_or(default_min),
                    params.change_time_max.unwrap_or(default_max),
                );
                let (default_min, default_max) = generator.gust_duration_range();
                generator.set_gust_duration_range(
                    params.gust_duration_min.unwrap_or(default_min),
                    params.gust_duration_max.unwrap_or(default_max),
                );
                let (default_min, default_max) = generator.turbulence_time_range();
                generator.set_turbulence_time_range(
                    params.turbulence_time_min.unwrap_or(default_min),
                    params.turbulence_time_max.unwrap_or(default_max),
                );
                if let Some(value) = params.gust_strength {
                    generator.gust_strength = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.rise_bias {
                    generator.rise_bias = value.clamp(-1.0, 1.0);
                }
                if let Some(value) = params.gust_brightness {
                    generator.gust_brightness = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.turbulence_brightness {
                    generator.turbulence_brightness = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.low_gain {
                    generator.low_gain = value;
                }
                if let Some(value) = params.body_gain {
                    generator.body_gain = value;
                }
                if let Some(value) = params.mid_gain {
                    generator.mid_gain = value;
                }
                if let Some(value) = params.presence_gain {
                    generator.presence_gain = value;
                }
                if let Some(value) = params.air_gain {
                    generator.air_gain = value;
                }
                if let Some(value) = params.turbulence_depth {
                    generator.turbulence_depth = value;
                }
                if let Some(value) = params.master_gain {
                    generator.master_gain = value;
                }
                Box::new(generator)
            }
            SynthSpec::SoftWind(params) => {
                let mut generator = SoftWindSource::new(
                    position,
                    params.min_speed.unwrap_or(1.0),
                    params.max_speed.unwrap_or(5.0),
                    params.seed.unwrap_or(default_seed),
                );
                let (default_min, default_max) = generator.change_time_range();
                generator.set_change_time_range(
                    params.change_time_min.unwrap_or(default_min),
                    params.change_time_max.unwrap_or(default_max),
                );
                let (default_min, default_max) = generator.gust_duration_range();
                generator.set_gust_duration_range(
                    params.gust_duration_min.unwrap_or(default_min),
                    params.gust_duration_max.unwrap_or(default_max),
                );
                let (default_min, default_max) = generator.turbulence_time_range();
                generator.set_turbulence_time_range(
                    params.turbulence_time_min.unwrap_or(default_min),
                    params.turbulence_time_max.unwrap_or(default_max),
                );
                if let Some(value) = params.gust_strength {
                    generator.gust_strength = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.rise_bias {
                    generator.rise_bias = value.clamp(-1.0, 1.0);
                }
                if let Some(value) = params.turbulence_depth {
                    generator.turbulence_depth = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.gust_brightness {
                    generator.gust_brightness = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.turbulence_brightness {
                    generator.turbulence_brightness = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.low_gain {
                    generator.low_gain = value.max(0.0);
                }
                if let Some(value) = params.body_gain {
                    generator.body_gain = value.max(0.0);
                }
                if let Some(value) = params.mid_gain {
                    generator.mid_gain = value.max(0.0);
                }
                if let Some(value) = params.presence_gain {
                    generator.presence_gain = value.max(0.0);
                }
                if let Some(value) = params.air_gain {
                    generator.air_gain = value.max(0.0);
                }
                if let Some(value) = params.master_gain {
                    generator.master_gain = value.clamp(0.0, 2.0);
                }
                Box::new(generator)
            }
            SynthSpec::StormWind(params) => {
                let mut generator = StormWindSource::new(
                    position,
                    params.min_speed.unwrap_or(8.0),
                    params.max_speed.unwrap_or(18.0),
                    params.seed.unwrap_or(default_seed),
                );
                let (default_min, default_max) = generator.change_time_range();
                generator.set_change_time_range(
                    params.change_time_min.unwrap_or(default_min),
                    params.change_time_max.unwrap_or(default_max),
                );
                let (default_min, default_max) = generator.gust_duration_range();
                generator.set_gust_duration_range(
                    params.gust_duration_min.unwrap_or(default_min),
                    params.gust_duration_max.unwrap_or(default_max),
                );
                let (default_min, default_max) = generator.turbulence_time_range();
                generator.set_turbulence_time_range(
                    params.turbulence_time_min.unwrap_or(default_min),
                    params.turbulence_time_max.unwrap_or(default_max),
                );
                if let Some(value) = params.gust_strength {
                    generator.gust_strength = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.rise_bias {
                    generator.rise_bias = value.clamp(-1.0, 1.0);
                }
                if let Some(value) = params.turbulence_depth {
                    generator.turbulence_depth = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.gust_brightness {
                    generator.gust_brightness = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.turbulence_brightness {
                    generator.turbulence_brightness = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.debris_level {
                    generator.debris_level = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.structure_level {
                    generator.structure_level = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.pressure_gain {
                    generator.pressure_gain = value.max(0.0);
                }
                if let Some(value) = params.roar_gain {
                    generator.roar_gain = value.max(0.0);
                }
                if let Some(value) = params.shear_gain {
                    generator.shear_gain = value.max(0.0);
                }
                if let Some(value) = params.tear_gain {
                    generator.tear_gain = value.max(0.0);
                }
                if let Some(value) = params.master_gain {
                    generator.master_gain = value.clamp(0.0, 2.0);
                }
                Box::new(generator)
            }
            SynthSpec::Rain(params) => {
                let mut generator = RainSource::new(
                    position,
                    params.intensity.unwrap_or(0.5),
                    params.seed.unwrap_or(default_seed),
                );
                if let Some(value) = params.drop_rate {
                    generator.drop_rate = value;
                }
                if let Some(value) = params.drip_rate {
                    generator.drip_rate = value;
                }
                if let Some(value) = params.hiss_gain {
                    generator.hiss_gain = value;
                }
                if let Some(value) = params.brown_gain {
                    generator.brown_gain = value;
                }
                if let Some(value) = params.impact_gain {
                    generator.impact_gain = value;
                }
                if let Some(value) = params.bubble_gain {
                    generator.bubble_gain = value;
                }
                if let Some(value) = params.master_gain {
                    generator.master_gain = value;
                }
                Box::new(generator)
            }
            SynthSpec::RainV2(params) => {
                let mut generator = RainSourceV2::new(
                    position,
                    params.intensity.unwrap_or(0.5),
                    params.seed.unwrap_or(default_seed),
                );
                if let Some(value) = params.drip_rate {
                    generator.drip_rate = value;
                }
                if let Some(value) = params.impact_gain {
                    generator.impact_gain = value;
                }
                if let Some(value) = params.bubble_gain {
                    generator.bubble_gain = value;
                }
                if let Some(value) = params.bed_gain {
                    generator.bed_gain = value;
                }
                if let Some(value) = params.texture_gain {
                    generator.texture_gain = value;
                }
                if let Some(value) = params.master_gain {
                    generator.master_gain = value;
                }
                Box::new(generator)
            }
            SynthSpec::River(params) => {
                let mut generator = RiverSource::new(
                    position,
                    params.min_flow_speed.unwrap_or(0.3),
                    params.max_flow_speed.unwrap_or(1.2),
                    params.seed.unwrap_or(default_seed),
                );
                let (default_min, default_max) = generator.change_time_range();
                generator.set_change_time_range(
                    params.change_time_min.unwrap_or(default_min),
                    params.change_time_max.unwrap_or(default_max),
                );
                let (default_min, default_max) = generator.eddy_time_range();
                generator.set_eddy_time_range(
                    params.eddy_time_min.unwrap_or(default_min),
                    params.eddy_time_max.unwrap_or(default_max),
                );
                if let Some(value) = params.eddy_depth {
                    generator.eddy_depth = value.clamp(0.0, 1.0);
                }
                if let Some(value) = params.body_gain {
                    generator.body_gain = value.max(0.0);
                }
                if let Some(value) = params.current_gain {
                    generator.current_gain = value.max(0.0);
                }
                if let Some(value) = params.bubble_activity {
                    generator.bubble_activity = value.max(0.0);
                }
                if let Some(value) = params.splash_rate {
                    generator.splash_rate = value.max(0.0);
                }
                if let Some(value) = params.splash_gain {
                    generator.splash_gain = value.max(0.0);
                }
                if let Some(value) = params.spray_gain {
                    generator.spray_gain = value.max(0.0);
                }
                if let Some(value) = params.master_gain {
                    generator.master_gain = value.clamp(0.0, 2.0);
                }
                Box::new(generator)
            }
            SynthSpec::Waves(params) => {
                let mut generator = WaveSource::new(
                    position,
                    params.period.unwrap_or(6.0),
                    params.crash_prob.unwrap_or(0.25),
                    params.seed.unwrap_or(default_seed),
                );
                if let Some(value) = params.roar_level {
                    generator.roar_level = value;
                }
                if let Some(value) = params.hiss_level {
                    generator.hiss_level = value;
                }
                if let Some(value) = params.crash_gain {
                    generator.crash_gain = value;
                }
                if let Some(value) = params.master_gain {
                    generator.master_gain = value;
                }
                Box::new(generator)
            }
        }
    }
}

fn default_directivity() -> String {
    "omni".into()
}

/// SPL value in dB at 1 meter (IEC 61672 measurement distance).
/// Always a numeric value — no presets, no magic strings, just real-world dB.
type SplValue = f32;

fn resolve_spl(db: f32) -> SoundProfile {
    SoundProfile { reference_spl: db }
}

#[derive(Deserialize)]
pub struct AtmosphereConfig {
    #[serde(default = "default_temperature")]
    pub temperature_c: f32,
    #[serde(default = "default_humidity")]
    pub humidity_pct: f32,
    #[serde(default = "default_pressure")]
    pub pressure_kpa: f32,
}

impl Default for AtmosphereConfig {
    fn default() -> Self {
        Self {
            temperature_c: 20.0,
            humidity_pct: 50.0,
            pressure_kpa: 101.325,
        }
    }
}

fn default_temperature() -> f32 {
    20.0
}
fn default_humidity() -> f32 {
    50.0
}
fn default_pressure() -> f32 {
    101.325
}

// ── Build result ────────────────────────────────────────────────────────────

pub struct BuildResult {
    pub scene: AudioScene,
    pub scene_json: String,
    pub source_names: Vec<String>,
    /// Pipeline mix stage names (for TUI display).
    pub pipeline_post: Vec<String>,
    /// Channel labels for TUI display (e.g. ["FL", "FR", "C", "LFE", "RL", "RR"]).
    pub channel_labels: Vec<String>,
}

/// Result of building sources.
struct BuildSourcesResult {
    sources: Vec<Box<dyn atrium_core::source::SoundSource>>,
    metas: Vec<SourceMeta>,
    /// Per-source spectral profile bands (24 Bark bands, dB relative to RMS).
    spectral_profiles: Vec<[f32; crate::audio::spectral_profile::BARK_BANDS]>,
    /// Per-source base amplitude (sone-based gain, before spatial attenuation).
    source_amplitudes: Vec<f32>,
}

/// Default color palette for sources when no color is specified in YAML.
const SOURCE_COLORS: &[&str] = &[
    "#ff6b35", "#ffc107", "#ce93d8", "#4fc3f7", "#66bb6a", "#ef5350", "#ff8a65", "#ab47bc",
    "#26c6da", "#9ccc65",
];

/// Metadata collected during source building, serialized to JSON for the browser.
struct SourceMeta {
    name: String,
    color: String,
    spl: f32,
    ref_dist: f32,
    amplitude: f32,
    audible_radius: f32,
    directivity: String,
    directivity_alpha: f32,
    spread: f32,
    position: [f32; 3],
    orbit_radius: f32,
    orbit_speed: f32,
    synth_kind: Option<String>,
    emitter_kind: String,
}

// ── Loading & building ──────────────────────────────────────────────────────

/// Load and deserialize a YAML file into any serde-compatible type.
fn load_yaml<T: serde::de::DeserializeOwned>(path: &str) -> Result<T, Box<dyn std::error::Error>> {
    // Warn about absolute or parent-traversing paths (not sandboxed, but logged)
    if std::path::Path::new(path).is_absolute() || path.contains("..") {
        eprintln!("warning: loading file from non-relative path: {path}");
    }
    let contents = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
    serde_yaml::from_str(&contents).map_err(|e| format!("{}: {}", path, e).into())
}

impl SceneConfig {
    pub fn load(path: &str) -> Result<Self, Box<dyn std::error::Error>> {
        load_yaml(path)
    }

    pub fn build(self) -> Result<BuildResult, Box<dyn std::error::Error>> {
        // Load environment (virtual acoustic space) from file
        let environment_cfg: EnvironmentConfig = load_yaml(&self.environment)?;
        let environment = BoxRoom::new(
            environment_cfg.width,
            environment_cfg.depth,
            environment_cfg.height,
        );

        // Spawn point: where the atrium center sits in the environment.
        // Defaults to the center of the environment floor.
        let spawn = match environment_cfg.spawn {
            Some(arr) => Vec3::new(arr[0], arr[1], arr[2]),
            None => Vec3::new(
                environment_cfg.width / 2.0,
                environment_cfg.depth / 2.0,
                0.0,
            ),
        };

        // Load atrium (physical speaker room) dimensions — optional, defaults to environment
        let atrium_cfg: AtriumConfig = if let Some(ref path) = self.atrium {
            load_yaml(path)?
        } else {
            AtriumConfig {
                width: environment_cfg.width,
                depth: environment_cfg.depth,
                height: environment_cfg.height,
            }
        };

        // Scene positions are atrium-local — offset by spawn to get world coordinates.
        let listener_pos = arr_to_vec3(self.listener.position) + spawn;
        let listener = Listener::new(listener_pos, self.listener.yaw_degrees.to_radians());

        // Build speaker layout (atrium-local positions offset by spawn)
        let speaker_layout = self.build_speakers(spawn);
        let render_mode = parse_render_mode(&self.speakers.render_mode);

        // Decode audio and build sources (also collects metadata for the browser)
        let build = self.build_sources(spawn)?;
        let mut sources = build.sources;
        let source_metas = build.metas;
        let mut spectral_profiles = build.spectral_profiles;
        let mut source_amplitudes = build.source_amplitudes;

        // Pre-warm the source pool to a fixed 16 slots with silent placeholders.
        // Live add/remove only swaps a Box into/out of an existing slot, so the
        // parallel vectors and pipeline topology never grow on the audio thread
        // (which would allocate). Scenes with >16 sources are truncated.
        const MAX_SOURCES: usize = atrium_core::telemetry::MAX_SOURCES;
        if sources.len() > MAX_SOURCES {
            eprintln!(
                "warning: scene has {} sources; truncating to the {MAX_SOURCES}-slot pool",
                sources.len()
            );
            sources.truncate(MAX_SOURCES);
            spectral_profiles.truncate(MAX_SOURCES);
            source_amplitudes.truncate(MAX_SOURCES);
        }
        while sources.len() < MAX_SOURCES {
            sources.push(Box::new(crate::audio::silence_node::SilenceNode));
            spectral_profiles.push([0.0; crate::audio::spectral_profile::BARK_BANDS]);
            source_amplitudes.push(0.0);
        }

        let distance_model = DistanceModel {
            ref_distance: self.distance_model.ref_distance,
            max_distance: self.distance_model.max_distance,
            rolloff: self.distance_model.rolloff,
            model: parse_distance_model(&self.distance_model.model),
        };

        // Load atmosphere from file (or defaults if omitted)
        let atmosphere = match &self.atmosphere {
            Some(path) => {
                let cfg: AtmosphereConfig = load_yaml(path)?;
                AtmosphericParams {
                    temperature_c: cfg.temperature_c,
                    humidity_pct: cfg.humidity_pct,
                    pressure_kpa: cfg.pressure_kpa,
                }
            }
            None => AtmosphericParams::default(),
        };

        // Build comprehensive JSON for the browser (all computed values)
        let scene_json = self.build_scene_json(
            &environment_cfg,
            &atrium_cfg,
            spawn,
            &speaker_layout,
            &source_metas,
            &atmosphere,
        );

        let initial_source_states: Vec<InitialSourceState> = self
            .sources
            .iter()
            .map(|entry| InitialSourceState {
                position: arr_to_vec3(entry.position) + spawn,
                orbit_radius: entry.orbit_radius,
                orbit_speed: entry.orbit_speed,
            })
            .collect();

        // Build composable pipelines
        let ground = GroundProperties::mixed(environment_cfg.ground_factor);

        let wall_materials = build_wall_materials(&environment_cfg);
        // Environment's wall_reflectivity is authoritative
        let effective_reflectivity = environment_cfg.wall_reflectivity;
        let (environment_min, environment_max) = environment.bounds();
        let pipeline_params = PipelineParams {
            sample_rate: 48000.0, // will be recalibrated in init_pipelines
            hrtf_path: self.hrtf,
            er_wall_reflectivity: effective_reflectivity,
            distance_model,
            dbap_rolloff_db: self.speakers.dbap_rolloff_db,
            wall_materials: wall_materials.clone(),
            environment_min,
            environment_max,
            reverb_rt60_seconds: environment_cfg.rt60_seconds,
            reverb_pre_delay_ms: environment_cfg.pre_delay_ms,
        };
        let pipelines = build_all_pipelines(&pipeline_params);
        let active_pipeline = render_mode;
        let active_channel_mode = match speaker_layout.total_channels() {
            2 => ChannelMode::Stereo,
            4 => ChannelMode::Quad,
            _ => ChannelMode::Surround51,
        };

        let source_count = sources.len();
        let scene = AudioScene {
            initial_listener_pos: listener_pos,
            initial_listener_yaw: self.listener.yaw_degrees.to_radians(),
            initial_master_gain: self.master_gain,
            initial_source_states,
            initial_atmosphere: atmosphere,
            initial_render_mode: render_mode,
            listener,
            sources,
            environment: Box::new(environment),
            master_gain: self.master_gain,
            sample_rate: 0.0, // set by audio backend
            distance_model,
            speaker_layout,
            atmosphere,
            telemetry_out: None,
            scene_edits: None,
            retired_out: None,
            telemetry_counter: 0,
            telemetry_interval: 6, // ~15 Hz at 512-sample buffers; calibrated later
            #[cfg(feature = "memprof")]
            memprof: crate::engine::memprof::MemProfiler::new(),
            pipelines,
            active_pipeline,
            active_channel_mode,
            ground,
            barriers: Vec::new(),
            wall_materials,
            measurement_mode: false,
            perceptual_layer: crate::pipeline::perceptual::PerceptualLayer::new(source_count),
            spectral_profiles,
            source_amplitudes,
            perceptual_states: Vec::new(),
        };

        let source_names: Vec<String> = source_metas.iter().map(|m| m.name.clone()).collect();

        // Build pipeline description for TUI display
        let pipeline_post = scene.mix_stage_names();

        let channel_labels: Vec<String> = match self.speakers.layout.as_str() {
            "5.1" => ["FL", "FR", "C", "LFE", "RL", "RR"].iter(),
            "quad" => ["FL", "FR", "—", "—", "RL", "RR"].iter(),
            _ => ["L", "R"].iter(),
        }
        .map(|s| s.to_string())
        .collect();

        Ok(BuildResult {
            scene,
            scene_json,
            source_names,
            pipeline_post,
            channel_labels,
        })
    }

    fn build_speakers(&self, spawn: Vec3) -> SpeakerLayout {
        let p = &self.speakers.positions;
        let s = |arr: [f32; 3]| arr_to_vec3(arr) + spawn;
        match self.speakers.layout.as_str() {
            "5.1" => SpeakerLayout::surround_5_1(
                s(p.fl.unwrap_or([-3.0, 2.0, 0.0])),
                s(p.fr.unwrap_or([3.0, 2.0, 0.0])),
                s(p.c.unwrap_or([0.0, 2.0, 0.0])),
                s(p.rl.unwrap_or([-3.0, -2.0, 0.0])),
                s(p.rr.unwrap_or([3.0, -2.0, 0.0])),
            ),
            "quad" => SpeakerLayout::quad(
                s(p.fl.unwrap_or([-3.0, 2.0, 0.0])),
                s(p.fr.unwrap_or([3.0, 2.0, 0.0])),
                s(p.rl.unwrap_or([-3.0, -2.0, 0.0])),
                s(p.rr.unwrap_or([3.0, -2.0, 0.0])),
            ),
            _ => SpeakerLayout::stereo(
                s(p.l.or(p.fl).unwrap_or([-3.0, 2.0, 0.0])),
                s(p.r.or(p.fr).unwrap_or([3.0, 2.0, 0.0])),
            ),
        }
    }

    fn build_scene_json(
        &self,
        environment_cfg: &EnvironmentConfig,
        atrium_cfg: &AtriumConfig,
        spawn: Vec3,
        layout: &SpeakerLayout,
        source_metas: &[SourceMeta],
        atmosphere: &AtmosphericParams,
    ) -> String {
        // Speakers
        let channel_labels = match self.speakers.layout.as_str() {
            "5.1" => &["FL", "FR", "C", "LFE", "RL", "RR"][..],
            "quad" => &["FL", "FR", "—", "—", "RL", "RR"][..],
            _ => &["L", "R"][..],
        };
        let mut speakers = Vec::new();
        for i in 0..layout.speaker_count() {
            if let Some(sp) = layout.speaker_by_index(i) {
                let label = channel_labels.get(sp.channel).unwrap_or(&"?");
                speakers.push(serde_json::json!({
                    "label": label,
                    "x": sp.position.x,
                    "y": sp.position.y,
                    "z": sp.position.z,
                    "channel": sp.channel,
                }));
            }
        }

        // Sources (all computed values from the engine, world coordinates)
        let sources: Vec<_> = source_metas
            .iter()
            .map(|s| {
                let world_pos = arr_to_vec3(s.position) + spawn;
                serde_json::json!({
                    "name": s.name,
                    "color": s.color,
                    "spl": s.spl,
                    "ref_dist": s.ref_dist,
                    "amplitude": s.amplitude,
                    "audible_radius": s.audible_radius,
                    "directivity": s.directivity,
                    "directivity_alpha": s.directivity_alpha,
                    "spread": s.spread,
                    "position": [world_pos.x, world_pos.y, world_pos.z],
                    "orbit_radius": s.orbit_radius,
                    "orbit_speed": s.orbit_speed,
                    "synth_kind": s.synth_kind,
                    "emitter_kind": s.emitter_kind,
                })
            })
            .collect();

        let listener_world = arr_to_vec3(self.listener.position) + spawn;
        serde_json::json!({
            "type": "scene_state",
            "room": {
                "width": environment_cfg.width,
                "depth": environment_cfg.depth,
                "height": environment_cfg.height,
            },
            "atrium": {
                "width": atrium_cfg.width,
                "depth": atrium_cfg.depth,
                "height": atrium_cfg.height,
            },
            "spawn": {
                "x": spawn.x,
                "y": spawn.y,
                "z": spawn.z,
            },
            "listener": {
                "x": listener_world.x,
                "y": listener_world.y,
                "z": listener_world.z,
                "yaw": self.listener.yaw_degrees.to_radians(),
            },
            "master_gain": self.master_gain,
            "distance_model": {
                "model": self.distance_model.model,
                "ref_distance": self.distance_model.ref_distance,
                "max_distance": self.distance_model.max_distance,
                "rolloff": self.distance_model.rolloff,
            },
            "normalization": {
                "spl_threshold": self.normalization.spl_threshold,
                "target_rms": self.normalization.target_rms,
            },
            "render_mode": self.speakers.render_mode,
            "dbap_rolloff_db": self.speakers.dbap_rolloff_db,
            "channel_mode": ChannelMode::valid_for(parse_render_mode(&self.speakers.render_mode))
                .last().map(|m| m.as_str()).unwrap_or("5.1"),
            "render_modes": RenderMode::ALL.iter().map(|m| {
                serde_json::json!({
                    "mode": m.as_str(),
                    "channel_modes": ChannelMode::valid_for(*m).iter()
                        .map(|c| c.as_str()).collect::<Vec<_>>(),
                })
            }).collect::<Vec<_>>(),
            "speakers": speakers,
            "total_channels": layout.total_channels(),
            "lfe_channel": layout.lfe_channel(),
            "sources": sources,
            "atmosphere": {
                "temperature_c": atmosphere.temperature_c,
                "humidity_pct": atmosphere.humidity_pct,
                "pressure_kpa": atmosphere.pressure_kpa,
            },
            "experiments": [],
        })
        .to_string()
    }

    fn build_sources(&self, spawn: Vec3) -> Result<BuildSourcesResult, Box<dyn std::error::Error>> {
        let norm = &self.normalization;
        let mut nodes: Vec<Box<dyn atrium_core::source::SoundSource>> = Vec::new();
        let mut metas: Vec<SourceMeta> = Vec::new();
        let mut spectral_profiles = Vec::<[f32; crate::audio::spectral_profile::BARK_BANDS]>::new();
        let mut source_amplitudes = Vec::<f32>::new();

        let global_ref_dist = self.distance_model.ref_distance;
        let max_dist = self.distance_model.max_distance;

        for (i, entry) in self.sources.iter().enumerate() {
            // Merge referenced def (if any) with inline overrides.
            let resolved = entry.resolve()?;
            let position = arr_to_vec3(entry.position) + spawn;
            let built = match &resolved.sound {
                ResolvedSound::Sample { audio_path } => build_one_source(
                    audio_path,
                    resolved.reference_spl,
                    &resolved.directivity,
                    resolved.spread,
                    position,
                    entry.orbit_radius,
                    entry.orbit_speed,
                    norm,
                    global_ref_dist,
                    max_dist,
                )?,
                ResolvedSound::Synth { spec } => {
                    if entry.orbit_radius != 0.0 || entry.orbit_speed != 0.0 {
                        return Err(format!(
                            "source `{}`: synth sources don't support orbit (motion belongs to the behavior layer)",
                            entry.display_name()
                        )
                        .into());
                    }
                    build_one_synth_source(
                        spec,
                        resolved.reference_spl,
                        &resolved.directivity,
                        resolved.spread,
                        position,
                        // Slot-stable default seed: reloading the scene sounds
                        // statistically identical, multiple synths decorrelate.
                        1000 + i as u64,
                        norm,
                        global_ref_dist,
                        max_dist,
                    )?
                }
            };

            let name = entry.display_name();
            let color = entry
                .color
                .clone()
                .unwrap_or_else(|| SOURCE_COLORS[i % SOURCE_COLORS.len()].to_string());

            println!(
                "  {} → SPL={:.0} dB, amplitude={:.4}, ref_dist={:.2}m, audible={:.2}m",
                name, resolved.reference_spl, built.amplitude, built.ref_dist, built.audible_radius,
            );

            let synth_kind = match &resolved.sound {
                ResolvedSound::Synth { spec } => Some(spec.kind_name().to_string()),
                ResolvedSound::Sample { .. } => None,
            };
            let emitter_kind = built.source.emitter_kind().as_str().to_string();
            metas.push(SourceMeta {
                name,
                color,
                spl: resolved.reference_spl,
                ref_dist: built.ref_dist,
                amplitude: built.amplitude,
                audible_radius: built.audible_radius,
                directivity: resolved.directivity.clone(),
                directivity_alpha: built.directivity_alpha,
                spread: resolved.spread,
                position: entry.position,
                orbit_radius: entry.orbit_radius,
                orbit_speed: entry.orbit_speed,
                synth_kind,
                emitter_kind,
            });

            spectral_profiles.push(built.bands);
            source_amplitudes.push(built.amplitude);
            nodes.push(built.source);
        }

        Ok(BuildSourcesResult {
            sources: nodes,
            metas,
            spectral_profiles,
            source_amplitudes,
        })
    }
}

// ── Single-source builder (shared by scene build + live add) ────────────────

/// A fully-built source plus the derived values the UI and perceptual layer
/// need. Produced on the control thread and, for live adds, shipped into a pool
/// slot via [`crate::engine::edit::SceneEdit`].
pub struct BuiltSource {
    pub source: Box<dyn atrium_core::source::SoundSource>,
    /// 24 Bark-band spectral profile (dB relative to RMS).
    pub bands: [f32; crate::audio::spectral_profile::BARK_BANDS],
    /// Base playback amplitude (sone-based gain, before spatial attenuation).
    pub amplitude: f32,
    /// Per-source reference distance (m) derived from SPL.
    pub ref_dist: f32,
    /// Free-field audible radius (m) at the hearing threshold.
    pub audible_radius: f32,
    /// Directivity polar coefficient (1.0 omni, 0.5 cardioid, …).
    pub directivity_alpha: f32,
}

/// Decode a source audio file and build one `SoundSource` with its amplitude,
/// spectral profile, and reference distance. Used both when building a scene
/// and when adding a source live (the same lossless path, so a live-added
/// source matches a reloaded one exactly). `position` is world coordinates
/// (already offset by the environment spawn point).
#[allow(clippy::too_many_arguments)]
pub fn build_one_source(
    audio_path: &str,
    reference_spl: f32,
    directivity: &str,
    spread: f32,
    position: Vec3,
    orbit_radius: f32,
    orbit_speed: f32,
    normalization: &NormalizationConfig,
    global_ref_dist: f32,
    max_distance: f32,
) -> Result<BuiltSource, Box<dyn std::error::Error>> {
    let buffer = Arc::new(decode_file(Path::new(audio_path))?);
    let profile = resolve_spl(reference_spl);
    let amplitude = profile.amplitude(
        buffer.rms,
        normalization.target_rms,
        normalization.spl_reference,
    );
    let ref_dist = profile.ref_distance(global_ref_dist);
    let pattern = parse_directivity(directivity);
    let bands = buffer.spectral_profile.bands;
    let audible_radius = profile.audible_radius(normalization.spl_threshold, max_distance);

    // Base amplitude at 0 dB SPL, so a live SPL edit can rescale without
    // re-decoding: amplitude = unit_amplitude · 10^(spl/20).
    let rms_correction = if buffer.rms > 1e-6 {
        normalization.target_rms / buffer.rms
    } else {
        1.0
    };
    let unit_amplitude = rms_correction * 10.0_f32.powf(-normalization.spl_reference / 20.0);

    let mut node = TestNode::new(buffer, position, orbit_radius, orbit_speed);
    node.amplitude = amplitude;
    node.unit_amplitude = unit_amplitude;
    node.ref_dist = ref_dist;
    node.pattern = pattern;
    node.spread = spread;

    Ok(BuiltSource {
        source: Box::new(node),
        bands,
        amplitude,
        ref_dist,
        audible_radius,
        directivity_alpha: pattern.alpha(),
    })
}

/// Seconds of preview audio rendered control-side to measure a synth source's
/// RMS and Bark-band spectral profile — the same calibration measurements
/// `build_one_source` reads from a decoded file. Long enough to average over
/// gust cycles and drop statistics.
pub const SYNTH_PREVIEW_SECONDS: f32 = 20.0;
/// Sample rate for the preview render. The profile is Bark-band coarse, so a
/// device actually running at 44.1 kHz changes nothing meaningful.
pub const SYNTH_PREVIEW_SAMPLE_RATE: u32 = 48_000;

/// Build one procedural synth `SoundSource` with its amplitude, spectral
/// profile, and reference distance. Mirrors `build_one_source` exactly, except
/// the calibration measurements come from a preview render instead of a
/// decoded file, so synth and sample sources obey the same loudness model.
#[allow(clippy::too_many_arguments)]
pub fn build_one_synth_source(
    spec: &SynthSpec,
    reference_spl: f32,
    directivity: &str,
    spread: f32,
    position: Vec3,
    default_seed: u64,
    normalization: &NormalizationConfig,
    global_ref_dist: f32,
    max_distance: f32,
) -> Result<BuiltSource, Box<dyn std::error::Error>> {
    // Preview render: measure RMS + spectral profile exactly like a decoded file.
    let sample_rate = SYNTH_PREVIEW_SAMPLE_RATE as f32;
    let mut preview_generator = spec.build_generator(position, default_seed);
    let total_samples = (SYNTH_PREVIEW_SECONDS * sample_rate) as usize;
    let mut samples = vec![0.0_f32; total_samples];
    for sample in samples.iter_mut() {
        *sample = preview_generator.next_sample(sample_rate);
    }
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / total_samples as f32).sqrt();
    if rms < 1e-6 {
        return Err(format!(
            "synth source `{}` rendered silence — check its parameters",
            spec.kind_name()
        )
        .into());
    }
    let bands =
        crate::audio::spectral_profile::compute_profile(&samples, SYNTH_PREVIEW_SAMPLE_RATE).bands;

    let profile = resolve_spl(reference_spl);
    let amplitude = profile.amplitude(rms, normalization.target_rms, normalization.spl_reference);
    let ref_dist = profile.ref_distance(global_ref_dist);
    let pattern = parse_directivity(directivity);
    let audible_radius = profile.audible_radius(normalization.spl_threshold, max_distance);

    // Base amplitude at 0 dB SPL, mirroring build_one_source.
    let rms_correction = normalization.target_rms / rms;
    let unit_amplitude = rms_correction * 10.0_f32.powf(-normalization.spl_reference / 20.0);

    // Fresh generator so the audible source starts from a clean state rather
    // than wherever the preview render left its envelopes.
    let mut node = SynthNode::new(spec.build_generator(position, default_seed), position);
    node.amplitude = amplitude;
    node.unit_amplitude = unit_amplitude;
    node.ref_dist = ref_dist;
    node.pattern = pattern;
    node.spread = spread;

    Ok(BuiltSource {
        source: Box::new(node),
        bands,
        amplitude,
        ref_dist,
        audible_radius,
        directivity_alpha: pattern.alpha(),
    })
}

// ── String → enum helpers ───────────────────────────────────────────────────

fn parse_directivity(s: &str) -> DirectivityPattern {
    match s {
        "omni" => DirectivityPattern::Omni,
        "cardioid" => DirectivityPattern::cardioid(),
        "supercardioid" => DirectivityPattern::supercardioid(),
        _ => {
            eprintln!("warning: unknown directivity '{}', defaulting to omni", s);
            DirectivityPattern::Omni
        }
    }
}

fn parse_distance_model(s: &str) -> DistanceModelType {
    match s {
        "linear" => DistanceModelType::Linear,
        "inverse" => DistanceModelType::Inverse,
        "exponential" => DistanceModelType::Exponential,
        _ => DistanceModelType::Inverse,
    }
}

fn parse_render_mode(s: &str) -> RenderMode {
    match s {
        "world_locked" => RenderMode::WorldLocked,
        "vbap" => RenderMode::Vbap,
        "hrtf" => RenderMode::Hrtf,
        "dbap" => RenderMode::Dbap,
        "ambisonics" => RenderMode::Ambisonics,
        _ => RenderMode::Vbap,
    }
}

fn arr_to_vec3(a: [f32; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The default scene must load a *reflective* environment. This guards the
    /// root-cause fix: the old default (`riverside`) was an open field whose
    /// walls reflect nothing, so the engine produced dry panning with no tail.
    #[test]
    fn atrium_environment_is_reflective_unlike_open_field() {
        let atrium: EnvironmentConfig =
            load_yaml("environments/atrium.yaml").expect("atrium.yaml should load");
        let riverside: EnvironmentConfig =
            load_yaml("environments/riverside.yaml").expect("riverside.yaml should load");

        // Every atrium wall reflects a meaningful fraction of energy.
        let atrium_walls = build_wall_materials(&atrium);
        for (i, wall) in atrium_walls.iter().enumerate() {
            assert!(
                wall.broadband_reflection_gain() > 0.5,
                "atrium wall {i} should be reflective, got gain {}",
                wall.broadband_reflection_gain()
            );
        }

        // The open field's default walls reflect essentially nothing.
        let riverside_walls = build_wall_materials(&riverside);
        assert!(
            riverside_walls[0].broadband_reflection_gain() < 0.01,
            "riverside default walls should be non-reflecting (open air), got {}",
            riverside_walls[0].broadband_reflection_gain()
        );

        // The atrium exposes explicit tail knobs; the open field leaves them unset.
        assert_eq!(atrium.rt60_seconds, Some(1.6));
        assert_eq!(atrium.pre_delay_ms, Some(12.0));
        assert_eq!(riverside.rt60_seconds, None);
    }

    /// The default scene file itself must point at the reflective atrium — the
    /// one-line change that unlocks reflections + tail for the shipped config.
    #[test]
    fn default_scene_uses_the_atrium_environment() {
        let scene: SceneConfig =
            load_yaml("scenes/default.yaml").expect("default.yaml should load");
        assert_eq!(scene.environment, "environments/atrium.yaml");
    }

    // ── Synth source definitions ────────────────────────────────────────────

    /// A sample def must still parse into the Sample variant.
    #[test]
    fn sample_source_def_parses() {
        let yaml = "path: assets/campfire.mp3\nreference_spl: 68.0\nspread: 0.6\n";
        match serde_yaml::from_str::<SourceDef>(yaml).expect("sample def should parse") {
            SourceDef::Sample(sample) => {
                assert_eq!(sample.path, "assets/campfire.mp3");
                assert_eq!(sample.reference_spl, 68.0);
            }
            SourceDef::Synth(_) => panic!("parsed as synth"),
        }
    }

    /// Every synth kind must parse — with bare-integer YAML values, which
    /// exercise serde's flatten/Content numeric path (a known footgun).
    #[test]
    fn synth_source_defs_parse_with_integer_values() {
        let cases = [
            (
                "synth: canopy_wind\nmin_speed: 2\nmax_speed: 8\nfoliage_density: 1\nreference_spl: 48\n",
                "canopy_wind",
            ),
            (
                "synth: field_wind\nmin_speed: 1\nmax_speed: 8\nchange_time_min: 20\nchange_time_max: 50\nreference_spl: 55\n",
                "field_wind",
            ),
            (
                "synth: soft_wind\nmin_speed: 1\nmax_speed: 5\ngust_brightness: 1\nreference_spl: 42\n",
                "soft_wind",
            ),
            (
                "synth: storm_wind\nmin_speed: 8\nmax_speed: 18\nturbulence_depth: 1\nreference_spl: 60\n",
                "storm_wind",
            ),
            ("synth: rain\nintensity: 0.5\nreference_spl: 55\n", "rain"),
            (
                "synth: rain_v2\nintensity: 0.5\nreference_spl: 55\n",
                "rain_v2",
            ),
            (
                "synth: river\nmin_flow_speed: 0.3\nmax_flow_speed: 1.2\nreference_spl: 45\n",
                "river",
            ),
            (
                "synth: waves\nperiod: 6\ncrash_prob: 0.25\nreference_spl: 60\n",
                "waves",
            ),
        ];
        for (yaml, expected_kind) in cases {
            match serde_yaml::from_str::<SourceDef>(yaml)
                .unwrap_or_else(|e| panic!("{expected_kind} def should parse: {e}"))
            {
                SourceDef::Synth(synth) => {
                    assert_eq!(synth.spec.kind_name(), expected_kind);
                    assert_eq!(
                        synth.reference_spl as i32,
                        match expected_kind {
                            "waves" => 60,
                            "canopy_wind" => 48,
                            "soft_wind" => 42,
                            "river" => 45,
                            "storm_wind" => 60,
                            _ => 55,
                        }
                    );
                }
                SourceDef::Sample(_) => panic!("{expected_kind} parsed as sample"),
            }
        }
    }

    #[test]
    fn storm_wind_controls_parse_and_build_as_a_field() {
        let source: SynthSourceDef = serde_yaml::from_str(
            "synth: storm_wind\n\
             min_speed: 8\n\
             max_speed: 18\n\
             change_time_min: 12\n\
             change_time_max: 40\n\
             gust_duration_min: 1.5\n\
             gust_duration_max: 14\n\
             gust_strength: 0.55\n\
             turbulence_depth: 0.65\n\
             debris_level: 0.06\n\
             structure_level: 0.05\n\
             pressure_gain: 1.03\n\
             roar_gain: 1.50\n\
             shear_gain: 0.70\n\
             tear_gain: 0.35\n\
             reference_spl: 60\n",
        )
        .unwrap();
        let SynthSpec::StormWind(params) = &source.spec else {
            panic!("expected storm_wind");
        };
        assert_eq!(params.min_speed, Some(8.0));
        assert_eq!(params.max_speed, Some(18.0));
        assert_eq!(params.gust_strength, Some(0.55));
        assert_eq!(params.turbulence_depth, Some(0.65));
        assert_eq!(params.debris_level, Some(0.06));
        assert_eq!(params.structure_level, Some(0.05));
        assert_eq!(params.pressure_gain, Some(1.03));
        assert_eq!(params.roar_gain, Some(1.50));
        assert_eq!(params.shear_gain, Some(0.70));
        assert_eq!(params.tear_gain, Some(0.35));

        let built = build_one_synth_source(
            &source.spec,
            source.reference_spl,
            "omni",
            1.0,
            Vec3::ZERO,
            45,
            &NormalizationConfig::default(),
            1.0,
            40.0,
        )
        .expect("storm wind should build");
        assert_eq!(
            built.source.emitter_kind(),
            atrium_core::source::EmitterKind::Field
        );
    }

    #[test]
    fn canopy_wind_controls_parse_and_build_as_a_field() {
        let source: SynthSourceDef = serde_yaml::from_str(
            "synth: canopy_wind\n\
             min_speed: 2\n\
             max_speed: 8\n\
             change_time_min: 15\n\
             change_time_max: 45\n\
             gust_duration_min: 2\n\
             gust_duration_max: 8\n\
             foliage_density: 0.75\n\
             leaf_dryness: 0.25\n\
             branch_level: 0.12\n\
             reference_spl: 48\n",
        )
        .unwrap();
        let SynthSpec::CanopyWind(params) = &source.spec else {
            panic!("expected canopy_wind");
        };
        assert_eq!(params.min_speed, Some(2.0));
        assert_eq!(params.max_speed, Some(8.0));
        assert_eq!(params.foliage_density, Some(0.75));
        assert_eq!(params.leaf_dryness, Some(0.25));
        assert_eq!(params.branch_level, Some(0.12));

        let built = build_one_synth_source(
            &source.spec,
            source.reference_spl,
            "omni",
            1.0,
            Vec3::ZERO,
            42,
            &NormalizationConfig::default(),
            1.0,
            40.0,
        )
        .expect("canopy wind should build");
        assert_eq!(
            built.source.emitter_kind(),
            atrium_core::source::EmitterKind::Field
        );
    }

    #[test]
    fn field_wind_bounded_driver_controls_parse() {
        let source: SynthSourceDef = serde_yaml::from_str(
            "synth: field_wind\n\
             min_speed: 2\n\
             max_speed: 5\n\
             change_time_min: 20\n\
             change_time_max: 50\n\
             gust_duration_min: 3\n\
             gust_duration_max: 10\n\
             turbulence_time_min: 0.12\n\
             turbulence_time_max: 0.80\n\
             gust_strength: 0.35\n\
             rise_bias: 0.25\n\
             gust_brightness: 0.20\n\
             turbulence_brightness: 0.10\n\
             reference_spl: 55\n",
        )
        .unwrap();
        let SynthSpec::FieldWind(params) = source.spec else {
            panic!("expected field_wind");
        };
        assert_eq!(params.min_speed, Some(2.0));
        assert_eq!(params.max_speed, Some(5.0));
        assert_eq!(params.change_time_min, Some(20.0));
        assert_eq!(params.change_time_max, Some(50.0));
        assert_eq!(params.gust_duration_min, Some(3.0));
        assert_eq!(params.gust_duration_max, Some(10.0));
        assert_eq!(params.turbulence_time_min, Some(0.12));
        assert_eq!(params.turbulence_time_max, Some(0.80));
        assert_eq!(params.gust_strength, Some(0.35));
        assert_eq!(params.rise_bias, Some(0.25));
        assert_eq!(params.gust_brightness, Some(0.20));
        assert_eq!(params.turbulence_brightness, Some(0.10));
    }

    #[test]
    fn soft_wind_controls_parse_and_build_as_a_field() {
        let source: SynthSourceDef = serde_yaml::from_str(
            "synth: soft_wind\n\
             min_speed: 1\n\
             max_speed: 5\n\
             change_time_min: 10\n\
             change_time_max: 24\n\
             gust_duration_min: 0.8\n\
             gust_duration_max: 4\n\
             turbulence_time_min: 0.12\n\
             turbulence_time_max: 0.8\n\
             gust_brightness: 0.18\n\
             turbulence_brightness: 0.10\n\
             reference_spl: 42\n",
        )
        .unwrap();
        let SynthSpec::SoftWind(params) = &source.spec else {
            panic!("expected soft_wind");
        };
        assert_eq!(params.min_speed, Some(1.0));
        assert_eq!(params.max_speed, Some(5.0));
        assert_eq!(params.turbulence_time_min, Some(0.12));
        assert_eq!(params.turbulence_time_max, Some(0.8));
        assert_eq!(params.gust_brightness, Some(0.18));
        assert_eq!(params.turbulence_brightness, Some(0.10));

        let built = build_one_synth_source(
            &source.spec,
            source.reference_spl,
            "omni",
            1.0,
            Vec3::ZERO,
            46,
            &NormalizationConfig::default(),
            1.0,
            40.0,
        )
        .expect("soft wind should build");
        assert_eq!(
            built.source.emitter_kind(),
            atrium_core::source::EmitterKind::Field
        );
    }

    #[test]
    fn river_controls_parse_and_build_as_an_object() {
        let source: SynthSourceDef = serde_yaml::from_str(
            "synth: river\n\
             min_flow_speed: 0.3\n\
             max_flow_speed: 1.2\n\
             change_time_min: 15\n\
             change_time_max: 110\n\
             eddy_time_min: 0.35\n\
             eddy_time_max: 1.25\n\
             eddy_depth: 0.65\n\
             bubble_activity: 0.75\n\
             splash_rate: 0.45\n\
             reference_spl: 45\n",
        )
        .unwrap();
        let SynthSpec::River(params) = &source.spec else {
            panic!("expected river");
        };
        assert_eq!(params.min_flow_speed, Some(0.3));
        assert_eq!(params.max_flow_speed, Some(1.2));
        assert_eq!(params.eddy_time_min, Some(0.35));
        assert_eq!(params.eddy_time_max, Some(1.25));
        assert_eq!(params.eddy_depth, Some(0.65));
        assert_eq!(params.bubble_activity, Some(0.75));
        assert_eq!(params.splash_rate, Some(0.45));

        let built = build_one_synth_source(
            &source.spec,
            source.reference_spl,
            "omni",
            1.0,
            Vec3::ZERO,
            47,
            &NormalizationConfig::default(),
            1.0,
            40.0,
        )
        .expect("river should build");
        assert_eq!(
            built.source.emitter_kind(),
            atrium_core::source::EmitterKind::Object
        );
    }

    #[test]
    fn field_wind_builds_as_a_field_emitter() {
        let source: SynthSourceDef = serde_yaml::from_str(
            "synth: field_wind\nmin_speed: 1\nmax_speed: 8\nchange_time_min: 20\nchange_time_max: 50\nreference_spl: 55\n",
        )
        .unwrap();
        let built = build_one_synth_source(
            &source.spec,
            source.reference_spl,
            "omni",
            1.0,
            Vec3::ZERO,
            42,
            &NormalizationConfig::default(),
            1.0,
            40.0,
        )
        .expect("field wind should build");
        assert_eq!(
            built.source.emitter_kind(),
            atrium_core::source::EmitterKind::Field
        );
    }

    /// Storm speed bounds must reach the DSP, including its high-speed tearing.
    #[test]
    fn storm_wind_speed_range_reaches_the_dsp() {
        let calm_spec: SynthSourceDef = serde_yaml::from_str(
            "synth: storm_wind\nmin_speed: 10\nmax_speed: 10\nreference_spl: 60\n",
        )
        .unwrap();
        let fast_spec: SynthSourceDef = serde_yaml::from_str(
            "synth: storm_wind\nmin_speed: 25\nmax_speed: 25\nreference_spl: 60\n",
        )
        .unwrap();

        let bright_fraction = |spec: &SynthSpec| {
            let mut generator = spec.build_generator(Vec3::ZERO, 7);
            for _ in 0..48_000 {
                generator.next_sample(48_000.0); // warm up filters
            }
            let mut hp = crate::synth::noise::OnePoleHP::new(1_800.0, 48_000.0);
            let (mut full, mut high) = (0.0_f64, 0.0_f64);
            for _ in 0..192_000 {
                let s = generator.next_sample(48_000.0);
                full += (s * s) as f64;
                let h = hp.process(s);
                high += (h * h) as f64;
            }
            (high / full.max(1e-12)) as f32
        };

        let calm = bright_fraction(&calm_spec.spec);
        let fast = bright_fraction(&fast_spec.spec);
        assert!(
            fast > calm * 1.5,
            "storm force should have more tearing energy (calm {calm}, fast {fast})"
        );
    }

    /// A synth def whose parameters produce silence must fail loudly at build
    /// time, not play an inaudible source.
    #[test]
    fn silent_synth_source_is_a_build_error() {
        let spec = SynthSpec::Rain(RainSynthParams {
            intensity: Some(0.0),
            ..Default::default()
        });
        let result = build_one_synth_source(
            &spec,
            55.0,
            "omni",
            0.9,
            Vec3::ZERO,
            1,
            &NormalizationConfig::default(),
            1.0,
            40.0,
        );
        assert!(result.is_err(), "zero-intensity rain should fail the build");
    }

    /// The synth test scene must build end-to-end with calibrated sources.
    #[test]
    fn synth_test_scene_builds_with_calibrated_sources() {
        let scene = SceneConfig::load("scenes/synth-test.yaml").expect("scene should load");
        let result = scene.build().expect("scene should build");
        let expected = ["Field Wind", "Waves", "Rain v2", "Rain v1"];
        assert_eq!(&result.source_names[..expected.len()], &expected);
        for (index, name) in expected.iter().enumerate() {
            assert!(
                result.scene.source_amplitudes[index] > 0.0,
                "source {name} should have positive amplitude"
            );
        }
    }

    #[test]
    fn canopy_wind_scene_builds_end_to_end() {
        let scene = SceneConfig::load("scenes/canopy-wind-only.yaml").expect("scene should load");
        let result = scene.build().expect("canopy scene should build");
        assert_eq!(
            result.source_names.first().map(String::as_str),
            Some("Canopy Wind")
        );
        assert!(result.scene.source_amplitudes[0] > 0.0);
        assert_eq!(
            result.scene.sources[0].emitter_kind(),
            atrium_core::source::EmitterKind::Field
        );
    }

    #[test]
    fn storm_wind_scene_builds_end_to_end() {
        let scene = SceneConfig::load("scenes/storm-wind-only.yaml").expect("scene should load");
        let result = scene.build().expect("storm scene should build");
        assert_eq!(
            result.source_names.first().map(String::as_str),
            Some("Storm Wind")
        );
        assert!(result.scene.source_amplitudes[0] > 0.0);
        assert_eq!(
            result.scene.sources[0].emitter_kind(),
            atrium_core::source::EmitterKind::Field
        );
    }

    #[test]
    fn soft_wind_scene_builds_end_to_end() {
        let scene = SceneConfig::load("scenes/soft-wind-only.yaml").expect("scene should load");
        let result = scene.build().expect("soft-wind scene should build");
        assert_eq!(
            result.source_names.first().map(String::as_str),
            Some("Soft Wind")
        );
        assert!(result.scene.source_amplitudes[0] > 0.0);
        assert_eq!(
            result.scene.sources[0].emitter_kind(),
            atrium_core::source::EmitterKind::Field
        );
    }

    #[test]
    fn river_scene_builds_end_to_end() {
        let scene = SceneConfig::load("scenes/river-only.yaml").expect("scene should load");
        let result = scene.build().expect("river scene should build");
        assert_eq!(
            result.source_names.first().map(String::as_str),
            Some("River")
        );
        assert!(result.scene.source_amplitudes[0] > 0.0);
        assert_eq!(
            result.scene.sources[0].emitter_kind(),
            atrium_core::source::EmitterKind::Object
        );
    }
}
