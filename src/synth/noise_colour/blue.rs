//! Blue noise as noise apps play it, after the example blue noise
//! recording Atrium was given.
//!
//! The textbook blue noise has a density per hertz rising with frequency,
//! 3 dB per octave, all the way up. The example rises about that much
//! through the middle (steeper from 1 to 3 kHz), is cut below about
//! 100 Hz, and is flat above about 5 kHz. Its density per hertz against
//! 1 kHz, in third octaves: −11.9 dB at 50 Hz, −6.1 at 125 Hz, −2.6 at
//! 500 Hz, +4.5 at 2 kHz, +7.4 at 5 kHz, +8.0 from 8 kHz up.
//!
//! Atrium's blue is a fit to it, of three named parts, in power:
//! - a flat floor;
//! - a high shelf on it, rising by [`SHELF_DB`] (12.6 dB) round
//!   [`SHELF_CORNER_HZ`] (3.2 kHz) as a second-order step,
//!   `1 + H · r / (1 + r)` with `r = (f / corner)²`, flat above;
//! - a first-order high-pass at [`HIGH_PASS_HZ`] (105 Hz) under both.
//!
//! Fitted by least squares over the third octaves 50 Hz to 12.5 kHz it is
//! 0.32 dB RMS from the recording and at most 0.8 dB from it in any band.
//!
//! White noise shaped in blocks in the frequency domain ([`super::shaped`]),
//! as the magnitudes in hertz, so it is the same at every rate.

use super::shaped::Shaped;

/// How far the shelf lifts the top over the floor (dB).
const SHELF_DB: f64 = 12.57;
/// Where the shelf is halfway up, in power (Hz).
const SHELF_CORNER_HZ: f64 = 3_228.0;
/// The first-order high-pass under it all (Hz).
const HIGH_PASS_HZ: f64 = 105.1;
/// Gain that brings the RMS to about 0.1 at 48 kHz (measured).
const LEVEL: f64 = 0.04387;

/// Blue's power gain at `frequency_hz`, the floor being 1.
fn power_gain(frequency_hz: f64) -> f64 {
    let shelf = (frequency_hz / SHELF_CORNER_HZ).powi(2);
    let high_pass = (frequency_hz / HIGH_PASS_HZ).powi(2);
    (1.0 + 10.0_f64.powf(SHELF_DB / 10.0) * shelf / (1.0 + shelf)) * high_pass / (1.0 + high_pass)
}

pub(super) struct Blue {
    shaped: Shaped,
}

impl Blue {
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
