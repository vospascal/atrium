//! MultichannelRenderer — per-path VBAP gain ramp × sample per channel.
//!
//! Used by VBAP mode. Iterates over propagation paths from the PathResolver.
//! For the **direct path**, computes full VBAP gains (angular panning +
//! distance attenuation + directivity). For **reflection paths**,
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
use crate::pipeline::renderer::SourceContext;
use crate::pipeline::renderer::{OutputBuffer, Renderer};

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
                    // Full VBAP gains: angular + distance + directivity
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
                    // Reflection: use LUT for fast angular panning
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

#[cfg(test)]
mod tests {
    use super::*;
    use atrium_core::directivity::DirectivityPattern;
    use atrium_core::listener::Listener;
    use atrium_core::panner::DistanceModelType;
    use atrium_core::source::SoundSource;
    use atrium_core::speaker::{DistanceParams, SourceSpatial};
    use atrium_core::types::Vec3;

    use crate::audio::distance::DistanceModel;
    use crate::pipeline::environment::Environment;
    use crate::pipeline::path::PathContribution;

    const SAMPLE_RATE: f32 = 48000.0;
    const CHANNELS: usize = 6;
    const FRAMES: usize = 512;

    /// A steady unit signal, so every output sample is its gain.
    struct UnitSignal;

    impl SoundSource for UnitSignal {
        fn next_sample(&mut self, _sample_rate: f32) -> f32 {
            1.0
        }
        fn position(&self) -> Vec3 {
            Vec3::ZERO
        }
        fn tick(&mut self, _dt: f32) {}
    }

    /// Quad on a 4 × 4 m square around a listener at the origin facing +X:
    /// FL (−2, 2) is at azimuth 135°, FR (2, 2) at 45°, … (channels 0, 1, 4, 5).
    fn quad_layout() -> SpeakerLayout {
        SpeakerLayout::quad(
            Vec3::new(-2.0, 2.0, 0.0),
            Vec3::new(2.0, 2.0, 0.0),
            Vec3::new(-2.0, -2.0, 0.0),
            Vec3::new(2.0, -2.0, 0.0),
        )
    }

    fn inverse_law() -> DistanceModel {
        DistanceModel {
            model: DistanceModelType::Inverse,
            ref_distance: 1.0,
            max_distance: 1000.0,
            rolloff: 1.0,
        }
    }

    /// Render `buffers` buffers of a unit signal; returns every buffer and
    /// the reverb send of the last one.
    fn render(
        source_position: Vec3,
        paths: &PathSet,
        reverb_send_level: f32,
        buffers: usize,
    ) -> (Vec<Vec<f32>>, Vec<f32>) {
        let layout = quad_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let environment = Environment::anechoic(
            Vec3::new(-500.0, -500.0, 0.0),
            Vec3::new(500.0, 500.0, 500.0),
        );
        let distance_model = inverse_law();
        let directivity = DirectivityPattern::Omni;
        let ctx = SourceContext {
            listener: &listener,
            source_pos: source_position,
            source_orientation: Vec3::new(1.0, 0.0, 0.0),
            source_directivity: &directivity,
            source_spread: 0.0,
            source_ref_distance: 1.0,
            path_target: Vec3::ZERO,
            environment: &environment,
            sample_rate: SAMPLE_RATE,
            distance_model: &distance_model,
            layout: &layout,
            reverb_send: reverb_send_level,
        };
        let mut renderer = MultichannelRenderer::new();
        renderer.ensure_topology(1, &layout, SAMPLE_RATE);
        let mut chains: Vec<PathEffectChain> = (0..MAX_PATHS)
            .map(|_| PathEffectChain::new(Vec::new()))
            .collect();
        let mut blocks = Vec::new();
        let mut reverb = Vec::new();
        for _ in 0..buffers {
            let mut block = vec![0.0f32; FRAMES * CHANNELS];
            reverb = vec![0.0f32; FRAMES * CHANNELS];
            let mut out = OutputBuffer {
                buffer: &mut block,
                reverb_send: Some(&mut reverb),
                channels: CHANNELS,
                num_frames: FRAMES,
                sample_rate: SAMPLE_RATE,
            };
            renderer.render_source(0, &mut UnitSignal, &ctx, paths, &mut chains, &mut out);
            blocks.push(block);
        }
        (blocks, reverb)
    }

    fn path(kind: PathKind, direction: Vec3, distance: f32, gain: f32) -> PathSet {
        let mut paths = PathSet::new();
        paths.push(PathContribution {
            kind,
            direction: direction.normalize(),
            distance,
            delay_seconds: 0.0,
            gain,
            wall_index: None,
        });
        paths
    }

    /// The direct path plays the layout's full VBAP gains (pan × distance ×
    /// directivity), reached by a linear ramp from silence: frame n of the
    /// first buffer is gain · n / 512, and the second buffer holds the gain.
    #[test]
    fn direct_path_ramps_linearly_to_the_layout_gains() {
        let source_position = Vec3::new(3.0, 1.0, 0.0);
        let paths = path(
            PathKind::Direct,
            source_position,
            source_position.length(),
            1.0,
        );
        let (blocks, _) = render(source_position, &paths, 0.0, 2);

        let layout = quad_layout();
        let directivity = DirectivityPattern::Omni;
        let mut expected = layout.compute_gains_with_spread(
            &Listener::new(Vec3::ZERO, 0.0),
            &SourceSpatial {
                position: source_position,
                orientation: Vec3::new(1.0, 0.0, 0.0),
                directivity: &directivity,
            },
            &DistanceParams {
                ref_distance: 1.0,
                max_distance: 1000.0,
                rolloff: 1.0,
                model: DistanceModelType::Inverse,
            },
            0.0,
        );
        layout.apply_mask(&mut expected);
        assert!(expected.gains.iter().any(|gain| *gain > 0.1));

        for ch in 0..CHANNELS {
            for frame in [0usize, 128, 511] {
                let actual = blocks[0][frame * CHANNELS + ch];
                let wanted = expected.gains[ch] * frame as f32 / FRAMES as f32;
                assert!(
                    (actual - wanted).abs() < 1e-6,
                    "ch {ch} frame {frame}: {actual} vs {wanted}"
                );
            }
            for frame in 0..FRAMES {
                assert!((blocks[1][frame * CHANNELS + ch] - expected.gains[ch]).abs() < 1e-6);
            }
        }
    }

    /// A reflection arriving from FL's exact direction (azimuth 135°, an
    /// integer LUT entry) lands only in FL, at wall gain × inverse-law gain:
    /// 0.6 × (1 m / 2 m) = 0.3.
    #[test]
    fn reflection_from_a_speaker_direction_plays_only_that_speaker() {
        let paths = path(PathKind::Reflection, Vec3::new(-1.0, 1.0, 0.0), 2.0, 0.6);
        let (blocks, _) = render(Vec3::new(3.0, 1.0, 0.0), &paths, 0.0, 2);
        let settled = &blocks[1][100 * CHANNELS..101 * CHANNELS];
        assert!((settled[0] - 0.3).abs() < 1e-4, "FL {}", settled[0]);
        for (ch, gain) in settled.iter().enumerate().skip(1) {
            assert!(
                gain.abs() < 1e-4,
                "channel {ch} should be silent, got {gain}"
            );
        }
    }

    /// Late-reverb send = source signal × ctx.reverb_send on the channel-0
    /// mono bus, independent of distance and pan (the send level already
    /// encodes d²/(d² + d_c²)).
    #[test]
    fn reverb_send_is_the_source_signal_times_the_send_level() {
        let source_position = Vec3::new(10.0, 0.0, 0.0);
        let paths = path(PathKind::Direct, source_position, 10.0, 1.0);
        let (_, reverb) = render(source_position, &paths, 0.8, 1);
        for frame in 0..FRAMES {
            assert!((reverb[frame * CHANNELS] - 0.8).abs() < 1e-6);
            assert!(reverb[frame * CHANNELS + 1..(frame + 1) * CHANNELS]
                .iter()
                .all(|sample| *sample == 0.0));
        }
    }
}
