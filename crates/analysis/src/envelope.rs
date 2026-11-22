//! The envelope of one band of a signal, as McDermott and Simoncelli (2011)
//! take it: the magnitude of the band's analytic signal (its Hilbert
//! envelope), compressed, lowpassed and sampled at 400 Hz.
//!
//! The band is cut from the whole file's spectrum and shifted down to
//! baseband, so its analytic signal comes from an inverse transform only as
//! long as the band is wide: a shift in frequency changes the phase of the
//! analytic signal, not its magnitude. The compressed envelope is then
//! lowpassed by keeping its spectrum below 200 Hz, and that spectrum's
//! inverse at 400 points a second is the envelope at exactly 400 Hz.

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;
use rustfft::FftPlanner;

use crate::spectrum::{Spectrum, ENVELOPE_RATE};

/// The analytic band is sampled at least this fast (Hz) before it is
/// compressed, so the harmonics compression makes fold back above 200 Hz,
/// where the lowpass removes them.
const MINIMUM_ANALYTIC_RATE: f64 = 1600.0;

/// The envelope, at [`ENVELOPE_RATE`], of `spectrum` through a zero-phase
/// filter of amplitude `response(frequency)` that is zero outside
/// `[low, high)` Hz, raised to `exponent` (1 to leave it uncompressed), over
/// the signal's length.
pub(crate) fn band_envelope(
    spectrum: &Spectrum,
    low: f64,
    high: f64,
    response: impl Fn(f64) -> f64,
    exponent: f64,
) -> Vec<f64> {
    let spacing = spectrum.hertz_per_bin();
    // Whole by construction of the spectrum's size.
    let envelope_points = (ENVELOPE_RATE as f64 / spacing).round() as usize;
    let output_length = (spectrum.length() as f64 * ENVELOPE_RATE as f64
        / spectrum.sample_rate() as f64)
        .floor() as usize;
    let range = spectrum.bin_range(low, high);
    if range.is_empty() || output_length == 0 {
        return vec![0.0; output_length];
    }

    // The analytic signal at baseband: 2/N for every positive frequency, 1/N
    // for DC, so its magnitude is the band's Hilbert envelope.
    let mut analytic_points = envelope_points;
    while analytic_points < 2 * range.len()
        || (analytic_points as f64) * spacing < MINIMUM_ANALYTIC_RATE
    {
        analytic_points *= 2;
    }
    let size = spectrum.size() as f32;
    let mut analytic = vec![Complex::new(0.0f32, 0.0); analytic_points];
    for (slot, index) in analytic.iter_mut().zip(range.clone()) {
        let scale = if index == 0 { 1.0 } else { 2.0 } / size;
        *slot = spectrum.bins()[index] * (response(index as f64 * spacing) as f32 * scale);
    }
    FftPlanner::<f32>::new()
        .plan_fft_inverse(analytic_points)
        .process(&mut analytic);

    let mut real_planner = RealFftPlanner::<f32>::new();
    let forward = real_planner.plan_fft_forward(analytic_points);
    let mut compressed: Vec<f32> = analytic
        .iter()
        .map(|value| (value.norm() as f64).powf(exponent) as f32)
        .collect();
    let mut envelope_spectrum = forward.make_output_vec();
    forward
        .process(&mut compressed, &mut envelope_spectrum)
        .expect("buffers are the planner's own");

    // Keep below 200 Hz; the 200 Hz bin itself is the new Nyquist and is
    // dropped, as an ideal lowpass at it would.
    let inverse = real_planner.plan_fft_inverse(envelope_points);
    let mut kept = inverse.make_input_vec();
    let kept_length = kept.len();
    kept[..kept_length - 1].copy_from_slice(&envelope_spectrum[..kept_length - 1]);
    kept[0].im = 0.0;
    let mut envelope = inverse.make_output_vec();
    inverse
        .process(&mut kept, &mut envelope)
        .expect("DC and Nyquist are real");
    let scale = 1.0 / analytic_points as f64;
    envelope
        .iter()
        .take(output_length)
        .map(|value| *value as f64 * scale)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::{sine, RATE};

    /// A 1 kHz sine of amplitude 0.5 whose amplitude rises to 1.5 and falls
    /// back at 4 Hz: its envelope is 0.5·(2 + sin), read back sample for
    /// sample away from the ends.
    #[test]
    fn the_envelope_of_an_amplitude_modulated_tone() {
        let seconds = 4.0;
        let tone: Vec<f32> = sine(1000.0, 1.0, seconds)
            .iter()
            .enumerate()
            .map(|(index, carrier)| {
                let time = index as f64 / RATE as f64;
                let amplitude = 0.5 * (2.0 + (std::f64::consts::TAU * 4.0 * time).sin());
                (*carrier as f64 * amplitude) as f32
            })
            .collect();
        let spectrum = Spectrum::of(&tone, RATE);
        let envelope = band_envelope(&spectrum, 800.0, 1200.0, |_| 1.0, 1.0);
        assert_eq!(envelope.len(), 1600);
        for (index, value) in envelope.iter().enumerate().skip(100).take(1400) {
            let time = index as f64 / 400.0;
            let expected = 0.5 * (2.0 + (std::f64::consts::TAU * 4.0 * time).sin());
            assert!((value - expected).abs() < 2e-3, "{time} s: {value}");
        }
    }
}
