//! Onsets, found as librosa's `onset_detect` finds them with its defaults
//! (librosa 0.10, ISC licence; the method is followed here, no code is
//! copied): spectral flux on a log-power mel spectrogram, then a peak picker
//! with a moving maximum, a moving mean and a refractory wait.
//!
//! - Spectrogram: 2048-point Hann frames every 512 samples, centred (the
//!   signal padded with 1024 zeros at each end); power; 128 mel bands
//!   (Slaney's mel scale and area normalisation) up to Nyquist; to decibels,
//!   floored 80 dB below the loudest cell.
//! - Flux: per frame the mean over bands of the rise since the frame before,
//!   rises only; shifted three frames later (librosa's `lag + n_fft //
//!   (2·hop)`), then scaled to run from 0 to 1.
//! - Peaks: a frame is an onset when it is the largest of itself and the
//!   `0.03·rate // hop` frames before it, at least 0.07 above the mean of the
//!   `0.1·rate // hop` frames before to as many after, and more than
//!   `0.03·rate // hop` frames after the last onset.
//!
//! Two departures. The decibels are taken relative to the loudest cell
//! rather than to 1. The flux is a difference of decibels, so the two give
//! the same onsets wherever librosa's 10⁻¹⁰ power floor does not bite; here
//! a render at the engine's quiet level and the same render normalised give
//! the same onsets.
//!
//! And a peak must also rise at least [`MINIMUM_RISE_DECIBELS`] above the
//! median rise of the second around it. librosa scales the flux to run from
//! 0 to 1, so in steady noise its own frame-to-frame wobble is picked as
//! onsets: the meadow's wind alone gave 67 a minute. In decibels, 99.9 % of
//! that wind's frames rise by at most 0.5 dB more than the median, while a
//! bird's syllable starting stands 2 to 8 dB above it. The threshold is ours,
//! set from those measurements, not from a publication.

use std::sync::Arc;

use realfft::{RealFftPlanner, RealToComplex};

pub(crate) const FFT_SIZE: usize = 2048;
pub(crate) const HOP: usize = 512;
pub(crate) const MEL_BANDS: usize = 128;
const TOP_DECIBELS: f64 = 80.0;
/// librosa's `lag + n_fft // (2·hop)`.
pub(crate) const FLUX_DELAY_FRAMES: usize = 1 + FFT_SIZE / (2 * HOP);
pub(crate) const DELTA: f64 = 0.07;
/// How far above the median rise of the surrounding second a peak's rise
/// (mean over bands, dB a frame) must stand to count as something starting.
const MINIMUM_RISE_DECIBELS: f64 = 0.75;
/// The span the median rise is taken over (s), centred on the frame.
const MEDIAN_SPAN_SECONDS: f64 = 1.0;
/// The first frame whose flux compares two frames that hold no padding from
/// before the file's start: frame t covers samples from t·hop − n_fft/2.
pub(crate) const FIRST_WHOLE_FLUX_FRAME: usize = FLUX_DELAY_FRAMES + FFT_SIZE / (2 * HOP);

/// The onset times (s) in `channels`, their mel powers averaged.
pub(crate) fn onsets(channels: &[Vec<f32>], sample_rate: u32) -> Vec<f64> {
    let Some(length) = channels.first().map(Vec::len) else {
        return Vec::new();
    };
    let frames = 1 + length / HOP;
    let mut analyser = MelFrames::new(sample_rate);
    let mut mel = vec![vec![0.0f64; MEL_BANDS]; frames];
    for channel in channels {
        for (frame, bands) in mel.iter_mut().enumerate() {
            // Centred: frame t covers samples t·hop − 1024 to t·hop + 1023.
            analyser.add(
                |offset| {
                    let position = (frame * HOP + offset) as isize - (FFT_SIZE / 2) as isize;
                    if position >= 0 && (position as usize) < length {
                        channel[position as usize]
                    } else {
                        0.0
                    }
                },
                channels.len(),
                bands,
            );
        }
    }

    let loudest = mel.iter().flatten().fold(0.0f64, |a, b| a.max(*b));
    if loudest <= 0.0 {
        return Vec::new();
    }
    let decibels: Vec<Vec<f64>> = mel
        .iter()
        .map(|bands| {
            bands
                .iter()
                .map(|value| decibels(*value, loudest))
                .collect()
        })
        .collect();
    let mut envelope = vec![0.0f64; frames];
    for frame in FLUX_DELAY_FRAMES..frames {
        envelope[frame] = rise(
            &decibels[frame - FLUX_DELAY_FRAMES],
            &decibels[frame - FLUX_DELAY_FRAMES + 1],
        );
    }
    // Rises standing clear of the steady wobble, in decibels, before the
    // envelope is scaled.
    let clear = standing_clear(&envelope, frames_in(MEDIAN_SPAN_SECONDS, sample_rate) / 2);
    // librosa's last flux value is cut; the envelope keeps one per frame.
    let lowest = envelope.iter().copied().fold(f64::INFINITY, f64::min);
    envelope.iter_mut().for_each(|value| *value -= lowest);
    let highest = envelope.iter().copied().fold(0.0f64, f64::max);
    if highest <= 0.0 {
        return Vec::new();
    }
    envelope.iter_mut().for_each(|value| *value /= highest);

    let windows = PeakWindows::new(sample_rate);
    pick_peaks(
        &envelope,
        windows.before_maximum,
        windows.after_maximum,
        windows.before_mean,
        windows.after_mean,
        windows.wait,
    )
    .into_iter()
    .filter(|frame| clear[*frame])
    .map(|frame| frame as f64 * HOP as f64 / sample_rate as f64)
    .collect()
}

/// Whole frames of `seconds` at `sample_rate`, as librosa rounds them.
pub(crate) fn frames_in(seconds: f64, sample_rate: u32) -> usize {
    (seconds * sample_rate as f64 / HOP as f64).floor() as usize
}

/// The frames the live median rise looks back over: the past second.
pub(crate) fn median_span_frames(sample_rate: u32) -> usize {
    frames_in(MEDIAN_SPAN_SECONDS, sample_rate)
}

/// The mel spectrogram one frame at a time: each channel's 2048-point Hann
/// frame transformed and its mel powers added, `1 / channels` of each.
pub(crate) struct MelFrames {
    filters: Vec<(usize, Vec<f64>)>,
    window: Vec<f64>,
    forward: Arc<dyn RealToComplex<f64>>,
    input: Vec<f64>,
    spectrum: Vec<realfft::num_complex::Complex<f64>>,
    power: Vec<f64>,
}

impl MelFrames {
    pub(crate) fn new(sample_rate: u32) -> Self {
        let mut planner = RealFftPlanner::<f64>::new();
        let forward = planner.plan_fft_forward(FFT_SIZE);
        let spectrum = forward.make_output_vec();
        Self {
            filters: mel_filters(sample_rate),
            window: (0..FFT_SIZE)
                .map(|index| {
                    0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / FFT_SIZE as f64).cos()
                })
                .collect(),
            input: forward.make_input_vec(),
            power: vec![0.0f64; spectrum.len()],
            spectrum,
            forward,
        }
    }

    /// Add one channel's frame to `bands`: `sample(offset)` is the frame's
    /// sample at `offset` (0 to 2047), zero outside the signal.
    pub(crate) fn add(
        &mut self,
        sample: impl Fn(usize) -> f32,
        channels: usize,
        bands: &mut [f64],
    ) {
        for (offset, value) in self.input.iter_mut().enumerate() {
            *value = sample(offset) as f64 * self.window[offset];
        }
        self.forward
            .process(&mut self.input, &mut self.spectrum)
            .expect("buffers are the planner's own");
        for (slot, bin) in self.power.iter_mut().zip(&self.spectrum) {
            *slot = bin.norm_sqr();
        }
        for (band, (start, weights)) in bands.iter_mut().zip(&self.filters) {
            *band += weights
                .iter()
                .zip(&self.power[*start..])
                .map(|(weight, value)| weight * value)
                .sum::<f64>()
                / channels as f64;
        }
    }
}

/// A mel power in decibels relative to the loudest cell, floored
/// [`TOP_DECIBELS`] below it.
pub(crate) fn decibels(value: f64, loudest: f64) -> f64 {
    (10.0 * (value / loudest).log10()).max(-TOP_DECIBELS)
}

/// The flux from one frame of mel decibels to the next: the mean over bands
/// of the rises, rises only.
pub(crate) fn rise(earlier: &[f64], later: &[f64]) -> f64 {
    earlier
        .iter()
        .zip(later)
        .map(|(earlier, later)| (later - earlier).max(0.0))
        .sum::<f64>()
        / MEL_BANDS as f64
}

/// Whether each frame's rise stands at least [`MINIMUM_RISE_DECIBELS`] above
/// the median rise of the frames up to `half_span` before and after it, and
/// compares no padding from before the file's start.
fn standing_clear(rises: &[f64], half_span: usize) -> Vec<bool> {
    let mut window = Vec::with_capacity(2 * half_span + 1);
    (0..rises.len())
        .map(|frame| {
            frame >= FIRST_WHOLE_FLUX_FRAME
                && stands_clear(
                    rises[frame],
                    &rises
                        [frame.saturating_sub(half_span)..(frame + half_span + 1).min(rises.len())],
                    &mut window,
                )
        })
        .collect()
}

/// Whether `rise` stands at least [`MINIMUM_RISE_DECIBELS`] above the median
/// of `neighbourhood`; `scratch` holds the sorted copy.
pub(crate) fn stands_clear(rise: f64, neighbourhood: &[f64], scratch: &mut Vec<f64>) -> bool {
    scratch.clear();
    scratch.extend_from_slice(neighbourhood);
    scratch.sort_by(f64::total_cmp);
    rise - scratch[scratch.len() / 2] >= MINIMUM_RISE_DECIBELS
}

/// librosa's `peak_pick` windows (frames) at a sample rate: the moving
/// maximum over 30 ms before, the moving mean over 100 ms either side, and a
/// 30 ms wait after a peak.
pub(crate) struct PeakWindows {
    pub(crate) before_maximum: usize,
    pub(crate) after_maximum: usize,
    pub(crate) before_mean: usize,
    pub(crate) after_mean: usize,
    pub(crate) wait: usize,
}

impl PeakWindows {
    pub(crate) fn new(sample_rate: u32) -> Self {
        Self {
            before_maximum: frames_in(0.03, sample_rate),
            after_maximum: 1,
            before_mean: frames_in(0.10, sample_rate),
            after_mean: frames_in(0.10, sample_rate) + 1,
            wait: frames_in(0.03, sample_rate),
        }
    }
}

/// librosa's greedy `peak_pick`: n is a peak when it passes [`is_peak`], and
/// the next candidate after a peak is `wait` + 1 later.
fn pick_peaks(
    values: &[f64],
    before_maximum: usize,
    after_maximum: usize,
    before_mean: usize,
    after_mean: usize,
    wait: usize,
) -> Vec<usize> {
    let windows = PeakWindows {
        before_maximum,
        after_maximum,
        before_mean,
        after_mean,
        wait,
    };
    let mut peaks = Vec::new();
    let mut index = 0;
    while index < values.len() {
        if is_peak(values, index, &windows, DELTA) {
            peaks.push(index);
            index += windows.wait + 1;
        } else {
            index += 1;
        }
    }
    peaks
}

/// Whether x[n] is the largest of x[n − before_maximum ..= n + after_maximum
/// − 1] and at least `delta` above the mean of x[n − before_mean ..= n +
/// after_mean − 1], both clipped to the series.
pub(crate) fn is_peak(values: &[f64], index: usize, windows: &PeakWindows, delta: f64) -> bool {
    let window = |before: usize, after: usize| {
        let start = index.saturating_sub(before);
        let end = (index + after).min(values.len());
        &values[start..end]
    };
    let largest = window(windows.before_maximum, windows.after_maximum)
        .iter()
        .copied()
        .fold(f64::NEG_INFINITY, f64::max);
    let around = window(windows.before_mean, windows.after_mean);
    let mean = around.iter().sum::<f64>() / around.len() as f64;
    values[index] == largest && values[index] >= mean + delta
}

/// Slaney's mel scale (librosa's default, `htk=False`): linear below 1 kHz,
/// logarithmic above.
fn hertz_to_mel(frequency: f64) -> f64 {
    let linear_step = 200.0 / 3.0;
    let log_step = 6.4f64.ln() / 27.0;
    if frequency >= 1000.0 {
        1000.0 / linear_step + (frequency / 1000.0).ln() / log_step
    } else {
        frequency / linear_step
    }
}

fn mel_to_hertz(mel: f64) -> f64 {
    let linear_step = 200.0 / 3.0;
    let log_step = 6.4f64.ln() / 27.0;
    let break_mel = 1000.0 / linear_step;
    if mel >= break_mel {
        1000.0 * ((mel - break_mel) * log_step).exp()
    } else {
        mel * linear_step
    }
}

/// The 128 triangular mel filters from 0 Hz to Nyquist with Slaney's area
/// normalisation, each as its first bin and its weights from there.
fn mel_filters(sample_rate: u32) -> Vec<(usize, Vec<f64>)> {
    let nyquist = sample_rate as f64 / 2.0;
    let top = hertz_to_mel(nyquist);
    let edges: Vec<f64> = (0..MEL_BANDS + 2)
        .map(|index| mel_to_hertz(top * index as f64 / (MEL_BANDS + 1) as f64))
        .collect();
    let bins = FFT_SIZE / 2 + 1;
    let bin_hertz = |bin: usize| bin as f64 * sample_rate as f64 / FFT_SIZE as f64;
    (0..MEL_BANDS)
        .map(|band| {
            let (low, centre, high) = (edges[band], edges[band + 1], edges[band + 2]);
            let normalisation = 2.0 / (high - low);
            let weights: Vec<f64> = (0..bins)
                .map(|bin| {
                    let frequency = bin_hertz(bin);
                    let rising = (frequency - low) / (centre - low);
                    let falling = (high - frequency) / (high - centre);
                    rising.min(falling).max(0.0) * normalisation
                })
                .collect();
            let first = weights.iter().position(|weight| *weight > 0.0).unwrap_or(0);
            let last = weights
                .iter()
                .rposition(|weight| *weight > 0.0)
                .unwrap_or(0);
            (first, weights[first..=last.max(first)].to_vec())
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::{Noise, RATE};

    /// 5 ms bursts at RMS 0.5 every 0.5 s in noise 30 dB lower: 120 a
    /// minute, half a second apart.
    #[test]
    fn clicks_every_half_second() {
        let mut noise = Noise::new(9);
        let mut signal = noise.white(0.5 * 10f64.powf(-30.0 / 20.0), 60.0);
        let burst = (0.005 * RATE as f64) as usize;
        for click in 0..120 {
            let start = (0.25 + 0.5 * click as f64) * RATE as f64;
            for index in 0..burst {
                signal[start as usize + index] += (0.5 * noise.gaussian()) as f32;
            }
        }
        let found = onsets(&[signal], RATE);
        let per_minute = found.len() as f64;
        assert!((per_minute - 120.0).abs() <= 2.0, "{per_minute} a minute");
        let intervals: Vec<f64> = found.windows(2).map(|pair| pair[1] - pair[0]).collect();
        let median = crate::statistics::quantile(&intervals, 0.5).unwrap();
        assert!((median - 0.5).abs() <= 0.01, "median {median}");
    }

    /// Steady noise has no onsets: its frame-to-frame wobble never stands
    /// clear of its own median.
    #[test]
    fn steady_noise_has_no_onsets() {
        let mut noise = Noise::new(4);
        let signal = noise.white(0.1, 60.0);
        assert_eq!(onsets(&[signal], RATE), Vec::<f64>::new());
    }

    /// A rise counts once it stands the threshold clear of the second around
    /// it, and not in the frames that reach before the file's start.
    #[test]
    fn a_rise_must_stand_clear_of_its_surroundings() {
        let mut rises = vec![1.0; 200];
        rises[100] = 1.0 + MINIMUM_RISE_DECIBELS;
        rises[150] = 1.0 + MINIMUM_RISE_DECIBELS - 0.01;
        rises[1] = 10.0;
        let clear = standing_clear(&rises, 46);
        let found: Vec<usize> = (0..200).filter(|frame| clear[*frame]).collect();
        assert_eq!(found, vec![100]);
    }

    #[test]
    fn the_mel_scale_is_slaneys() {
        assert!((hertz_to_mel(1000.0) - 15.0).abs() < 1e-12);
        assert!((mel_to_hertz(hertz_to_mel(5000.0)) - 5000.0).abs() < 1e-9);
        // Linear below 1 kHz: 200/3 Hz a mel.
        assert!((hertz_to_mel(200.0) - 3.0).abs() < 1e-12);
    }

    /// Peaks as librosa picks them: the larger of two close bumps, and none
    /// within the wait after it.
    #[test]
    fn peaks_wait_and_must_stand_out() {
        let mut values = vec![0.0; 40];
        values[10] = 1.0;
        values[11] = 0.9;
        values[30] = 0.05; // not 0.07 above its neighbourhood's mean
        assert_eq!(pick_peaks(&values, 2, 1, 9, 10, 2), vec![10]);
    }
}
