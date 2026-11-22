//! The cut: a long recording into its takes, the rests between them, its
//! types and bouts, every decision with the values behind it
//! (docs/cutter-plan.md). Pure: the same samples give the same cut, and the
//! binary only reads and writes files.
//!
//! One file per step, in the order they run:
//!
//! - `band`: 1, the band the events stand furthest above the bed in, and the
//!   level in it
//! - `bed`: 2, the local bed
//! - `events`: 3, events with hysteresis
//! - `criterion`: 4, the bout criterion: one take or a rest
//! - `edges`: 5, edges and fades
//! - `drops`: 6, what is dropped
//! - `measures`: 7, each take's level, SNR and centroid
//! - `types` and `bouts`: 8, takes grouped by shape, and runs of one type
//!
//! And `record` (the types cut.json holds), `summary` (the one-screen text),
//! `tests` (the plan's checks on synthetic signals and the frog).

mod band;
mod bed;
mod bouts;
mod criterion;
mod drops;
mod edges;
mod events;
mod measures;
mod record;
mod summary;
mod types;

#[cfg(test)]
mod tests;

pub use record::{
    BandSpread, BoutCriterion, Bouts, CriterionDecision, Cut, CutSettings, DetectionBand,
    DropReason, Dropped, Event, Gaps, LevelSeries, OneProcess, Process, Run, Take, Thresholds,
    TwoProcesses, TypeGroup, Types,
};
pub use summary::cut_summary;

use crate::signal::Signal;
use crate::spectrum::Spectrum;
use crate::statistics::quantile;
use band::STEP_SECONDS;
use edges::{Placed, Readings};
use record::CUT_VERSION;

/// The level series cut.json and the picture keep: one value every 10 ms.
const SERIES_STEP_SECONDS: f64 = 0.01;

/// `signal` cut into takes. Pure: the same samples and settings give the
/// same cut.
pub fn cut(signal: &Signal, settings: &CutSettings) -> Cut {
    let rate = signal.sample_rate;
    let frames = signal.frames();
    let mono = measures::mono(signal.interleaved, signal.channels);
    let (band, level) = {
        let spectrum = Spectrum::of(&mono, rate);
        let band = band::choose(&spectrum, settings.band);
        let level = band::level(&spectrum, &band);
        (band, level)
    };
    let levels = &level.decibels;
    let bed = bed::local_bed(levels, STEP_SECONDS);
    let found = events::find(levels, &bed);
    let criterion = criterion::fit(&found, STEP_SECONDS);
    let seconds_of = |reading: usize| reading as f64 * STEP_SECONDS;

    // Takes too short to be calls go first: they are not the animal, and
    // must not hold back a neighbour's edges or split a rest.
    let mut dropped = Vec::new();
    let joined: Vec<(usize, usize)> = criterion::join(&found, &criterion)
        .into_iter()
        .filter(|(first, last)| {
            let (start, end) = (
                seconds_of(found[*first].first),
                seconds_of(found[*last].last),
            );
            let short = drops::too_short(end - start);
            if short {
                dropped.push(Dropped {
                    start_seconds: start,
                    end_seconds: end,
                    reason: DropReason::Shorter,
                });
            }
            !short
        })
        .collect();
    let spans: Vec<(usize, usize)> = joined
        .iter()
        .map(|(first, last)| (found[*first].first, found[*last].last))
        .collect();
    let placed = edges::place(
        &spans,
        &Readings {
            levels,
            bed: &bed,
            step_seconds: STEP_SECONDS,
            frame_seconds: level.frame_seconds,
            sample_rate: rate,
            frames,
        },
    );
    let frame_seconds = |frame: usize| frame as f64 / rate as f64;
    let gap_seconds: Vec<f64> = placed
        .windows(2)
        .map(|pair| {
            (frame_seconds(pair[1].first_frame) - frame_seconds(pair[0].end_frame)).max(0.0)
        })
        .collect();

    // The takes kept, each with its events.
    let mut kept: Vec<(Placed, &[events::Found])> = Vec::new();
    for (placed, (first, last)) in placed.iter().zip(&joined) {
        match drops::reason(placed, signal) {
            Some(reason) => dropped.push(Dropped {
                start_seconds: frame_seconds(placed.first_frame),
                end_seconds: frame_seconds(placed.end_frame),
                reason,
            }),
            None => kept.push((*placed, &found[*first..=*last])),
        }
    }
    dropped.sort_by(|a, b| a.start_seconds.total_cmp(&b.start_seconds));

    let measured: Vec<measures::Measured> = kept
        .iter()
        .map(|(placed, events)| {
            measures::measure(
                signal,
                (placed.first_frame, placed.end_frame, placed.fade_frames),
                events,
                levels,
                &bed,
            )
        })
        .collect();
    let take_samples: Vec<Vec<f32>> = kept
        .iter()
        .map(|(placed, _)| {
            measures::mono(
                &edges::faded(
                    signal,
                    placed.first_frame,
                    placed.end_frame,
                    placed.fade_frames,
                ),
                signal.channels,
            )
        })
        .collect();
    let similarity = types::similarity(&take_samples, rate, band.low_hertz, band.high_hertz);
    let type_of = types::group(&similarity);
    let type_names: Vec<String> = type_of.iter().map(|index| types::name(*index)).collect();
    let loudest = measured
        .iter()
        .map(|measured| measured.level_decibels)
        .fold(f64::NEG_INFINITY, f64::max);
    let mut counted = vec![0usize; type_of.iter().max().map_or(0, |largest| largest + 1)];
    let takes: Vec<Take> = kept
        .iter()
        .zip(&measured)
        .zip(&type_of)
        .enumerate()
        .map(|(index, (((placed, events), measured), type_index))| {
            counted[*type_index] += 1;
            let type_name = types::name(*type_index);
            Take {
                number: index + 1,
                name: format!("{type_name} {}", counted[*type_index]),
                type_name,
                first_frame: placed.first_frame,
                end_frame: placed.end_frame,
                fade_frames: placed.fade_frames,
                start_seconds: frame_seconds(placed.first_frame),
                end_seconds: frame_seconds(placed.end_frame),
                sounding_seconds: [
                    seconds_of(events[0].first),
                    seconds_of(events[events.len() - 1].last),
                ],
                events: events.len(),
                duration_seconds: frame_seconds(placed.end_frame - placed.first_frame),
                level_decibels: measured.level_decibels,
                relative_to_loudest_decibels: measured.level_decibels - loudest,
                snr_decibels: measured.snr_decibels,
                centroid_hertz: measured.centroid_hertz,
                flagged: measured.snr_decibels < measures::FLAG_BELOW_SNR_DECIBELS,
            }
        })
        .collect();
    let groups = (0..counted.len())
        .map(|type_index| TypeGroup {
            name: types::name(type_index),
            takes: takes
                .iter()
                .filter(|take| take.type_name == types::name(type_index))
                .map(|take| take.number)
                .collect(),
        })
        .collect();

    let every = (SERIES_STEP_SECONDS / STEP_SECONDS).round() as usize;
    Cut {
        cut_version: CUT_VERSION,
        sample_rate: rate,
        channels: signal.channels,
        seconds: signal.seconds(),
        thresholds: Thresholds {
            bed_percentile: 100.0 * bed::BED_PERCENTILE,
            bed_window_seconds: bed::BED_WINDOW_SECONDS,
            start_above_bed_decibels: events::START_ABOVE_BED_DECIBELS,
            end_above_bed_decibels: events::END_ABOVE_BED_DECIBELS,
            edge_above_bed_decibels: edges::EDGE_ABOVE_BED_DECIBELS,
            longest_attack_seconds: edges::LONGEST_ATTACK_SECONDS,
            longest_decay_seconds: edges::LONGEST_DECAY_SECONDS,
            fade_seconds: edges::FADE_SECONDS,
            shortest_take_seconds: drops::SHORTEST_TAKE_SECONDS,
            flag_below_snr_decibels: measures::FLAG_BELOW_SNR_DECIBELS,
            band_within_decibels: band::WITHIN_LARGEST_DECIBELS,
            type_similarity: types::TYPE_SIMILARITY,
        },
        band,
        level: LevelSeries {
            step_seconds: SERIES_STEP_SECONDS,
            band_decibels: levels
                .chunks(every)
                .map(|chunk| chunk.iter().copied().fold(f64::NEG_INFINITY, f64::max))
                .collect(),
            bed_decibels: bed.iter().step_by(every).copied().collect(),
        },
        events: found
            .iter()
            .map(|event| Event {
                start_seconds: seconds_of(event.first),
                end_seconds: seconds_of(event.last),
                peak_above_bed_decibels: event.peak_above_bed,
            })
            .collect(),
        criterion,
        gaps: Gaps {
            p10: quantile(&gap_seconds, 0.1),
            median: quantile(&gap_seconds, 0.5),
            p90: quantile(&gap_seconds, 0.9),
            seconds: gap_seconds,
        },
        bouts: bouts::runs(&type_names),
        types: Types { similarity, groups },
        takes,
        dropped,
    }
}
