//! One narrow band of a channel as a complex signal at baseband: the band's
//! analytic signal shifted down by a whole number of bins, sampled at
//! 400 Hz. Its magnitude is the band's Hilbert envelope; its phase, plus the
//! shift, its instantaneous frequency.
//!
//! As `envelope.rs` does, the band is cut from the whole file's spectrum,
//! whose bin spacing divides 400 Hz exactly, so an inverse transform of
//! 400 / spacing points gives the samples at exactly 400 Hz.

use realfft::num_complex::Complex;
use rustfft::FftPlanner;

use crate::spectrum::{Spectrum, ENVELOPE_RATE};

pub(crate) struct Baseband {
    /// At [`ENVELOPE_RATE`], over the signal's length.
    pub(crate) values: Vec<Complex<f32>>,
    /// The frequency subtracted (Hz).
    pub(crate) shift_hertz: f64,
}

/// The band's edges roll off over this much (Hz), raised-cosine, inside the
/// band. A hard-edged band rings in time as 1/t (Gibbs): a steady sine read
/// through a 4 Hz band wandered by 1 % in level two seconds from the file's
/// start. Lines lie at least 0.75 Hz inside a cluster's band, so they stay in
/// the flat part.
const ROLL_OFF_HERTZ: f64 = 0.5;

/// The band `[low, high)` Hz of `spectrum` at baseband, shifted by the bin
/// nearest its middle. The band must be narrower than 400 Hz.
pub(crate) fn baseband(spectrum: &Spectrum, low: f64, high: f64) -> Baseband {
    let spacing = spectrum.hertz_per_bin();
    let points = (ENVELOPE_RATE as f64 / spacing).round() as usize;
    let length = (spectrum.length() as f64 * ENVELOPE_RATE as f64 / spectrum.sample_rate() as f64)
        .floor() as usize;
    let centre_bin = ((low + high) / 2.0 / spacing).round() as i64;
    let range = spectrum.bin_range(low, high);
    assert!(
        range.len() < points,
        "a band of {} Hz is too wide for a 400 Hz baseband",
        high - low
    );
    let mut values = vec![Complex::new(0.0f32, 0.0); points];
    let scale = 2.0 / spectrum.size() as f32;
    let roll_off = ROLL_OFF_HERTZ.min((high - low) / 2.0);
    let gain = |frequency: f64| {
        let inside = (frequency - low).min(high - frequency);
        if inside >= roll_off {
            1.0
        } else {
            0.5 - 0.5 * (std::f64::consts::PI * inside.max(0.0) / roll_off).cos()
        }
    };
    for index in range {
        let slot = (index as i64 - centre_bin).rem_euclid(points as i64) as usize;
        values[slot] = spectrum.bins()[index] * (scale * gain(index as f64 * spacing) as f32);
    }
    FftPlanner::<f32>::new()
        .plan_fft_inverse(points)
        .process(&mut values);
    values.truncate(length);
    Baseband {
        values,
        shift_hertz: centre_bin as f64 * spacing,
    }
}

impl Baseband {
    /// The instantaneous frequency (Hz) between sample `index − 1` and
    /// `index`.
    pub(crate) fn frequency_at(&self, index: usize) -> f64 {
        let step = self.values[index] * self.values[index - 1].conj();
        self.shift_hertz
            + (step.im as f64).atan2(step.re as f64) * ENVELOPE_RATE as f64 / std::f64::consts::TAU
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::{sine, RATE};

    /// A 250.3 Hz sine of amplitude 0.4, from 2 s in to 2 s before the end
    /// (the span the partials are read over: the 0.5 Hz roll-off takes about
    /// 1 / 0.5 Hz = 2 s to settle after the sine starts or stops; 1.5 s from
    /// the end the magnitude is still 0.75 % off). Measured at every sample
    /// of the span: the magnitude within 0.38 % (0.03 dB; under 0.08 % from
    /// 2.5 s on) and the frequency within 0.0051 Hz (0.035 cents); asserted
    /// within 0.5 % and 0.006 Hz.
    #[test]
    fn a_sine_at_baseband() {
        let tone = sine(250.3, 0.4, 10.0);
        let spectrum = Spectrum::of(&tone, RATE);
        let band = baseband(&spectrum, 248.0, 252.0);
        assert_eq!(band.values.len(), 4000);
        for index in 800..3200 {
            let magnitude = band.values[index].norm();
            assert!((magnitude - 0.4).abs() < 2e-3, "{index}: {magnitude}");
            let frequency = band.frequency_at(index);
            assert!((frequency - 250.3).abs() < 6e-3, "{index}: {frequency}");
        }
    }
}
