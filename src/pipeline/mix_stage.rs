//! Post-mix processing stage.
//!
//! MixStages operate on the full interleaved output buffer after all sources
//! have been mixed. They run sequentially in the order defined by the pipeline.
//!
//! Each stage receives the full mix context for post-mix processing.

use atrium_core::listener::Listener;
use atrium_core::speaker::SpeakerLayout;
use atrium_core::types::Vec3;

use crate::audio::atmosphere::AtmosphericParams;
use crate::pipeline::path::WallMaterial;

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
    /// Number of channels the renderer actually writes to.
    /// HRTF always renders stereo (2) even on multichannel layouts.
    /// FDN reverb uses this to avoid spreading wet signal to unused channels.
    pub render_channels: usize,
    /// Optional reverb send buffer (pre-weighted by per-source d/d_c).
    /// FDN reverb stages read from this instead of the main buffer for
    /// delay line injection, preserving per-source direct-to-reverberant balance.
    pub reverb_input: Option<&'a [f32]>,
    /// Wall reflectivity (0.0–1.0) for Sabine RT60 computation in FDN stages.
    pub wall_reflectivity: f32,
    /// Per-wall surface materials for frequency-dependent RT60 computation.
    pub wall_materials: &'a [WallMaterial; 6],
    pub atmosphere: &'a AtmosphericParams,
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
