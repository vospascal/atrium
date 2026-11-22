//! IEC 61672-1's A-weighting for a stream: the analytic curve as a
//! linear-phase FIR, applied by overlap-save, its delay taken back out.
//!
//! The whole-file analyser weights by a gain on one transform of the whole
//! file, exact and zero-phase. A stream has no whole file, and the usual IIR
//! filter made by the bilinear transform is a decibel low at 10 kHz (see the
//! crate README). The curve's own zero-phase impulse response, cut to
//! ±[`DELAY_SECONDS`], is the same filter seen through a window: its lowest
//! poles (20.6 Hz, double) have decayed to under half a percent by then, so
//! the FIR holds the curve to a few hundredths of a decibel from 31.5 Hz up.
//! Reading each span [`DELAY_SECONDS`] late makes it zero-phase again, so a
//! column's level is the whole-file analyser's level of the same 50 ms.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::weighting::a_weighting;

/// Half the FIR's length (s): how far before and after a sample the
/// weighting reaches, and how late a weighted span can be read.
const DELAY_SECONDS: f64 = 0.0427;

/// The FIR's design transform is this many times its length, so the curve is
/// sampled finely enough that the cut impulse response is the curve's.
const DESIGN_OVERSAMPLING: usize = 16;

pub(super) struct AWeighting {
    /// Taps either side of the centre.
    delay: usize,
    /// Weighted samples per call.
    span: usize,
    /// The FIR's transform at the overlap-save length.
    response: Vec<Complex<f64>>,
    forward: Arc<dyn RealToComplex<f64>>,
    inverse: Arc<dyn ComplexToReal<f64>>,
    input: Vec<f64>,
    spectrum: Vec<Complex<f64>>,
    output: Vec<f64>,
}

impl AWeighting {
    /// Weighting `span` samples a call at `sample_rate`.
    pub(super) fn new(sample_rate: u32, span: usize) -> Self {
        let delay = (DELAY_SECONDS * sample_rate as f64).round() as usize;
        let taps = 2 * delay + 1;
        let impulse = zero_phase_impulse(sample_rate, taps);
        let size = (span + taps - 1).next_power_of_two();
        let mut planner = RealFftPlanner::<f64>::new();
        let forward = planner.plan_fft_forward(size);
        let inverse = planner.plan_fft_inverse(size);
        // Causal: tap k is the impulse response at k − delay.
        let mut input = forward.make_input_vec();
        input[..taps].copy_from_slice(&impulse);
        let mut response = forward.make_output_vec();
        forward
            .process(&mut input, &mut response)
            .expect("buffers are the planner's own");
        Self {
            delay,
            span,
            response,
            input,
            spectrum: forward.make_output_vec(),
            output: inverse.make_output_vec(),
            forward,
            inverse,
        }
    }

    /// How many samples after a span the input must reach before the span
    /// can be weighted.
    pub(super) fn delay(&self) -> usize {
        self.delay
    }

    /// The A-weighted samples `start .. start + span` into `weighted`, from
    /// `sample(index)`, the input at `index` (zero before the stream began),
    /// which is read from `start − delay` to `start + span + delay`.
    pub(super) fn weigh(&mut self, sample: impl Fn(i64) -> f32, start: i64, weighted: &mut [f64]) {
        let taps = 2 * self.delay + 1;
        let first = start - self.delay as i64;
        let last = start + (self.span + self.delay) as i64;
        for (offset, slot) in self.input.iter_mut().enumerate() {
            let index = first + offset as i64;
            *slot = if index < last {
                sample(index) as f64
            } else {
                0.0
            };
        }
        self.forward
            .process(&mut self.input, &mut self.spectrum)
            .expect("buffers are the planner's own");
        for (bin, response) in self.spectrum.iter_mut().zip(&self.response) {
            *bin *= response;
        }
        let size = self.output.len();
        self.spectrum[0].im = 0.0;
        self.spectrum[size / 2].im = 0.0;
        self.inverse
            .process(&mut self.spectrum, &mut self.output)
            .expect("DC and Nyquist are real");
        // Overlap-save: the first taps − 1 outputs wrapped round; the rest are
        // the causal FIR's, `delay` late, which the segment's start undid.
        let scale = 1.0 / size as f64;
        for (slot, value) in weighted
            .iter_mut()
            .zip(&self.output[taps - 1..taps - 1 + self.span])
        {
            *slot = value * scale;
        }
    }
}

/// The A-weighting's zero-phase impulse response at `sample_rate`, `taps`
/// long (odd) and centred: the curve sampled on a fine grid of frequencies,
/// transformed back, cut to the taps.
fn zero_phase_impulse(sample_rate: u32, taps: usize) -> Vec<f64> {
    let size = (taps * DESIGN_OVERSAMPLING).next_power_of_two();
    let mut planner = RealFftPlanner::<f64>::new();
    let inverse = planner.plan_fft_inverse(size);
    let spacing = sample_rate as f64 / size as f64;
    let mut bins = inverse.make_input_vec();
    for (index, bin) in bins.iter_mut().enumerate() {
        *bin = Complex::new(a_weighting(index as f64 * spacing), 0.0);
    }
    let mut response = inverse.make_output_vec();
    inverse
        .process(&mut bins, &mut response)
        .expect("a real curve has real ends");
    let half = taps / 2;
    (0..taps)
        .map(|tap| {
            let lag = tap as isize - half as isize;
            response[lag.rem_euclid(size as isize) as usize] / size as f64
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::spectrum::Spectrum;
    use crate::statistics::power_decibels;
    use crate::test_signals::{sine, Noise, RATE};

    /// The streamed weighting of `samples`, 2400 at a time.
    fn streamed(samples: &[f32]) -> Vec<f64> {
        let span = 2400;
        let mut weighting = AWeighting::new(RATE, span);
        let mut weighted = vec![0.0; samples.len() / span * span];
        for (index, chunk) in weighted.chunks_mut(span).enumerate() {
            let sample = |at: i64| {
                if at >= 0 && (at as usize) < samples.len() {
                    samples[at as usize]
                } else {
                    0.0
                }
            };
            weighting.weigh(sample, (index * span) as i64, chunk);
        }
        weighted
    }

    fn mean_square(values: &[f64]) -> f64 {
        values.iter().map(|value| value * value).sum::<f64>() / values.len() as f64
    }

    /// LA − LZ of a sine through the FIR at the IEC 61672-1 table
    /// frequencies, −39.4, −19.1, 0.0 and −2.5 dB (as the whole-file
    /// weighting's test), and against the analytic curve within 0.05 dB.
    #[test]
    fn the_fir_holds_the_curve() {
        for (frequency, expected) in [
            (31.5, -39.4),
            (100.0, -19.1),
            (1000.0, 0.0),
            (10_000.0, -2.5),
        ] {
            let tone = sine(frequency, 0.5, 3.0);
            let weighted = streamed(&tone);
            // The middle second, clear of the start.
            let middle = &weighted[RATE as usize..2 * RATE as usize];
            let difference = power_decibels(mean_square(middle)) - power_decibels(0.125);
            assert!(
                (difference - expected).abs() <= 0.2,
                "{frequency} Hz: {difference:.3} dB, expected {expected}"
            );
            let analytic = 20.0 * a_weighting(frequency).log10();
            assert!(
                (difference - analytic).abs() <= 0.05,
                "{frequency} Hz: {difference:.3} dB, the curve {analytic:.3}"
            );
        }
    }

    /// Sample for sample, the stream's weighting of noise is the whole-file
    /// weighting's, but for the FIR's cut tail: within 0.5 % of the RMS.
    #[test]
    fn streamed_is_the_whole_file_weighting() {
        let noise = Noise::new(21).white(0.1, 4.0);
        let whole = Spectrum::of(&noise, RATE).filtered(a_weighting);
        let weighted = streamed(&noise);
        let error: Vec<f64> = weighted
            .iter()
            .zip(&whole)
            .skip(RATE as usize / 2)
            .take(3 * RATE as usize)
            .map(|(streamed, whole)| streamed - *whole as f64)
            .collect();
        let relative = (mean_square(&error)
            / mean_square(
                &whole[RATE as usize / 2..]
                    .iter()
                    .map(|value| *value as f64)
                    .collect::<Vec<_>>(),
            ))
        .sqrt();
        assert!(relative < 0.005, "relative error {relative}");
    }
}
