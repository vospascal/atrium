//! Post-mix processing stage.
//!
//! MixStages operate on the full interleaved output buffer after all sources
//! have been mixed. They run sequentially in the order defined by the pipeline.
//!
//! Each stage receives the full mix context for post-mix processing.

use atrium_core::listener::Listener;
use atrium_core::speaker::SpeakerLayout;
use atrium_core::types::Vec3;

/// Context for post-mix stages.
pub struct MixContext<'a> {
    pub listener: &'a Listener,
    pub layout: &'a SpeakerLayout,
    pub sample_rate: f32,
    pub channels: usize,
    pub room_min: Vec3,
    pub room_max: Vec3,
    /// Runtime master gain from AudioScene. Flows through each render call.
    pub master_gain: f32,
}

/// Post-mix processing stage. Processes the full interleaved output buffer in-place.
pub trait MixStage: Send {
    /// Initialize with room geometry and sample rate.
    /// Called once when audio parameters become known, and again if they change.
    fn init(&mut self, _ctx: &MixContext) {}

    /// Process the interleaved output buffer in place.
    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext);

    /// Reset all internal state (delay lines, filter state).
    fn reset(&mut self) {}

    /// Human-readable name for profiling/debugging.
    fn name(&self) -> &str;
}
