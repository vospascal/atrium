//! Scene-level configuration: the scene file itself and the small
//! sub-configs it embeds (listener, speakers, distance model, normalization),
//! plus the atrium and atmosphere files a scene references.

use atrium_world::Entities;
use serde::{Deserialize, Serialize};

/// A scene: references environment, atrium, source, processor, and atmosphere
/// files, adds listener placement, speaker layout, and mixing parameters.
#[derive(Deserialize, Serialize, Clone)]
pub struct SceneConfig {
    /// The name a player shows (e.g. "River Bend"). Omit to derive one from
    /// the file name.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// Path to environment definition file (e.g. "environments/riverside.yaml").
    /// The virtual acoustic space where sources live.
    #[serde(alias = "room")]
    pub environment: String,
    /// Path to atrium definition file (e.g. "rooms/atrium_6x4.yaml").
    /// The physical speaker room dimensions. Optional — defaults to environment dims.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atrium: Option<String>,
    pub listener: ListenerConfig,
    #[serde(default = "default_master_gain")]
    pub master_gain: f32,
    #[serde(default)]
    pub distance_model: DistanceModelConfig,
    pub speakers: SpeakerConfig,
    #[serde(default)]
    pub normalization: NormalizationConfig,
    /// The sounds of the scene, each under its id, in the order they take
    /// the engine's voices.
    pub sounds: Entities,
    /// Path to atmosphere definition file (e.g. "atmospheres/default.yaml").
    /// Omit for standard conditions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atmosphere: Option<String>,
    /// Path to SOFA HRTF file for HRTF rendering (e.g. "assets/hrtf/default.sofa").
    /// Defaults to "assets/hrtf/default.sofa" if omitted.
    #[serde(default = "default_hrtf_path")]
    pub hrtf: String,
    /// What every choice of the session follows from: which take, every rest
    /// and every small change of level and pitch. Left out, each play is a
    /// new session; set, the scene plays the same way every time.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub seed: Option<u64>,
    /// What moves the place's conditions, and the bands it keeps them in.
    /// Left out, a still, dry, bright day.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weather: Option<WeatherConfig>,
}

/// The weather of a scene:
///
/// ```yaml
/// weather:
///   story: passing storm   # held, drifting or passing storm
///   wind: [0.05, 0.25]     # the band it drifts in; one number holds it there
///   rain: 0
///   daylight: 1
/// ```
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
pub struct WeatherConfig {
    pub story: String,
    #[serde(default = "Band::none")]
    pub wind: Band,
    #[serde(default = "Band::none")]
    pub rain: Band,
    #[serde(default = "Band::none")]
    pub storm: Band,
    #[serde(default = "Band::full")]
    pub daylight: Band,
}

/// One condition's band: a value it is held at, or the lowest and highest it
/// drifts between, each from 0 to 1.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, PartialEq)]
#[serde(untagged)]
pub enum Band {
    At(f32),
    Between([f32; 2]),
}

impl Band {
    fn none() -> Self {
        Band::At(0.0)
    }

    fn full() -> Self {
        Band::At(1.0)
    }

    /// Lowest and highest.
    pub fn range(self) -> (f32, f32) {
        match self {
            Band::At(value) => (value, value),
            Band::Between([low, high]) => (low, high),
        }
    }
}

impl SceneConfig {
    /// The same scene for headphones: two channels, rendered binaurally.
    ///
    /// What a scene is heard on is the listener's choice, made when the scene
    /// starts. It is not a second scene file that differs in two lines.
    pub fn for_headphones(mut self) -> Self {
        self.speakers.layout = "stereo".into();
        self.speakers.render_mode = "hrtf".into();
        self
    }
}

fn default_master_gain() -> f32 {
    1.0
}

fn default_hrtf_path() -> String {
    "assets/hrtf/default.sofa".into()
}

#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
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
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fl: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub fr: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub c: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rl: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub rr: Option<[f32; 3]>,
    // stereo
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub l: Option<[f32; 3]>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
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

/// Atrium geometry (loaded from `rooms/*.yaml` or inline).
/// The physical speaker room — only dimensions, no acoustic properties.
#[derive(Deserialize)]
pub(super) struct AtriumConfig {
    pub width: f32,
    pub depth: f32,
    pub height: f32,
}

#[derive(Deserialize)]
pub(super) struct AtmosphereConfig {
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

#[cfg(test)]
mod tests {
    use crate::config::build::load_yaml;

    use super::*;

    /// The default scene file itself must point at the reflective atrium — the
    /// one-line change that unlocks reflections + tail for the shipped config.
    #[test]
    fn default_scene_uses_the_atrium_environment() {
        let scene: SceneConfig =
            load_yaml("scenes/default.yaml").expect("default.yaml should load");
        assert_eq!(scene.environment, "environments/atrium.yaml");
    }
}
