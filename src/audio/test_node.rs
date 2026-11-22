use std::sync::Arc;

use crate::audio::decode::AudioBuffer;
use crate::audio::signal_meter::SignalMeter;
use atrium_core::directivity::DirectivityPattern;
use atrium_core::source::{EmitterKind, SoundSource};
use atrium_core::telemetry::SourceSignal;
use atrium_core::types::Vec3;

/// A looping buffer player that orbits a center point.
pub struct TestNode {
    buffer: Arc<AudioBuffer>,
    playback_pos: f64, // f64 for sub-sample precision over long durations
    pub amplitude: f32,
    /// Base amplitude at 0 dB SPL: `amplitude = unit_amplitude · 10^(spl/20)`.
    /// Set by the scene builder so a live SPL edit can rescale amplitude without
    /// re-decoding. 0.0 = unset (SPL edits then have no effect).
    pub unit_amplitude: f32,
    pub orbit_radius: f32,
    pub orbit_speed: f32, // radians per second
    pub orbit_center: Vec3,
    orbit_angle: f32,
    pub pattern: DirectivityPattern,
    pub spread: f32,
    /// The arc it arrives from when ambient, in degrees (360 all round).
    arc_degrees: f32,
    pub ref_dist: f32,
    /// The heading it was turned to (radians). Until it is turned, it faces
    /// its orbit centre.
    facing: Option<f32>,
    muted: bool,
    emitter_kind: EmitterKind,
    signal_meter: Option<SignalMeter>,
}

impl TestNode {
    pub fn new(
        buffer: Arc<AudioBuffer>,
        orbit_center: Vec3,
        orbit_radius: f32,
        orbit_speed: f32,
    ) -> Self {
        Self {
            buffer,
            playback_pos: 0.0,
            amplitude: 0.5,
            unit_amplitude: 0.0,
            orbit_radius,
            orbit_speed,
            orbit_center,
            orbit_angle: 0.0,
            pattern: DirectivityPattern::OMNI,
            spread: 0.0,
            arc_degrees: atrium_core::source::ALL_ROUND_DEGREES,
            ref_dist: 1.0,
            facing: None,
            muted: false,
            emitter_kind: EmitterKind::Object,
            signal_meter: None,
        }
    }

    pub(crate) fn meter_signal(&mut self) {
        self.signal_meter = Some(SignalMeter::new(self.buffer.rms * self.amplitude));
    }
}

impl SoundSource for TestNode {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        let samples = &self.buffer.samples;
        if samples.is_empty() {
            return 0.0;
        }

        // Linear interpolation between samples for smooth playback
        let pos = self.playback_pos;
        let idx = pos as usize;
        let frac = (pos - idx as f64) as f32;

        let s0 = samples[idx % samples.len()];
        let s1 = samples[(idx + 1) % samples.len()];
        let sample = s0 + (s1 - s0) * frac;

        // Advance playback position (use buffer's native sample rate)
        self.playback_pos += self.buffer.sample_rate as f64 / sample_rate as f64;
        if self.playback_pos >= samples.len() as f64 {
            self.playback_pos -= samples.len() as f64;
        }

        if self.muted {
            // Still advance playback so it stays in sync when unmuted
            return 0.0;
        }
        let output = sample * self.amplitude;
        if let Some(meter) = &mut self.signal_meter {
            meter.push(output, sample_rate);
        }
        output
    }

    fn signal(&self) -> Option<SourceSignal> {
        self.signal_meter.as_ref().map(SignalMeter::signal)
    }

    fn emitter_kind(&self) -> EmitterKind {
        self.emitter_kind
    }

    fn set_emitter_kind(&mut self, kind: EmitterKind) {
        self.emitter_kind = kind;
    }

    fn position(&self) -> Vec3 {
        Vec3::new(
            self.orbit_center.x + self.orbit_radius * self.orbit_angle.cos(),
            self.orbit_center.y + self.orbit_radius * self.orbit_angle.sin(),
            self.orbit_center.z,
        )
    }

    fn tick(&mut self, dt: f32) {
        self.orbit_angle += self.orbit_speed * dt;
        if self.orbit_angle > std::f32::consts::TAU {
            self.orbit_angle -= std::f32::consts::TAU;
        }
    }

    /// The heading it was turned to. A source that was never turned faces
    /// its orbit centre, so one that circles the listener points at them.
    fn orientation(&self) -> Vec3 {
        if let Some(yaw) = self.facing {
            return Vec3::new(yaw.cos(), yaw.sin(), 0.0);
        }
        let pos = self.position();
        let d = self.orbit_center - pos;
        let len = (d.x * d.x + d.y * d.y + d.z * d.z).sqrt();
        if len < 1e-6 {
            Vec3::new(1.0, 0.0, 0.0)
        } else {
            Vec3::new(d.x / len, d.y / len, d.z / len)
        }
    }

    fn directivity(&self) -> DirectivityPattern {
        self.pattern
    }

    fn is_muted(&self) -> bool {
        self.muted
    }

    fn set_muted(&mut self, muted: bool) {
        if muted && !self.muted {
            if let Some(meter) = &mut self.signal_meter {
                meter.reset();
            }
        }
        self.muted = muted;
    }

    fn set_position(&mut self, position: Vec3) {
        self.orbit_center = position;
    }

    fn spread(&self) -> f32 {
        self.spread
    }

    fn set_spread(&mut self, spread: f32) {
        self.spread = spread;
    }

    fn arc_degrees(&self) -> f32 {
        self.arc_degrees
    }

    fn set_arc_degrees(&mut self, degrees: f32) {
        self.arc_degrees = degrees;
    }

    fn set_reference_spl(&mut self, spl: f32) -> f32 {
        if self.unit_amplitude > 0.0 {
            self.amplitude = self.unit_amplitude * 10.0_f32.powf(spl / 20.0);
        }
        self.amplitude
    }

    fn set_directivity(&mut self, pattern: DirectivityPattern) {
        self.pattern = pattern;
    }

    fn set_facing(&mut self, yaw: f32) {
        self.facing = Some(yaw);
    }

    fn set_orbit_speed(&mut self, speed: f32) {
        self.orbit_speed = speed;
    }

    fn set_orbit_radius(&mut self, radius: f32) {
        self.orbit_radius = radius;
    }

    fn set_orbit_angle(&mut self, angle: f32) {
        self.orbit_angle = angle;
    }

    fn orbit_center(&self) -> Vec3 {
        self.orbit_center
    }

    fn orbit_radius(&self) -> f32 {
        self.orbit_radius
    }

    fn ref_distance(&self) -> f32 {
        self.ref_dist
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::spectral_profile::SpectralProfile;
    use std::f32::consts::TAU;

    fn buffer(samples: Vec<f32>, sample_rate: u32) -> Arc<AudioBuffer> {
        Arc::new(AudioBuffer {
            samples,
            sample_rate,
            rms: 0.0,
            spectral_profile: SpectralProfile::default(),
        })
    }

    fn ramp_node(sample_rate: u32) -> TestNode {
        let samples = (0..8).map(|index| index as f32).collect();
        let mut node = TestNode::new(buffer(samples, sample_rate), Vec3::ZERO, 2.0, 1.0);
        node.amplitude = 1.0;
        node
    }

    fn render(node: &mut TestNode, count: usize, sample_rate: f32) -> Vec<f32> {
        (0..count).map(|_| node.next_sample(sample_rate)).collect()
    }

    /// Same rate: plays the buffer verbatim and loops seamlessly.
    #[test]
    fn plays_the_buffer_at_native_rate_and_loops() {
        let mut node = ramp_node(48_000);
        assert_eq!(
            render(&mut node, 18, 48_000.0),
            vec![
                0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0, 0.0, 1.0, 2.0, 3.0, 4.0, 5.0, 6.0, 7.0,
                0.0, 1.0
            ]
        );
    }

    /// Half-rate buffer at 48 kHz advances 0.5 per sample with linear
    /// interpolation: the in-between samples are exact midpoints; the last
    /// sample interpolates toward the first (loop point).
    #[test]
    fn upsampling_interpolates_linearly_between_neighbours() {
        let mut node = ramp_node(24_000);
        let output = render(&mut node, 16, 48_000.0);
        let expected = [
            0.0, 0.5, 1.0, 1.5, 2.0, 2.5, 3.0, 3.5, 4.0, 4.5, 5.0, 5.5, 6.0, 6.5, 7.0,
        ];
        assert_eq!(&output[..15], &expected);
        // Between 7 and the wrapped 0: (7 + 0) / 2.
        assert_eq!(output[15], 3.5);
    }

    /// A double-rate buffer is read at 2 samples per output sample.
    #[test]
    fn downsampling_skips_samples() {
        let mut node = ramp_node(96_000);
        assert_eq!(
            render(&mut node, 6, 48_000.0),
            vec![0.0, 2.0, 4.0, 6.0, 0.0, 2.0]
        );
    }

    /// Reading position tracks buffer_rate / host_rate exactly over a long run
    /// (f64 accumulation, one wrap subtraction per overflow).
    #[test]
    fn playback_position_is_precise_over_a_long_run() {
        let length = 1_000;
        let samples: Vec<f32> = (0..length).map(|index| index as f32).collect();
        let mut node = TestNode::new(buffer(samples, 44_100), Vec3::ZERO, 1.0, 1.0);
        let steps = 5_000_000usize;
        for _ in 0..steps {
            node.next_sample(48_000.0);
        }
        let expected = (steps as f64 * 44_100.0 / 48_000.0) % length as f64;
        assert!(
            (node.playback_pos - expected).abs() < 1e-4,
            "{} vs {expected}",
            node.playback_pos
        );
    }

    #[test]
    fn empty_buffer_is_silent() {
        let mut node = TestNode::new(buffer(Vec::new(), 48_000), Vec3::ZERO, 1.0, 1.0);
        assert_eq!(render(&mut node, 10, 48_000.0), vec![0.0; 10]);
    }

    /// Muted nodes return silence but keep their playhead moving, so unmuting
    /// resumes in sync.
    #[test]
    fn muting_keeps_the_playhead_running() {
        let mut node = ramp_node(48_000);
        node.set_muted(true);
        assert!(node.is_muted());
        assert_eq!(render(&mut node, 3, 48_000.0), vec![0.0; 3]);
        node.set_muted(false);
        assert_eq!(render(&mut node, 3, 48_000.0), vec![3.0, 4.0, 5.0]);
    }

    #[test]
    fn amplitude_scales_the_output() {
        let mut node = ramp_node(48_000);
        node.amplitude = 0.25;
        assert_eq!(render(&mut node, 4, 48_000.0), vec![0.0, 0.25, 0.5, 0.75]);
    }

    /// amplitude = unit_amplitude x 10^(spl / 20).
    #[test]
    fn reference_spl_follows_the_decibel_law() {
        let mut node = ramp_node(48_000);
        node.unit_amplitude = 0.001;
        let at_40 = node.set_reference_spl(40.0);
        let at_60 = node.set_reference_spl(60.0);
        assert!((at_40 - 0.1).abs() < 1e-6);
        assert!((at_60 / at_40 - 10.0).abs() < 1e-4);
        let mut uncalibrated = ramp_node(48_000);
        uncalibrated.amplitude = 0.3;
        assert_eq!(uncalibrated.set_reference_spl(90.0), 0.3);
    }

    /// The node sits on a circle of `orbit_radius` around the centre at the
    /// centre's height; angle 0 is +x and a quarter turn is +y.
    #[test]
    fn orbit_position_is_on_the_circle() {
        let centre = Vec3::new(1.0, 2.0, 3.0);
        let mut node = TestNode::new(buffer(vec![0.0], 48_000), centre, 2.5, 0.0);
        assert!(node.position().distance_to(Vec3::new(3.5, 2.0, 3.0)) < 1e-6);
        for angle in [0.3_f32, 1.2, 2.9, 4.4, 6.0] {
            node.set_orbit_angle(angle);
            let position = node.position();
            assert!((position.distance_to(centre) - 2.5).abs() < 1e-5);
            assert_eq!(position.z, 3.0);
            assert!((position.x - (1.0 + 2.5 * angle.cos())).abs() < 1e-5);
            assert!((position.y - (2.0 + 2.5 * angle.sin())).abs() < 1e-5);
        }
    }

    /// angle += speed x dt, wrapped into [0, 2 pi] for positive speeds.
    #[test]
    fn tick_advances_the_orbit_angle_and_wraps() {
        let mut node = TestNode::new(buffer(vec![0.0], 48_000), Vec3::ZERO, 1.0, 2.0);
        node.tick(0.5);
        assert!((node.orbit_angle - 1.0).abs() < 1e-6);
        for _ in 0..1_000 {
            node.tick(0.01);
            assert!((0.0..=TAU + 0.02).contains(&node.orbit_angle));
        }
        // 1 + 1000 x 0.02 = 21 rad = 3.34 turns + 0.2 rad residue.
        let expected = (1.0 + 1_000.0 * 0.02) % TAU;
        assert!(
            (node.orbit_angle - expected).abs() < 1e-3,
            "{} vs {expected}",
            node.orbit_angle
        );
    }

    /// Faces the orbit centre with a unit vector; defaults to +x when the
    /// radius is zero (no direction to face).
    #[test]
    fn orientation_points_at_the_orbit_centre() {
        let mut node = TestNode::new(
            buffer(vec![0.0], 48_000),
            Vec3::new(1.0, 1.0, 0.0),
            3.0,
            0.0,
        );
        for angle in [0.0_f32, 1.0, 2.5, 4.0] {
            node.set_orbit_angle(angle);
            let facing = node.orientation();
            assert!((facing.length() - 1.0).abs() < 1e-5);
            let towards_centre = (node.orbit_center() - node.position()) * (1.0 / 3.0);
            assert!(facing.distance_to(towards_centre) < 1e-5);
        }
        node.set_orbit_radius(0.0);
        assert_eq!(node.orientation(), Vec3::new(1.0, 0.0, 0.0));
    }

    /// A sound that was turned keeps that heading wherever it is: north is
    /// +Y (yaw π/2), west is −X (yaw π), and the vector stays flat and unit
    /// length. Being turned wins over facing the orbit centre.
    #[test]
    fn a_turned_source_faces_where_it_was_turned() {
        let mut node = TestNode::new(
            buffer(vec![0.0], 48_000),
            Vec3::new(1.0, 1.0, 0.0),
            3.0,
            0.0,
        );
        node.set_facing(std::f32::consts::FRAC_PI_2);
        assert!(node.orientation().distance_to(Vec3::new(0.0, 1.0, 0.0)) < 1e-6);
        node.set_orbit_angle(2.0);
        assert!(node.orientation().distance_to(Vec3::new(0.0, 1.0, 0.0)) < 1e-6);
        node.set_facing(std::f32::consts::PI);
        assert!(node.orientation().distance_to(Vec3::new(-1.0, 0.0, 0.0)) < 1e-6);
    }

    #[test]
    fn orbit_setters_and_scene_state_round_trip() {
        let mut node = ramp_node(48_000);
        node.set_orbit_speed(1.5);
        node.set_orbit_radius(4.0);
        node.set_position(Vec3::new(9.0, 8.0, 7.0));
        node.set_spread(0.6);
        node.set_arc_degrees(120.0);
        assert_eq!(node.arc_degrees(), 120.0);
        assert_eq!(node.orbit_radius(), 4.0);
        assert_eq!(node.orbit_center(), Vec3::new(9.0, 8.0, 7.0));
        assert_eq!(node.spread(), 0.6);
        assert_eq!(node.orbit_speed, 1.5);
        assert_eq!(node.ref_distance(), 1.0);
        node.set_directivity(DirectivityPattern::cardioid());
        assert!(matches!(node.directivity(), DirectivityPattern::Polar { alpha } if alpha == 0.5));
    }
}
