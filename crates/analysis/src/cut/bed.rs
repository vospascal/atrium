//! Step 2: the local bed, the band level's 10th percentile over 10 s, as
//! Raven estimates noise (a percentile over a block).
//!
//! It has to be local. The frog recording's bed steps 11 dB down at 40 s
//! (−67 dB before, −78 after, as 10th percentiles over 10 s windows); one bed
//! for the whole file put the first 41 s 15 dB above it, one long "take".
//!
//! And a window centred on the moment is not local enough at such a step. Its
//! 10th percentile drops as soon as a tenth of it lies past the step, so for
//! 4 s before a step down the loud bed is measured against the quiet one,
//! 11 dB above it: noise alone then reaches the start threshold. So the bed
//! is taken over the 10 s before the moment and over the 10 s after it, and
//! of the two the one the level around the moment reaches (its median over
//! the second around it): the larger if both, the smaller if neither. Before
//! the step only the window behind is all loud, and the level is loud; after
//! it only the window ahead is all quiet, and the level is quiet.

/// The bed is this fraction's percentile of the level.
pub(super) const BED_PERCENTILE: f64 = 0.10;
/// Over this window, on either side of the moment (s).
pub(super) const BED_WINDOW_SECONDS: f64 = 10.0;
/// The level around the moment is the median over this span, centred (s),
/// taken every 10 ms.
const AROUND_SECONDS: f64 = 1.0;
const AROUND_STEP_SECONDS: f64 = 0.01;
/// Each side's bed is taken every this many seconds and drawn straight
/// between.
const BED_STEP_SECONDS: f64 = 0.5;

/// The bed under every reading of `levels` (dB, one every `step_seconds`).
pub(super) fn local_bed(levels: &[f64], step_seconds: f64) -> Vec<f64> {
    if levels.is_empty() {
        return Vec::new();
    }
    let window = ((BED_WINDOW_SECONDS / step_seconds).round() as usize).max(1);
    let every = ((BED_STEP_SECONDS / step_seconds).round() as usize).max(1);
    let mut scratch = Vec::with_capacity(window + 1);
    // Each side's bed, every half second and drawn straight between. A
    // window cut short by the file's start or end is lengthened on its other
    // side, so near the ends both are the first or last 10 s.
    let behind = interpolated(levels.len(), every, |centre| {
        let start = centre.saturating_sub(window);
        let end = (start + window + 1).min(levels.len());
        percentile(&levels[start..end], BED_PERCENTILE, &mut scratch)
    });
    let ahead = interpolated(levels.len(), every, |centre| {
        let end = (centre + window + 1).min(levels.len());
        let start = end.saturating_sub(window + 1);
        percentile(&levels[start..end], BED_PERCENTILE, &mut scratch)
    });
    // The level around each moment, every 10 ms.
    let around = ((AROUND_SECONDS / 2.0 / step_seconds).round() as usize).max(1);
    let level_every = ((AROUND_STEP_SECONDS / step_seconds).round() as usize).max(1);
    let medians: Vec<f64> = (0..levels.len())
        .step_by(level_every)
        .map(|centre| {
            let nearby =
                &levels[centre.saturating_sub(around)..(centre + around + 1).min(levels.len())];
            percentile(nearby, 0.5, &mut scratch)
        })
        .collect();
    (0..levels.len())
        .map(|index| {
            let level = medians
                [((index as f64 / level_every as f64).round() as usize).min(medians.len() - 1)];
            let (lower, higher) = if behind[index] <= ahead[index] {
                (behind[index], ahead[index])
            } else {
                (ahead[index], behind[index])
            };
            if higher <= level {
                higher
            } else {
                lower
            }
        })
        .collect()
}

/// `value(centre)` every `every` readings of `length` (and at the last), drawn
/// straight between.
fn interpolated(length: usize, every: usize, mut value: impl FnMut(usize) -> f64) -> Vec<f64> {
    let last = length - 1;
    let mut centres: Vec<usize> = (0..length).step_by(every).collect();
    if centres.last().is_some_and(|centre| *centre < last) {
        centres.push(last);
    }
    let points: Vec<(usize, f64)> = centres
        .into_iter()
        .map(|centre| (centre, value(centre)))
        .collect();
    let mut values = Vec::with_capacity(length);
    for pair in points.windows(2) {
        let ((from, from_value), (to, to_value)) = (pair[0], pair[1]);
        for index in from..to {
            let share = (index - from) as f64 / (to - from) as f64;
            values.push(from_value + (to_value - from_value) * share);
        }
    }
    values.push(points.last().map_or(0.0, |(_, value)| *value));
    values
}

/// The `fraction` percentile of `values`, by nearest rank.
fn percentile(values: &[f64], fraction: f64, scratch: &mut Vec<f64>) -> f64 {
    scratch.clear();
    scratch.extend_from_slice(values);
    let rank = ((scratch.len() - 1) as f64 * fraction).round() as usize;
    *scratch.select_nth_unstable_by(rank, f64::total_cmp).1
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A level that steps 11 dB down halfway: the bed is the loud one up to
    /// the step and the quiet one from it.
    #[test]
    fn the_bed_follows_a_step() {
        let step = 0.01;
        let levels: Vec<f64> = (0..6000)
            .map(|index| if index < 3000 { -67.0 } else { -78.0 })
            .collect();
        let bed = local_bed(&levels, step);
        assert_eq!(bed.len(), levels.len());
        for (index, value) in bed.iter().enumerate() {
            assert_eq!(*value, levels[index], "at {index}");
        }
        // The other way, a step up.
        let rising: Vec<f64> = levels.iter().rev().copied().collect();
        let bed = local_bed(&rising, step);
        assert_eq!(bed[2900], -78.0);
        assert_eq!(bed[3100], -67.0);
    }

    /// Calls loud for a third of the time leave the bed at the quiet.
    #[test]
    fn calls_do_not_raise_the_bed() {
        let levels: Vec<f64> = (0..3000)
            .map(|index| if index % 300 < 100 { -20.0 } else { -60.0 })
            .collect();
        let bed = local_bed(&levels, 0.01);
        assert!(bed.iter().all(|value| *value == -60.0));
    }
}
