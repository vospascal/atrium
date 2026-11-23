//! White noise: the same power in every hertz.
//!
//! Uniform white noise from the xorshift generator, scaled so its density
//! per hertz is the same at every host rate. At 48 kHz its RMS is exactly
//! 0.1: a uniform draw on [−1, 1) has an RMS of 1/√3.

use crate::synth::noise::{white_noise_density_scale, Rng};

/// Gain that brings a uniform draw's RMS of 1/√3 to 0.1 at 48 kHz.
const LEVEL: f32 = 0.1 * 1.732_050_8;

pub(super) struct White {
    rng: Rng,
    sample_rate_cached: f32,
    gain: f32,
}

impl White {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed),
            sample_rate_cached: 0.0,
            gain: LEVEL,
        }
    }

    #[inline]
    pub(super) fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            self.gain = LEVEL * white_noise_density_scale(sample_rate);
        }
        self.rng.next_bipolar() * self.gain
    }
}
