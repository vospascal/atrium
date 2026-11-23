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

use atrium_core::commands::SynthParam;
use atrium_core::layers::LayerMix;
use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;

use std::f32::consts::TAU;

use super::noise::{white_noise_density_scale, BrownNoise, OnePoleHP, OnePoleLP, PinkNoise, Rng};

const RING_SIZE: usize = 8192;
const RING_MASK: usize = RING_SIZE - 1;
const MAX_BURST: usize = 8192;

/// `env_state` below this magnitude is flushed to exact zero. The 0.7-per-sample
/// decay rounds 0.7 x the smallest subnormal back up to itself, so without a
/// flush the smoother would idle on a subnormal forever after the rain stops.
const ENVELOPE_FLUSH_THRESHOLD: f32 = 1.0e-20;

/// Corner of the one-pole smoother on the ring readout: -ln(0.7) · 48 kHz / 2π,
/// the 0.7-per-sample coefficient it was tuned with at 48 kHz.
const RING_SMOOTHING_CORNER_HZ: f32 = 2_724.796;

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

    // One-pole smoother on ring output (prevents clicky bursts). The
    // per-sample coefficient is derived from RING_SMOOTHING_CORNER_HZ.
    env_state: f32,
    env_smooth: f32,
    sample_rate_cached: f32,

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
            env_smooth: 0.0,
            sample_rate_cached: 0.0,
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

        // Fresh filters per burst (stack-allocated, cheap).
        // LP lowered 3500→1800 Hz: the drop "ticks" read as "too high in tone"
        // (audit + user). Warmer taps; the pipeline darkens further downstream.
        let mut hp = OnePoleHP::new(200.0_f32.max(cut_hz * 0.6), sample_rate);
        let mut lp = OnePoleLP::new(1800.0, sample_rate);
        let white_noise_scale = white_noise_density_scale(sample_rate);

        for i in 0..len {
            let window = (-0.5 * ((i as f32 - mid) / sigma).powi(2)).exp();
            let white = self.rng.next_bipolar() * white_noise_scale;
            self.burst_scratch[i] = lp.process(hp.process(white)) * window;
        }

        len
    }

    /// Generate a decaying sine "bubble" resonance (~1.6 kHz, ~15 ms).
    /// Lowered from 3300 Hz: a fixed high ping is the textbook "metallic"
    /// artifact (Farnell) and the main "too high in tone" culprit in v1.
    fn generate_bubble(&mut self, sample_rate: f32) -> usize {
        let freq = 1600.0_f32;
        let len = ((0.015 * sample_rate) as usize).min(MAX_BURST);
        let tau = 0.003 * sample_rate; // decay time constant

        for i in 0..len {
            let env = (-(i as f32) / tau).exp();
            self.burst_scratch[i] =
                (std::f32::consts::TAU * freq * i as f32 / sample_rate).sin() * env;
        }

        len
    }

    /// Accumulate scratch buffer into the ring, starting one slot ahead of the
    /// read position: `ring_idx` has already been read this sample, so a
    /// burst written there would wait a whole ring revolution to be heard.
    fn add_to_ring(&mut self, len: usize, gain: f32) {
        let base = self.ring_idx + 1;
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
    /// What changes while it plays; any other setting is not one it has.
    fn set_synth_param(&mut self, param: SynthParam, value: f32) {
        // `f32::clamp` passes NaN through, and one NaN would turn every later
        // sample into NaN: keep the previous value instead.
        if !value.is_finite() {
            return;
        }
        match param {
            SynthParam::Intensity => self.intensity = value.clamp(0.0, 1.0),
            SynthParam::DropRate => self.drop_rate = value.max(0.0),
            SynthParam::DripRate => self.drip_rate = value.max(0.0),
            SynthParam::MasterGain => self.master_gain = value.clamp(0.0, 2.0),
            _ => {}
        }
    }

    fn next_sample(&mut self, sample_rate: f32) -> f32 {
        self.next_mixed(sample_rate, &mut LayerMix::untouched())
    }

    /// Three layers, in this order: hiss, rumble and drops (impacts, bubbles and drips share one buffer).
    fn layer_count(&self) -> usize {
        3
    }

    #[inline]
    fn next_mixed(&mut self, sample_rate: f32, mix: &mut LayerMix) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            self.env_smooth = (-TAU * RING_SMOOTHING_CORNER_HZ / sample_rate).exp();
        }
        let intensity = self.intensity;
        let drop_rate = self.drop_rate * intensity;
        let drip_rate = self.drip_rate * intensity;
        let impact_gain = self.impact_gain * intensity;
        let hiss_gain = self.hiss_gain * intensity;
        let brown_gain = self.brown_gain * intensity;

        // 1. Background noise bed
        let hiss = hiss_gain * self.pink.next_sample(sample_rate);
        let rumble = brown_gain * self.brown.next_sample(sample_rate);

        // 2. Read ring buffer with envelope smoothing
        let tail = self.ring[self.ring_idx];
        self.ring[self.ring_idx] = 0.0;
        self.env_state = self.env_state * self.env_smooth + tail * (1.0 - self.env_smooth);
        if self.env_state.abs() < ENVELOPE_FLUSH_THRESHOLD {
            self.env_state = 0.0;
        }

        let sample = mix.sum(0, &[hiss, rumble, self.env_state]);

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
mod tests;
