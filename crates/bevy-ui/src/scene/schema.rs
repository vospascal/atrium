//! Stable JSON scene schema — the persistence/share format.
//!
//! These types define the canonical scene file format. They are independent of
//! Bevy's runtime representation and should remain stable across engine versions.
//!
//! At runtime, `import` converts these into ECS entities with components.
//! `export` does the reverse: ECS world → SceneDescription → JSON.

use bevy::prelude::*;
use serde::{Deserialize, Serialize};

/// Complete scene description. Serializes to/from JSON.
#[derive(Resource, Serialize, Deserialize, Clone, Debug)]
pub struct SceneDescription {
    /// Schema version for forward compatibility.
    #[serde(default = "default_version")]
    pub version: u32,
    pub environment: EnvironmentDescription,
    pub atrium: AtriumDescription,
    pub listener: ListenerDescription,
    pub sources: Vec<SourceDescription>,
    pub speakers: SpeakerLayoutDescription,
    #[serde(default = "default_render_mode")]
    pub render_mode: String,
    #[serde(default = "default_master_gain")]
    pub master_gain: f32,
    #[serde(default)]
    pub distance_model: DistanceModelDescription,
    #[serde(default)]
    pub atmosphere: AtmosphereDescription,
}

fn default_version() -> u32 {
    1
}
fn default_render_mode() -> String {
    "vbap".into()
}
fn default_master_gain() -> f32 {
    1.0
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct EnvironmentDescription {
    pub width: f32,
    pub depth: f32,
    pub height: f32,
    #[serde(default)]
    pub spawn: [f32; 3],
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AtriumDescription {
    pub width: f32,
    pub depth: f32,
    pub height: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct ListenerDescription {
    pub position: [f32; 3],
    #[serde(default)]
    pub yaw_degrees: f32,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SourceDescription {
    /// Stable identifier. Generated if omitted.
    #[serde(default)]
    pub id: String,
    pub name: String,
    /// Hex color string (e.g. "#ff6b35").
    #[serde(default = "default_color")]
    pub color: String,
    pub position: [f32; 3],
    /// Reference SPL at 1 meter (dB).
    #[serde(default = "default_spl")]
    pub spl: f32,
    /// Reference distance for attenuation.
    #[serde(default = "default_ref_distance")]
    pub ref_distance: f32,
    /// Directivity type: "omni", "cardioid", "supercardioid".
    #[serde(default = "default_directivity")]
    pub directivity: String,
    /// Alpha for polar patterns.
    #[serde(default = "default_directivity_alpha")]
    pub directivity_alpha: f32,
    /// MDAP spread.
    #[serde(default)]
    pub spread: f32,
    /// Orbit radius (0.0 = stationary).
    #[serde(default)]
    pub orbit_radius: f32,
    /// Orbit speed (rad/s).
    #[serde(default)]
    pub orbit_speed: f32,
}

fn default_color() -> String {
    "#ffffff".into()
}
fn default_spl() -> f32 {
    80.0
}
fn default_ref_distance() -> f32 {
    1.0
}
fn default_directivity() -> String {
    "omni".into()
}
fn default_directivity_alpha() -> f32 {
    1.0
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SpeakerDescription {
    /// Stable identifier (e.g. "fl").
    #[serde(default)]
    pub id: String,
    pub label: String,
    pub position: [f32; 3],
    pub channel: usize,
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct SpeakerLayoutDescription {
    /// Layout name: "stereo", "quad", "5.1".
    pub layout: String,
    pub speakers: Vec<SpeakerDescription>,
    #[serde(default = "default_dbap_rolloff")]
    pub dbap_rolloff_db: f32,
}

fn default_dbap_rolloff() -> f32 {
    6.0
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct DistanceModelDescription {
    #[serde(default = "default_model")]
    pub model: String,
    #[serde(default = "default_dm_ref_distance")]
    pub ref_distance: f32,
    #[serde(default = "default_max_distance")]
    pub max_distance: f32,
    #[serde(default = "default_rolloff")]
    pub rolloff: f32,
}

impl Default for DistanceModelDescription {
    fn default() -> Self {
        Self {
            model: "inverse".into(),
            ref_distance: 1.0,
            max_distance: 20.0,
            rolloff: 1.0,
        }
    }
}

fn default_model() -> String {
    "inverse".into()
}
fn default_dm_ref_distance() -> f32 {
    1.0
}
fn default_max_distance() -> f32 {
    20.0
}
fn default_rolloff() -> f32 {
    1.0
}

#[derive(Serialize, Deserialize, Clone, Debug)]
pub struct AtmosphereDescription {
    #[serde(default = "default_temperature")]
    pub temperature_c: f32,
    #[serde(default = "default_humidity")]
    pub humidity_pct: f32,
    #[serde(default = "default_pressure")]
    pub pressure_kpa: f32,
}

impl Default for AtmosphereDescription {
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

// ── Hex color helpers ───────────────────────────────────────────────────────

/// Parse a hex color string like "#ff6b35" into [r, g, b] floats in 0..1.
pub fn parse_hex_color(hex: &str) -> [f32; 3] {
    let hex = hex.trim_start_matches('#');
    if hex.len() < 6 {
        return [1.0, 1.0, 1.0];
    }
    let r = u8::from_str_radix(&hex[0..2], 16).unwrap_or(255) as f32 / 255.0;
    let g = u8::from_str_radix(&hex[2..4], 16).unwrap_or(255) as f32 / 255.0;
    let b = u8::from_str_radix(&hex[4..6], 16).unwrap_or(255) as f32 / 255.0;
    [r, g, b]
}

/// Convert [r, g, b] floats to a hex string like "#ff6b35".
pub fn color_to_hex(color: [f32; 3]) -> String {
    let r = (color[0].clamp(0.0, 1.0) * 255.0).round() as u8;
    let g = (color[1].clamp(0.0, 1.0) * 255.0).round() as u8;
    let b = (color[2].clamp(0.0, 1.0) * 255.0).round() as u8;
    format!("#{:02x}{:02x}{:02x}", r, g, b)
}
