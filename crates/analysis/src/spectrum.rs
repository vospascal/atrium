//! The spectrum of a whole channel, in one Fourier transform.
//!
//! Most measures filter: A- and C-weighting, third-octave and octave bands,
//! the cochlear subbands of the texture. Each is done here as a gain on the
//! bins of one transform of the whole file, which is exact at every
//! frequency (a 48 kHz bilinear A-weighting filter is a decibel off at
//! 10 kHz) and zero-phase. The file is padded with a quarter second of
//! silence so a filter's ringing does not wrap round from the end to the
//! start.
//!
//! The padded length is chosen so the bin spacing divides 400 Hz exactly
//! (`size` a multiple of `sample_rate / gcd(sample_rate, 400)`): the
//! envelopes are then sampled at exactly 400 Hz by an inverse transform,
//! with no interpolation.

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

/// Silence after the signal (s), so zero-phase filters do not wrap.
const GUARD_SECONDS: f64 = 0.25;

/// The rate envelopes are sampled at (Hz), as McDermott and Simoncelli's.
pub(crate) const ENVELOPE_RATE: u32 = 400;

/// One channel's spectrum, positive frequencies only.
pub(crate) struct Spectrum {
    bins: Vec<Complex<f32>>,
    /// The transform's length: the signal and its padding.
    size: usize,
    /// The signal's length in samples, without padding.
    length: usize,
    sample_rate: u32,
}

impl Spectrum {
    pub(crate) fn of(samples: &[f32], sample_rate: u32) -> Self {
        let size = transform_size(samples.len(), sample_rate);
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(size);
        let mut input = forward.make_input_vec();
        input[..samples.len()].copy_from_slice(samples);
        let mut bins = forward.make_output_vec();
        forward
            .process(&mut input, &mut bins)
            .expect("buffers are the planner's own");
        Self {
            bins,
            size,
            length: samples.len(),
            sample_rate,
        }
    }

    pub(crate) fn hertz_per_bin(&self) -> f64 {
        self.sample_rate as f64 / self.size as f64
    }

    pub(crate) fn size(&self) -> usize {
        self.size
    }

    pub(crate) fn length(&self) -> usize {
        self.length
    }

    pub(crate) fn sample_rate(&self) -> u32 {
        self.sample_rate
    }

    pub(crate) fn bins(&self) -> &[Complex<f32>] {
        &self.bins
    }

    /// The bins whose centres lie in `[low, high)` Hz, clipped to the
    /// spectrum.
    pub(crate) fn bin_range(&self, low: f64, high: f64) -> std::ops::Range<usize> {
        let spacing = self.hertz_per_bin();
        let first = (low / spacing).ceil().max(0.0) as usize;
        let end = ((high / spacing).ceil().max(0.0) as usize).min(self.bins.len());
        first.min(end)..end
    }

    /// The mean square, over the signal's length, of the part of it between
    /// `low` and `high` Hz, each bin weighted by `gain(frequency)`² (Parseval:
    /// the bins of a real transform each stand for their mirror too, but for
    /// DC and Nyquist).
    pub(crate) fn mean_square(&self, low: f64, high: f64, gain: impl Fn(f64) -> f64) -> f64 {
        if self.length == 0 {
            return 0.0;
        }
        let spacing = self.hertz_per_bin();
        let nyquist_bin = self.size / 2;
        let mut sum = 0.0f64;
        for index in self.bin_range(low, high) {
            let weight = if index == 0 || index == nyquist_bin {
                1.0
            } else {
                2.0
            };
            let frequency = index as f64 * spacing;
            sum += weight * self.bins[index].norm_sqr() as f64 * gain(frequency).powi(2);
        }
        sum / (self.size as f64 * self.length as f64)
    }

    /// Σ f·P and Σ P over the bins between `low` and `high`, for a
    /// power-weighted mean frequency.
    pub(crate) fn frequency_moments(&self, low: f64, high: f64) -> (f64, f64) {
        let spacing = self.hertz_per_bin();
        self.bin_range(low, high)
            .map(|index| {
                let power = self.bins[index].norm_sqr() as f64;
                (index as f64 * spacing * power, power)
            })
            .fold((0.0, 0.0), |(moment, total), (weighted, power)| {
                (moment + weighted, total + power)
            })
    }

    /// The signal through a zero-phase filter of amplitude `gain(frequency)`.
    pub(crate) fn filtered(&self, gain: impl Fn(f64) -> f64) -> Vec<f32> {
        let spacing = self.hertz_per_bin();
        let mut bins: Vec<Complex<f32>> = self
            .bins
            .iter()
            .enumerate()
            .map(|(index, bin)| bin * gain(index as f64 * spacing) as f32)
            .collect();
        bins[0].im = 0.0;
        let last = bins.len() - 1;
        bins[last].im = 0.0;
        let mut planner = RealFftPlanner::<f32>::new();
        let inverse = planner.plan_fft_inverse(self.size);
        let mut output = inverse.make_output_vec();
        inverse
            .process(&mut bins, &mut output)
            .expect("DC and Nyquist are real");
        let scale = 1.0 / self.size as f32;
        output.truncate(self.length);
        output.iter_mut().for_each(|sample| *sample *= scale);
        output
    }
}

/// The smallest transform at least a quarter second longer than `length`
/// whose bin spacing divides [`ENVELOPE_RATE`]: a multiple of
/// `sample_rate / gcd(sample_rate, 400)` times a power of two, and even.
fn transform_size(length: usize, sample_rate: u32) -> usize {
    let step = (sample_rate / greatest_common_divisor(sample_rate, ENVELOPE_RATE)) as usize;
    let needed = length + (GUARD_SECONDS * sample_rate as f64).ceil() as usize;
    let mut size = step * 2;
    while size < needed {
        size *= 2;
    }
    size
}

fn greatest_common_divisor(mut first: u32, mut second: u32) -> u32 {
    while second != 0 {
        (first, second) = (second, first % second);
    }
    first
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::{sine, RATE};

    #[test]
    fn the_size_makes_the_envelope_rate_exact() {
        // 48 kHz: step 120, so 400 Hz is a whole number of bins.
        let size = transform_size(48_000, 48_000);
        assert_eq!(size % 120, 0);
        assert!(size >= 48_000 + 12_000);
        let spacing = 48_000.0 / size as f64;
        assert!((400.0 / spacing).fract().abs() < 1e-9);
        // 44.1 kHz: step 441.
        let size = transform_size(44_100, 44_100);
        assert_eq!(size % 441, 0);
        assert_eq!(size % 2, 0);
    }

    /// Parseval: a sine of amplitude 0.5 has mean square 0.125, and filtering
    /// with gain 1 gives it back. A one-second sine is a sine through a
    /// one-second rectangular window, which leaks about 2 / (π²·T·Δ) = 0.2 %
    /// of its power further than Δ = 100 Hz away.
    #[test]
    fn mean_square_and_identity_filter() {
        let tone = sine(1000.0, 0.5, 1.0);
        let spectrum = Spectrum::of(&tone, RATE);
        let total = spectrum.mean_square(0.0, 24_000.0, |_| 1.0);
        assert!((total - 0.125).abs() < 1e-4, "{total}");
        let near = spectrum.mean_square(900.0, 1100.0, |_| 1.0);
        assert!(near / total > 0.997, "{near}");
        let back = spectrum.filtered(|_| 1.0);
        let error = back
            .iter()
            .zip(&tone)
            .map(|(a, b)| (a - b).abs())
            .fold(0.0f32, f32::max);
        assert!(error < 1e-4, "{error}");
    }
}
