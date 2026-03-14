//! YAML-driven scene configuration.
//!
//! A scene file (`scenes/*.yaml`) wires together separate config files:
//!   - `environments/*.yaml` — virtual acoustic space (dimensions, wall materials)
//!   - `rooms/*.yaml`        — atrium geometry (physical speaker room)
//!   - `sources/*.yaml`      — sound identity (audio file, SPL, directivity)
//!   - `processors/*.yaml`   — effect chain (early reflections, reverb)
//!   - `atmospheres/*.yaml`  — atmospheric absorption conditions
//!
//! The scene itself only adds placement (positions, orbits) and mixing
//! parameters (speakers, normalization, distance model).

use std::path::Path;
use std::sync::Arc;

use serde::Deserialize;

use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::decode::decode_file;
use crate::audio::distance::DistanceModel;
use crate::audio::propagation::GroundProperties;
use crate::audio::sound_profile::SoundProfile;
use crate::audio::test_node::TestNode;
use crate::engine::scene::{AudioScene, InitialSourceState};
use crate::pipeline::{build_all_pipelines, PipelineParams};
use crate::world::room::{BoxRoom, Room};
use crate::world::types::Vec3;
use atrium_core::directivity::DirectivityPattern;
use atrium_core::listener::Listener;
use atrium_core::panner::DistanceModelType;
use atrium_core::speaker::{ChannelMode, RenderMode, SpeakerLayout};

// ── Top-level scene config ──────────────────────────────────────────────────

/// A scene: references environment, atrium, source, processor, and atmosphere
/// files, adds listener placement, speaker layout, and mixing parameters.
#[derive(Deserialize)]
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
    /// Path to processors definition file (e.g. "processors/small_room.yaml").
    /// Omit for no processors.
    pub processors: Option<String>,
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

#[derive(Deserialize)]
pub struct ListenerConfig {
    pub position: [f32; 3],
    #[serde(default)]
    pub yaw_degrees: f32,
}

#[derive(Deserialize)]
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

#[derive(Deserialize)]
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

#[derive(Deserialize, Default)]
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

#[derive(Deserialize)]
pub struct NormalizationConfig {
    #[serde(default = "default_target_rms")]
    pub target_rms: f32,
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
            spl_threshold: 20.0,
        }
    }
}

fn default_target_rms() -> f32 {
    0.5
}
fn default_spl_threshold() -> f32 {
    20.0
}

// ── Source configs (scene entry + file definition) ──────────────────────────

/// Scene entry: references a source file and places it in the scene.
#[derive(Deserialize)]
pub struct SourceEntry {
    /// Path to the source definition YAML file (e.g. "sources/djembe.yaml").
    pub source: String,
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

/// Source definition (loaded from `sources/*.yaml`): intrinsic sound properties.
#[derive(Deserialize)]
pub struct SourceDef {
    pub path: String,
    pub reference_spl: SplValue,
    #[serde(default = "default_directivity")]
    pub directivity: String,
    #[serde(default)]
    pub spread: f32,
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
#[serde(tag = "type")]
pub enum ProcessorConfig {
    #[serde(rename = "early_reflections")]
    EarlyReflections {
        #[serde(default = "default_er_reflectivity")]
        wall_reflectivity: f32,
    },
}

fn default_er_reflectivity() -> f32 {
    crate::pipeline::DEFAULT_WALL_REFLECTIVITY
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

        let listener_pos = arr_to_vec3(self.listener.position);
        let listener = Listener::new(listener_pos, self.listener.yaw_degrees.to_radians());

        // Build speaker layout
        let speaker_layout = self.build_speakers();
        let render_mode = parse_render_mode(&self.speakers.render_mode);

        // Decode audio and build sources (also collects metadata for the browser)
        let build = self.build_sources()?;
        let sources = build.sources;
        let source_metas = build.metas;
        let spectral_profiles = build.spectral_profiles;
        let source_amplitudes = build.source_amplitudes;

        let distance_model = DistanceModel {
            ref_distance: self.distance_model.ref_distance,
            max_distance: self.distance_model.max_distance,
            rolloff: self.distance_model.rolloff,
            model: parse_distance_model(&self.distance_model.model),
        };

        // Load processor params from file (feeds pipeline construction)
        let mut er_wall_reflectivity: Option<f32> = None;
        if let Some(path) = &self.processors {
            let defs: Vec<ProcessorConfig> = load_yaml(path)?;
            for def in &defs {
                match def {
                    ProcessorConfig::EarlyReflections { wall_reflectivity } => {
                        er_wall_reflectivity = Some(*wall_reflectivity);
                    }
                }
            }
        }

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
            &speaker_layout,
            &source_metas,
            &atmosphere,
        );

        let initial_source_states: Vec<InitialSourceState> = self
            .sources
            .iter()
            .map(|entry| InitialSourceState {
                position: arr_to_vec3(entry.position),
                orbit_radius: entry.orbit_radius,
                orbit_speed: entry.orbit_speed,
            })
            .collect();

        // Build composable pipelines
        let ground = GroundProperties::mixed(environment_cfg.ground_factor);

        let wall_materials = build_wall_materials(&environment_cfg);
        // Environment's wall_reflectivity takes precedence; processor override is fallback
        let effective_reflectivity =
            er_wall_reflectivity.unwrap_or(environment_cfg.wall_reflectivity);
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
        };
        let pipelines = build_all_pipelines(&pipeline_params);
        let active_pipeline = render_mode;

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
            telemetry_counter: 0,
            telemetry_interval: 6, // ~15 Hz at 512-sample buffers; calibrated later
            #[cfg(feature = "memprof")]
            memprof: crate::engine::memprof::MemProfiler::new(),
            pipelines,
            active_pipeline,
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

    fn build_speakers(&self) -> SpeakerLayout {
        let p = &self.speakers.positions;
        match self.speakers.layout.as_str() {
            "5.1" => SpeakerLayout::surround_5_1(
                arr_to_vec3(p.fl.unwrap_or([0.0, 4.0, 0.0])),
                arr_to_vec3(p.fr.unwrap_or([6.0, 4.0, 0.0])),
                arr_to_vec3(p.c.unwrap_or([3.0, 4.0, 0.0])),
                arr_to_vec3(p.rl.unwrap_or([0.0, 0.0, 0.0])),
                arr_to_vec3(p.rr.unwrap_or([6.0, 0.0, 0.0])),
            ),
            "quad" => SpeakerLayout::quad(
                arr_to_vec3(p.fl.unwrap_or([0.0, 4.0, 0.0])),
                arr_to_vec3(p.fr.unwrap_or([6.0, 4.0, 0.0])),
                arr_to_vec3(p.rl.unwrap_or([0.0, 0.0, 0.0])),
                arr_to_vec3(p.rr.unwrap_or([6.0, 0.0, 0.0])),
            ),
            _ => SpeakerLayout::stereo(
                arr_to_vec3(p.l.or(p.fl).unwrap_or([0.0, 4.0, 0.0])),
                arr_to_vec3(p.r.or(p.fr).unwrap_or([6.0, 4.0, 0.0])),
            ),
        }
    }

    fn build_scene_json(
        &self,
        environment_cfg: &EnvironmentConfig,
        atrium_cfg: &AtriumConfig,
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

        // Sources (all computed values from the engine)
        let sources: Vec<_> = source_metas
            .iter()
            .map(|s| {
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
                    "position": s.position,
                    "orbit_radius": s.orbit_radius,
                    "orbit_speed": s.orbit_speed,
                })
            })
            .collect();

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
            "listener": {
                "x": self.listener.position[0],
                "y": self.listener.position[1],
                "z": self.listener.position[2],
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

    fn build_sources(&self) -> Result<BuildSourcesResult, Box<dyn std::error::Error>> {
        // Load all source definitions first
        let defs: Vec<SourceDef> = self
            .sources
            .iter()
            .map(|entry| {
                let contents = std::fs::read_to_string(&entry.source)
                    .map_err(|e| format!("{}: {}", entry.source, e))?;
                serde_yaml::from_str(&contents)
                    .map_err(|e| format!("{}: {}", entry.source, e).into())
            })
            .collect::<Result<Vec<_>, Box<dyn std::error::Error>>>()?;

        let norm = &self.normalization;
        let mut nodes: Vec<Box<dyn atrium_core::source::SoundSource>> = Vec::new();
        let mut metas: Vec<SourceMeta> = Vec::new();
        let mut spectral_profiles = Vec::<[f32; crate::audio::spectral_profile::BARK_BANDS]>::new();
        let mut source_amplitudes = Vec::<f32>::new();

        let global_ref_dist = self.distance_model.ref_distance;

        // The loudest source in the scene maps to gain 1.0; all others scale down.
        let max_source_spl = defs
            .iter()
            .map(|d| d.reference_spl)
            .fold(f32::NEG_INFINITY, f32::max);

        for (i, (entry, def)) in self.sources.iter().zip(defs.iter()).enumerate() {
            let buffer = Arc::new(decode_file(Path::new(&def.path))?);
            let profile = resolve_spl(def.reference_spl);
            let amplitude = profile.amplitude(buffer.rms, norm.target_rms, max_source_spl);
            let ref_dist = profile.ref_distance(global_ref_dist);
            let pattern = parse_directivity(&def.directivity);
            let bands = buffer.spectral_profile.bands;

            let mut node = TestNode::new(
                buffer,
                arr_to_vec3(entry.position),
                entry.orbit_radius,
                entry.orbit_speed,
            );
            node.amplitude = amplitude;
            node.ref_dist = ref_dist;
            node.pattern = pattern;
            node.spread = def.spread;

            let name = entry.name.as_deref().unwrap_or_else(|| {
                Path::new(&entry.source)
                    .file_stem()
                    .and_then(|s| s.to_str())
                    .unwrap_or("?")
            });
            let color = entry
                .color
                .clone()
                .unwrap_or_else(|| SOURCE_COLORS[i % SOURCE_COLORS.len()].to_string());

            let max_dist = self.distance_model.max_distance;
            let audible_radius = profile.audible_radius(norm.spl_threshold, max_dist);

            println!(
                "  {} → SPL={:.0} dB, amplitude={:.4}, ref_dist={:.2}m, audible={:.2}m",
                name, profile.reference_spl, amplitude, ref_dist, audible_radius,
            );

            metas.push(SourceMeta {
                name: name.to_string(),
                color,
                spl: profile.reference_spl,
                ref_dist,
                amplitude,
                audible_radius,
                directivity: def.directivity.clone(),
                directivity_alpha: pattern.alpha(),
                spread: def.spread,
                position: entry.position,
                orbit_radius: entry.orbit_radius,
                orbit_speed: entry.orbit_speed,
            });

            spectral_profiles.push(bands);
            source_amplitudes.push(amplitude);
            nodes.push(Box::new(node));
        }

        Ok(BuildSourcesResult {
            sources: nodes,
            metas,
            spectral_profiles,
            source_amplitudes,
        })
    }
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
