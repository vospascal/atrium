use std::sync::Arc;

use crate::audio::decode::AudioBuffer;
use crate::world::types::Vec3;
use atrium_core::directivity::DirectivityPattern;
use atrium_core::source::SoundSource;

/// A looping buffer player that orbits a center point.
pub struct TestNode {
    buffer: Arc<AudioBuffer>,
    playback_pos: f64, // f64 for sub-sample precision over long durations
    pub amplitude: f32,
    pub orbit_radius: f32,
    pub orbit_speed: f32, // radians per second
    pub orbit_center: Vec3,
    orbit_angle: f32,
    pub pattern: DirectivityPattern,
    pub spread: f32,
    pub ref_dist: f32,
    muted: bool,
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
            orbit_radius,
            orbit_speed,
            orbit_center,
            orbit_angle: 0.0,
            pattern: DirectivityPattern::OMNI,
            spread: 0.0,
            ref_dist: 1.0,
            muted: false,
        }
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
        sample * self.amplitude
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

    /// Faces toward the orbit center (i.e. toward the listener).
    fn orientation(&self) -> Vec3 {
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

    fn set_orbit_speed(&mut self, speed: f32) {
        self.orbit_speed = speed;
    }

    fn set_orbit_radius(&mut self, radius: f32) {
        self.orbit_radius = radius;
    }

    fn set_orbit_angle(&mut self, angle: f32) {
        self.orbit_angle = angle;
    }

    fn ref_distance(&self) -> f32 {
        self.ref_dist
    }
}
