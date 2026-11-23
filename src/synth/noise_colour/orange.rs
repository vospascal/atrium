//! Orange noise: there is no agreed definition. The description Atrium had
//! calls it a blend of "red noise, a little bit of yellow noise, and brown
//! noise", deep and soothing, and yellow has no definition either. The
//! example orange noise recording Atrium was given has a density per hertz
//! against 1 kHz of +47.5 dB at 63 Hz (peaking at +48.4 near 50 Hz and
//! falling below), +24.7 at 250 Hz, +10.6 at 500, −7.1 at 2 kHz, −16 at
//! 4 kHz and −26.2 at 8 kHz. Atrium's orange is two layers fitted to it:
//! - deep: a dark brown, 1/f² under a second-order Butterworth low-pass at
//!   130 Hz, which carries nearly all of the power (22 dB more than the
//!   bright layer);
//! - bright: red (1/f², Brownian), 24 dB under the deep layer's 1/f² line,
//!   softened above 3 kHz by a first-order low-pass. The two densities meet
//!   at about 520 Hz; above it the bright layer is the sound's top;
//! - both under a second-order Butterworth high-pass at 40 Hz, so the
//!   density peaks near 50 Hz as the recording's does. It is on each layer,
//!   so the layers still add up to the sound.
//!
//! As fitted, the model is within 0.8 dB of the recording at every band
//! given from 63 Hz to 8 kHz; measured from the noise, within 1.2 dB. Atrium's own brown ([`super::brown`], low-pass at
//! 285 Hz) with red under it cannot be: its brown falls 12.6 dB from 63 to
//! 250 Hz where the recording falls 22.8, and the best balance of the two
//! still misses three bands by 4.9 dB. So the deep layer is darker than
//! Atrium's brown, and the bright one is red with its top softened.
//!
//! The two are the sound's two layers, so a fader can rebalance them. Both
//! are white noise shaped in blocks in the frequency domain
//! ([`super::shaped`]), from two streams of the seed, so they are not
//! correlated.

use super::brown::brownian_power_gain;
use super::dispatch::MOST_LAYERS;
use super::shaped::Shaped;

/// The deep layer's second-order low-pass (Hz).
const DEEP_LOW_PASS_HZ: f64 = 130.0;
/// The bright layer's first-order low-pass (Hz).
const BRIGHT_LOW_PASS_HZ: f64 = 3_000.0;
/// How far the bright layer's density is under the deep one's 1/f² line,
/// below both low-passes (dB).
pub(super) const BRIGHT_UNDER_DEEP_DB: f64 = 24.0;
/// The high-pass on both layers (Hz).
const HIGH_PASS_HZ: f64 = 40.0;
/// Gain on both layers that brings their sum's RMS to about 0.1 at 48 kHz
/// (measured).
const LEVEL: f64 = 0.2149;
/// The bright layer draws from its own stream: seeded from the deep one's
/// seed plus this.
const BRIGHT_SEED_OFFSET: u64 = 1;

fn high_pass(frequency_hz: f64) -> f64 {
    let ratio = (frequency_hz / HIGH_PASS_HZ).powi(4);
    ratio / (1.0 + ratio)
}

/// The deep layer's power gain at `frequency_hz`.
pub(super) fn deep_power_gain(frequency_hz: f64) -> f64 {
    brownian_power_gain(frequency_hz) / (1.0 + (frequency_hz / DEEP_LOW_PASS_HZ).powi(4))
        * high_pass(frequency_hz)
}

/// The bright layer's power gain at `frequency_hz`.
pub(super) fn bright_power_gain(frequency_hz: f64) -> f64 {
    brownian_power_gain(frequency_hz) / (1.0 + (frequency_hz / BRIGHT_LOW_PASS_HZ).powi(2))
        * high_pass(frequency_hz)
        * 10.0_f64.powf(-BRIGHT_UNDER_DEEP_DB / 10.0)
}

pub(super) struct Orange {
    deep: Shaped,
    bright: Shaped,
}

impl Orange {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            deep: Shaped::new(deep_power_gain, LEVEL, seed),
            bright: Shaped::new(
                bright_power_gain,
                LEVEL,
                seed.wrapping_add(BRIGHT_SEED_OFFSET),
            ),
        }
    }

    /// The deep layer's next sample, then the bright one's.
    #[inline]
    pub(super) fn next_layers(&mut self, sample_rate: f32) -> [f32; MOST_LAYERS] {
        [
            self.deep.next_sample(sample_rate),
            self.bright.next_sample(sample_rate),
        ]
    }
}
