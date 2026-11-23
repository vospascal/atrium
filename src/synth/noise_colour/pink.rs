//! Pink noise: 1/f, the same power in every octave, −3 dB per octave in
//! density per hertz.
//!
//! Paul Kellet's seven-pole filter of white noise (`noise::PinkNoiseFull`),
//! which holds −3 dB per octave within 0.3 dB from 100 Hz to 16 kHz at 44.1,
//! 48 and 96 kHz (`noise` tests) and here from 63 Hz to 8 kHz (this
//! module's tests). Its coefficients are per sample, published for 44.1 kHz,
//! yet the density per hertz of a 1/f spectrum does not depend on the rate
//! the poles run at: only its band edges move (about 8 Hz at the bottom at
//! 44.1 kHz, 17 Hz at 96 kHz). Its RMS is 0.108 at every rate.

use crate::synth::noise::PinkNoiseFull;

pub(super) struct Pink {
    noise: PinkNoiseFull,
}

impl Pink {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            noise: PinkNoiseFull::new(seed),
        }
    }

    #[inline]
    pub(super) fn next_sample(&mut self) -> f32 {
        self.noise.next_sample()
    }
}
