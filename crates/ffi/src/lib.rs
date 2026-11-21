//! The Atrium engine as seen from Kotlin.
//!
//! This crate is the whole surface the Kotlin Multiplatform app can reach:
//! [`list_scenes`], [`list_presets`], [`head_trackers`], one [`Engine`] object, the records it
//! hands out, and its error. OxiKT
//! generates the Kotlin for desktop JVM and iOS from these declarations.

mod api;
mod engine;
mod head_tracking;
mod history;
mod library;
mod output;
mod scenes;

pub use api::{
    Band, Bout, Conditions, EditHistory, EmitterKind, EngineError, Follow, Gap, LoudnessMap,
    PlannedTake, Position, Probe, SceneDescription, SourceDescription, SourceSignal,
    SourceTelemetry, Speaker, Story, Take, Telemetry, Weather,
};
pub use engine::Engine;
pub use head_tracking::{head_trackers, HeadTracker, HeadTrackingStatus};
pub use library::{list_presets, Preset};
pub use output::{ChannelMode, Output, RenderMode};
pub use scenes::{list_scenes, SceneFile, SoundSummary};

uniffi::setup_scaffolding!();
