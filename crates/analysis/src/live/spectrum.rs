//! A column's spectrum: band levels in rows log-spaced from 20 Hz to 20 kHz,
//! from the picture's window and rows (`frequency_rows`), as powers rather
//! than relative to the loudest cell, so a level means the same all session.
//!
//! Two windows a column, centred a quarter and three quarters into it (the
//! picture averages the frames centred in a column the same way). A bin
//! stands for the band a bin wide around its centre, and gives each row the
//! share of its power that its band overlaps the row's. The picture's mean
//! of the bins in a row is a level relative to the loudest cell; as an
//! absolute band level it would be off by the row's width over its whole
//! bins (up to 3 dB where rows are one or two bins wide), and a sine would
//! read up to 0.7 dB high or low at 1 kHz. Shared by overlap, the rows add
//! up to the power between 20 Hz and 20 kHz, noise lies smooth across rows
//! narrower than a bin, and a sine's window lobe adds up to its level.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{RealFftPlanner, RealToComplex};

use crate::frequency_rows::{fraction_frequency, hann, window_length};

/// Rows from 20 Hz to 20 kHz: a tenth of an octave each.
pub(super) const ROWS: usize = 96;

/// Windows a column.
const WINDOWS: usize = 2;

pub(super) struct ColumnSpectrum {
    window: Vec<f64>,
    /// 2 / (N·Σw²): a bin's |X|² as the mean square it stands for, its mirror
    /// included.
    bin_scale: f64,
    /// Per row, the bins it overlaps and the share of each.
    rows: Vec<Vec<(usize, f64)>>,
    forward: Arc<dyn RealToComplex<f64>>,
    input: Vec<f64>,
    output: Vec<Complex<f64>>,
    powers: Vec<f64>,
}

impl ColumnSpectrum {
    pub(super) fn new(sample_rate: u32) -> Self {
        let length = window_length(sample_rate);
        let window: Vec<f64> = hann(length).into_iter().map(f64::from).collect();
        let window_energy: f64 = window.iter().map(|value| value * value).sum();
        let bins = length / 2 + 1;
        let bin_hertz = sample_rate as f64 / length as f64;
        let rows = row_edges()
            .windows(2)
            .map(|edges| overlaps(edges[0], edges[1], bin_hertz, bins))
            .collect();
        let mut planner = RealFftPlanner::<f64>::new();
        let forward = planner.plan_fft_forward(length);
        Self {
            bin_scale: 2.0 / (length as f64 * window_energy),
            window,
            rows,
            input: forward.make_input_vec(),
            output: forward.make_output_vec(),
            powers: vec![0.0; bins],
            forward,
        }
    }

    /// How far past a column's end its last window reaches (samples; zero or
    /// less when it ends inside the column).
    pub(super) fn reach_after(&self, column_length: usize) -> i64 {
        let centre = centre_in_column(WINDOWS - 1, column_length) as i64;
        centre + (self.window.len() / 2) as i64 - column_length as i64
    }

    /// How far before a column's start its first window reaches (samples).
    pub(super) fn reach_before(&self, column_length: usize) -> i64 {
        (self.window.len() / 2) as i64 - centre_in_column(0, column_length) as i64
    }

    /// Add one channel's band powers in the column starting at `start`,
    /// times `share`, to `rows`: `sample(index)` is the channel's sample at
    /// `index`, zero before the stream began.
    pub(super) fn add(
        &mut self,
        sample: impl Fn(i64) -> f32,
        start: i64,
        column_length: usize,
        share: f64,
        rows: &mut [f64],
    ) {
        self.powers.fill(0.0);
        let half = (self.window.len() / 2) as i64;
        for window in 0..WINDOWS {
            let first = start + centre_in_column(window, column_length) as i64 - half;
            for (offset, slot) in self.input.iter_mut().enumerate() {
                *slot = sample(first + offset as i64) as f64 * self.window[offset];
            }
            self.forward
                .process(&mut self.input, &mut self.output)
                .expect("buffers are the planner's own");
            for (power, bin) in self.powers.iter_mut().zip(&self.output) {
                *power += bin.norm_sqr() * self.bin_scale / WINDOWS as f64;
            }
        }
        for (row, overlaps) in rows.iter_mut().zip(&self.rows) {
            *row += overlaps
                .iter()
                .map(|(bin, overlap)| self.powers[*bin] * overlap)
                .sum::<f64>()
                * share;
        }
    }
}

/// The bins whose bands (a bin wide, centred on the bin) overlap `low` to
/// `high` Hz, each with the share of its band inside; none above the last
/// bin.
fn overlaps(low: f64, high: f64, bin_hertz: f64, bins: usize) -> Vec<(usize, f64)> {
    let first = ((low / bin_hertz - 0.5).floor().max(0.0)) as usize;
    let last = ((high / bin_hertz + 0.5).ceil() as usize).min(bins - 1);
    (first..=last)
        .filter_map(|bin| {
            let band_low = (bin as f64 - 0.5) * bin_hertz;
            let band_high = (bin as f64 + 0.5) * bin_hertz;
            let overlap = (high.min(band_high) - low.max(band_low)) / bin_hertz;
            (overlap > 0.0).then_some((bin, overlap))
        })
        .collect()
}

/// Where window `window` is centred in a column (samples from its start).
fn centre_in_column(window: usize, column_length: usize) -> usize {
    (2 * window + 1) * column_length / (2 * WINDOWS)
}

/// The rows' edges (Hz), lowest first: row r runs from edge r to edge r + 1.
pub(super) fn row_edges() -> Vec<f64> {
    (0..=ROWS)
        .map(|edge| fraction_frequency(edge as f64 / ROWS as f64))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::statistics::power_decibels;
    use crate::test_signals::{sine, Noise, RATE};

    const COLUMN: usize = 2400;

    fn column_rows(samples: &[f32], start: usize) -> Vec<f64> {
        let mut spectrum = ColumnSpectrum::new(RATE);
        let mut rows = vec![0.0; ROWS];
        let sample = |at: i64| {
            if at >= 0 && (at as usize) < samples.len() {
                samples[at as usize]
            } else {
                0.0
            }
        };
        spectrum.add(sample, start as i64, COLUMN, 1.0, &mut rows);
        rows
    }

    /// A 1 kHz sine at −20 dBFS RMS: the rows of its window lobe add up to
    /// −20.00 dB, the row holding 1 kHz alone within 1 dB of it, and rows an
    /// octave away below −60 dB (a Hann window's sidelobes fall 18 dB an
    /// octave from −31 dB).
    #[test]
    fn a_sine_is_its_level_in_its_rows() {
        let tone = sine(1000.0, 0.1 * 2f64.sqrt(), 1.0);
        let rows = column_rows(&tone, 12_000);
        let edges = row_edges();
        let row = edges.iter().position(|edge| *edge > 1000.0).unwrap() - 1;
        let lobe: f64 = rows[row - 2..=row + 2].iter().sum();
        assert!(
            (power_decibels(lobe) + 20.0).abs() < 0.01,
            "{}",
            power_decibels(lobe)
        );
        assert!(
            power_decibels(rows[row]) > -21.0,
            "{}",
            power_decibels(rows[row])
        );
        for (index, power) in rows.iter().enumerate() {
            if edges[index + 1] < 500.0 || edges[index] > 2000.0 {
                assert!(power_decibels(*power) < -60.0, "row {index}");
            }
        }
    }

    /// White noise of mean square 0.01: the rows add up to its power between
    /// 20 Hz and 20 kHz, 0.01 · 19 980 / 24 000, within 0.2 dB (one column
    /// of noise wobbles), and a row's power is its share of the width.
    #[test]
    fn noise_is_spread_by_bandwidth() {
        let noise = Noise::new(31).white(0.1, 1.0);
        let mut total = vec![0.0; ROWS];
        let columns = 16;
        for column in 0..columns {
            let rows = column_rows(&noise, 2400 + column * COLUMN);
            for (sum, row) in total.iter_mut().zip(rows) {
                *sum += row / columns as f64;
            }
        }
        let expected = 0.01 * 19_980.0 / 24_000.0;
        let sum: f64 = total.iter().sum();
        assert!(
            (power_decibels(sum) - power_decibels(expected)).abs() < 0.2,
            "{} against {}",
            power_decibels(sum),
            power_decibels(expected)
        );
        // The 5 kHz row: its width over 24 kHz, of 0.01.
        let edges = row_edges();
        let row = edges.iter().position(|edge| *edge > 5000.0).unwrap() - 1;
        let share = 0.01 * (edges[row + 1] - edges[row]) / 24_000.0;
        assert!(
            (power_decibels(total[row]) - power_decibels(share)).abs() < 1.0,
            "{} against {}",
            power_decibels(total[row]),
            power_decibels(share)
        );
    }
}
