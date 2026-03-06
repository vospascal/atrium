//! AmbisonicsRenderer — decode B-format to speaker gains, then gain ramp × sample.
//!
//! The AmbisonicsEncodeStage writes FOA B-format (W, Y, X) into
//! channel_gains[0..2]. This renderer decodes to speaker gains via
//! FoaDecoder, then applies per-sample linear interpolation.
//!
//! The decoder is rebuilt per-buffer because speaker azimuths are
//! listener-relative and the listener can move/turn each frame.

use atrium_core::ambisonics::{BFormat, FoaDecoder};
use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};

use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// FOA decode + gain-ramp renderer.
#[derive(Default)]
pub struct AmbisonicsRenderer {
    decoder: Option<FoaDecoder>,
    prev_gains: Vec<[f32; MAX_CHANNELS]>,
    /// Cached layout info for decoder rebuild.
    speaker_count: usize,
}

impl AmbisonicsRenderer {
    pub fn new() -> Self {
        Self::default()
    }
}

impl Renderer for AmbisonicsRenderer {
    #[allow(clippy::needless_range_loop)]
    fn render_source(
        &mut self,
        source_idx: usize,
        source: &mut dyn atrium_core::source::SoundSource,
        source_stages: &mut [&mut dyn SourceStage],
        ctx: &SourceContext,
        src_out: &SourceOutput,
        out: &mut OutputBuffer,
    ) {
        // Rebuild decoder with current listener position/yaw.
        // Only on first source per buffer (all sources share the same listener).
        if source_idx == 0 {
            self.decoder = Some(FoaDecoder::from_listener(
                ctx.layout.speakers(),
                self.speaker_count,
                ctx.listener,
            ));
        }

        let decoder = match &self.decoder {
            Some(d) => d,
            None => return,
        };

        // Decode B-format to speaker gains
        let bformat = BFormat {
            w: src_out.channel_gains.gains[0],
            y: src_out.channel_gains.gains[1],
            x: src_out.channel_gains.gains[2],
        };
        let target = decoder.decode(&bformat);

        let inv_frames = 1.0 / out.num_frames as f32;
        let prev = &self.prev_gains[source_idx];

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
        "ambisonics"
    }

    fn ensure_topology(&mut self, source_count: usize, layout: &SpeakerLayout, _sample_rate: f32) {
        self.speaker_count = layout.speaker_count();

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
