//! Red noise, Brownian noise in physics: 1/f², −6 dB per octave in density
//! per hertz, the spectrum of a random walk. (Atrium's "brown" is darker
//! still, after an example recording; see [`super::brown`].)
//!
//! White noise through a leaky integrator (`noise::BrownNoise`: a random
//! walk with a 15.3 Hz leak, specified in hertz and density per hertz, so
//! the same at every rate), then a second-order Butterworth high-pass at
//! 20 Hz. The leak alone keeps the walk bounded, but leaves it flat below
//! 15 Hz down to 0 Hz, and that infrasound would be half of its power; the
//! high-pass takes it out and leaves nothing at 0 Hz, so it cannot drift.
//! Together they are within 0.3 dB of 1/f² at 63 Hz and 0.1 dB at 125 Hz.
//! At the top the walk's per-sample integrator is 0.4 dB above 1/f² at
//! 8 kHz at 48 kHz.

use crate::audio::filters::Biquad;
use crate::synth::noise::BrownNoise;

/// The high-pass under the audible band (Hz).
const HIGH_PASS_HZ: f32 = 20.0;
/// Gain that brings the RMS to about 0.1 at 48 kHz (measured: the walk
/// through the high-pass has an RMS of 0.0597).
const LEVEL: f32 = 1.675;

pub(super) struct Red {
    walk: BrownNoise,
    high_pass: Biquad,
    sample_rate_cached: f32,
}

impl Red {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            walk: BrownNoise::new(seed),
            high_pass: Biquad::unity(),
            sample_rate_cached: 0.0,
        }
    }

    #[inline]
    pub(super) fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.sample_rate_cached = sample_rate;
            self.high_pass = Biquad::highpass(HIGH_PASS_HZ, sample_rate);
        }
        self.high_pass.process(self.walk.next_sample(sample_rate)) * LEVEL
    }
}
