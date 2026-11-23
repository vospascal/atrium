//! Violet noise: f², +6 dB per octave in density per hertz.
//!
//! Differentiated white noise, `w[n] − w[n−1]`, whose power gain
//! 4·sin²(πf/rate) is (2πf/rate)² well below the rate's limit. The
//! difference is scaled by rate / 48 kHz, so the density per hertz is the
//! same at every rate. Near the top it falls short of f²: at 8 kHz by
//! 0.4 dB at 48 kHz, at 16 kHz by 1.6 dB (`sin x < x`).

use crate::synth::noise::{white_noise_density_scale, Rng, REFERENCE_SAMPLE_RATE};

/// Gain that brings the RMS to 0.1 at 48 kHz: the difference of two
/// independent uniform draws has an RMS of √(2/3).
const LEVEL: f32 = 0.1 * 1.224_744_9;

pub(super) struct Violet {
    rng: Rng,
    previous: f32,
    sample_rate_cached: f32,
    gain: f32,
}

impl Violet {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            rng: Rng::new(seed),
            previous: 0.0,
            sample_rate_cached: 0.0,
            gain: LEVEL,
        }
    }

    #[inline]
    pub(super) fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            // The white density per hertz held, then the difference's gain
            // at a fixed frequency, (2πf / rate)², held: one rate / 48 kHz.
            self.gain = LEVEL
                * white_noise_density_scale(sample_rate)
                * (sample_rate / REFERENCE_SAMPLE_RATE);
        }
        let white = self.rng.next_bipolar();
        let difference = white - self.previous;
        self.previous = white;
        difference * self.gain
    }
}
