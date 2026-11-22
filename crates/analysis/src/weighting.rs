//! Frequency weightings A and C and the FAST time weighting, as IEC 61672-1
//! defines them.
//!
//! The weightings are the standard's analytic curves, applied as gains on
//! the bins of the whole file's spectrum (see `spectrum`), so they hold to
//! the hundredth of a decibel at every frequency, where a digital IIR filter
//! made by the bilinear transform is 1 dB off at 10 kHz at 48 kHz.

/// The poles of the weightings (Hz), IEC 61672-1 Annex E.
const POLE_LOW: f64 = 20.598_997;
const POLE_A_FIRST: f64 = 107.652_65;
const POLE_A_SECOND: f64 = 737.862_23;
const POLE_HIGH: f64 = 12_194.217;

/// The FAST time constant (s).
const FAST_SECONDS: f64 = 0.125;

/// LAF is read every 10 ms for the percentile levels: FAST smooths over
/// 125 ms, so nothing is missed between readings.
pub(crate) const PERCENTILE_STEP_SECONDS: f64 = 0.01;

/// The over-time series keep the largest LAF in each 25 ms, so a short
/// event's peak is kept exactly however coarse the series.
pub(crate) const LAF_STEP_SECONDS: f64 = 0.025;

/// The A-weighting's amplitude at `frequency` Hz, 1 at 1 kHz.
pub(crate) fn a_weighting(frequency: f64) -> f64 {
    a_unnormalised(frequency) / a_unnormalised(1000.0)
}

/// The C-weighting's amplitude at `frequency` Hz, 1 at 1 kHz.
pub(crate) fn c_weighting(frequency: f64) -> f64 {
    c_unnormalised(frequency) / c_unnormalised(1000.0)
}

fn a_unnormalised(frequency: f64) -> f64 {
    let square = frequency * frequency;
    POLE_HIGH.powi(2) * square * square
        / ((square + POLE_LOW.powi(2))
            * ((square + POLE_A_FIRST.powi(2)) * (square + POLE_A_SECOND.powi(2))).sqrt()
            * (square + POLE_HIGH.powi(2)))
}

fn c_unnormalised(frequency: f64) -> f64 {
    let square = frequency * frequency;
    POLE_HIGH.powi(2) * square / ((square + POLE_LOW.powi(2)) * (square + POLE_HIGH.powi(2)))
}

/// A power signal through the FAST exponential time weighting, read three
/// ways.
pub(crate) struct TimeWeighted {
    /// The largest value anywhere: LAFmax, as power.
    pub(crate) maximum: f64,
    /// The value every [`PERCENTILE_STEP_SECONDS`].
    pub(crate) readings: Vec<f64>,
    /// The largest value in each [`LAF_STEP_SECONDS`].
    pub(crate) step_maxima: Vec<f64>,
}

/// FAST one power sample at a time: y[n] = a·y[n−1] + (1 − a)·x[n],
/// a = e^(−1/(τ·rate)). Shared by the whole-file series and the live
/// analyser, so the two weight time identically.
pub(crate) struct Fast {
    decay: f64,
    value: f64,
}

impl Fast {
    /// Starting from the power `initial`.
    pub(crate) fn new(sample_rate: u32, initial: f64) -> Self {
        Self {
            decay: (-1.0 / (FAST_SECONDS * sample_rate as f64)).exp(),
            value: initial,
        }
    }

    /// The weighted power after one more sample of `power`.
    pub(crate) fn step(&mut self, power: f64) -> f64 {
        self.value = self.decay * self.value + (1.0 - self.decay) * power;
        self.value
    }
}

/// `power` (squared samples) through FAST: y[n] = a·y[n−1] + (1 − a)·x[n],
/// a = e^(−1/(τ·rate)), the discrete form of IEC 61672-1's
/// (1/τ)∫p²(ξ)e^(−(t−ξ)/τ)dξ.
///
/// It starts from the mean power of the first 125 ms rather than from
/// silence, so a file does not open with a quarter second of rising level
/// that pulls L90 down.
pub(crate) fn fast(power: &[f64], sample_rate: u32) -> TimeWeighted {
    let rate = sample_rate as f64;
    let opening = ((FAST_SECONDS * rate) as usize).clamp(1, power.len().max(1));
    let mut averager = Fast::new(
        sample_rate,
        crate::statistics::mean(&power[..opening.min(power.len())]),
    );
    let reading_step = ((PERCENTILE_STEP_SECONDS * rate).round() as usize).max(1);
    let maxima_step = ((LAF_STEP_SECONDS * rate).round() as usize).max(1);
    let mut maximum = 0.0f64;
    let mut readings = Vec::with_capacity(power.len() / reading_step + 1);
    let mut step_maxima = Vec::with_capacity(power.len() / maxima_step + 1);
    let mut step_maximum = 0.0f64;
    for (index, sample) in power.iter().enumerate() {
        let value = averager.step(*sample);
        maximum = maximum.max(value);
        step_maximum = step_maximum.max(value);
        if index % reading_step == reading_step - 1 {
            readings.push(value);
        }
        if index % maxima_step == maxima_step - 1 {
            step_maxima.push(step_maximum);
            step_maximum = 0.0;
        }
    }
    if !power.len().is_multiple_of(maxima_step) {
        step_maxima.push(step_maximum);
    }
    TimeWeighted {
        maximum,
        readings,
        step_maxima,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::Spectrum;
    use crate::statistics::power_decibels;
    use crate::test_signals::{sine, RATE};

    /// LA − LZ of a sine through the whole chain (spectrum, gain, Parseval)
    /// at the IEC 61672-1 table frequencies: −39.4, −19.1, 0.0, −2.5 dB.
    #[test]
    fn a_weighting_matches_the_standard() {
        for (frequency, expected) in [
            (31.5, -39.4),
            (100.0, -19.1),
            (1000.0, 0.0),
            (10_000.0, -2.5),
        ] {
            let tone = sine(frequency, 0.5, 2.0);
            let spectrum = Spectrum::of(&tone, RATE);
            let weighted = spectrum.mean_square(0.0, 24_000.0, a_weighting);
            let flat = spectrum.mean_square(0.0, 24_000.0, |_| 1.0);
            let difference = power_decibels(weighted) - power_decibels(flat);
            assert!(
                (difference - expected).abs() <= 0.2,
                "{frequency} Hz: {difference:.2} dB, expected {expected}"
            );
        }
    }

    /// LC − LZ: −3.0, −0.3, 0.0, −4.4 dB.
    #[test]
    fn c_weighting_matches_the_standard() {
        for (frequency, expected) in [(31.5, -3.0), (100.0, -0.3), (1000.0, 0.0), (10_000.0, -4.4)]
        {
            let tone = sine(frequency, 0.5, 2.0);
            let spectrum = Spectrum::of(&tone, RATE);
            let weighted = spectrum.mean_square(0.0, 24_000.0, c_weighting);
            let flat = spectrum.mean_square(0.0, 24_000.0, |_| 1.0);
            let difference = power_decibels(weighted) - power_decibels(flat);
            assert!(
                (difference - expected).abs() <= 0.2,
                "{frequency} Hz: {difference:.2} dB, expected {expected}"
            );
        }
    }

    /// IEC 61672-1's toneburst response: a 4 kHz burst of T seconds reads
    /// LAFmax 10·log10(1 − e^(−T/0.125)) below the steady sine: −1.0, −4.8
    /// and −18.0 dB for 200, 50 and 2 ms. Measured through the A-weighting
    /// and FAST as the report does.
    #[test]
    fn fast_toneburst_response() {
        let laf_max = |samples: &[f32]| {
            let weighted = Spectrum::of(samples, RATE).filtered(a_weighting);
            let power: Vec<f64> = weighted.iter().map(|s| (*s as f64).powi(2)).collect();
            power_decibels(fast(&power, RATE).maximum)
        };
        let steady = laf_max(&sine(4000.0, 0.5, 2.0));
        for (seconds, expected) in [(0.2, -1.0), (0.05, -4.8), (0.002, -18.0)] {
            let mut burst = vec![0.0f32; RATE as usize];
            burst.extend(sine(4000.0, 0.5, seconds));
            burst.extend(vec![0.0f32; RATE as usize]);
            let difference = laf_max(&burst) - steady;
            assert!(
                (difference - expected).abs() <= 0.1,
                "{seconds} s: {difference:.2} dB, expected {expected}"
            );
        }
    }
}
