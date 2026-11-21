//! What the two ears of the measured head receive, on average, from sound
//! arriving from every direction at once.
//!
//! A diffuse field is mutually incoherent sound of equal strength from all
//! directions. At the ears it is described by three spectra: each ear's
//! power, the sum over directions of |H_ear|², and the cross-spectrum of the
//! two ears, the sum of H_left · H_right*, each direction weighted by the
//! solid angle it covers. The cross-spectrum over the two powers is the
//! diffuse-field interaural coherence of this head.
//!
//! The SOFA reader does not list its measurement positions, so the solid
//! angles are found by asking: directions spread evenly over the sphere (a
//! Fibonacci lattice) are each given the nearest measured response, and a
//! measured direction weighs as many as are nearest to it. That is its
//! spherical Voronoi cell, the solid angle it represents. Below the lowest
//! measured elevation the nearest is the lowest ring, which is also what the
//! renderer plays for a point source down there.
//!
//! Nearest, not interpolated: the reader's interpolation blends neighbouring
//! responses sample by sample, and blending responses that arrive at
//! slightly different times dulls the treble. Averaged over the sphere it
//! takes 1.8 dB from the 8 kHz third octave and 2.6 dB from 12.5 kHz of the
//! shipped set; the measured responses themselves lose nothing.

use std::collections::HashMap;
use std::hash::{Hash, Hasher};

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;
use sofar::reader::{Filter, Sofar};

/// Directions of the Fibonacci lattice. The shipped set has 710 measured
/// directions, so each gets about three lattice points; that already puts
/// the ears' diffuse power within 0.01 dB of the exact Voronoi weighting in
/// every third octave from 100 Hz to 16 kHz, and the coherence within 0.002.
const LATTICE_DIRECTIONS: usize = 2048;

/// The three diffuse-field spectra of a head on one frequency grid: bin `k`
/// is `k · sample_rate / fft_size` Hz, from 0 to half the sample rate.
pub(in crate::pipeline::renderers::binaural) struct DiffuseStatistics {
    /// Each ear's power, the solid-angle weighted mean of |H|² (weights sum
    /// to one, so it is on the same scale as one measured response).
    pub(in crate::pipeline::renderers::binaural) left_power: Vec<f64>,
    pub(in crate::pipeline::renderers::binaural) right_power: Vec<f64>,
    /// The real part of the weighted mean of H_left · H_right*. For a head
    /// symmetric left to right it is the whole of it.
    pub(in crate::pipeline::renderers::binaural) cross_power: Vec<f64>,
}

/// One measured direction and how many lattice points it is nearest to.
struct MeasuredDirection {
    left: Vec<f32>,
    right: Vec<f32>,
    left_delay_seconds: f32,
    right_delay_seconds: f32,
    nearest_to: usize,
}

impl DiffuseStatistics {
    /// The statistics of the responses in `sofa`, on a grid of `fft_size`
    /// bins at `sample_rate`. Called when the HRIRs are loaded, never on the
    /// audio thread.
    pub(in crate::pipeline::renderers::binaural) fn measure(
        sofa: &Sofar,
        sample_rate: f32,
        fft_size: usize,
    ) -> Self {
        let directions = measured_directions(sofa);
        let filter_length = sofa.filter_len();
        // A power or cross spectrum of a response L samples long is the
        // transform of a correlation 2L − 1 samples long (and as much again
        // as the ears' delays differ). Sampled on a grid at least that long
        // it is known exactly between the bins, so it is summed on a short
        // grid and carried to the long one exactly.
        let largest_interaural_delay = directions
            .iter()
            .map(|direction| {
                ((direction.left_delay_seconds - direction.right_delay_seconds).abs() * sample_rate)
                    .ceil() as usize
            })
            .max()
            .unwrap_or(0);
        let short_size = (2 * (filter_length + largest_interaural_delay)).next_power_of_two();
        let short_bins = short_size / 2 + 1;
        let mut planner = RealFftPlanner::<f64>::new();
        let forward = planner.plan_fft_forward(short_size);
        let mut left_power = vec![0.0f64; short_bins];
        let mut right_power = vec![0.0f64; short_bins];
        let mut cross = vec![Complex::new(0.0f64, 0.0); short_bins];
        let mut time = forward.make_input_vec();
        let mut left_spectrum = forward.make_output_vec();
        let mut right_spectrum = forward.make_output_vec();
        for direction in &directions {
            let weight = direction.nearest_to as f64 / LATTICE_DIRECTIONS as f64;
            for (response, spectrum) in [
                (&direction.left, &mut left_spectrum),
                (&direction.right, &mut right_spectrum),
            ] {
                time.fill(0.0);
                for (slot, &sample) in time.iter_mut().zip(response.iter()) {
                    *slot = sample as f64;
                }
                forward
                    .process(&mut time, spectrum)
                    .expect("forward transform of an HRIR");
            }
            // The set's own per-ear delays, which the responses leave out
            // (the shipped set has none: they are in its responses).
            let interaural_delay =
                (direction.left_delay_seconds - direction.right_delay_seconds) as f64;
            for bin in 0..short_bins {
                let mut product = left_spectrum[bin] * right_spectrum[bin].conj();
                if interaural_delay != 0.0 {
                    let frequency = bin as f64 * sample_rate as f64 / short_size as f64;
                    product *= Complex::from_polar(
                        1.0,
                        -std::f64::consts::TAU * frequency * interaural_delay,
                    );
                }
                left_power[bin] += weight * left_spectrum[bin].norm_sqr();
                right_power[bin] += weight * right_spectrum[bin].norm_sqr();
                cross[bin] += weight * product;
            }
        }
        Self {
            left_power: on_long_grid(&real_spectrum(&left_power), short_size, fft_size)
                .iter()
                .map(|bin| bin.re)
                .collect(),
            right_power: on_long_grid(&real_spectrum(&right_power), short_size, fft_size)
                .iter()
                .map(|bin| bin.re)
                .collect(),
            cross_power: on_long_grid(&cross, short_size, fft_size)
                .iter()
                .map(|bin| bin.re)
                .collect(),
        }
    }
}

fn real_spectrum(values: &[f64]) -> Vec<Complex<f64>> {
    values
        .iter()
        .map(|&value| Complex::new(value, 0.0))
        .collect()
}

/// The spectrum `short` (of a real sequence `short_size` long) on a grid of
/// `long_size` bins: back to the correlation, zero-padded between its
/// positive and negative lags, and forward again.
fn on_long_grid(short: &[Complex<f64>], short_size: usize, long_size: usize) -> Vec<Complex<f64>> {
    let mut planner = RealFftPlanner::<f64>::new();
    let inverse = planner.plan_fft_inverse(short_size);
    let forward = planner.plan_fft_forward(long_size);
    let mut spectrum = short.to_vec();
    // A real sequence's spectrum is real at 0 Hz and at half the rate.
    spectrum[0].im = 0.0;
    let last = spectrum.len() - 1;
    spectrum[last].im = 0.0;
    let mut correlation = inverse.make_output_vec();
    inverse
        .process(&mut spectrum, &mut correlation)
        .expect("inverse transform of a diffuse spectrum");
    let mut padded = forward.make_input_vec();
    let half = short_size / 2;
    for lag in 0..half {
        padded[lag] = correlation[lag] / short_size as f64;
    }
    for lag in half..short_size {
        padded[long_size - short_size + lag] = correlation[lag] / short_size as f64;
    }
    let mut long = forward.make_output_vec();
    forward
        .process(&mut padded, &mut long)
        .expect("forward transform of a diffuse correlation");
    long
}

/// Every measured direction nearest to at least one lattice point, with the
/// number of points it is nearest to. Two lookups name the same measured
/// direction exactly when they return the same response, bit for bit; a
/// few samples spread over the response find the candidates, and the whole
/// response decides.
fn measured_directions(sofa: &Sofar) -> Vec<MeasuredDirection> {
    let mut filter = Filter::new(sofa.filter_len());
    let mut found: Vec<MeasuredDirection> = Vec::new();
    let mut candidates: HashMap<u64, Vec<usize>> = HashMap::new();
    let probe_step = (sofa.filter_len() / 16).max(1);
    let golden_angle = std::f64::consts::PI * (3.0 - 5f64.sqrt());
    for point in 0..LATTICE_DIRECTIONS {
        let up = 1.0 - (2.0 * point as f64 + 1.0) / LATTICE_DIRECTIONS as f64;
        let across = (1.0 - up * up).sqrt();
        let azimuth = point as f64 * golden_angle;
        sofa.filter_nointerp(
            (across * azimuth.cos()) as f32,
            (across * azimuth.sin()) as f32,
            up as f32,
            &mut filter,
        );
        let mut hasher = std::collections::hash_map::DefaultHasher::new();
        for response in [&filter.left, &filter.right] {
            for sample in response.iter().step_by(probe_step) {
                sample.to_bits().hash(&mut hasher);
            }
        }
        let bucket = candidates.entry(hasher.finish()).or_default();
        let same = bucket.iter().copied().find(|&index| {
            let direction = &found[index];
            direction.left[..] == filter.left[..]
                && direction.right[..] == filter.right[..]
                && direction.left_delay_seconds == filter.ldelay
                && direction.right_delay_seconds == filter.rdelay
        });
        match same {
            Some(index) => found[index].nearest_to += 1,
            None => {
                bucket.push(found.len());
                found.push(MeasuredDirection {
                    left: filter.left.to_vec(),
                    right: filter.right.to_vec(),
                    left_delay_seconds: filter.ldelay,
                    right_delay_seconds: filter.rdelay,
                    nearest_to: 1,
                });
            }
        }
    }
    found
}
