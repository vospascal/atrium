//! MultichannelRenderer — per-path VBAP gain ramp × sample per channel.
//!
//! Used by VBAP mode. Iterates over propagation paths from the PathResolver.
//! For the **direct path**, computes full VBAP gains (angular panning +
//! distance attenuation + directivity + hearing). For **reflection paths**,
//! uses a pre-computed 1° gain lookup table for fast angular panning,
//! with the reflection's energy carried by path.gain.
//!
//! Each path gets its own set of per-channel gains, interpolated per-sample
//! (click-free gain ramp over the buffer duration, typically 10.7ms at 512
//! samples / 48kHz — within the 10–50ms range for zipper-free VBAP panning
//! per Pulkki 1997 and Gandemer 2018). With `DirectPathResolver` (1 path,
//! gain=1.0), output is identical to the pre-path architecture.

use atrium_core::panner::distance_gain_at_model;
use atrium_core::speaker::{SpeakerLayout, VbapLookup, MAX_CHANNELS};

use crate::pipeline::path::{PathEffectChain, PathKind, PathSet, MAX_PATHS};
use crate::pipeline::renderer::{OutputBuffer, Renderer};
use crate::pipeline::SourceContext;

/// Multichannel per-path gain-ramp renderer for VBAP mode.
pub struct MultichannelRenderer {
    /// Previous per-channel gains per source per path.
    /// Indexed [source_idx][path_idx][channel].
    prev_gains: Vec<[[f32; MAX_CHANNELS]; MAX_PATHS]>,
    /// Pre-computed VBAP panning gains at 1° resolution for reflection paths.
    vbap_lut: Option<VbapLookup>,
    /// Experimental: enable Gjørup et al. stereo polarity inversion.
    /// Only applies to stereo (2-speaker) layouts on direct paths.
    pub extended_vbap: bool,
    /// Maximum extension as fraction of speaker span (0.0–0.6). Default: 0.4.
    /// Gjørup et al. validated up to ~40% with 8 participants; beyond 0.6 the
    /// polarity-inversion illusion breaks down.
    pub vbap_extension: f32,
}

impl Default for MultichannelRenderer {
    fn default() -> Self {
        Self {
            prev_gains: Vec::new(),
            vbap_lut: None,
            extended_vbap: false,
            vbap_extension: 0.4,
        }
    }
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
        ctx: &SourceContext,
        paths: &PathSet,
        path_effects: &mut [PathEffectChain],
        out: &mut OutputBuffer,
    ) {
        let path_slice = paths.as_slice();

        // Update VBAP lookup table if listener has moved
        if let Some(ref mut lut) = self.vbap_lut {
            lut.update(ctx.layout, ctx.listener);
        }

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
                        ctx.listener,
                        &source_spatial,
                        &dist_params,
                        ctx.source_spread,
                    );
                    ctx.layout.apply_mask(&mut g);
                    g
                }
                _ if self.extended_vbap && ctx.layout.speaker_count() == 2 => {
                    // Extended VBAP: allow polarity inversion for stereo (Gjørup et al.)
                    let mut g = ctx.layout.compute_vbap_panning_extended(
                        ctx.listener,
                        path.direction,
                        self.vbap_extension,
                    );
                    ctx.layout.apply_mask(&mut g);
                    g
                }
                _ => {
                    // Reflection/diffraction: use LUT for fast angular panning
                    let mut g = if let Some(ref lut) = self.vbap_lut {
                        lut.lookup(path.direction)
                    } else {
                        ctx.layout
                            .compute_vbap_panning(ctx.listener, path.direction)
                    };
                    // Apply distance model — same attenuation law as the direct path.
                    // path.gain carries reflectivity only; distance is applied here.
                    let dist_gain = distance_gain_at_model(
                        ctx.listener.position,
                        ctx.listener.position + path.direction * path.distance,
                        ctx.source_ref_distance,
                        ctx.distance_model.max_distance,
                        ctx.distance_model.rolloff,
                        ctx.distance_model.model,
                    );
                    for ch in 0..g.gains.len() {
                        g.gains[ch] *= dist_gain;
                    }
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

            // Accumulate all propagation paths, each with its own VBAP gains.
            let sample = raw;
            let base = frame * out.channels;
            for (pi, path) in path_slice.iter().enumerate() {
                // Per-path effects (air absorption, etc.) — each path gets its own filter.
                let filtered = path_effects[pi].process_sample(sample);
                let path_sample = filtered * path.gain;
                for ch in 0..out.channels {
                    let gain = prev[pi][ch] + (target_gains[pi][ch] - prev[pi][ch]) * t;
                    out.buffer[base + ch] += path_sample * gain;
                }
                // Only the direct path feeds the late reverb send.
                // Reflection paths are already reverberant energy (handled by
                // the early-reflection stage); feeding them again would overcount.
                if path.kind == PathKind::Direct {
                    if let Some(ref mut rev) = out.reverb_send {
                        rev[base] += path_sample * ctx.reverb_send;
                    }
                }
            }
        }

        // Store targets as prev for next buffer
        self.prev_gains[source_idx] = target_gains;
    }

    fn name(&self) -> &str {
        "multichannel"
    }

    fn ensure_topology(&mut self, source_count: usize, layout: &SpeakerLayout, _sample_rate: f32) {
        while self.prev_gains.len() < source_count {
            self.prev_gains.push([[0.0; MAX_CHANNELS]; MAX_PATHS]);
        }
        if self.vbap_lut.is_none() {
            self.vbap_lut = Some(VbapLookup::new(layout.total_channels()));
        }
    }

    fn reset(&mut self) {
        for gains in &mut self.prev_gains {
            *gains = [[0.0; MAX_CHANNELS]; MAX_PATHS];
        }
    }
}
