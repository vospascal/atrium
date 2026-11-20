//! The Atrium engine as seen from Kotlin.
//!
//! This crate is the whole surface the Kotlin Multiplatform app can reach:
//! [`list_scenes`], [`head_trackers`], one [`Engine`] object, the records it
//! hands out, and its error. OxiKT
//! generates the Kotlin for desktop JVM and iOS from these declarations.

mod api;
mod engine;
mod head_tracking;
mod output;
mod scenes;

pub use api::{
    EmitterKind, EngineError, Position, SceneDescription, SourceDescription, SourceSignal,
    SourceTelemetry, Telemetry,
};
pub use engine::Engine;
pub use head_tracking::{head_trackers, HeadTracker, HeadTrackingStatus};
pub use output::{ChannelMode, Output, RenderMode};
pub use scenes::{list_scenes, SceneFile, SoundSummary};

uniffi::setup_scaffolding!();
