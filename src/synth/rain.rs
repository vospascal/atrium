// Procedural rain synthesis.
//
// Port of RainNoiseProcessor.ts (v3.0). Generates:
//   • Pink + brown noise bed (the continuous "hiss" of rainfall)
//   • Stochastic raindrop impacts — Gaussian-windowed broadband bursts
//     with physically-modeled drop size distribution (Medwin & Palmer '94)
//   • Bubble resonances (~3.3 kHz decaying sine) for large drops
//   • Sporadic gutter/roof drips
//
// The TS version included an inline Freeverb — we omit it because the
// scene's FdnReverb processor handles reverberation with proper room geometry.

use crate::world::types::Vec3;
use atrium_core::source::SoundSource;

use super::noise::{BrownNoise, OnePoleHP, OnePoleLP, PinkNoise, Rng};

const RING_SIZE: usize = 8192;
const RING_MASK: usize = RING_SIZE - 1;
const MAX_BURST: usize = 8192;

/// Procedural rain sound source.
///
/// Place it in the scene like any other `SoundSource` — the pipeline spatializes
/// it and adds reverb. For surround rain, place multiple
/// `RainSource`s at different positions around the room.
pub struct RainSource {
    // Noise generators
    pink: PinkNoise,
    brown: BrownNoise,

    // Drop impact ring buffer — bursts are summed into this circular buffer
    // and read out one sample at a time with envelope smoothing.
    ring: Box<[f32; RING_SIZE]>,
    ring_idx: usize,

    // One-pole smoother on ring output (prevents clicky bursts)
    env_state: f32,
    env_smooth: f32,

    // Tuning knobs (all pub for runtime tweaking)
    /// Rain intensity 0–1: scales drop rate, noise level, everything.
    pub intensity: f32,
    /// Base drop impacts per second at intensity=1.
    pub drop_rate: f32,
    /// Base roof/gutter drips per second at intensity=1.
    pub drip_rate: f32,
    /// Background pink hiss gain.
    pub hiss_gain: f32,
    /// Low-frequency brown noise gain.
    pub brown_gain: f32,
    /// Raindrop impact loudness.
    pub impact_gain: f32,
    /// Bubble resonance gain (set to 0 to disable).
    pub bubble_gain: f32,
    /// Output trim.
    pub master_gain: f32,

    // Pre-allocated scratch buffer for burst generation (no alloc in hot path)
    burst_scratch: Vec<f32>,

    /// World-space position of this rain source.
    pub position: Vec3,

    rng: Rng,
}

impl RainSource {
    /// Create a new rain source.
    ///
    /// - `position`: where in the room the rain originates (e.g. overhead skylight)
    /// - `intensity`: 0.0 (silence) to 1.0 (heavy downpour)
    /// - `seed`: PRNG seed — use different seeds for multiple rain sources
    pub fn new(position: Vec3, intensity: f32, seed: u64) -> Self {
        Self {
            pink: PinkNoise::new(seed.wrapping_add(1)),
            brown: BrownNoise::new(seed.wrapping_add(2)),
            ring: Box::new([0.0; RING_SIZE]),
            ring_idx: 0,
            env_state: 0.0,
            env_smooth: 0.7,
            intensity: intensity.clamp(0.0, 1.0),
            drop_rate: 150.0,
            drip_rate: 8.0,
            hiss_gain: 0.4,
            brown_gain: 0.3,
            impact_gain: 0.6,
            bubble_gain: 0.2,
            master_gain: 1.0,
            burst_scratch: vec![0.0; MAX_BURST],
            position,
            rng: Rng::new(seed),
        }
    }

    /// Generate a Gaussian-windowed filtered noise burst into the scratch buffer.
    /// Returns the number of samples written.
    fn generate_burst(&mut self, dur_sec: f32, cut_hz: f32, sample_rate: f32) -> usize {
        let len = ((dur_sec * sample_rate) as usize).clamp(8, MAX_BURST);
        let mid = len as f32 / 2.0;
        let sigma = len as f32 / 4.0; // wide envelope for smooth bursts

        // Fresh filters per burst (stack-allocated, cheap)
        let mut hp = OnePoleHP::new(200.0_f32.max(cut_hz * 0.6), sample_rate);
        let mut lp = OnePoleLP::new(3500.0, sample_rate);

        for i in 0..len {
            let window = (-0.5 * ((i as f32 - mid) / sigma).powi(2)).exp();
            let white = self.rng.next_bipolar();
            self.burst_scratch[i] = lp.process(hp.process(white)) * window;
        }

        len
    }

    /// Generate a decaying sine "bubble" resonance (~3.3 kHz, ~15 ms).
    fn generate_bubble(&mut self, sample_rate: f32) -> usize {
        let freq = 3300.0_f32;
        let len = ((0.015 * sample_rate) as usize).min(MAX_BURST);
        let tau = 0.003 * sample_rate; // decay time constant

        for i in 0..len {
            let env = (-(i as f32) / tau).exp();
            self.burst_scratch[i] =
                (std::f32::consts::TAU * freq * i as f32 / sample_rate).sin() * env;
        }

        len
    }

    /// Accumulate scratch buffer into the ring at an offset from current position.
    fn add_to_ring(&mut self, len: usize, gain: f32) {
        let base = self.ring_idx;
        for i in 0..len {
            let p = (base + i) & RING_MASK;
            self.ring[p] += self.burst_scratch[i] * gain;
        }
    }

    /// Pick a raindrop radius [mm] based on intensity.
    ///
    /// Distribution loosely follows Medwin & Palmer '94 (log-normal diameter PDF):
    ///   light rain → mostly small drops (0.95–1.2 mm)
    ///   heavy rain → larger drops up to 3.5 mm
    fn pick_drop_radius(&mut self) -> f32 {
        let r = self.rng.next_f32();
        if self.intensity < 0.3 {
            // Drizzle: mostly tiny
            if r < 0.8 {
                0.95 + self.rng.next_f32() * 0.25
            } else {
                1.4 + self.rng.next_f32() * 0.6
            }
        } else if self.intensity < 0.7 {
            // Moderate: mixed sizes
            if r < 0.5 {
                1.0 + self.rng.next_f32() * 0.4
            } else if r < 0.85 {
                1.6 + self.rng.next_f32() * 0.9
            } else {
                2.4 + self.rng.next_f32() * 1.2
            }
        } else {
            // Heavy: skewed toward large
            if r < 0.3 {
                1.2 + self.rng.next_f32() * 0.6
            } else {
                2.0 + self.rng.next_f32() * 1.5
            }
        }
    }
}

impl SoundSource for RainSource {
    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        let intensity = self.intensity;
        let drop_rate = self.drop_rate * intensity;
        let drip_rate = self.drip_rate * intensity;
        let impact_gain = self.impact_gain * intensity;
        let hiss_gain = self.hiss_gain * intensity;
        let brown_gain = self.brown_gain * intensity;

        // 1. Background noise bed
        let base = hiss_gain * self.pink.next_sample() + brown_gain * self.brown.next_sample();

        // 2. Read ring buffer with envelope smoothing
        let tail = self.ring[self.ring_idx];
        self.ring[self.ring_idx] = 0.0;
        self.env_state = self.env_state * self.env_smooth + tail * (1.0 - self.env_smooth);

        let sample = base + self.env_state;

        // 3. Stochastic raindrop impacts
        if self.rng.next_f32() < drop_rate / sample_rate {
            let radius = self.pick_drop_radius();
            let dur = 0.025 * radius; // larger drops → longer burst
            let hp_cut = 600.0 / radius.sqrt(); // larger drops → lower pitch
            let amp = impact_gain * (0.3 + self.rng.next_f32() * 0.7);

            let len = self.generate_burst(dur, hp_cut, sample_rate);
            self.add_to_ring(len, amp);

            // Bubble resonance for large drops hitting water
            if self.bubble_gain > 0.0 && radius > 1.2 && self.rng.next_f32() < 0.5 {
                let bub_len = self.generate_bubble(sample_rate);
                self.add_to_ring(bub_len, amp * self.bubble_gain);
            }
        }

        // 4. Sporadic gutter/roof drips (larger, less frequent)
        if self.rng.next_f32() < drip_rate / sample_rate {
            let radius = self.pick_drop_radius() + 0.4; // bias toward bigger drops
            let dur = 0.025 * radius;
            let hp_cut = 600.0 / radius.sqrt();
            let amp = impact_gain * 0.8;

            let len = self.generate_burst(dur, hp_cut, sample_rate);
            self.add_to_ring(len, amp);
        }

        // 5. Advance ring position
        self.ring_idx = (self.ring_idx + 1) & RING_MASK;

        sample * self.master_gain
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
    fn rain_produces_nonzero_output() {
        let mut rain = RainSource::new(Vec3::ZERO, 0.5, 42);
        let sum: f32 = (0..48000).map(|_| rain.next_sample(48000.0).abs()).sum();
        assert!(sum > 0.0, "rain source is silent");
    }

    #[test]
    fn rain_silent_at_zero_intensity() {
        let mut rain = RainSource::new(Vec3::ZERO, 0.0, 42);
        let sum: f32 = (0..48000).map(|_| rain.next_sample(48000.0).abs()).sum();
        assert_eq!(sum, 0.0, "rain should be silent at zero intensity");
    }

    #[test]
    fn rain_louder_at_higher_intensity() {
        let mut light = RainSource::new(Vec3::ZERO, 0.2, 42);
        let mut heavy = RainSource::new(Vec3::ZERO, 0.9, 42);

        let energy_light: f32 = (0..48000).map(|_| light.next_sample(48000.0).powi(2)).sum();
        let energy_heavy: f32 = (0..48000).map(|_| heavy.next_sample(48000.0).powi(2)).sum();

        assert!(
            energy_heavy > energy_light,
            "heavy rain ({energy_heavy}) should be louder than light ({energy_light})"
        );
    }
}
