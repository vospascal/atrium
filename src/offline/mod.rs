//! A scene rendered to a file instead of the speakers: exactly what the
//! engine plays, faster than real time, the same on every run.
//!
//! The render drives the scene the way the audio callback does, block by
//! block through `AudioScene::process_commands` and `AudioScene::render`,
//! prepared the way the output device prepares it. Nothing here renders sound
//! itself, so a file cannot drift from what is heard.
//!
//! One thing differs from a playing scene, deliberately: the plan is driven in
//! step with the blocks (`crate::plan::SteppedPlanner`) instead of on its own
//! thread against the wall clock. Rendered faster than real time, that thread
//! would fall behind and takes would go missing or arrive late, differently on
//! every run. In step, the takes are the ones the live planner hands over, and
//! the same seed gives the same file, bit for bit.
//!
//! - `settings`: what to render: how long, from when, for which output
//! - `rendering`: the scene prepared and played into frames
//! - `statistics`: peak and RMS per channel
//! - `provenance`: which scene file and which commit made a file
//! - `files`: the WAV and its JSON sidecar

mod files;
mod provenance;
mod rendering;
mod settings;
mod statistics;

#[cfg(test)]
mod tests;

pub use files::{write_files, Written};
pub use provenance::Provenance;
pub use rendering::{Rendered, Rendering};
pub use settings::{Output, RenderSettings};
pub use statistics::ChannelStatistics;
