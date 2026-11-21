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

/// The distance law a scene's `distance_model.model` names. A name it does
/// not know is refused rather than taken as the inverse law, so a typo in a
/// scene file is said and not heard as a quietly different falloff.
pub(super) fn parse_distance_model(name: &str) -> Result<DistanceModelType, String> {
    match name {
        "linear" => Ok(DistanceModelType::Linear),
        "inverse" => Ok(DistanceModelType::Inverse),
        "exponential" => Ok(DistanceModelType::Exponential),
        other => Err(format!(
            "distance_model.model `{other}` is not one of linear, inverse, exponential"
        )),
    }
}

/// The render mode a scene's `speakers.render_mode` names. A name it does not
/// know is refused rather than played as VBAP.
pub(super) fn parse_render_mode(name: &str) -> Result<RenderMode, String> {
    match name {
        "world_locked" => Ok(RenderMode::WorldLocked),
        "vbap" => Ok(RenderMode::Vbap),
        "hrtf" => Ok(RenderMode::Hrtf),
        "dbap" => Ok(RenderMode::Dbap),
        "ambisonics" => Ok(RenderMode::Ambisonics),
        other => Err(format!(
            "speakers.render_mode `{other}` is not one of world_locked, vbap, hrtf, dbap, ambisonics"
        )),
    }
}

pub(super) fn arr_to_vec3(a: [f32; 3]) -> Vec3 {
    Vec3::new(a[0], a[1], a[2])
}
