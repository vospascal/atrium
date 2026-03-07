//! DbapRenderer — per-path DBAP gain ramp × sample per channel.
//!
//! Used by DBAP mode. Iterates over propagation paths from the PathResolver.
//! For each path, computes DBAP gains from the path's apparent origin position
//! (source position for direct, image-source position for reflections).
//! DBAP is listener-independent: gains depend on source→speaker distances only.
//!
//! Each path gets its own set of per-channel gains, interpolated per-sample
//! (click-free gain ramp). With `DirectPathResolver` (1 path, gain=1.0),
//! output is identical to the pre-path architecture.

use atrium_core::dbap::{dbap_gains, DbapParams};
use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};

use crate::pipeline::path::{PathEffectChain, PathKind, PathSet, MAX_PATHS};
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Multichannel per-path gain-ramp renderer for DBAP mode.
pub struct DbapRenderer {
    /// Previous per-channel gains per source per path.
    /// Indexed [source_idx][path_idx][channel].
    prev_gains: Vec<[[f32; MAX_CHANNELS]; MAX_PATHS]>,
    params: DbapParams,
    weights: Vec<f32>,
}

impl DbapRenderer {
    pub fn new(params: DbapParams) -> Self {
        Self {
            prev_gains: Vec::new(),
            params,
            weights: Vec::new(),
        }
    }
}

impl Renderer for DbapRenderer {
    #[allow(clippy::needless_range_loop)]
    fn render_source(
        &mut self,
        source_idx: usize,
        source: &mut dyn atrium_core::source::SoundSource,
        source_stages: &mut [&mut dyn SourceStage],
        ctx: &SourceContext,
        src_out: &SourceOutput,
        paths: &PathSet,
        path_effects: &mut [PathEffectChain],
        out: &mut OutputBuffer,
    ) {
        let path_slice = paths.as_slice();
        let speakers = ctx.layout.speakers();
        let speaker_count = ctx.layout.speaker_count();

        // Compute per-path DBAP gains for this buffer
        let mut target_gains = [[0.0f32; MAX_CHANNELS]; MAX_PATHS];
        for (pi, path) in path_slice.iter().enumerate() {
            let source_pos = match path.kind {
                PathKind::Direct => ctx.source_pos,
                // Reconstruct image-source position from direction + distance.
                // direction is "unit vector from target toward apparent origin",
                // target = listener position.
                _ => ctx.listener.position + path.direction * path.distance,
            };

            let mut gains = dbap_gains(
                source_pos,
                speakers,
                speaker_count,
                &self.weights,
                &self.params,
            );
            ctx.layout.apply_mask(&mut gains);
            target_gains[pi] = gains.gains;
        }

        let inv_frames = 1.0 / out.num_frames as f32;
        let prev = &self.prev_gains[source_idx];

        for frame in 0..out.num_frames {
            let t = frame as f32 * inv_frames;
            let raw = source.next_sample(out.sample_rate);

            // Per-sample source stage DSP (ground effect, etc.)
            let mut sample = raw;
            for stage in source_stages.iter_mut() {
                sample = stage.process_sample(sample);
            }

            // Apply broadband modifiers (ground effect gain, etc.)
            sample *= src_out.gain_modifier;

            // Accumulate all propagation paths, each with its own DBAP gains.
            let base = frame * out.channels;
            for (pi, path) in path_slice.iter().enumerate() {
                let filtered = path_effects[pi].process_sample(sample);
                let path_sample = filtered * path.gain;
                for ch in 0..out.channels {
                    let gain = prev[pi][ch] + (target_gains[pi][ch] - prev[pi][ch]) * t;
                    out.buffer[base + ch] += path_sample * gain;
                }
            }
        }

        // Store targets as prev for next buffer
        self.prev_gains[source_idx] = target_gains;
    }

    fn name(&self) -> &str {
        "dbap"
    }

    fn ensure_topology(&mut self, source_count: usize, layout: &SpeakerLayout, _sample_rate: f32) {
        while self.prev_gains.len() < source_count {
            self.prev_gains.push([[0.0; MAX_CHANNELS]; MAX_PATHS]);
        }
        let sc = layout.speaker_count();
        if self.weights.len() != sc {
            self.weights = vec![1.0; sc];
        }
    }

    fn reset(&mut self) {
        for gains in &mut self.prev_gains {
            *gains = [[0.0; MAX_CHANNELS]; MAX_PATHS];
        }
    }
}
