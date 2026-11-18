//! Speaker delay compensation.
//!
//! Aligns time-of-arrival across speakers by delaying nearer channels.
//! Two modes:
//!
//! - **Static calibration** (WorldLocked): fixed delays from installation
//!   measurement. Does NOT track listener position.
//! - **Listener-relative** (VBAP): recomputes per-speaker delays every buffer
//!   based on current listener position.
//!
//! Uses per-channel circular delay buffers with power-of-2 capacity for
//! branch-free bitmask wrapping. Linear interpolation for fractional-sample
//! accuracy.

use atrium_core::speaker::MAX_CHANNELS;

use crate::pipeline::mix_stage::{MixContext, MixStage};

/// Delay compensation mode.
#[derive(Clone, Copy, Debug)]
enum Mode {
    /// Fixed delays — set once at init, never updated.
    Static,
    /// Recomputed every buffer from listener position.
    ListenerRelative,
}

/// Post-mix delay compensation stage.
pub struct DelayCompStage {
    mode: Mode,
    buffers: Vec<Vec<f32>>,
    write_pos: Vec<usize>,
    delays: Vec<f32>,
    capacity: usize,
}

impl DelayCompStage {
    /// WorldLocked mode: fixed delays based on installation calibration.
    pub fn static_calibration() -> Self {
        Self::new_inner(Mode::Static)
    }

    /// VBAP mode: delays track listener position each buffer.
    pub fn listener_relative() -> Self {
        Self::new_inner(Mode::ListenerRelative)
    }

    fn new_inner(mode: Mode) -> Self {
        Self {
            mode,
            buffers: Vec::new(),
            write_pos: Vec::new(),
            delays: vec![0.0; MAX_CHANNELS],
            capacity: 0,
        }
    }

    /// Compute per-speaker delays so all channels arrive simultaneously.
    /// Nearer speakers get MORE delay (aligned to the farthest).
    /// Channels without a speaker in the current layout get zero delay, so a
    /// re-init with fewer speakers does not keep the old layout's delays.
    fn compute_delays(&mut self, ctx: &MixContext) {
        self.delays.fill(0.0);
        let layout = ctx.layout;
        let target_pos = match self.mode {
            // For static: use room center as reference point
            Mode::Static => ctx.environment.centre(),
            // For listener-relative: use actual listener position
            Mode::ListenerRelative => ctx.listener.position,
        };

        let mut d_max = 0.0f32;
        for i in 0..layout.speaker_count() {
            if let Some(speaker) = layout.speaker_by_index(i) {
                let d = (speaker.position - target_pos).length();
                d_max = d_max.max(d);
            }
        }

        for i in 0..layout.speaker_count() {
            if let Some(speaker) = layout.speaker_by_index(i) {
                let d = (speaker.position - target_pos).length();
                let delay_seconds = (d_max - d) / ctx.environment.atmosphere.speed_of_sound();
                let delay_samples = delay_seconds * ctx.sample_rate;
                if speaker.channel < self.delays.len() {
                    self.delays[speaker.channel] = delay_samples;
                }
            }
        }
    }

    /// Ensure buffers are large enough for the room geometry.
    fn ensure_capacity(&mut self, ctx: &MixContext) {
        let room_diag = (ctx.environment.max - ctx.environment.min).length();
        let max_delay_samples = (room_diag / ctx.environment.atmosphere.speed_of_sound()
            * ctx.sample_rate)
            .ceil() as usize;
        let needed = max_delay_samples.next_power_of_two().max(1024);

        if needed > self.capacity || self.buffers.len() < ctx.channels {
            self.capacity = needed;
            self.buffers = vec![vec![0.0; needed]; ctx.channels];
            self.write_pos = vec![0; ctx.channels];
        }
    }

    #[inline]
    fn process_channel(&mut self, ch: usize, input: f32) -> f32 {
        let cap = self.capacity;
        let mask = cap - 1;

        let wp = self.write_pos[ch];
        self.buffers[ch][wp] = input;
        self.write_pos[ch] = (wp + 1) & mask;

        let delay = self.delays[ch];
        if delay < 0.5 {
            return input;
        }
        let delay_clamped = delay.min((cap - 2) as f32);
        let delay_int = delay_clamped as usize;
        let frac = delay_clamped - delay_int as f32;

        let idx0 = (wp + cap - delay_int) & mask;
        let idx1 = (wp + cap - delay_int - 1) & mask;

        let s0 = self.buffers[ch][idx0];
        let s1 = self.buffers[ch][idx1];
        s0 + (s1 - s0) * frac
    }
}

impl MixStage for DelayCompStage {
    fn init(&mut self, ctx: &MixContext) {
        self.ensure_capacity(ctx);
        // Static mode: compute delays once at init
        if matches!(self.mode, Mode::Static) {
            self.compute_delays(ctx);
        }
    }

    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        if self.capacity == 0 {
            return;
        }

        // Listener-relative: update delays every buffer
        if matches!(self.mode, Mode::ListenerRelative) {
            self.compute_delays(ctx);
        }

        let channels = ctx.channels;
        let num_frames = buffer.len() / channels;
        for frame in 0..num_frames {
            let base = frame * channels;
            for ch in 0..channels {
                buffer[base + ch] = self.process_channel(ch, buffer[base + ch]);
            }
        }
    }

    fn reset(&mut self) {
        for buf in &mut self.buffers {
            buf.fill(0.0);
        }
        for wp in &mut self.write_pos {
            *wp = 0;
        }
    }

    fn name(&self) -> &str {
        "delay_comp"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::atmosphere::AtmosphericParams;
    use crate::audio::propagation::GroundProperties;
    use crate::pipeline::environment::Environment;
    use crate::pipeline::wall_material::WallMaterial;
    use atrium_core::listener::Listener;
    use atrium_core::speaker::{Speaker, SpeakerLayout};
    use atrium_core::types::Vec3;

    const SAMPLE_RATE: f32 = 48_000.0;

    /// Three speakers at distinct distances from the room centre (origin):
    /// 1.0 m, 2.5 m and 4.0 m.
    fn three_speaker_layout() -> SpeakerLayout {
        SpeakerLayout::new(
            &[
                Speaker {
                    position: Vec3::new(1.0, 0.0, 0.0),
                    channel: 0,
                },
                Speaker {
                    position: Vec3::new(0.0, 2.5, 0.0),
                    channel: 1,
                },
                Speaker {
                    position: Vec3::new(0.0, 0.0, -4.0),
                    channel: 2,
                },
            ],
            None,
            3,
        )
    }

    fn test_environment(atmosphere: AtmosphericParams) -> Environment {
        Environment {
            min: Vec3::new(-5.0, -5.0, -5.0),
            max: Vec3::new(5.0, 5.0, 5.0),
            wall_materials: [WallMaterial::HARD_WALL; 6],
            ground: GroundProperties::hard(),
            atmosphere,
        }
    }

    fn make_context<'a>(
        layout: &'a SpeakerLayout,
        listener: &'a Listener,
        environment: &'a Environment,
    ) -> MixContext<'a> {
        MixContext {
            listener,
            layout,
            sample_rate: SAMPLE_RATE,
            channels: layout.total_channels(),
            environment,
            master_gain: 1.0,
            render_channels: layout.total_channels(),
            reverb_input: None,
            measurement_mode: false,
        }
    }

    /// Centroid (in samples) and total weight of one channel of an interleaved
    /// buffer. For a linearly interpolated impulse the centroid is exactly the
    /// (fractional) delay.
    fn impulse_centroid(buffer: &[f32], channels: usize, channel: usize) -> (f64, f64) {
        let mut weighted = 0.0f64;
        let mut total = 0.0f64;
        for frame in 0..buffer.len() / channels {
            let sample = buffer[frame * channels + channel] as f64;
            weighted += frame as f64 * sample;
            total += sample;
        }
        (weighted / total, total)
    }

    fn impulse_buffer(channels: usize, frames: usize, impulse_frame: usize) -> Vec<f32> {
        let mut buffer = vec![0.0f32; channels * frames];
        for channel in 0..channels {
            buffer[impulse_frame * channels + channel] = 1.0;
        }
        buffer
    }

    /// delay_samples = (d_max − d) / c · sample_rate, per channel, for the
    /// static (room-centre) reference.
    #[test]
    fn static_delays_equal_distance_difference_over_speed_of_sound() {
        let layout = three_speaker_layout();
        let listener = Listener::new(Vec3::new(3.0, 3.0, 3.0), 0.0);
        let atmosphere = AtmosphericParams::default();
        let environment = test_environment(atmosphere);
        let ctx = make_context(&layout, &listener, &environment);
        let mut stage = DelayCompStage::static_calibration();
        stage.init(&ctx);

        let speed_of_sound = 331.3 + 0.606 * 20.0; // 343.42 m/s at 20 °C
        for (channel, distance) in [(0usize, 1.0f32), (1, 2.5), (2, 4.0)] {
            let expected = (4.0 - distance) / speed_of_sound * SAMPLE_RATE;
            assert!(
                (stage.delays[channel] - expected).abs() < 1e-2,
                "ch{channel}: {} vs {expected}",
                stage.delays[channel]
            );
        }
        // 3.0 m difference at 343.42 m/s, 48 kHz = 419.3 samples (8.74 ms).
        assert!((stage.delays[0] - 419.3).abs() < 0.1, "{}", stage.delays[0]);
        assert_eq!(stage.delays[2], 0.0, "farthest speaker is the reference");
    }

    /// The point of the stage: impulses fed to speakers at different distances
    /// arrive at the reference point at the same time. delay + d/c is equal for
    /// every channel to within a thousandth of a sample, and unity gain is kept.
    #[test]
    fn static_mode_aligns_arrival_times_at_room_centre() {
        let layout = three_speaker_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let atmosphere = AtmosphericParams::default();
        let environment = test_environment(atmosphere);
        let ctx = make_context(&layout, &listener, &environment);
        let mut stage = DelayCompStage::static_calibration();
        stage.init(&ctx);

        let impulse_frame = 100;
        let mut buffer = impulse_buffer(3, 1024, impulse_frame);
        stage.process(&mut buffer, &ctx);

        let speed_of_sound = atmosphere.speed_of_sound() as f64;
        let mut arrival_times = Vec::new();
        for (channel, distance) in [(0usize, 1.0f64), (1, 2.5), (2, 4.0)] {
            let (centroid, weight) = impulse_centroid(&buffer, 3, channel);
            assert!((weight - 1.0).abs() < 1e-5, "ch{channel} gain {weight}");
            arrival_times.push(centroid + distance / speed_of_sound * SAMPLE_RATE as f64);
        }
        for arrival in &arrival_times {
            assert!(
                (arrival - arrival_times[0]).abs() < 1e-2,
                "arrivals differ: {arrival_times:?}"
            );
        }
        // And the reference (farthest) speaker is not delayed at all.
        assert_eq!(buffer[impulse_frame * 3 + 2], 1.0);
    }

    /// Listener-relative mode aligns arrivals at the *listener* and tracks the
    /// listener between buffers.
    #[test]
    fn listener_relative_mode_aligns_at_listener_and_follows_movement() {
        let layout = three_speaker_layout();
        let atmosphere = AtmosphericParams::default();
        let environment = test_environment(atmosphere);
        let speed_of_sound = atmosphere.speed_of_sound() as f64;
        let mut stage = DelayCompStage::listener_relative();
        let initial_listener = Listener::new(Vec3::ZERO, 0.0);
        stage.init(&make_context(&layout, &initial_listener, &environment));

        for listener_position in [Vec3::new(2.0, 1.0, 0.0), Vec3::new(-1.0, -2.0, 1.5)] {
            let listener = Listener::new(listener_position, 0.0);
            let ctx = make_context(&layout, &listener, &environment);
            stage.reset();
            let mut buffer = impulse_buffer(3, 2048, 100);
            stage.process(&mut buffer, &ctx);

            let positions = [
                Vec3::new(1.0, 0.0, 0.0),
                Vec3::new(0.0, 2.5, 0.0),
                Vec3::new(0.0, 0.0, -4.0),
            ];
            let mut arrivals = Vec::new();
            for (channel, position) in positions.iter().enumerate() {
                let distance = (*position - listener_position).length() as f64;
                let (centroid, _) = impulse_centroid(&buffer, 3, channel);
                arrivals.push(centroid + distance / speed_of_sound * SAMPLE_RATE as f64);
            }
            for arrival in &arrivals {
                assert!(
                    (arrival - arrivals[0]).abs() < 1e-2,
                    "listener {listener_position:?}: arrivals {arrivals:?}"
                );
            }
        }
    }

    /// The speed of sound comes from the atmosphere: at 0 °C (331.3 m/s) the
    /// delays are longer by 343.42/331.3 than at 20 °C.
    #[test]
    fn delays_scale_with_temperature_dependent_speed_of_sound() {
        let layout = three_speaker_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let warm = AtmosphericParams::default();
        let cold = AtmosphericParams {
            temperature_c: 0.0,
            ..AtmosphericParams::default()
        };
        let warm_environment = test_environment(warm);
        let cold_environment = test_environment(cold);
        let mut warm_stage = DelayCompStage::static_calibration();
        warm_stage.init(&make_context(&layout, &listener, &warm_environment));
        let mut cold_stage = DelayCompStage::static_calibration();
        cold_stage.init(&make_context(&layout, &listener, &cold_environment));

        let expected_ratio = 343.42 / 331.3;
        let ratio = cold_stage.delays[0] / warm_stage.delays[0];
        assert!((ratio - expected_ratio).abs() < 1e-4, "ratio {ratio}");
        // 3.0 m at exactly 331.3 m/s.
        assert!((cold_stage.delays[0] - 3.0 / 331.3 * SAMPLE_RATE).abs() < 1e-2);
    }

    /// An exactly integer delay moves the impulse by exactly that many frames
    /// with no smearing (interpolation fraction is 0).
    #[test]
    fn integer_delay_is_sample_exact() {
        let layout = three_speaker_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let atmosphere = AtmosphericParams::default();
        let environment = test_environment(atmosphere);
        let ctx = make_context(&layout, &listener, &environment);
        let mut stage = DelayCompStage::static_calibration();
        stage.init(&ctx);
        stage.delays[0] = 100.0;

        let mut buffer = impulse_buffer(3, 512, 50);
        stage.process(&mut buffer, &ctx);
        for frame in 0..512 {
            let expected = if frame == 150 { 1.0 } else { 0.0 };
            assert_eq!(buffer[frame * 3], expected, "frame {frame}");
        }
    }

    /// Half-sample delay splits the impulse equally between neighbouring frames.
    #[test]
    fn fractional_delay_interpolates_linearly() {
        let layout = three_speaker_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let atmosphere = AtmosphericParams::default();
        let environment = test_environment(atmosphere);
        let ctx = make_context(&layout, &listener, &environment);
        let mut stage = DelayCompStage::static_calibration();
        stage.init(&ctx);
        stage.delays[0] = 10.25;

        let mut buffer = impulse_buffer(3, 256, 20);
        stage.process(&mut buffer, &ctx);
        assert!((buffer[30 * 3] - 0.75).abs() < 1e-6, "{}", buffer[30 * 3]);
        assert!((buffer[31 * 3] - 0.25).abs() < 1e-6, "{}", buffer[31 * 3]);
    }

    /// Splitting the same signal into small buffers must give bit-identical
    /// output to one big buffer: the delay line is continuous across calls.
    #[test]
    fn delay_line_is_continuous_across_buffer_boundaries() {
        let layout = three_speaker_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let atmosphere = AtmosphericParams::default();
        let environment = test_environment(atmosphere);
        let ctx = make_context(&layout, &listener, &environment);

        let frames = 2048;
        let mut input = vec![0.0f32; 3 * frames];
        for (index, sample) in input.iter_mut().enumerate() {
            *sample = ((index as f32) * 0.173).sin();
        }

        let mut whole_stage = DelayCompStage::static_calibration();
        whole_stage.init(&ctx);
        let mut whole = input.clone();
        whole_stage.process(&mut whole, &ctx);

        let mut chunked_stage = DelayCompStage::static_calibration();
        chunked_stage.init(&ctx);
        let mut chunked = input.clone();
        for chunk in chunked.chunks_mut(3 * 97) {
            chunked_stage.process(chunk, &ctx);
        }
        assert_eq!(whole, chunked);
    }

    /// Pathological geometry (speaker 1 km away) clamps instead of indexing out
    /// of the ring buffer, and never produces NaN.
    #[test]
    fn extreme_delay_is_clamped_to_buffer_capacity() {
        let layout = SpeakerLayout::new(
            &[
                Speaker {
                    position: Vec3::new(0.5, 0.0, 0.0),
                    channel: 0,
                },
                Speaker {
                    position: Vec3::new(1000.0, 0.0, 0.0),
                    channel: 1,
                },
            ],
            None,
            2,
        );
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let atmosphere = AtmosphericParams::default();
        let environment = test_environment(atmosphere);
        let ctx = make_context(&layout, &listener, &environment);
        let mut stage = DelayCompStage::static_calibration();
        stage.init(&ctx);
        assert!(
            stage.delays[0] > stage.capacity as f32,
            "test must exceed capacity"
        );
        let mut buffer = impulse_buffer(2, 8192, 10);
        stage.process(&mut buffer, &ctx);
        assert!(buffer.iter().all(|sample| sample.is_finite()));
    }

    /// `reset` empties the ring buffers so stale audio is not replayed.
    #[test]
    fn reset_clears_delay_lines() {
        let layout = three_speaker_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let atmosphere = AtmosphericParams::default();
        let environment = test_environment(atmosphere);
        let ctx = make_context(&layout, &listener, &environment);
        let mut stage = DelayCompStage::static_calibration();
        stage.init(&ctx);
        let mut buffer = impulse_buffer(3, 64, 60);
        stage.process(&mut buffer, &ctx);
        stage.reset();
        let mut silence = vec![0.0f32; 3 * 1024];
        stage.process(&mut silence, &ctx);
        assert!(silence.iter().all(|&sample| sample == 0.0));
    }

    /// Delays below half a sample are bypassed (the input is returned as-is),
    /// so the farthest speaker is a true zero-latency reference.
    #[test]
    fn farthest_speaker_has_zero_latency() {
        let layout = three_speaker_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let atmosphere = AtmosphericParams::default();
        let environment = test_environment(atmosphere);
        let ctx = make_context(&layout, &listener, &environment);
        let mut stage = DelayCompStage::static_calibration();
        stage.init(&ctx);
        let mut buffer: Vec<f32> = (0..3 * 64).map(|index| index as f32).collect();
        let original = buffer.clone();
        stage.process(&mut buffer, &ctx);
        for frame in 0..64 {
            assert_eq!(buffer[frame * 3 + 2], original[frame * 3 + 2]);
        }
    }

    /// Re-initialising with a layout that no longer has a speaker on a channel
    /// must clear that channel's delay. `compute_delays` used to overwrite only
    /// the channels that currently have a speaker, so the old delay lingered.
    /// Latent: `init_pipelines` is only called once at start-up today.
    #[test]
    fn reinit_with_fewer_speakers_clears_stale_delays() {
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let atmosphere = AtmosphericParams::default();
        let environment = test_environment(atmosphere);
        let full_layout = three_speaker_layout();
        let mut stage = DelayCompStage::static_calibration();
        stage.init(&make_context(&full_layout, &listener, &environment));
        assert!(stage.delays[0] > 1.0);

        let reduced_layout = SpeakerLayout::new(
            &[
                Speaker {
                    position: Vec3::new(0.0, 2.5, 0.0),
                    channel: 1,
                },
                Speaker {
                    position: Vec3::new(0.0, 0.0, -4.0),
                    channel: 2,
                },
            ],
            None,
            3,
        );
        stage.init(&make_context(&reduced_layout, &listener, &environment));
        assert_eq!(stage.delays[0], 0.0, "channel 0 has no speaker any more");
    }
}
