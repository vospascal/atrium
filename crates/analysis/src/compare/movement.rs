//! Movement: how two files' tones and noise move. Their partials seldom sit
//! at the same frequencies, so they are compared as a whole, by the median
//! over each file's partials, and one to one where both have a partial at
//! the same pitch (within 3 %, nearest first): a focus tone against a
//! recording of the same note. The slow movement compares octave by octave,
//! the swells a minute (files of different lengths compare), the pulse when
//! both have one.

use super::comparison::{
    BeatLists, Difference, MovementDifferences, PartialDifference, PartialsDifferences,
    PulseDifferences, SlowBandDifference, SlowDifferences, SwellDifferences,
};
use crate::report::{Partial, Report};
use crate::statistics::quantile;

/// Partials this close (a share of the reference's frequency) are one.
const SAME_PITCH: f64 = 0.03;
/// A partial is detuned between the ears when its phase turns at least
/// this fast (Hz) ...
const DETUNED_HERTZ: f64 = 0.3;
/// ... and its rate varies between 10 s stretches by less than this share
/// of the rate.
const DETUNED_STEADINESS: f64 = 0.5;

pub(super) fn compare(reference: &Report, other: &Report) -> MovementDifferences {
    let (first, second) = (&reference.movement, &other.movement);
    let ears = |report: &Report| {
        report
            .space
            .as_ref()
            .is_some_and(|space| space.pair.is_some())
    };
    let both_ears = ears(reference) && ears(other);
    let median = |partials: &[Partial], value: fn(&Partial) -> Option<f64>| {
        quantile(&partials.iter().filter_map(value).collect::<Vec<_>>(), 0.5)
    };
    let medians = |value: fn(&Partial) -> Option<f64>| {
        Difference::of_both(
            median(&first.partials, value),
            median(&second.partials, value),
        )
    };
    let partials = PartialsDifferences {
        count: Difference::of(first.partials.len() as f64, second.partials.len() as f64),
        lines_median: medians(|partial| Some(partial.lines as f64)),
        width_median_hertz: medians(|partial| Some(width(partial))),
        level_p10_p90_median_decibels: medians(|partial| partial.level_p10_p90_decibels),
        pitch_standard_deviation_median_cents: medians(|partial| {
            partial.pitch_standard_deviation_cents
        }),
        detuned: both_ears.then(|| {
            Difference::of(
                detuned_count(&first.partials) as f64,
                detuned_count(&second.partials) as f64,
            )
        }),
    };

    let (slow_first, slow_second) = (&first.slow, &second.slow);
    let slow = SlowDifferences {
        octave_bands: slow_first
            .octave_bands
            .iter()
            .filter_map(|one| {
                let two = slow_second
                    .octave_bands
                    .iter()
                    .find(|two| two.centre_hertz == one.centre_hertz)?;
                Some(SlowBandDifference {
                    centre_hertz: one.centre_hertz,
                    standard_deviation_decibels: Difference::of(
                        one.standard_deviation_decibels,
                        two.standard_deviation_decibels,
                    ),
                    modulation_slope: Difference::of_both(
                        one.modulation_slope,
                        two.modulation_slope,
                    ),
                })
            })
            .collect(),
        broadband_standard_deviation_decibels: Difference::of(
            slow_first.broadband_standard_deviation_decibels,
            slow_second.broadband_standard_deviation_decibels,
        ),
        broadband_modulation_slope: Difference::of_both(
            slow_first.broadband_modulation_slope,
            slow_second.broadband_modulation_slope,
        ),
        neighbour_correlation: Difference::of_both(
            slow_first.neighbour_correlation,
            slow_second.neighbour_correlation,
        ),
        all_pairs_correlation: Difference::of_both(
            slow_first.all_pairs_correlation,
            slow_second.all_pairs_correlation,
        ),
        centroid_standard_deviation_octaves: Difference::of(
            slow_first.centroid_standard_deviation_octaves,
            slow_second.centroid_standard_deviation_octaves,
        ),
        centroid_level_correlation: Difference::of_both(
            slow_first.centroid_level_correlation,
            slow_second.centroid_level_correlation,
        ),
    };

    let per_minute = |report: &Report| {
        let minutes = report.signal.seconds / 60.0;
        if minutes > 0.0 {
            report.movement.swells.count as f64 / minutes
        } else {
            0.0
        }
    };
    let swells = SwellDifferences {
        per_minute: Difference::of(per_minute(reference), per_minute(other)),
        rise_median_seconds: Difference::of_both(
            first.swells.rise_median_seconds,
            second.swells.rise_median_seconds,
        ),
        decay_median_seconds: Difference::of_both(
            first.swells.decay_median_seconds,
            second.swells.decay_median_seconds,
        ),
        rate_of_change_skewness: Difference::of_both(
            first.swells.rate_of_change_skewness,
            second.swells.rate_of_change_skewness,
        ),
    };

    let both_pulses = first.pulse.as_ref().zip(second.pulse.as_ref());
    let pulse_difference = |value: fn(&crate::report::Pulse) -> f64| {
        both_pulses.map(|(one, two)| Difference::of(value(one), value(two)))
    };
    let pulse = PulseDifferences {
        reference: first.pulse.clone(),
        other: second.pulse.clone(),
        rate_hertz: pulse_difference(|pulse| pulse.rate_hertz),
        depth: pulse_difference(|pulse| pulse.depth),
        rise_seconds: pulse_difference(|pulse| pulse.rise_seconds),
        decay_seconds: pulse_difference(|pulse| pulse.decay_seconds),
    };

    MovementDifferences {
        tonal_share: Difference::of(first.tonal_share, second.tonal_share),
        partials,
        matched_partials: matched(&first.partials, &second.partials, both_ears),
        binaural_beats: BeatLists {
            reference: first
                .binaural_beats
                .iter()
                .map(|beat| beat.beat_hertz)
                .collect(),
            other: second
                .binaural_beats
                .iter()
                .map(|beat| beat.beat_hertz)
                .collect(),
        },
        slow,
        swells,
        pulse,
    }
}

fn width(partial: &Partial) -> f64 {
    partial.high_hertz - partial.low_hertz
}

/// The partials whose ears turn steadily apart.
fn detuned_count(partials: &[Partial]) -> usize {
    partials
        .iter()
        .filter(|partial| {
            partial.interaural.as_ref().is_some_and(|ears| {
                ears.rotation_hertz.abs() >= DETUNED_HERTZ
                    && ears.rotation_spread_hertz.is_some_and(|spread| {
                        spread < DETUNED_STEADINESS * ears.rotation_hertz.abs()
                    })
            })
        })
        .count()
}

/// The partials both files have, paired nearest first within 3 %.
fn matched(first: &[Partial], second: &[Partial], both_ears: bool) -> Vec<PartialDifference> {
    let mut candidates: Vec<(f64, usize, usize)> = Vec::new();
    for (one_index, one) in first.iter().enumerate() {
        for (two_index, two) in second.iter().enumerate() {
            let gap = (two.frequency_hertz - one.frequency_hertz).abs();
            if gap <= SAME_PITCH * one.frequency_hertz {
                candidates.push((gap, one_index, two_index));
            }
        }
    }
    candidates.sort_by(|a, b| a.0.total_cmp(&b.0));
    let mut used_first = vec![false; first.len()];
    let mut used_second = vec![false; second.len()];
    let mut pairs = Vec::new();
    for (_, one_index, two_index) in candidates {
        if used_first[one_index] || used_second[two_index] {
            continue;
        }
        used_first[one_index] = true;
        used_second[two_index] = true;
        let (one, two) = (&first[one_index], &second[two_index]);
        pairs.push(PartialDifference {
            frequency_hertz: Difference::of(one.frequency_hertz, two.frequency_hertz),
            relative_decibels: Difference::of(one.relative_decibels, two.relative_decibels),
            width_hertz: Difference::of(width(one), width(two)),
            level_p10_p90_decibels: Difference::of_both(
                one.level_p10_p90_decibels,
                two.level_p10_p90_decibels,
            ),
            pitch_standard_deviation_cents: Difference::of_both(
                one.pitch_standard_deviation_cents,
                two.pitch_standard_deviation_cents,
            ),
            rotation_hertz: if both_ears {
                Difference::of_both(
                    one.interaural.as_ref().map(|ears| ears.rotation_hertz),
                    two.interaural.as_ref().map(|ears| ears.rotation_hertz),
                )
            } else {
                None
            },
        });
    }
    pairs.sort_by(|a, b| {
        a.frequency_hertz
            .reference
            .total_cmp(&b.frequency_hertz.reference)
    });
    pairs
}
