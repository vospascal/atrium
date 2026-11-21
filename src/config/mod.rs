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

mod build;
mod description;
mod environment;
mod presets;
mod scene;
mod scene_file;
mod scene_json;
mod sound_builder;
mod source;
mod synth;

#[cfg(test)]
mod build_tests;

pub use build::BuildResult;
pub use description::{
    SceneDescription, SceneSummary, SoundSummary, SourceDescription, SpeakerDescription,
    TakeDescription,
};
pub use presets::{list_presets, read_preset, PresetSummary};
pub use scene::{Atmosphere, AtmosphereConfig, Band, ListenerConfig, SceneConfig, WeatherConfig};
pub use scene_file::{saved_by_editor, EDITOR_MARK};
pub use sound_builder::{
    bearing_for_heading, name_and_color, radiation_of, yaw_for_bearing, BuiltSound, SoundBuilder,
};
