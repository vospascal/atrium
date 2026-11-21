//! Reading what a scene names: its YAML files, and the strings and arrays it
//! writes for render modes, distance models and points.

use atrium_core::panner::DistanceModelType;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

/// Load and deserialize a YAML file into any serde-compatible type.
pub(super) fn load_yaml<T: serde::de::DeserializeOwned>(
    path: &str,
) -> Result<T, Box<dyn std::error::Error>> {
    // Warn about absolute or parent-traversing paths (not sandboxed, but logged)
    if std::path::Path::new(path).is_absolute() || path.contains("..") {
        eprintln!("warning: loading file from non-relative path: {path}");
    }
    let contents = std::fs::read_to_string(path).map_err(|e| format!("{}: {}", path, e))?;
    serde_yaml::from_str(&contents).map_err(|e| format!("{}: {}", path, e).into())
}

pub(super) fn parse_distance_model(s: &str) -> DistanceModelType {
    match s {
        "linear" => DistanceModelType::Linear,
        "inverse" => DistanceModelType::Inverse,
        "exponential" => DistanceModelType::Exponential,
        _ => DistanceModelType::Inverse,
    }
}

pub(super) fn parse_render_mode(s: &str) -> RenderMode {
    match s {
        "world_locked" => RenderMode::WorldLocked,
        "vbap" => RenderMode::Vbap,
        "hrtf" => RenderMode::Hrtf,
        "dbap" => RenderMode::Dbap,
        "ambisonics" => RenderMode::Ambisonics,
        _ => RenderMode::Vbap,
    }
}

pub(super) fn arr_to_vec3(a: [f32; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}
