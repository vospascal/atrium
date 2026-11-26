//! The slow movement of each octave's level: 1 s Hann frames every 0.5 s,
//! the channels' powers summed, from 63 Hz to 8 kHz and over the whole
//! spectrum. How much each level moves, how that movement spreads over
//! 0.01–0.5 Hz (the slope of its spectrum, log-log), whether the octaves
//! move together, and whether the colour brightens as the level rises.
//!
//! A band that is silent for a while would read −200 dB there and swamp
//! every statistic, so each level series is floored 60 dB under its own
//! loudest frame.

use super::short_time::{for_each_frame, frame_count};
use crate::bands::octaves_from_63;
use crate::report::{SlowBand, SlowMovement};
use crate::statistics::{correlation, mean, power_decibels, standard_deviation};

/// The frame and the hop (s).
const FRAME_SECONDS: f64 = 1.0;
const HOP_SECONDS: f64 = 0.5;
/// Each series is floored this far under its loudest frame (dB).
const FLOOR_DECIBELS: f64 = 60.0;
/// The modulation rates the slope is fitted over (Hz); the lowest is raised
/// to two cycles of the file.
const SLOPE_LOWEST_HERTZ: f64 = 0.01;
const SLOPE_HIGHEST_HERTZ: f64 = 0.5;
/// The centroid is taken over the audible range (Hz).
const CENTROID_LOW: f64 = 20.0;
const CENTROID_HIGH: f64 = 20_000.0;

/// Each octave's level, the broadband level and the centroid, per frame.
pub(crate) struct Levels {
    pub(crate) centres_hertz: Vec<f64>,
    /// dB, one series per octave, floored.
    pub(crate) bands: Vec<Vec<f64>>,
    /// dB, 20 Hz to Nyquist, floored: what the swells are read from.
    pub(crate) broadband: Vec<f64>,
    /// log2 of the centroid (Hz), and the broadband level, for the frames
    /// that have any power.
    centroid_octaves: Vec<f64>,
    centroid_levels: Vec<f64>,
}

/// The time between frames (s).
pub(crate) const STEP_SECONDS: f64 = HOP_SECONDS;

pub(crate) fn levels(channels: &[Vec<f32>], sample_rate: u32) -> Levels {
    let nyquist = sample_rate as f64 / 2.0;
    let bands = octaves_from_63(nyquist);
    let window = (FRAME_SECONDS * sample_rate as f64) as usize;
    let hop = (HOP_SECONDS * sample_rate as f64) as usize;
    let length = channels.first().map_or(0, Vec::len);
    let frames = frame_count(length, window, hop);
    let spacing = sample_rate as f64 / window as f64;
    let bin = |hertz: f64| ((hertz / spacing).ceil() as usize).min(window / 2 + 1);
    let ranges: Vec<std::ops::Range<usize>> = bands
        .iter()
        .map(|band| bin(band.low)..bin(band.high))
        .collect();
    let broadband = bin(CENTROID_LOW)..window / 2 + 1;
    let centroid_range = bin(CENTROID_LOW)..bin(CENTROID_HIGH);

    let mut band_power = vec![vec![0.0f64; frames]; bands.len()];
    let mut total_power = vec![0.0f64; frames];
    let mut moment = vec![0.0f64; frames];
    let mut centroid_power = vec![0.0f64; frames];
    for samples in channels {
        for_each_frame(samples, window, hop, |frame, power| {
            for (series, range) in band_power.iter_mut().zip(&ranges) {
                series[frame] += power[range.clone()].iter().sum::<f64>();
            }
            total_power[frame] += power[broadband.clone()].iter().sum::<f64>();
            for index in centroid_range.clone() {
                moment[frame] += index as f64 * spacing * power[index];
                centroid_power[frame] += power[index];
            }
        });
    }
    let broadband_levels = floored(&total_power);
    let (centroid_octaves, centroid_levels) = moment
        .iter()
        .zip(&centroid_power)
        .zip(&broadband_levels)
        .filter(|((_, power), _)| **power > 0.0)
        .map(|((moment, power), level)| ((moment / power).max(CENTROID_LOW).log2(), *level))
        .unzip();
    Levels {
        centres_hertz: bands.iter().map(|band| band.nominal).collect(),
        bands: band_power.iter().map(|series| floored(series)).collect(),
        broadband: broadband_levels,
        centroid_octaves,
        centroid_levels,
    }
}

/// What the levels say about the file's slow movement.
pub(crate) fn slow(levels: &Levels) -> SlowMovement {
    let octave_bands = levels
        .centres_hertz
        .iter()
        .zip(&levels.bands)
        .map(|(centre, series)| SlowBand {
            centre_hertz: *centre,
            standard_deviation_decibels: standard_deviation(series),
            modulation_slope: modulation_slope(series, STEP_SECONDS),
        })
        .collect();
    let pairs = |neighbours_only: bool| -> Option<f64> {
        let mut values = Vec::new();
        for first in 0..levels.bands.len() {
            for second in first + 1..levels.bands.len() {
                if neighbours_only && second != first + 1 {
                    continue;
                }
                if let Some(value) = correlation(&levels.bands[first], &levels.bands[second]) {
                    values.push(value);
                }
            }
        }
        (!values.is_empty()).then(|| mean(&values))
    };
    SlowMovement {
        octave_bands,
        broadband_standard_deviation_decibels: standard_deviation(&levels.broadband),
        broadband_modulation_slope: modulation_slope(&levels.broadband, STEP_SECONDS),
        neighbour_correlation: pairs(true),
        all_pairs_correlation: pairs(false),
        centroid_standard_deviation_octaves: standard_deviation(&levels.centroid_octaves),
        centroid_level_correlation: correlation(&levels.centroid_octaves, &levels.centroid_levels),
    }
}

/// Powers as decibels, floored [`FLOOR_DECIBELS`] under the loudest.
fn floored(powers: &[f64]) -> Vec<f64> {
    let levels: Vec<f64> = powers.iter().map(|power| power_decibels(*power)).collect();
    let loudest = levels.iter().copied().fold(f64::NEG_INFINITY, f64::max);
    levels
        .iter()
        .map(|level| level.max(loudest - FLOOR_DECIBELS))
        .collect()
}

/// The slope of `series`' power spectrum against rate, log-log, fitted to
/// the mean power in third-octave rate bands from 0.01 Hz (or two cycles of
/// the series) to 0.5 Hz. None with fewer than four bands, or no movement.
pub(crate) fn modulation_slope(series: &[f64], step_seconds: f64) -> Option<f64> {
    let length = series.len();
    if length < 8 {
        return None;
    }
    let average = mean(series);
    let taper =
        |index: usize| 0.5 - 0.5 * (std::f64::consts::TAU * index as f64 / length as f64).cos();
    let mut planner = realfft::RealFftPlanner::<f64>::new();
    let forward = planner.plan_fft_forward(length);
    let mut input: Vec<f64> = series
        .iter()
        .enumerate()
        .map(|(index, value)| (value - average) * taper(index))
        .collect();
    let mut output = forward.make_output_vec();
    forward
        .process(&mut input, &mut output)
        .expect("buffers are the planner's own");
    let duration = length as f64 * step_seconds;
    let lowest = SLOPE_LOWEST_HERTZ.max(2.0 / duration);
    let mut points = Vec::new();
    let mut band_low = lowest;
    while band_low * 2f64.powf(1.0 / 3.0) <= SLOPE_HIGHEST_HERTZ {
        let band_high = band_low * 2f64.powf(1.0 / 3.0);
        let powers: Vec<f64> = output
            .iter()
            .enumerate()
            .filter(|(index, _)| {
                let rate = *index as f64 / duration;
                rate >= band_low && rate < band_high
            })
            .map(|(_, bin)| bin.norm_sqr())
            .collect();
        let power = mean(&powers);
        if !powers.is_empty() && power > 0.0 {
            points.push(((band_low * band_high).sqrt().log10(), power.log10()));
        }
        band_low = band_high;
    }
    if points.len() < 4 {
        return None;
    }
    let x_mean = mean(&points.iter().map(|(x, _)| *x).collect::<Vec<_>>());
    let y_mean = mean(&points.iter().map(|(_, y)| *y).collect::<Vec<_>>());
    let (mut covariance, mut variance) = (0.0, 0.0);
    for (x, y) in &points {
        covariance += (x - x_mean) * (y - y_mean);
        variance += (x - x_mean).powi(2);
    }
    (variance > 0.0).then(|| covariance / variance)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_signals::{Noise, RATE};

    /// A level series whose spectrum is ∝ rate^−2: a line at every rate the
    /// series resolves, amplitude ∝ 1/rate, random phases. The fitted slope
    /// is −2.
    #[test]
    fn the_slope_of_a_brown_level() {
        let mut noise = Noise::new(5);
        let length = 1200;
        let step = 0.5;
        let duration = length as f64 * step;
        let mut series = vec![0.0f64; length];
        for cycles in 1..length / 2 {
            let rate = cycles as f64 / duration;
            let phase = std::f64::consts::TAU * noise.gaussian();
            for (index, value) in series.iter_mut().enumerate() {
                let time = index as f64 * step;
                *value += (std::f64::consts::TAU * rate * time + phase).sin() / rate;
            }
        }
        let slope = modulation_slope(&series, step).expect("long enough");
        assert!((slope + 2.0).abs() < 0.25, "{slope}");
    }

    /// Two noises, one in the 1 kHz octave and one in the 4 kHz, each swung
    /// ±3 dB at its own rate: each octave's level moves with a standard
    /// deviation near 3/√2 = 2.12 dB (plus the frames' own scatter, about
    /// 0.3 dB for 1 s of a band this wide), and the two do not move together.
    #[test]
    fn octaves_moving_apart() {
        let seconds = 120.0;
        let mut noise = Noise::new(8);
        let swing = |rate: f64, index: usize| {
            let time = index as f64 / RATE as f64;
            10f64.powf(3.0 * (std::f64::consts::TAU * rate * time).sin() / 20.0)
        };
        let band = |samples: Vec<f32>, low: f64, high: f64| -> Vec<f32> {
            let spectrum = crate::spectrum::Spectrum::of(&samples, RATE);
            spectrum.filtered(|frequency| {
                if (low..high).contains(&frequency) {
                    1.0
                } else {
                    0.0
                }
            })
        };
        let low = band(noise.white(0.1, seconds), 800.0, 1200.0);
        let high = band(noise.white(0.1, seconds), 3200.0, 4800.0);
        let samples: Vec<f32> = low
            .iter()
            .zip(&high)
            .enumerate()
            .map(|(index, (low, high))| {
                (*low as f64 * swing(0.05, index) + *high as f64 * swing(0.13, index)) as f32
            })
            .collect();
        let levels = levels(&[samples], RATE);
        assert_eq!(levels.broadband.len(), 239);
        assert_eq!(levels.centres_hertz.len(), 8);
        let band = |centre: f64| {
            let index = levels
                .centres_hertz
                .iter()
                .position(|nominal| *nominal == centre)
                .expect("the octave");
            &levels.bands[index]
        };
        let movement = slow(&levels);
        let spread = |centre: f64| {
            movement
                .octave_bands
                .iter()
                .find(|band| band.centre_hertz == centre)
                .expect("the octave")
                .standard_deviation_decibels
        };
        assert!((spread(1000.0) - 2.12).abs() < 0.3, "{}", spread(1000.0));
        assert!((spread(4000.0) - 2.12).abs() < 0.3, "{}", spread(4000.0));
        let together = correlation(band(1000.0), band(4000.0)).expect("both move");
        assert!(together.abs() < 0.2, "{together}");
    }
}
