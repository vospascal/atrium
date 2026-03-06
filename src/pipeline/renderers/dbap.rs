//! DbapRenderer — gain ramp × sample per channel.
//!
//! Structurally identical to MultichannelRenderer. SourceStages compute
//! per-channel DBAP gains in `SourceOutput::channel_gains`. This renderer
//! applies those gains with per-sample linear interpolation (click-free).

use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};

use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Multichannel gain-ramp renderer for DBAP mode.
#[derive(Default)]
pub struct DbapRenderer {
    prev_gains: Vec<[f32; MAX_CHANNELS]>,
}

impl DbapRenderer {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Renderer for DbapRenderer {
    #[allow(clippy::needless_range_loop)]
    fn render_source(
        &mut self,
        source_idx: usize,
        source: &mut dyn atrium_core::source::SoundSource,
        source_stages: &mut [&mut dyn SourceStage],
        _ctx: &SourceContext,
        src_out: &SourceOutput,
        out: &mut OutputBuffer,
    ) {
        let inv_frames = 1.0 / out.num_frames as f32;
        let prev = &self.prev_gains[source_idx];
        let target = &src_out.channel_gains;

        for frame in 0..out.num_frames {
            let t = frame as f32 * inv_frames;
            let raw = source.next_sample(out.sample_rate);

            let mut sample = raw;
            for stage in source_stages.iter_mut() {
                sample = stage.process_sample(sample);
            }

            sample *= src_out.gain_modifier;

            let base = frame * out.channels;
            for ch in 0..out.channels {
                let gain = prev[ch] + (target.gains[ch] - prev[ch]) * t;
                out.buffer[base + ch] += sample * gain;
            }
        }

        self.prev_gains[source_idx] = target.gains;
    }

    fn name(&self) -> &str {
        "dbap"
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
