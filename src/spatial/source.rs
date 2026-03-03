use std::sync::Arc;

use crate::audio::decode::AudioBuffer;
use crate::world::types::Vec3;

/// Trait for anything that generates audio samples and has a position.
/// Implementations must be Send (moved to audio thread) and must not allocate.
pub trait SoundSource: Send {
    /// Generate the next mono sample.
    fn next_sample(&mut self, sample_rate: f32) -> f32;

    /// Current world-space position.
    fn position(&self) -> Vec3;

    /// Whether this source is still producing audio.
    fn is_active(&self) -> bool {
        true
    }

    /// Advance time-varying state (orbit, etc.). Called once per buffer.
    fn tick(&mut self, dt: f32);
}

/// A looping buffer player that orbits a center point.
pub struct TestNode {
    buffer: Arc<AudioBuffer>,
    playback_pos: f64, // f64 for sub-sample precision over long durations
    pub amplitude: f32,
    pub orbit_radius: f32,
    pub orbit_speed: f32, // radians per second
    pub orbit_center: Vec3,
    orbit_angle: f32,
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
        }
    }
}

impl SoundSource for TestNode {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
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
        self.playback_pos += self.buffer.sample_rate as f64 / _sample_rate as f64;
        if self.playback_pos >= samples.len() as f64 {
            self.playback_pos -= samples.len() as f64;
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
}
