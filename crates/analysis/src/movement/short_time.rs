//! Power spectra of Hann-windowed frames, handed one at a time to whoever
//! accumulates them, so no measure keeps every frame of every channel.

use realfft::RealFftPlanner;

/// Call `each` with the power spectrum |X|² (positive frequencies, bin `k` at
/// `k·sample_rate/window` Hz) of every whole frame of `window` samples,
/// `hop` apart. Nothing is called for a signal shorter than one frame.
pub(crate) fn for_each_frame(
    samples: &[f32],
    window: usize,
    hop: usize,
    mut each: impl FnMut(usize, &[f64]),
) {
    if window < 2 || hop == 0 || samples.len() < window {
        return;
    }
    let taper: Vec<f64> = (0..window)
        .map(|index| 0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / window as f64).cos())
        .collect();
    let mut planner = RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(window);
    let mut input = forward.make_input_vec();
    let mut output = forward.make_output_vec();
    let mut power = vec![0.0f64; output.len()];
    let frames = (samples.len() - window) / hop + 1;
    for frame in 0..frames {
        let start = frame * hop;
        for ((slot, sample), weight) in input
            .iter_mut()
            .zip(&samples[start..start + window])
            .zip(&taper)
        {
            *slot = *sample as f64 * weight;
        }
        forward
            .process(&mut input, &mut output)
            .expect("buffers are the planner's own");
        for (slot, bin) in power.iter_mut().zip(&output) {
            *slot = bin.norm_sqr();
        }
        each(frame, &power);
    }
}

/// How many frames [`for_each_frame`] gives.
pub(crate) fn frame_count(length: usize, window: usize, hop: usize) -> usize {
    if window < 2 || hop == 0 || length < window {
        0
    } else {
        (length - window) / hop + 1
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::{sine, RATE};

    /// A 1 kHz sine in 0.1 s frames (10 Hz bins) peaks in bin 100 of every
    /// frame, and the count matches.
    #[test]
    fn frames_of_a_sine_peak_at_its_bin() {
        let tone = sine(1000.0, 0.5, 1.0);
        let window = RATE as usize / 10;
        let mut seen = 0;
        for_each_frame(&tone, window, window / 2, |_, power| {
            let peak = power
                .iter()
                .enumerate()
                .max_by(|a, b| a.1.total_cmp(b.1))
                .map(|(index, _)| index);
            assert_eq!(peak, Some(100));
            seen += 1;
        });
        assert_eq!(seen, frame_count(tone.len(), window, window / 2));
        assert_eq!(seen, 19);
    }
}
