// Procedural ocean wave synthesis.
//
// Port of WaveNoiseProcessor.ts. Algorithm:
//   • Brown (integrated white) noise  → deep low-frequency roar
//   • Pink (1/f) noise               → broad-band hiss / wash
//   • Sine-LFO swell envelope        → slow rhythmic surging
//   • Stochastic crash component     → Gaussian-gated white noise when
//                                       breaker events fire

use std::f32::consts::TAU;

use crate::spatial::source::SoundSource;
use crate::world::types::Vec3;

use super::noise::{BrownNoise, PinkNoise, Rng};

/// Procedural ocean wave sound source.
///
/// The swell follows a slow sine LFO whose period matches real ocean wave
/// timings (3–10 s typical). Stochastic "crash" events layer white-noise
/// bursts with an early-peaked Gaussian window for a breaking-wave feel.
pub struct WaveSource {
    pink: PinkNoise,
    brown: BrownNoise,

    /// Running sample counter for swell LFO.
    t: u64,

    // Crash burst (computed inline — no allocation)
    crash_pos: usize,
    crash_len: usize,

    // Tuning knobs
    /// Seconds between swell peaks (3–10 typical for ocean).
    pub period: f32,
    /// Per-second probability of a breaker crash event.
    pub crash_prob: f32,
    /// Brown noise (deep roar) gain.
    pub roar_level: f32,
    /// Pink noise (wash / hiss) gain.
    pub hiss_level: f32,
    /// Crash burst gain.
    pub crash_gain: f32,
    /// Output trim.
    pub master_gain: f32,

    /// World-space position.
    pub position: Vec3,

    rng: Rng,
}

impl WaveSource {
    /// Create a new wave source.
    ///
    /// - `position`: where the waves originate (e.g. a far wall)
    /// - `period`: seconds between swell peaks (6 is a good default)
    /// - `crash_prob`: breaker probability per second (0.25 = occasional crashing)
    /// - `seed`: PRNG seed
    pub fn new(position: Vec3, period: f32, crash_prob: f32, seed: u64) -> Self {
        Self {
            pink: PinkNoise::new(seed.wrapping_add(1)),
            brown: BrownNoise::new(seed.wrapping_add(2)),
            t: 0,
            crash_pos: 0,
            crash_len: 0,
            period: period.max(1.0),
            crash_prob: crash_prob.clamp(0.0, 1.0),
            roar_level: 0.8,
            hiss_level: 0.3,
            crash_gain: 0.6,
            master_gain: 1.0,
            position,
            rng: Rng::new(seed),
        }
    }

    /// Inline Gaussian window for crash burst.
    /// Peak at 1/4 of duration for a "crashing" feel (asymmetric — fast attack, slow decay).
    fn crash_envelope(&self) -> f32 {
        if self.crash_len == 0 {
            return 0.0;
        }
        let mid = self.crash_len as f32 / 4.0; // peak earlier
        let sigma = self.crash_len as f32 / 6.0;
        let x = (self.crash_pos as f32 - mid) / sigma;
        (-0.5 * x * x).exp()
    }
}

impl SoundSource for WaveSource {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        let omega = TAU / (self.period * sample_rate);

        // 1. Swell envelope: slow sine oscillation mapped to [0, 1]
        let swell = 0.5 * (1.0 + (self.t as f32 * omega).sin());

        // 2. Trigger crash burst stochastically
        if self.crash_len == 0 && self.rng.next_f32() < self.crash_prob / sample_rate {
            self.crash_len = (0.4 * sample_rate) as usize; // 400 ms burst
            self.crash_pos = 0;
        }

        let mut crash = 0.0;
        if self.crash_len > 0 {
            let env = self.crash_envelope();
            crash = self.rng.next_bipolar() * env;
            self.crash_pos += 1;
            if self.crash_pos >= self.crash_len {
                self.crash_len = 0;
            }
        }

        // 3. Noise sources
        let roar = self.brown.next();
        let hiss = self.pink.next();

        // 4. Mix: noise bed modulated by swell + crash layer
        let sample = (self.roar_level * roar * swell
            + self.hiss_level * hiss * swell
            + self.crash_gain * crash)
            * self.master_gain;

        self.t = self.t.wrapping_add(1);
        sample
    }

    fn position(&self) -> Vec3 {
        self.position
    }

    fn tick(&mut self, _dt: f32) {}
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wave_produces_nonzero_output() {
        let mut wave = WaveSource::new(Vec3::ZERO, 6.0, 0.25, 42);
        let sum: f32 = (0..48000).map(|_| wave.next_sample(48000.0).abs()).sum();
        assert!(sum > 0.0, "wave source is silent");
    }

    #[test]
    fn wave_has_cyclic_energy() {
        let mut wave = WaveSource::new(Vec3::ZERO, 1.0, 0.0, 42);

        // With period=1s and crash_prob=0, energy should oscillate.
        // Sample first half-period and second half-period.
        let first_half: f32 = (0..24000)
            .map(|_| wave.next_sample(48000.0).powi(2))
            .sum();
        let second_half: f32 = (0..24000)
            .map(|_| wave.next_sample(48000.0).powi(2))
            .sum();

        // The two halves should have noticeably different energy
        // (one includes the swell peak, the other the trough)
        let ratio = first_half.max(second_half) / first_half.min(second_half);
        assert!(
            ratio > 1.2,
            "expected cyclic energy variation, got ratio {ratio}"
        );
    }
}
