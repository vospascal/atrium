//! The spectrogram: power over time and log frequency, 20 Hz to 20 kHz, one
//! cell a pixel, coloured over a fixed 70 dB below the loudest cell.
//!
//! Each pixel column is the mean power of the frames centred in it; each
//! row the mean power of the bins in its slice of log frequency, or, where a
//! row is narrower than a bin (the lowest octaves), the bins either side
//! interpolated.

use realfft::RealFftPlanner;

use super::canvas::Colour;
use crate::frequency_rows::{fraction_frequency, hann, window_length};

pub(crate) const RANGE_DECIBELS: f64 = 70.0;

/// Levels per pixel, the top row the highest frequency, in dB re the
/// loudest cell (0 at the loudest).
pub(crate) struct Grid {
    pub(crate) columns: usize,
    pub(crate) rows: usize,
    pub(crate) decibels: Vec<f64>,
}

/// The spectrogram of `channels`, their powers summed and scaled by
/// `channel_scale` (the mean of two ears, the sum of speakers).
pub(crate) fn grid(
    channels: &[Vec<f32>],
    sample_rate: u32,
    channel_scale: f64,
    columns: usize,
    rows: usize,
) -> Grid {
    let length = channels.first().map_or(0, Vec::len);
    let window_length = window_length(sample_rate);
    let window = hann(window_length);
    let column_samples = (length as f64 / columns as f64).max(1.0);
    let hop = ((window_length / 4) as f64).min(column_samples).max(1.0) as usize;
    let bins = window_length / 2 + 1;
    let mut column_power = vec![vec![0.0f64; bins]; columns];
    let mut column_frames = vec![0usize; columns];

    let mut planner = RealFftPlanner::<f32>::new();
    let forward = planner.plan_fft_forward(window_length);
    let mut input = forward.make_input_vec();
    let mut spectrum = forward.make_output_vec();
    let frames = length.div_ceil(hop);
    for frame in 0..frames {
        let centre = frame * hop + hop / 2;
        let column = ((centre as f64 / column_samples) as usize).min(columns - 1);
        column_frames[column] += 1;
        for channel in channels {
            for (offset, slot) in input.iter_mut().enumerate() {
                let position = (centre + offset) as isize - (window_length / 2) as isize;
                *slot = if position >= 0 && (position as usize) < length {
                    channel[position as usize] * window[offset]
                } else {
                    0.0
                };
            }
            forward
                .process(&mut input, &mut spectrum)
                .expect("buffers are the planner's own");
            for (sum, bin) in column_power[column].iter_mut().zip(&spectrum) {
                *sum += bin.norm_sqr() as f64 * channel_scale;
            }
        }
    }

    // Which bins make each row.
    let bin_hertz = sample_rate as f64 / window_length as f64;
    let nyquist = sample_rate as f64 / 2.0;
    enum RowSource {
        Bins(std::ops::Range<usize>),
        Between(usize, f64),
        Above,
    }
    let sources: Vec<RowSource> = (0..rows)
        .map(|row| {
            let bottom = fraction_frequency((rows - 1 - row) as f64 / rows as f64);
            let top = fraction_frequency((rows - row) as f64 / rows as f64);
            if bottom >= nyquist {
                return RowSource::Above;
            }
            let first = (bottom / bin_hertz).ceil() as usize;
            let end = ((top / bin_hertz).ceil() as usize).min(bins);
            if end > first {
                RowSource::Bins(first..end)
            } else {
                let position = (bottom * top).sqrt() / bin_hertz;
                RowSource::Between(position.floor() as usize, position.fract())
            }
        })
        .collect();

    let mut power = vec![0.0f64; rows * columns];
    for (column, (sums, count)) in column_power.iter().zip(&column_frames).enumerate() {
        let frames = (*count).max(1) as f64;
        for (row, source) in sources.iter().enumerate() {
            power[row * columns + column] = match source {
                RowSource::Bins(range) => {
                    sums[range.clone()].iter().sum::<f64>() / range.len() as f64 / frames
                }
                RowSource::Between(below, share) => {
                    let above = (*below + 1).min(bins - 1);
                    (sums[*below] * (1.0 - share) + sums[above] * share) / frames
                }
                RowSource::Above => 0.0,
            };
        }
    }
    let loudest = power.iter().copied().fold(0.0f64, f64::max).max(1e-300);
    Grid {
        columns,
        rows,
        decibels: power
            .iter()
            .map(|value| 10.0 * (value.max(1e-300) / loudest).log10())
            .collect(),
    }
}

/// The colour of a level `decibels` below the loudest cell: dark to bright
/// through purple, red and yellow (matplotlib's "inferno", sampled at nine
/// points), so brighter is always louder, in colour and in grey.
pub(crate) fn colour(decibels: f64) -> Colour {
    const STOPS: [[f64; 3]; 9] = [
        [0.0, 0.0, 4.0],
        [31.0, 12.0, 72.0],
        [85.0, 15.0, 109.0],
        [136.0, 34.0, 106.0],
        [186.0, 54.0, 85.0],
        [227.0, 89.0, 51.0],
        [249.0, 140.0, 10.0],
        [249.0, 201.0, 50.0],
        [252.0, 255.0, 164.0],
    ];
    let fraction = ((decibels + RANGE_DECIBELS) / RANGE_DECIBELS).clamp(0.0, 1.0);
    let position = fraction * (STOPS.len() - 1) as f64;
    let below = (position.floor() as usize).min(STOPS.len() - 2);
    let share = position - below as f64;
    let mix = |channel: usize| {
        (STOPS[below][channel] * (1.0 - share) + STOPS[below + 1][channel] * share).round() as u8
    };
    [mix(0), mix(1), mix(2)]
}
