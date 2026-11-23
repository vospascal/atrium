//! Grey noise as sleep and tinnitus apps play it, after the example grey
//! noise recording Atrium was given.
//!
//! The textbook grey noise is white noise shaped by the inverse of an
//! equal-loudness contour (ISO 226), so that every frequency sounds about
//! as loud as every other. That inverse rises again above 5 kHz, by some
//! 12 to 14 dB at 8 to 12 kHz against 1 kHz at 40 phon. The example does
//! not: it is broadly shaped like the contour, with more bass and a shallow
//! dip near 2 to 3 kHz where the ear is most sensitive, but flat at the top
//! and gentler throughout. Its density per hertz against 1 kHz, in third
//! octaves: +25.4 dB at 31.5 Hz, +18.5 at 125 Hz, +7.0 at 500 Hz, −5.5 at
//! 2.5 kHz, −2.3 at 12.5 kHz.
//!
//! Atrium's grey is a fit to it, of three named parts, in power:
//! - a flat floor;
//! - a low part, [`LOW_OVER_FLOOR_DB`] (28.9 dB) over the floor below
//!   [`LOW_CORNER_HZ`] (54 Hz) and falling 6 dB per octave above it (a
//!   first-order low-pass), added to the floor;
//! - a dip, [`DIP_DB`] (5.0 dB) deep at [`DIP_CENTRE_HZ`] (1964 Hz), a
//!   Gaussian over log frequency [`DIP_WIDTH_OCTAVES`] (0.9) octave wide
//!   (one standard deviation), multiplying the rest.
//!
//! Fitted by least squares over the third octaves 31.5 Hz to 12.5 kHz it is
//! 0.18 dB RMS from the recording and at most 0.6 dB from it in any band.
//! A fourth-order Butterworth high-pass at 20 Hz bounds it below.
//!
//! White noise shaped in blocks in the frequency domain ([`super::shaped`]),
//! as the magnitudes in hertz, so it is the same at every rate.

use super::shaped::Shaped;

/// How far the low part is over the floor, below its corner (dB).
const LOW_OVER_FLOOR_DB: f64 = 28.9;
/// The low part's corner (Hz): a first-order low-pass.
const LOW_CORNER_HZ: f64 = 54.2;
/// How deep the dip is at its centre (dB).
const DIP_DB: f64 = 5.0;
const DIP_CENTRE_HZ: f64 = 1_964.0;
/// The dip's width: one standard deviation, in octaves.
const DIP_WIDTH_OCTAVES: f64 = 0.905;
/// The high-pass under the audible band (Hz), fourth-order Butterworth.
const HIGH_PASS_HZ: f64 = 20.0;
/// Gain that brings the RMS to about 0.1 at 48 kHz (measured).
const LEVEL: f64 = 0.1022;

/// Grey's power gain at `frequency_hz`, the floor being 1.
pub(super) fn power_gain(frequency_hz: f64) -> f64 {
    let low =
        10.0_f64.powf(LOW_OVER_FLOOR_DB / 10.0) / (1.0 + (frequency_hz / LOW_CORNER_HZ).powi(2));
    let octaves = (frequency_hz / DIP_CENTRE_HZ).log2();
    let dip_db =
        DIP_DB * (-(octaves * octaves) / (2.0 * DIP_WIDTH_OCTAVES * DIP_WIDTH_OCTAVES)).exp();
    let high_pass = {
        let ratio = (frequency_hz / HIGH_PASS_HZ).powi(8);
        ratio / (1.0 + ratio)
    };
    (1.0 + low) * 10.0_f64.powf(-dip_db / 10.0) * high_pass
}

pub(super) struct Grey {
    shaped: Shaped,
}

impl Grey {
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
