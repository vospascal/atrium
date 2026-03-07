//! Per-source processing stage.
//!
//! SourceStages run once per source per buffer. They handle source-level DSP
//! that does NOT depend on the output path (envelopes, EQ, listener-relative
//! propagation for VBAP/HRTF modes).
//!
//! For WorldLocked mode, propagation is inlined in the renderer
//! because it is per-speaker, not per-source.

use atrium_core::directivity::DirectivityPattern;
use atrium_core::listener::Listener;
use atrium_core::speaker::{ChannelGains, SpeakerLayout};
use atrium_core::types::Vec3;

use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::distance::DistanceModel;
use crate::audio::propagation::GroundProperties;

/// Shared context for per-source stages. Borrowed from AudioScene per render call.
pub struct SourceContext<'a> {
    pub listener: &'a Listener,
    pub source_pos: Vec3,
    pub source_orientation: Vec3,
    pub source_directivity: &'a DirectivityPattern,
    pub source_spread: f32,
    pub source_ref_distance: f32,
    pub dist_to_listener: f32,
    pub atmosphere: &'a AtmosphericParams,
    pub room_min: Vec3,
    pub room_max: Vec3,
    pub ground: &'a GroundProperties,
    pub sample_rate: f32,
    pub distance_model: &'a DistanceModel,
    pub layout: &'a SpeakerLayout,
}

/// Per-source output accumulated by stages. Passed to the renderer.
pub struct SourceOutput {
    /// Broadband gain multiplier (ground effect, etc.).
    pub gain_modifier: f32,
    /// Target channel gains for multichannel rendering (VBAP).
    pub channel_gains: ChannelGains,
    /// Scalar distance gain for HRTF rendering.
    pub distance_gain: f32,
}

impl Default for SourceOutput {
    fn default() -> Self {
        Self {
            gain_modifier: 1.0,
            channel_gains: ChannelGains::silent(0),
            distance_gain: 1.0,
        }
    }
}

impl SourceOutput {
    /// Create with properly sized channel gains.
    pub fn default_for(total_channels: usize) -> Self {
        Self {
            gain_modifier: 1.0,
            channel_gains: ChannelGains::silent(total_channels),
            distance_gain: 1.0,
        }
    }
}

/// Per-source processing stage. Each implementation lives in its own file.
///
/// State is per-source: the pipeline holds one instance per source, managed
/// by `SourceStageBank` which uses factory functions to create new instances.
pub trait SourceStage: Send {
    /// Buffer-rate: update filter params, compute gains. Called once per source per buffer.
    fn process(&mut self, ctx: &SourceContext, output: &mut SourceOutput);

    /// Sample-rate: DSP in the tight inner loop. Default: passthrough.
    /// Override for sample-level processing (air absorption filter, reflection delays).
    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        sample
    }

    /// Human-readable name for profiling/debugging.
    fn name(&self) -> &str;

    /// Reset internal state (delay lines, filter state).
    fn reset(&mut self) {}
}
