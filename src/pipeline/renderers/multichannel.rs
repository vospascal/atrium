//! MultichannelRenderer — per-path VBAP gain ramp × sample per channel.
//!
//! Used by VBAP mode. Iterates over propagation paths from the PathResolver.
//! For the **direct path**, computes full VBAP gains (angular panning +
//! distance attenuation + directivity + hearing). For **reflection paths**,
//! computes angular-only VBAP gains (panning direction from path.direction),
//! with the reflection's energy carried by path.gain.
//!
//! Each path gets its own set of per-channel gains, interpolated per-sample
//! (click-free gain ramp). With `DirectPathResolver` (1 path, gain=1.0),
//! output is identical to the pre-path architecture.

use atrium_core::speaker::{RenderMode, SpeakerLayout, MAX_CHANNELS};

use crate::pipeline::path::{PathKind, PathSet, MAX_PATHS};
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Multichannel per-path gain-ramp renderer for VBAP mode.
#[derive(Default)]
pub struct MultichannelRenderer {
    /// Previous per-channel gains per source per path.
    /// Indexed [source_idx][path_idx][channel].
    prev_gains: Vec<[[f32; MAX_CHANNELS]; MAX_PATHS]>,
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
        ctx: &SourceContext,
        _src_out: &SourceOutput,
        paths: &PathSet,
        out: &mut OutputBuffer,
    ) {
        let path_slice = paths.as_slice();

        // Compute per-path VBAP gains for this buffer
        let mut target_gains = [[0.0f32; MAX_CHANNELS]; MAX_PATHS];
        for (pi, path) in path_slice.iter().enumerate() {
            let gains = match path.kind {
                PathKind::Direct => {
                    // Full VBAP gains: angular + distance + directivity + hearing
                    let dist_params = atrium_core::speaker::DistanceParams {
                        ref_distance: ctx.source_ref_distance,
                        max_distance: ctx.distance_model.max_distance,
                        rolloff: ctx.distance_model.rolloff,
                        model: ctx.distance_model.model,
                    };
                    let source_spatial = atrium_core::speaker::SourceSpatial {
                        position: ctx.source_pos,
                        orientation: ctx.source_orientation,
                        directivity: ctx.source_directivity,
                    };
                    let mut g = ctx.layout.compute_gains_with_spread(
                        RenderMode::Vbap,
                        ctx.listener,
                        &source_spatial,
                        &dist_params,
                        ctx.source_spread,
                    );
                    ctx.layout.apply_mask(&mut g);
                    g
                }
                _ => {
                    // Reflection/diffraction: angular panning only, energy in path.gain
                    let mut g = ctx
                        .layout
                        .compute_vbap_panning(ctx.listener, path.direction);
                    ctx.layout.apply_mask(&mut g);
                    g
                }
            };
            target_gains[pi] = gains.gains;
        }

        let inv_frames = 1.0 / out.num_frames as f32;
        let prev = &self.prev_gains[source_idx];

        for frame in 0..out.num_frames {
            let t = frame as f32 * inv_frames;
            let raw = source.next_sample(out.sample_rate);

            // Per-sample source stage DSP (air absorption filter, ground effect)
            let mut sample = raw;
            for stage in source_stages.iter_mut() {
                sample = stage.process_sample(sample);
            }

            // Apply broadband modifiers (ground effect gain, etc.)
            sample *= _src_out.gain_modifier;

            // Accumulate all propagation paths, each with its own VBAP gains.
            let base = frame * out.channels;
            for (pi, path) in path_slice.iter().enumerate() {
                let path_sample = sample * path.gain;
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
        "multichannel"
    }

    fn ensure_topology(&mut self, source_count: usize, _layout: &SpeakerLayout, _sample_rate: f32) {
        while self.prev_gains.len() < source_count {
            self.prev_gains.push([[0.0; MAX_CHANNELS]; MAX_PATHS]);
        }
    }

    fn reset(&mut self) {
        for gains in &mut self.prev_gains {
            *gains = [[0.0; MAX_CHANNELS]; MAX_PATHS];
        }
    }
}
