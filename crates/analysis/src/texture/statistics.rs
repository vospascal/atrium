//! The texture statistics of McDermott and Simoncelli (2011), on subband
//! envelopes sampled at 400 Hz, every average weighted by a window w(t) that
//! sums to one.

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

use super::filters::ModulationFilter;
use crate::spectrum::ENVELOPE_RATE;

/// The weighting window's ramp at each end (s): the paper's raised-cosine
/// half cycle over one second, shortened to a quarter of a short file.
const RAMP_SECONDS: f64 = 1.0;

/// The window w(t): one in the middle, raised-cosine ramps at the ends,
/// scaled to sum to one.
pub(crate) fn window(length: usize) -> Vec<f64> {
    let ramp = ((RAMP_SECONDS * ENVELOPE_RATE as f64) as usize).min(length / 4);
    let mut weights: Vec<f64> = (0..length)
        .map(|index| {
            let from_edge = index.min(length - 1 - index);
            if from_edge < ramp {
                0.5 * (1.0 - (std::f64::consts::PI * (from_edge as f64 + 0.5) / ramp as f64).cos())
            } else {
                1.0
            }
        })
        .collect();
    let sum: f64 = weights.iter().sum();
    if sum > 0.0 {
        weights.iter_mut().for_each(|weight| *weight /= sum);
    }
    weights
}

/// The weighted mean.
pub(crate) fn weighted_mean(values: &[f64], weights: &[f64]) -> f64 {
    values
        .iter()
        .zip(weights)
        .map(|(value, weight)| value * weight)
        .sum()
}

/// M1 to M4 of one envelope: μ, σ²/μ², skewness and kurtosis (not excess:
/// a Gaussian's is 3). All zero for an envelope that does not vary.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub(crate) struct Moments {
    pub(crate) mean: f64,
    pub(crate) variance: f64,
    pub(crate) variance_over_mean_squared: f64,
    pub(crate) skewness: f64,
    pub(crate) kurtosis: f64,
}

pub(crate) fn moments(envelope: &[f64], weights: &[f64]) -> Moments {
    let mean = weighted_mean(envelope, weights);
    let central = |power: i32| {
        envelope
            .iter()
            .zip(weights)
            .map(|(value, weight)| weight * (value - mean).powi(power))
            .sum::<f64>()
    };
    let variance = central(2);
    if variance <= 0.0 || mean <= 0.0 {
        return Moments {
            mean: mean.max(0.0),
            ..Moments::default()
        };
    }
    let deviation = variance.sqrt();
    Moments {
        mean,
        variance,
        variance_over_mean_squared: variance / (mean * mean),
        skewness: central(3) / deviation.powi(3),
        kurtosis: central(4) / variance.powi(2),
    }
}

/// C_jk: Σw(s_j − μ_j)(s_k − μ_k) / (σ_j·σ_k); zero when either does not
/// vary.
pub(crate) fn envelope_correlation(
    first: &[f64],
    first_moments: &Moments,
    second: &[f64],
    second_moments: &Moments,
    weights: &[f64],
) -> f64 {
    let scale = (first_moments.variance * second_moments.variance).sqrt();
    if scale <= 0.0 {
        return 0.0;
    }
    first
        .iter()
        .zip(second)
        .zip(weights)
        .map(|((a, b), weight)| weight * (a - first_moments.mean) * (b - second_moments.mean))
        .sum::<f64>()
        / scale
}

/// An envelope through modulation filters, each applied as a zero-phase
/// gain on its spectrum.
pub(crate) struct ModulationBands {
    spectrum: Vec<Complex<f64>>,
    length: usize,
}

impl ModulationBands {
    pub(crate) fn of(envelope: &[f64]) -> Self {
        let mut planner = RealFftPlanner::<f64>::new();
        let forward = planner.plan_fft_forward(envelope.len());
        let mut input = envelope.to_vec();
        let mut spectrum = forward.make_output_vec();
        forward
            .process(&mut input, &mut spectrum)
            .expect("buffers are the planner's own");
        Self {
            spectrum,
            length: envelope.len(),
        }
    }

    /// b_n(t): the envelope through `filter`.
    pub(crate) fn band(&self, filter: &ModulationFilter) -> Vec<f64> {
        let spacing = ENVELOPE_RATE as f64 / self.length as f64;
        let mut filtered: Vec<Complex<f64>> = self
            .spectrum
            .iter()
            .enumerate()
            .map(|(index, bin)| bin * filter.response(index as f64 * spacing))
            .collect();
        filtered[0].im = 0.0;
        if self.length.is_multiple_of(2) {
            // The Nyquist bin of an even length is real; an odd length has none.
            let last = filtered.len() - 1;
            filtered[last].im = 0.0;
        }
        let mut planner = RealFftPlanner::<f64>::new();
        let inverse = planner.plan_fft_inverse(self.length);
        let mut output = inverse.make_output_vec();
        inverse
            .process(&mut filtered, &mut output)
            .expect("DC and Nyquist are real");
        let scale = 1.0 / self.length as f64;
        output.iter_mut().for_each(|value| *value *= scale);
        output
    }
}

/// Σw·b² / σ²: a modulation band's power over the envelope's variance.
pub(crate) fn modulation_power(band: &[f64], weights: &[f64], variance: f64) -> f64 {
    if variance <= 0.0 {
        return 0.0;
    }
    band.iter()
        .zip(weights)
        .map(|(value, weight)| weight * value * value)
        .sum::<f64>()
        / variance
}

/// C1: Σw·b̃_j·b̃_k / (σ_j·σ_k) with σ = √(Σw·b̃²); zero when either is
/// silent.
pub(crate) fn band_correlation(first: &[f64], second: &[f64], weights: &[f64]) -> f64 {
    let mut product = 0.0;
    let mut first_power = 0.0;
    let mut second_power = 0.0;
    for ((a, b), weight) in first.iter().zip(second).zip(weights) {
        product += weight * a * b;
        first_power += weight * a * a;
        second_power += weight * b * b;
    }
    let scale = (first_power * second_power).sqrt();
    if scale <= 0.0 {
        0.0
    } else {
        product / scale
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_ramps_and_sums_to_one() {
        let weights = window(4000);
        assert!((weights.iter().sum::<f64>() - 1.0).abs() < 1e-12);
        // One second of ramp at 400 Hz: flat from sample 400 to 3599.
        assert!((weights[400] - weights[2000]).abs() < 1e-15);
        assert!(weights[0] < weights[200] && weights[200] < weights[399]);
    }

    /// The moments of a two-valued series, by hand: 1 and 3 in equal
    /// shares have mean 2, variance 1, M2 = 1/4, skewness 0, kurtosis 1.
    #[test]
    fn moments_of_a_two_valued_series() {
        let values: Vec<f64> = (0..1000).map(|index| [1.0, 3.0][index % 2]).collect();
        let weights = vec![1.0 / 1000.0; 1000];
        let found = moments(&values, &weights);
        assert!((found.mean - 2.0).abs() < 1e-12);
        assert!((found.variance_over_mean_squared - 0.25).abs() < 1e-12);
        assert!(found.skewness.abs() < 1e-12);
        assert!((found.kurtosis - 1.0).abs() < 1e-12);
    }
}
