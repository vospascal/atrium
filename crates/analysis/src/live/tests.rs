//! The live analyser against the whole-file one, on a signal shaped like a
//! render: a diffuse bed at the two ears and calls that start, panned.

use super::*;
use crate::onsets::onsets;
use crate::space::{binaural, octave_envelopes};
use crate::spectrum::Spectrum;
use crate::statistics::power_decibels;
use crate::test_signals::{interleave, Noise, RATE};
use crate::weighting::a_weighting;

const FULL_SCALE: f64 = 114.0;

/// `seconds` of two ears: independent pink beds at RMS 0.01 each, and every
/// 0.7 s a call, a burst of noise rising in 5 ms and dying away over 50 ms
/// (peak RMS 0.1), panned 6 dB to the left or to the right in turn.
fn meadow_like(seconds: f64) -> (Vec<f32>, Vec<f32>) {
    let mut noise = Noise::new(5);
    let mut left = noise.pink(0.01, seconds);
    let mut right = noise.pink(0.01, seconds);
    let call_length = (0.2 * RATE as f64) as usize;
    let attack = 0.005 * RATE as f64;
    let decay = 0.05 * RATE as f64;
    let mut start = (0.4 * RATE as f64) as usize;
    let mut call = 0;
    while start + call_length < left.len() {
        let (left_gain, right_gain) = if call % 2 == 0 {
            (1.0, 0.5)
        } else {
            (0.5, 1.0)
        };
        for index in 0..call_length {
            let position = index as f64;
            let envelope = (position / attack).min(1.0) * (-position / decay).exp();
            let value = 0.1 * envelope * noise.gaussian();
            left[start + index] += (value * left_gain) as f32;
            right[start + index] += (value * right_gain) as f32;
        }
        start += (0.7 * RATE as f64) as usize;
        call += 1;
    }
    (left, right)
}

/// Every column of `interleaved`, pushed `chunk` frames at a time from
/// clock sample `clock`.
fn live_columns(interleaved: &[f32], channels: usize, chunk: usize, clock: u64) -> Vec<Column> {
    let mut live = Live::new(RATE, channels, FULL_SCALE);
    let mut columns = Vec::new();
    for (index, block) in interleaved.chunks(chunk * channels).enumerate() {
        live.push(block, clock + (index * chunk) as u64);
        columns.extend(live.take_columns());
    }
    columns
}

/// Device-sized chunks and ragged ones give the same columns, bit for bit:
/// what a column holds depends on the samples, not on how they arrived.
#[test]
fn chunks_do_not_matter() {
    let (left, right) = meadow_like(6.0);
    let interleaved = interleave(&[&left, &right]);
    let device = live_columns(&interleaved, 2, 512, 0);
    let ragged = live_columns(&interleaved, 2, 37, 0);
    assert!(device.len() > 100, "{} columns", device.len());
    assert_eq!(device, ragged);
}

/// Live equals offline. Per 50 ms column, each ear's LAeq within 0.2 dB of
/// the whole-file A-weighting's over the same 50 ms; per 100 ms, width
/// within 0.03 and ILD within 0.3 dB of the report's frames (they are read
/// by the same code, so they agree exactly); onsets within one a minute of
/// the whole-file count over the same stretch.
#[test]
fn live_equals_offline() {
    let seconds = 60.0;
    let (left, right) = meadow_like(seconds);
    let interleaved = interleave(&[&left, &right]);
    let columns = live_columns(&interleaved, 2, 512, 0);
    let column_length = 2400;

    // Level: the whole-file weighting, by column.
    for (channel, samples) in [&left, &right].into_iter().enumerate() {
        let weighted = Spectrum::of(samples, RATE).filtered(a_weighting);
        let mut worst = 0.0f64;
        for (index, column) in columns.iter().enumerate() {
            let span = &weighted[index * column_length..(index + 1) * column_length];
            let power = span
                .iter()
                .map(|value| (*value as f64).powi(2))
                .sum::<f64>()
                / column_length as f64;
            let offline = power_decibels(power) + FULL_SCALE;
            worst = worst.max((column.leq_decibels[channel] - offline).abs());
        }
        assert!(worst <= 0.2, "channel {channel}: LAeq off by {worst:.3} dB");
    }

    // Width and ILD: column 2k + 1 ends where frame k does.
    let envelopes = (
        octave_envelopes(&Spectrum::of(&left, RATE)),
        octave_envelopes(&Spectrum::of(&right, RATE)),
    );
    let (_, frames) = binaural(
        ("L", "R"),
        &left,
        &right,
        RATE,
        (&envelopes.0, &envelopes.1),
    );
    let mut compared = 0;
    for (frame, (width, ild)) in frames.width.iter().zip(&frames.ild_decibels).enumerate() {
        let Some(column) = columns.get(2 * frame + 1) else {
            break;
        };
        match (width, column.width) {
            (Some(offline), Some(live)) => assert!((offline - live).abs() <= 0.03),
            (offline, live) => assert_eq!(offline.is_some(), live.is_some()),
        }
        match (ild, column.ild_decibels) {
            (Some(offline), Some(live)) => assert!((offline - live).abs() <= 0.3),
            (offline, live) => assert_eq!(offline.is_some(), live.is_some()),
        }
        compared += 1;
    }
    assert!(compared > 590, "{compared} frames compared");

    // Onsets: over the stretch the live columns cover.
    let covered = columns.len() as f64 * 0.05;
    let offline = onsets(&[left.clone(), right.clone()], RATE)
        .into_iter()
        .filter(|time| *time < covered)
        .count() as f64;
    let live = columns.iter().filter(|column| column.onset).count() as f64;
    let per_minute = |count: f64| count * 60.0 / covered;
    assert!(
        (per_minute(live) - per_minute(offline)).abs() <= 1.0,
        "live {:.1} a minute, offline {:.1}",
        per_minute(live),
        per_minute(offline)
    );
    // One call every 0.7 s: about 86 a minute.
    assert!(
        (per_minute(live) - 60.0 / 0.7).abs() <= 3.0,
        "{}",
        per_minute(live)
    );
}

/// A 1 kHz sine at RMS 0.1 (−20 dBFS) on a scale where RMS 1.0 is 114 dB
/// SPL reads 94.0 dB SPL in each ear's LAeq and LAF (A is 0 dB at 1 kHz)
/// and in the overall LAeq and LAF, and its rows add up to 94.0 dB SPL; the ears are averaged, so the
/// spectrum reads the level of one ear, not twice it.
#[test]
fn levels_are_calibrated() {
    let tone = crate::test_signals::sine(1000.0, 0.1 * 2f64.sqrt(), 2.0);
    let interleaved = interleave(&[&tone, &tone]);
    let columns = live_columns(&interleaved, 2, 512, 0);
    let column = &columns[20];
    // Two ears: their powers averaged, so the overall level is one ear's.
    for level in column
        .leq_decibels
        .iter()
        .chain(&column.laf_decibels)
        .chain([&column.leq_overall_decibels, &column.laf_overall_decibels])
    {
        assert!((level - 94.0).abs() < 0.01, "{level}");
    }
    let total: f64 = column
        .spectrum_decibels
        .iter()
        .map(|level| 10f64.powf((level - FULL_SCALE) / 10.0))
        .sum();
    assert!(
        (power_decibels(total) + FULL_SCALE - 94.0).abs() < 0.01,
        "{}",
        power_decibels(total) + FULL_SCALE
    );
    assert_eq!(column.width.map(|width| width < 1e-6), Some(true));
    assert_eq!(column.ild_decibels.map(|ild| ild.abs() < 1e-9), Some(true));
}

/// Columns are 2400 samples apart on the pushing clock; a push that does
/// not follow on (a dropped block) starts again from its own clock sample.
#[test]
fn columns_follow_the_clock_and_a_gap_starts_again() {
    let tone = crate::test_signals::sine(1000.0, 0.1, 1.0);
    let mut live = Live::new(RATE, 1, FULL_SCALE);
    for (index, block) in tone.chunks(480).enumerate() {
        live.push(block, 1_000_000 + (index * 480) as u64);
    }
    let first = live.take_columns();
    assert!(!first.is_empty());
    for (index, column) in first.iter().enumerate() {
        assert_eq!(column.clock_sample, 1_000_000 + index as u64 * 2400);
    }
    // A block lost: the next arrives 480 samples later than expected.
    let next = 1_000_000 + tone.len() as u64 + 480;
    for (index, block) in tone.chunks(480).enumerate() {
        live.push(block, next + (index * 480) as u64);
    }
    let second = live.take_columns();
    assert_eq!(second.first().map(|column| column.clock_sample), Some(next));
    assert!(first
        .iter()
        .chain(&second)
        .all(|column| column.width.is_none()));
}

/// Six channels are speakers: their powers add. The same noise at each of
/// L, R, C, Ls and Rs (LFE silent) reads 10·log10(5) = 6.99 dB above one
/// channel in the spectrum, the overall LAeq is the sum of the channels'
/// powers, and width and ILD come from L and R.
#[test]
fn speakers_add_and_their_pair_is_l_and_r() {
    let noise = Noise::new(8).white(0.1, 1.0);
    let silent = vec![0.0f32; noise.len()];
    let six = interleave(&[&noise, &noise, &noise, &silent, &noise, &noise]);
    let one = live_columns(&noise, 1, 512, 0);
    let all = live_columns(&six, 6, 512, 0);
    let row_total = |column: &Column| {
        column
            .spectrum_decibels
            .iter()
            .map(|level| 10f64.powf(level / 10.0))
            .sum::<f64>()
    };
    let difference = 10.0 * (row_total(&all[10]) / row_total(&one[10])).log10();
    assert!((difference - 6.99).abs() < 0.01, "{difference}");
    // The overall level is the sum of the channels' powers, column by column.
    for column in &all[5..] {
        let sum: f64 = column
            .leq_decibels
            .iter()
            .map(|level| 10f64.powf(level / 10.0))
            .sum();
        let overall = 10.0 * sum.log10();
        assert!(
            (column.leq_overall_decibels - overall).abs() < 1e-9,
            "{} against {overall}",
            column.leq_overall_decibels
        );
        assert!(column.laf_overall_decibels >= column.leq_overall_decibels - 1.0);
    }
    assert_eq!(all[10].leq_decibels.len(), 6);
    assert_eq!(all[10].width.map(|width| width < 1e-6), Some(true));
    assert!(one[10].width.is_none());
}
