//! A filter from a magnitude alone: the minimum-phase response with that
//! magnitude, and the way back from a spectrum to a response.
//!
//! Both the ambient sound's ear filters and the headphone equaliser are
//! specified by a magnitude per frequency (a measured power, or its
//! inverse), and both are realised the same way.

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

/// The minimum-phase spectrum whose power is `power`, by the real cepstrum
/// (Oppenheim and Schafer, "Discrete-Time Signal Processing", ch. 12): the
/// most compact response in time of all with that magnitude.
pub(super) fn minimum_phase_spectrum(power: &[f64], grid_size: usize) -> Vec<Complex<f64>> {
    let mut planner = RealFftPlanner::<f64>::new();
    let inverse = planner.plan_fft_inverse(grid_size);
    let forward = planner.plan_fft_forward(grid_size);
    let floor = power.iter().fold(0f64, |peak, &value| peak.max(value)) * 1e-10;
    let mut log_magnitude: Vec<Complex<f64>> = power
        .iter()
        .map(|&value| Complex::new(0.5 * value.max(floor).ln(), 0.0))
        .collect();
    let mut cepstrum = inverse.make_output_vec();
    inverse
        .process(&mut log_magnitude, &mut cepstrum)
        .expect("inverse transform of a log magnitude");
    let half = grid_size / 2;
    for (index, value) in cepstrum.iter_mut().enumerate() {
        let fold = match index {
            0 => 1.0,
            index if index < half => 2.0,
            index if index == half => 1.0,
            _ => 0.0,
        };
        *value *= fold / grid_size as f64;
    }
    let mut folded = forward.make_output_vec();
    forward
        .process(&mut cepstrum, &mut folded)
        .expect("forward transform of a folded cepstrum");
    folded.iter().map(|bin| bin.exp()).collect()
}

/// The real response whose spectrum is `spectrum`, on a grid `grid_size`.
pub(super) fn to_time(mut spectrum: Vec<Complex<f64>>, grid_size: usize) -> Vec<f64> {
    let mut planner = RealFftPlanner::<f64>::new();
    let inverse = planner.plan_fft_inverse(grid_size);
    spectrum[0].im = 0.0;
    let last = spectrum.len() - 1;
    spectrum[last].im = 0.0;
    let mut response = inverse.make_output_vec();
    inverse
        .process(&mut spectrum, &mut response)
        .expect("inverse transform of an ear filter");
    for sample in &mut response {
        *sample /= grid_size as f64;
    }
    response
}

/// `response` cut to `length` samples, its last `fade_length` faded out
/// with a raised cosine so it ends without a step, in single precision.
pub(super) fn truncated_with_fade_out(
    mut response: Vec<f64>,
    length: usize,
    fade_length: usize,
) -> Vec<f32> {
    response.truncate(length);
    for (index, sample) in response[length - fade_length..].iter_mut().enumerate() {
        let progress = (index as f64 + 0.5) / fade_length as f64;
        *sample *= 0.5 * (1.0 + (std::f64::consts::PI * progress).cos());
    }
    response.iter().map(|&sample| sample as f32).collect()
}
