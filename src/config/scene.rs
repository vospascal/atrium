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
    /// What the scene is: a place with weather (`nature`, the default, which
    /// a file need not say) or a listening session (`kind: listening`).
    #[serde(default, skip_serializing_if = "SceneKind::is_nature")]
    pub kind: SceneKind,
    /// Path to environment definition file (e.g. "environments/riverside.yaml").
    /// The virtual acoustic space where sources live. Left out, the space is
    /// dry: nothing reflects and nothing rings.
    #[serde(default, alias = "room", skip_serializing_if = "Option::is_none")]
    pub environment: Option<String>,
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
    /// The air of the place: a path to an atmosphere file (e.g.
    /// "atmospheres/default.yaml"), or the values themselves. Omit for
    /// standard conditions.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub atmosphere: Option<Atmosphere>,
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
    /// Left out, a still, dry, bright day. A listening session keeps it in
    /// its file but runs none, so a scene switched to listening and back has
    /// its weather again.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub weather: Option<WeatherConfig>,
}

/// The place a new nature scene starts in.
const NATURE_STARTS_IN: &str = "environments/meadow.yaml";

/// What a scene is, which decides what of it runs and what a UI shows.
#[derive(Deserialize, Serialize, Clone, Copy, Debug, Default, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum SceneKind {
    /// A place: its weather moves its sounds, and its air, daylight and
    /// space are part of it.
    #[default]
    Nature,
    /// Sounds to listen to, such as tones and binaural beats: no weather
    /// story runs, every condition stays at its default, the air keeps
    /// standard conditions and is still.
    Listening,
}

impl SceneKind {
    fn is_nature(&self) -> bool {
        *self == SceneKind::Nature
    }
}

/// The weather of a scene:
///
/// ```yaml
/// weather:
///   story: passing storm   # held, drifting or passing storm
///   wind: [0.05, 0.25]     # the band it drifts in; one number holds it there
///   rain: 0
///   daylight: 1
///   wind_from: 270         # compass degrees it blows from; here the west
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
    /// The compass bearing the wind comes from, in degrees: 0 north, 90
    /// east. Left out, the south-west.
    #[serde(
        default = "prevailing_wind",
        skip_serializing_if = "is_prevailing_wind"
    )]
    pub wind_from: f32,
}

fn prevailing_wind() -> f32 {
    crate::plan::PREVAILING_WIND_FROM
}

fn is_prevailing_wind(wind_from: &f32) -> bool {
    *wind_from == crate::plan::PREVAILING_WIND_FROM
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

impl WeatherConfig {
    /// The weather this names, with its bands, played from `seed`. A scene
    /// without a `weather:` section has `Weather::calm`.
    pub fn story(&self, seed: u64) -> Result<crate::plan::Weather, Box<dyn std::error::Error>> {
        use crate::plan::{Story, Weather};
        let weather = self;
        let story = Story::named(&weather.story).ok_or_else(|| {
            format!(
                "`story: {}` is not a weather story; use held, drifting or passing storm",
                weather.story
            )
        })?;
        if !weather.wind_from.is_finite() {
            return Err("`wind_from` must be a compass bearing in degrees".into());
        }
        Ok(Weather::new(
            story,
            [
                weather.wind.range(),
                weather.rain.range(),
                weather.storm.range(),
                weather.daylight.range(),
            ],
            seed,
        )
        .with_wind_from(weather.wind_from))
    }
}

impl SceneConfig {
    /// An empty scene of `kind`: no sounds, the listener in the middle
    /// facing north, the 5.1 room around them, levels calibrated as the
    /// shipped scenes are. A nature scene starts in the meadow, a listening
    /// session in the dry space; either can be given another place after.
    pub fn empty(kind: SceneKind) -> Self {
        Self {
            title: None,
            kind,
            environment: match kind {
                SceneKind::Nature => Some(NATURE_STARTS_IN.to_owned()),
                SceneKind::Listening => None,
            },
            atrium: None,
            listener: ListenerConfig {
                position: [0.0, 0.0, 1.2],
                yaw_degrees: 90.0,
            },
            master_gain: default_master_gain(),
            distance_model: DistanceModelConfig {
                max_distance: 40.0,
                ..DistanceModelConfig::default()
            },
            speakers: SpeakerConfig {
                layout: "5.1".into(),
                render_mode: default_render_mode(),
                positions: SpeakerPositions::default(),
                dbap_rolloff_db: default_dbap_rolloff(),
            },
            normalization: NormalizationConfig {
                target_rms: 0.1,
                spl_reference: 94.0,
                spl_threshold: 0.0,
            },
            sounds: Entities::default(),
            atmosphere: None,
            hrtf: default_hrtf_path(),
            seed: None,
            weather: None,
        }
    }

    /// The weather story the scene runs from `seed`: none for a listening
    /// session, whatever its file keeps; else its `weather:`, or a calm day.
    /// A story the file names wrongly is refused whatever the kind.
    pub fn weather_story(
        &self,
        seed: u64,
    ) -> Result<Option<crate::plan::Weather>, Box<dyn std::error::Error>> {
        let story = match &self.weather {
            Some(weather) => weather.story(seed)?,
            None => crate::plan::Weather::calm(seed),
        };
        Ok(match self.kind {
            SceneKind::Nature => Some(story),
            SceneKind::Listening => None,
        })
    }

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

impl NormalizationConfig {
    /// dB SPL of an RMS of 1.0: a source calibrated to `spl_reference` dB
    /// SPL plays at RMS `target_rms` (`config::built_source`), so RMS 1.0 is
    /// `spl_reference − 20·log10(target_rms)`. The analyser reads levels in
    /// dB SPL from it, offline from a render's sidecar and live from the
    /// playing scene.
    pub fn full_scale_db_spl(&self) -> f64 {
        self.spl_reference as f64 - 20.0 * (self.target_rms as f64).log10()
    }
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

/// Where a scene's air comes from: a file shared between scenes, or values
/// of its own, which is what an editor writes once the air is changed.
///
/// ```yaml
/// atmosphere: atmospheres/default.yaml
/// atmosphere: { temperature_c: 8, humidity_pct: 85 }
/// ```
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(untagged)]
pub enum Atmosphere {
    File(String),
    Air(AtmosphereConfig),
}

impl SceneConfig {
    /// The air sound travels through in this scene: standard conditions in
    /// a listening session, whatever its file keeps; else the air it names.
    pub fn air(&self) -> Result<AtmosphereConfig, Box<dyn std::error::Error>> {
        match self.kind {
            SceneKind::Nature => self.air_in_file(),
            SceneKind::Listening => Ok(AtmosphereConfig::default()),
        }
    }

    /// The air this scene's file names: its own values, its file's, or
    /// standard conditions.
    fn air_in_file(&self) -> Result<AtmosphereConfig, Box<dyn std::error::Error>> {
        match &self.atmosphere {
            Some(Atmosphere::File(path)) => super::parse::load_yaml(path),
            Some(Atmosphere::Air(air)) => Ok(air.clone()),
            None => Ok(AtmosphereConfig::default()),
        }
    }

    /// Give the scene air of its own at `temperature_c` and `humidity_pct`,
    /// at the pressure it had. A scene that named an atmosphere file stops
    /// following it.
    pub fn set_air(
        &mut self,
        temperature_c: f32,
        humidity_pct: f32,
    ) -> Result<(), Box<dyn std::error::Error>> {
        let pressure_kpa = self.air_in_file()?.pressure_kpa;
        self.atmosphere = Some(Atmosphere::Air(AtmosphereConfig {
            temperature_c,
            humidity_pct,
            pressure_kpa,
        }));
        Ok(())
    }
}

/// The air sound travels through.
#[derive(Deserialize, Serialize, Clone, Debug, PartialEq)]
#[serde(deny_unknown_fields)]
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

#[cfg(test)]
mod tests {
    use crate::config::parse::load_yaml;

    use super::*;

    /// The default scene file itself must point at the reflective atrium — the
    /// one-line change that unlocks reflections + tail for the shipped config.
    #[test]
    fn default_scene_uses_the_atrium_environment() {
        let scene: SceneConfig =
            load_yaml("scenes/default.yaml").expect("default.yaml should load");
        assert_eq!(
            scene.environment.as_deref(),
            Some("environments/atrium.yaml")
        );
    }

    /// 94 dB SPL at RMS 0.1 puts RMS 1.0 at 94 + 20 = 114 dB SPL; at the
    /// default RMS 0.5, 94 + 6.02.
    #[test]
    fn full_scale_follows_the_normalisation() {
        let at = |target_rms: f32| NormalizationConfig {
            target_rms,
            spl_reference: 94.0,
            ..NormalizationConfig::default()
        };
        assert!((at(0.1).full_scale_db_spl() - 114.0).abs() < 1e-5);
        assert!((at(0.5).full_scale_db_spl() - 100.0206).abs() < 1e-4);
    }
}
