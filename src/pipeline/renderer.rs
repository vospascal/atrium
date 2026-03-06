//! Renderer trait — the mode-specific core of the pipeline.
//!
//! Handles how a mono source signal becomes multichannel output.
//! Three implementations:
//!
//! - **MultichannelRenderer**: gain ramp × sample per channel (VBAP, Stereo)
//! - **WorldLockedRenderer**: per-speaker PathStages + gain ramp (WorldLocked)
//! - **BinauralRenderer**: HRTF FFT convolution to stereo (Binaural)
//!
//! The renderer owns PathStage instances and manages per-source state
//! (gain ramps, HRTF convolvers, etc.).

use atrium_core::speaker::SpeakerLayout;

use super::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Renders one source's samples into the output buffer.
///
/// Called after all SourceStages have run for this source.
pub trait Renderer: Send {
    /// Render one source into the output buffer.
    ///
    /// - `source_idx`: index into per-source state owned by this renderer
    /// - `source`: the SoundSource to pull samples from
    /// - `source_stages`: per-source stages for `process_sample()` in the inner loop
    /// - `ctx`: full source geometry (position, orientation, directivity)
    /// - `src_out`: gains/modifiers computed by SourceStages
    /// - `buffer`: interleaved output buffer to accumulate into
    fn render_source(
        &mut self,
        source_idx: usize,
        source: &mut dyn atrium_core::source::SoundSource,
        source_stages: &mut [&mut dyn SourceStage],
        ctx: &SourceContext,
        src_out: &SourceOutput,
        buffer: &mut [f32],
        channels: usize,
        num_frames: usize,
        sample_rate: f32,
    );

    /// Human-readable name for profiling/debugging.
    fn name(&self) -> &str;

    /// Resize internal state when sources, layout, or sample rate change.
    ///
    /// Called at init and whenever the topology changes (source added/removed,
    /// speaker mask changed, layout reconfigured).
    fn ensure_topology(&mut self, source_count: usize, layout: &SpeakerLayout, sample_rate: f32);

    /// Reset all per-source state (gain ramps, filters, convolvers).
    /// Called on mode switch to avoid artifacts from stale state.
    fn reset(&mut self);
}
