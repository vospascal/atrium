//! DbapRenderer — per-path DBAP gain ramp × sample per channel.
//!
//! Used by DBAP mode. Iterates over propagation paths from the PathResolver.
//! For each path, computes DBAP gains from the path's apparent origin position
//! (source position for direct, image-source position for reflections).
//! DBAP is listener-independent: gains depend on source→speaker distances only,
//! and propagation paths are resolved towards the middle of the speaker array
//! (`path_target`), so nothing in the speaker feed moves with the listener.
//!
//! Each path gets its own set of per-channel gains, interpolated per-sample
//! (click-free gain ramp). With `DirectPathResolver` (1 path, gain=1.0),
//! output is identical to the pre-path architecture.

use atrium_core::dbap::{dbap_gains, DbapParams};
use atrium_core::speaker::{SpeakerLayout, MAX_CHANNELS};

use crate::pipeline::path::{PathEffectChain, PathKind, PathSet, MAX_PATHS};
use crate::pipeline::renderer::SourceContext;
use crate::pipeline::renderer::{OutputBuffer, Renderer};

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
        ctx: &SourceContext,
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
                // direction is "unit vector from target toward apparent origin".
                _ => ctx.path_target + path.direction * path.distance,
            };

            let mut gains = dbap_gains(
                source_pos,
                speakers,
                speaker_count,
                &self.weights,
                &self.params,
            );
            ctx.layout.apply_mask(&mut gains);
            target_gains[pi] = gains.gains.map(|gain| gain * ctx.heard(path.kind));
        }

        let inv_frames = 1.0 / out.num_frames as f32;
        let prev = &self.prev_gains[source_idx];

        for frame in 0..out.num_frames {
            let t = frame as f32 * inv_frames;
            let raw = source.next_sample(out.sample_rate);

            // Accumulate all propagation paths, each with its own DBAP gains.
            let sample = raw;
            let base = frame * out.channels;
            for (pi, path) in path_slice.iter().enumerate() {
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
        "dbap"
    }

    fn path_target(
        &self,
        _listener: &atrium_core::listener::Listener,
        layout: &SpeakerLayout,
    ) -> atrium_core::types::Vec3 {
        layout.bounding_box_centre()
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

#[cfg(test)]
mod tests {
    use super::*;
    use atrium_core::directivity::DirectivityPattern;
    use atrium_core::listener::Listener;
    use atrium_core::panner::DistanceModelType;
    use atrium_core::source::SoundSource;
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

    fn quad_layout() -> SpeakerLayout {
        SpeakerLayout::quad(
            Vec3::new(-2.0, 2.0, 0.0),
            Vec3::new(2.0, 2.0, 0.0),
            Vec3::new(-2.0, -2.0, 0.0),
            Vec3::new(2.0, -2.0, 0.0),
        )
    }

    fn reference_gains(position: Vec3) -> [f32; MAX_CHANNELS] {
        let layout = quad_layout();
        let mut gains = dbap_gains(
            position,
            layout.speakers(),
            layout.speaker_count(),
            &[1.0; 4],
            &DbapParams::default(),
        );
        layout.apply_mask(&mut gains);
        gains.gains
    }

    /// Render two buffers of a unit signal with the listener at `listener_position`.
    fn render(
        listener_position: Vec3,
        source_position: Vec3,
        paths: &PathSet,
        reverb_send_level: f32,
    ) -> (Vec<Vec<f32>>, Vec<f32>) {
        let layout = quad_layout();
        let listener = Listener::new(listener_position, 0.0);
        let environment = Environment::anechoic(
            Vec3::new(-500.0, -500.0, 0.0),
            Vec3::new(500.0, 500.0, 500.0),
        );
        let distance_model = DistanceModel {
            model: DistanceModelType::Inverse,
            ref_distance: 1.0,
            max_distance: 1000.0,
            rolloff: 1.0,
        };
        let directivity = DirectivityPattern::Omni;
        let ctx = SourceContext {
            listener: &listener,
            source_pos: source_position,
            source_orientation: Vec3::new(1.0, 0.0, 0.0),
            source_directivity: &directivity,
            source_spread: 0.0,
            source_ref_distance: 1.0,
            path_target: listener_position,
            environment: &environment,
            sample_rate: SAMPLE_RATE,
            distance_model: &distance_model,
            layout: &layout,
            reverb_send: reverb_send_level,
            probe: atrium_core::probe::Probe::WHOLE,
        };
        let mut renderer = DbapRenderer::new(DbapParams::default());
        renderer.ensure_topology(1, &layout, SAMPLE_RATE);
        let mut chains: Vec<PathEffectChain> = (0..MAX_PATHS)
            .map(|_| PathEffectChain::new(Vec::new()))
            .collect();
        let mut blocks = Vec::new();
        let mut reverb = Vec::new();
        for _ in 0..2 {
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

    fn single_path(kind: PathKind, direction: Vec3, distance: f32, gain: f32) -> PathSet {
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

    /// The direct path plays `dbap_gains(source)` reached by a linear ramp:
    /// frame n of the first buffer is gain · n / 512; the second holds it.
    #[test]
    fn direct_path_ramps_linearly_to_the_dbap_gains() {
        let source_position = Vec3::new(1.0, 1.5, 0.0);
        let paths = single_path(
            PathKind::Direct,
            source_position,
            source_position.length(),
            1.0,
        );
        let (blocks, _) = render(Vec3::ZERO, source_position, &paths, 0.0);
        let expected = reference_gains(source_position);
        assert!(expected[1] > expected[4], "FR is nearest to (1, 1.5)");
        for ch in 0..CHANNELS {
            for frame in [0usize, 128, 511] {
                let actual = blocks[0][frame * CHANNELS + ch];
                let wanted = expected[ch] * frame as f32 / FRAMES as f32;
                assert!(
                    (actual - wanted).abs() < 1e-6,
                    "ch {ch} frame {frame}: {actual} vs {wanted}"
                );
            }
            assert!((blocks[1][300 * CHANNELS + ch] - expected[ch]).abs() < 1e-6);
        }
    }

    /// DBAP is listener-independent for the direct path: moving the listener
    /// does not change a single output sample.
    #[test]
    fn direct_path_output_does_not_depend_on_the_listener() {
        let source_position = Vec3::new(-1.0, 0.5, 0.0);
        let paths = single_path(PathKind::Direct, Vec3::new(1.0, 0.0, 0.0), 3.0, 1.0);
        let (at_centre, _) = render(Vec3::ZERO, source_position, &paths, 0.0);
        let (off_centre, _) = render(Vec3::new(1.5, -1.0, 0.0), source_position, &paths, 0.0);
        assert_eq!(at_centre, off_centre);
    }

    /// A reflection is panned from its image source, reconstructed as
    /// listener + direction × distance: listener (0.5, 0), direction −Y,
    /// 3 m → image at (0.5, −3). Its output is wall gain × DBAP(image).
    #[test]
    fn reflection_is_panned_from_its_image_source_position() {
        let listener_position = Vec3::new(0.5, 0.0, 0.0);
        let paths = single_path(PathKind::Reflection, Vec3::new(0.0, -1.0, 0.0), 3.0, 0.6);
        let (blocks, _) = render(listener_position, Vec3::new(0.5, 3.0, 0.0), &paths, 0.0);
        let expected = reference_gains(Vec3::new(0.5, -3.0, 0.0));
        assert!(expected[4] > expected[0], "image is behind: rear louder");
        for ch in 0..CHANNELS {
            assert!((blocks[1][300 * CHANNELS + ch] - 0.6 * expected[ch]).abs() < 1e-6);
        }
    }

    /// Late-reverb send = source signal × ctx.reverb_send on channel 0.
    #[test]
    fn reverb_send_is_the_source_signal_times_the_send_level() {
        let source_position = Vec3::new(10.0, 0.0, 0.0);
        let paths = single_path(PathKind::Direct, source_position, 10.0, 1.0);
        let (_, reverb) = render(Vec3::ZERO, source_position, &paths, 0.8);
        for frame in 0..FRAMES {
            assert!((reverb[frame * CHANNELS] - 0.8).abs() < 1e-6);
        }
    }
}
