//! Brown noise as sleep and noise apps play it: darker than Brownian noise.
//!
//! In physics Brownian noise is 1/f², the spectrum of a random walk, and
//! that is what Atrium's red is ([`super::red`]). The example brown noise
//! recording Atrium was given (a 60 s file of the kind sleep apps play)
//! falls far faster: −15.5 dB per octave over 63 Hz to 8 kHz, 45 dB above
//! 1 kHz at 63 Hz and 48 dB below it at 5 kHz. 1/f² through a second-order
//! low-pass at about 285 Hz fits it best (2.1 dB RMS error; third order at
//! 460 Hz fits worse, 3.7 dB). So Atrium's brown is that: the density of
//! red, `1/f²` with red's 15.3 Hz leak and 20 Hz second-order high-pass,
//! times a second-order Butterworth low-pass at 285 Hz, falling 18 dB per
//! octave above it.
//!
//! White noise shaped in blocks in the frequency domain ([`super::shaped`]),
//! as the analogue magnitudes in hertz, so it is the same at every rate: a
//! digital low-pass at 285 Hz made by the bilinear transform would be 1.6 dB
//! steeper at 8 kHz at 48 kHz than at 96 kHz.

use super::shaped::Shaped;

/// Red's leak, under which the walk is flat (Hz): `noise::BrownNoise`'s.
const LEAK_HZ: f64 = 15.294_174;
/// The high-pass under the audible band (Hz), as red's.
const HIGH_PASS_HZ: f64 = 20.0;
/// The low-pass that darkens it (Hz), fitted to the example recording.
pub(super) const LOW_PASS_HZ: f64 = 285.0;
/// Gain that brings the RMS to about 0.1 at 48 kHz (measured).
const LEVEL: f64 = 0.1361;

/// Red's power gain at `frequency_hz` as an analogue magnitude:
/// 1 kHz² / (f² + leak²) under the 20 Hz second-order Butterworth high-pass,
/// 1 at 1 kHz within 0.001 dB.
pub(super) fn brownian_power_gain(frequency_hz: f64) -> f64 {
    let square = frequency_hz * frequency_hz;
    let walk = 1.0e6 / (square + LEAK_HZ * LEAK_HZ);
    let high = (frequency_hz / HIGH_PASS_HZ).powi(4);
    walk * high / (1.0 + high)
}

/// Brown's power gain at `frequency_hz`: red's under the 285 Hz
/// second-order Butterworth low-pass.
pub(super) fn power_gain(frequency_hz: f64) -> f64 {
    brownian_power_gain(frequency_hz) / (1.0 + (frequency_hz / LOW_PASS_HZ).powi(4))
}

pub(super) struct Brown {
    shaped: Shaped,
}

impl Brown {
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
