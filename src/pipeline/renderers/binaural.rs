//! HrtfRenderer — HRTF FFT convolution to stereo headphones.
//!
//! Uses `SourceOutput::distance_gain` for attenuation and performs HRTF
//! convolution via FFTConvolver. Output is always stereo (channels 0, 1).

use fft_convolver::FFTConvolver;
use sofar::reader::{Filter, OpenOptions, Sofar};

use atrium_core::listener::Listener;
use atrium_core::speaker::SpeakerLayout;
use atrium_core::types::Vec3;

use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

const BLOCK_SIZE: usize = 128;
const FILTER_UPDATE_INTERVAL: usize = 4;

struct HrtfSource {
    conv_left: FFTConvolver<f32>,
    conv_right: FFTConvolver<f32>,
    prev_gain: f32,
}

pub struct HrtfRenderer {
    sofa: Option<Sofar>,
    sources: Vec<HrtfSource>,
    filter: Option<Filter>,
    mono_buf: Vec<f32>,
    left_buf: Vec<f32>,
    right_buf: Vec<f32>,
    update_counter: usize,
    sample_rate: f32,
}

impl HrtfRenderer {
    pub fn new(hrtf_path: &str, sample_rate: f32) -> Self {
        match Self::try_load(hrtf_path, sample_rate) {
            Ok(r) => r,
            Err(e) => {
                eprintln!("HRTF not available: {e}");
                Self {
                    sofa: None,
                    sources: Vec::new(),
                    filter: None,
                    mono_buf: vec![0.0; BLOCK_SIZE],
                    left_buf: vec![0.0; BLOCK_SIZE],
                    right_buf: vec![0.0; BLOCK_SIZE],
                    update_counter: 0,
                    sample_rate,
                }
            }
        }
    }

    fn try_load(hrtf_path: &str, sample_rate: f32) -> Result<Self, Box<dyn std::error::Error>> {
        let sofa = OpenOptions::new()
            .sample_rate(sample_rate)
            .open(hrtf_path)?;

        let filt_len = sofa.filter_len();
        let filter = Filter::new(filt_len);

        Ok(Self {
            sofa: Some(sofa),
            sources: Vec::new(),
            filter: Some(filter),
            mono_buf: vec![0.0; BLOCK_SIZE],
            left_buf: vec![0.0; BLOCK_SIZE],
            right_buf: vec![0.0; BLOCK_SIZE],
            update_counter: 0,
            sample_rate,
        })
    }

    fn new_source(sofa: &Sofar) -> Option<HrtfSource> {
        let filt_len = sofa.filter_len();
        let mut init_filter = Filter::new(filt_len);
        sofa.filter(0.0, 1.0, 0.0, &mut init_filter);

        let mut conv_left = FFTConvolver::default();
        let mut conv_right = FFTConvolver::default();
        conv_left.init(BLOCK_SIZE, &init_filter.left).ok()?;
        conv_right.init(BLOCK_SIZE, &init_filter.right).ok()?;

        Some(HrtfSource {
            conv_left,
            conv_right,
            prev_gain: 0.0,
        })
    }
}

/// Convert source position to SOFA listener-relative coordinates.
/// NOTE: Only yaw rotation is applied. Pitch and roll are ignored — the
/// atrium installation assumes a horizontal listener plane.
fn to_sofa_coords(source_pos: Vec3, listener: &Listener) -> (f32, f32, f32) {
    let d = source_pos - listener.position;
    let yaw = listener.yaw;
    let forward = d.x * yaw.cos() + d.y * yaw.sin();
    let right = d.x * yaw.sin() - d.y * yaw.cos();
    (forward, -right, d.z)
}

impl Renderer for HrtfRenderer {
    fn render_source(
        &mut self,
        source_idx: usize,
        source: &mut dyn atrium_core::source::SoundSource,
        source_stages: &mut [&mut dyn SourceStage],
        ctx: &SourceContext,
        src_out: &SourceOutput,
        out: &mut OutputBuffer,
    ) {
        let sofa = match &self.sofa {
            Some(s) => s,
            None => return,
        };

        if source_idx >= self.sources.len() {
            return;
        }

        let target_gain = src_out.distance_gain * src_out.gain_modifier;
        let prev_gain = self.sources[source_idx].prev_gain;
        let inv_frames = 1.0 / out.num_frames as f32;

        // Update HRTF filter periodically
        let should_update = self.update_counter.is_multiple_of(FILTER_UPDATE_INTERVAL);
        if should_update {
            let (sx, sy, sz) = to_sofa_coords(ctx.source_pos, ctx.listener);
            if let Some(ref mut filter) = self.filter {
                sofa.filter(sx, sy, sz, filter);
                let _ = self.sources[source_idx]
                    .conv_left
                    .set_response(&filter.left);
                let _ = self.sources[source_idx]
                    .conv_right
                    .set_response(&filter.right);
            }
        }

        // Process in blocks of BLOCK_SIZE for FFT convolution
        let mut frame = 0;
        while frame < out.num_frames {
            let block_len = (out.num_frames - frame).min(BLOCK_SIZE);

            if self.mono_buf.len() < block_len {
                self.mono_buf.resize(block_len, 0.0);
                self.left_buf.resize(block_len, 0.0);
                self.right_buf.resize(block_len, 0.0);
            }

            // Generate gain-ramped mono samples through source stages
            for i in 0..block_len {
                let t = (frame + i) as f32 * inv_frames;
                let gain = prev_gain + (target_gain - prev_gain) * t;
                let raw = source.next_sample(out.sample_rate);

                let mut sample = raw;
                for stage in source_stages.iter_mut() {
                    sample = stage.process_sample(sample);
                }

                self.mono_buf[i] = sample * gain;
            }

            // HRTF convolution: mono → L/R
            self.left_buf[..block_len].fill(0.0);
            let _ = self.sources[source_idx]
                .conv_left
                .process(&self.mono_buf[..block_len], &mut self.left_buf[..block_len]);

            self.right_buf[..block_len].fill(0.0);
            let _ = self.sources[source_idx].conv_right.process(
                &self.mono_buf[..block_len],
                &mut self.right_buf[..block_len],
            );

            // Accumulate into interleaved stereo output
            for i in 0..block_len {
                let base = (frame + i) * out.channels;
                out.buffer[base] += self.left_buf[i];
                if out.channels > 1 {
                    out.buffer[base + 1] += self.right_buf[i];
                }
            }

            frame += block_len;
        }

        self.sources[source_idx].prev_gain = target_gain;
        self.update_counter += 1;
    }

    fn name(&self) -> &str {
        "hrtf"
    }

    fn ensure_topology(&mut self, source_count: usize, _layout: &SpeakerLayout, sample_rate: f32) {
        self.sample_rate = sample_rate;
        if let Some(ref sofa) = self.sofa {
            while self.sources.len() < source_count {
                if let Some(src) = Self::new_source(sofa) {
                    self.sources.push(src);
                }
            }
        }
    }

    fn reset(&mut self) {
        for src in &mut self.sources {
            src.prev_gain = 0.0;
            // FFTConvolver doesn't have a reset — we accept the brief tail artifact
        }
        self.update_counter = 0;
    }
}
