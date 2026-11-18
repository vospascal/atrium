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
mod environment;
mod scene;
mod scene_json;
mod source;
mod synth;

#[cfg(test)]
mod build_tests;

pub use build::BuildResult;
pub use scene::SceneConfig;
