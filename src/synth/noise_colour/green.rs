//! Green noise: there is no agreed definition; the name is used for
//! different sounds. Atrium's follows the example green noise recording it
//! was given, of the kind sleep apps play: a soft, dark noise with the
//! treble taken away, no bump anywhere. Its density per hertz against 1 kHz
//! is +16.3 dB at 63 Hz, +13.2 at 125, +9.9 at 250, +7.1 at 500, −13.4 at
//! 2 kHz, −30.7 at 4 kHz and −52.6 at 8 kHz. The best fit, and what this
//! is: a density falling as 1/f^1.45 (−4.35 dB per octave) under a
//! third-order Butterworth low-pass at 1640 Hz, 1.6 dB RMS from the
//! recording over 63 Hz to 8 kHz (pink under a second-order low-pass at
//! 820 Hz fits worse, 2.5 dB). Like red, a second-order high-pass at 20 Hz
//! keeps the slope from piling up infrasound.
//!
//! White noise shaped in blocks in the frequency domain ([`super::shaped`]),
//! as analogue magnitudes in hertz, so it is the same at every rate.

use super::shaped::Shaped;

/// The fitted slope of the density, as a power of frequency.
pub(super) const SLOPE_EXPONENT: f64 = 1.45;
/// The third-order low-pass (Hz).
pub(super) const LOW_PASS_HZ: f64 = 1_640.0;
const HIGH_PASS_HZ: f64 = 20.0;
/// Gain that brings the RMS to about 0.1 at 48 kHz (measured).
const LEVEL: f64 = 0.2526;

/// Green's power gain at `frequency_hz`:
/// `(1000/f)^1.45 · 1 / (1 + (f/1640)⁶)` under the 20 Hz high-pass.
pub(super) fn power_gain(frequency_hz: f64) -> f64 {
    let slope = (1_000.0 / frequency_hz).powf(SLOPE_EXPONENT);
    let low = 1.0 / (1.0 + (frequency_hz / LOW_PASS_HZ).powi(6));
    let high = (frequency_hz / HIGH_PASS_HZ).powi(4);
    slope * low * high / (1.0 + high)
}

pub(super) struct Green {
    shaped: Shaped,
}

impl Green {
    pub(super) fn new(seed: u64) -> Self {
        Self {
            shaped: Shaped::new(power_gain, LEVEL, seed),
        }
    }

    #[inline]
    pub(super) fn next_sample(&mut self, sample_rate: f32) -> f32 {
        self.shaped.next_sample(sample_rate)
    }
}
