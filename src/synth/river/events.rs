// Short events written ahead into the river's ring: bubble resonances,
// obstacle splashes and the pops of splitting bubbles.

use std::f32::consts::TAU;

use super::{RiverSource, RING_MASK, RING_SIZE};
use crate::synth::noise::white_noise_density_scale;

impl RiverSource {
    /// Add a short, low-Q bubble resonance. Bubble radius maps to frequency
    /// through the Minnaert relationship; the procedurally sampled 1.2-7 mm
    /// population yields approximately 450-2700 Hz airborne components.
    pub(super) fn write_bubble(&mut self, freq: f32, decay: f32, gain: f32, sample_rate: f32) {
        let duration = (5.5 / decay).clamp(0.006, 0.050);
        let len = ((duration * sample_rate) as usize).clamp(16, RING_SIZE / 2);
        let attack_samples = (0.00045 * sample_rate).max(1.0);
        let phase = self.event_rng.next_f32() * TAU;
        let chirp = freq * (0.03 + 0.09 * self.event_rng.next_f32()) / duration;

        for i in 0..len {
            let t = i as f32 / sample_rate;
            let attack = (i as f32 / attack_samples).min(1.0);
            let envelope = attack * (-decay * t).exp();
            let oscillator = (phase + TAU * (freq * t + 0.5 * chirp * t * t)).sin();
            let index = (self.ring_idx + 1 + i) & RING_MASK;
            self.ring[index] += oscillator * envelope * gain;
        }
    }

    /// Add a short broadband collision with an obstacle or shallow stone.
    pub(super) fn write_splash(&mut self, duration: f32, cutoff: f32, gain: f32, sample_rate: f32) {
        let len = ((duration * sample_rate) as usize).clamp(16, RING_SIZE / 2);
        let attack_samples = (0.0012 * sample_rate).max(1.0);
        let dt = 1.0 / sample_rate;
        let lp_alpha = dt / (1.0 / (TAU * cutoff) + dt);
        let hp_alpha = (1.0 / (TAU * 220.0)) / (1.0 / (TAU * 220.0) + dt);
        let decay = 5.0 / duration.max(0.005);
        let white_noise_scale = white_noise_density_scale(sample_rate);
        let mut lp = 0.0_f32;
        let mut hp = 0.0_f32;
        let mut previous = 0.0_f32;

        for i in 0..len {
            let t = i as f32 * dt;
            let attack = (i as f32 / attack_samples).min(1.0);
            let envelope = attack * (-decay * t).exp();
            lp += lp_alpha * (self.event_rng.next_bipolar() * white_noise_scale - lp);
            hp = hp_alpha * (hp + lp - previous);
            previous = lp;
            let index = (self.ring_idx + 1 + i) & RING_MASK;
            self.ring[index] += hp * envelope * gain;
        }
    }

    /// Very short bubble-split/pop transient. These rare broadband ticks give
    /// the brilliance band the heavy-tailed envelope seen in real creek audio
    /// without turning the continuous current into hiss.
    pub(super) fn write_pop(&mut self, gain: f32, sample_rate: f32) {
        let duration = 0.0012 + 0.0028 * self.event_rng.next_f32();
        let len = ((duration * sample_rate) as usize).clamp(12, RING_SIZE / 4);
        let dt = 1.0 / sample_rate;
        let lp_alpha = dt / (1.0 / (TAU * 9_000.0) + dt);
        let hp_alpha = (1.0 / (TAU * 2_000.0)) / (1.0 / (TAU * 2_000.0) + dt);
        let white_noise_scale = white_noise_density_scale(sample_rate);
        let mut lp = 0.0_f32;
        let mut hp = 0.0_f32;
        let mut previous = 0.0_f32;

        for i in 0..len {
            let t = i as f32 * dt;
            let envelope = (-6.0 * t / duration).exp();
            lp += lp_alpha * (self.event_rng.next_bipolar() * white_noise_scale - lp);
            hp = hp_alpha * (hp + lp - previous);
            previous = lp;
            let index = (self.ring_idx + 1 + i) & RING_MASK;
            self.ring[index] += hp * envelope * gain;
        }
    }

    pub(super) fn spawn_bubble(&mut self, flow_norm: f32, sample_rate: f32) {
        // A broad bubble-radius population: large bubbles provide the 450-900
        // Hz body, smaller bubbles fill the 0.9-2.7 kHz babble band.
        let region = self.event_rng.next_f32();
        let freq = if region < 0.45 {
            450.0 + 450.0 * self.event_rng.next_f32()
        } else if region < 0.85 {
            900.0 + 800.0 * self.event_rng.next_f32()
        } else {
            1_700.0 + 1_000.0 * self.event_rng.next_f32()
        } * (0.90 + 0.16 * flow_norm.min(1.5));
        let decay = 170.0 + 300.0 * self.event_rng.next_f32() + 0.08 * freq;
        let amplitude = self.event_rng.next_f32();
        let gain =
            self.bubble_activity * (0.018 + 0.180 * amplitude.powi(5)) * (0.75 + 0.25 * flow_norm);
        self.write_bubble(freq, decay, gain, sample_rate);

        // A quiet coupled mode adds the fuller low-frequency cloud response
        // observed when bubbles are not acoustically isolated.
        if self.event_rng.next_f32() < 0.35 {
            let coupled = freq * (0.48 + 0.18 * self.event_rng.next_f32());
            self.write_bubble(coupled, decay * 0.80, gain * 0.24, sample_rate);
        }
        if self.event_rng.next_f32() < 0.16 {
            let pop_gain = self.spray_gain * (0.18 + 0.65 * self.event_rng.next_f32());
            self.write_pop(pop_gain, sample_rate);
        }
    }
}
