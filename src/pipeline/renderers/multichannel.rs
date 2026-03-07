//! MultichannelRenderer — path-based gain ramp × sample per channel.
//!
//! Used by VBAP mode. Iterates over propagation paths from the PathResolver,
//! multiplying each sample by the path's geometric gain. SourceStages compute
//! per-channel VBAP gains in `SourceOutput::channel_gains`. This renderer
//! applies those gains with per-sample linear interpolation (click-free).
//!
//! With `DirectPathResolver` (1 path, gain=1.0), output is identical to the
//! pre-path architecture. With `ImageSourceResolver`, each reflection path
//! will be panned to its own direction.

use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};

use crate::pipeline::path::PathSet;
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Multichannel gain-ramp renderer for VBAP mode.
#[derive(Default)]
pub struct MultichannelRenderer {
    /// Previous per-channel gains per source. Indexed [source_idx][channel].
    prev_gains: Vec<[f32; MAX_CHANNELS]>,
}

impl MultichannelRenderer {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Renderer for MultichannelRenderer {
    #[allow(clippy::needless_range_loop)]
    fn render_source(
        &mut self,
        source_idx: usize,
        source: &mut dyn atrium_core::source::SoundSource,
        source_stages: &mut [&mut dyn SourceStage],
        _ctx: &SourceContext,
        src_out: &SourceOutput,
        paths: &PathSet,
        out: &mut OutputBuffer,
    ) {
        let inv_frames = 1.0 / out.num_frames as f32;
        let prev = &self.prev_gains[source_idx];
        let target = &src_out.channel_gains;

        for frame in 0..out.num_frames {
            let t = frame as f32 * inv_frames;
            let raw = source.next_sample(out.sample_rate);

            // Per-sample source stage DSP (air absorption filter, reflections)
            let mut sample = raw;
            for stage in source_stages.iter_mut() {
                sample = stage.process_sample(sample);
            }

            // Apply ground effect and other broadband modifiers
            sample *= src_out.gain_modifier;

            // Accumulate all propagation paths (direct + reflections).
            // Each path contributes its geometric gain (wall reflectivity, etc.).
            for path in paths.as_slice() {
                let path_sample = sample * path.gain;

                let base = frame * out.channels;
                for ch in 0..out.channels {
                    let gain = prev[ch] + (target.gains[ch] - prev[ch]) * t;
                    out.buffer[base + ch] += path_sample * gain;
                }
            }
        }

        // Store target as prev for next buffer
        self.prev_gains[source_idx] = target.gains;
    }

    fn name(&self) -> &str {
        "multichannel"
    }

    fn ensure_topology(&mut self, source_count: usize, _layout: &SpeakerLayout, _sample_rate: f32) {
        while self.prev_gains.len() < source_count {
            self.prev_gains.push([0.0; MAX_CHANNELS]);
        }
    }

    fn reset(&mut self) {
        for gains in &mut self.prev_gains {
            gains.fill(0.0);
        }
    }
}
