//! The LAF and width series. Two different recordings do not line up in
//! time, so their series are compared as distributions; two renders of the
//! same scene and seed do, and for those also moment by moment.

use super::both_calibrated;
use super::comparison::{Difference, Moment, OverTimeDifferences, PercentileDifference};
use super::space::ears;
use crate::report::Report;
use crate::statistics::quantile;

/// LN for N from 95 down to 5 in steps of 5: the LAF distribution.
const LAF_EXCEEDED: [f64; 19] = [
    95.0, 90.0, 85.0, 80.0, 75.0, 70.0, 65.0, 60.0, 55.0, 50.0, 45.0, 40.0, 35.0, 30.0, 25.0, 20.0,
    15.0, 10.0, 5.0,
];

/// The width's distribution: its bed, median and top.
const WIDTH_EXCEEDED: [f64; 3] = [90.0, 50.0, 10.0];

pub(super) fn compare(reference: &Report, other: &Report, lines_up: bool) -> OverTimeDifferences {
    let relative = !both_calibrated(reference, other);
    let first = laf_exceeded(reference, relative);
    let second = laf_exceeded(other, relative);
    let laf_exceeded = first
        .iter()
        .zip(&second)
        .map(|((percent, one), (_, two))| PercentileDifference {
            exceeded_percent: *percent,
            values: Difference::of(*one, *two),
        })
        .collect();
    let widths = |report: &Report| -> Option<Vec<f64>> {
        ears(report)?;
        Some(report.over_time.width.iter().flatten().copied().collect())
    };
    let width_exceeded = match (widths(reference), widths(other)) {
        (Some(one), Some(two)) => WIDTH_EXCEEDED
            .iter()
            .filter_map(|percent| {
                Some(PercentileDifference {
                    exceeded_percent: *percent,
                    values: Difference::of_both(
                        exceeded(&one, *percent),
                        exceeded(&two, *percent),
                    )?,
                })
            })
            .collect(),
        _ => Vec::new(),
    };
    let (largest_laf, largest_width) = if lines_up {
        (
            largest_laf(reference, other, relative),
            largest_width(reference, other),
        )
    } else {
        (None, None)
    };
    OverTimeDifferences {
        laf_relative_to_l50: relative,
        laf_exceeded,
        width_exceeded,
        largest_laf,
        largest_width,
    }
}

/// The value exceeded `percent` % of the time.
fn exceeded(values: &[f64], percent: f64) -> Option<f64> {
    quantile(values, 1.0 - percent / 100.0)
}

/// LN of the file's 25 ms LAF series for each N of [`LAF_EXCEEDED`], in the
/// report's unit, or relative to its own L50 when `relative`.
pub(crate) fn laf_exceeded(report: &Report, relative: bool) -> Vec<(f64, f64)> {
    let series = &report.over_time.laf_overall;
    let median = if relative {
        exceeded(series, 50.0).unwrap_or(0.0)
    } else {
        0.0
    };
    LAF_EXCEEDED
        .iter()
        .filter_map(|percent| Some((*percent, exceeded(series, *percent)? - median)))
        .collect()
}

/// Where two lined-up LAF series differ most.
fn largest_laf(reference: &Report, other: &Report, relative: bool) -> Option<Moment> {
    let offset = |report: &Report| {
        if relative {
            exceeded(&report.over_time.laf_overall, 50.0).unwrap_or(0.0)
        } else {
            0.0
        }
    };
    let (first_offset, second_offset) = (offset(reference), offset(other));
    let pairs = reference
        .over_time
        .laf_overall
        .iter()
        .zip(&other.over_time.laf_overall)
        .map(|(one, two)| Some((one - first_offset, two - second_offset)));
    largest(pairs, reference.over_time.laf_step_seconds)
}

/// Where two lined-up width series differ most, over the frames both have.
fn largest_width(reference: &Report, other: &Report) -> Option<Moment> {
    ears(reference)?;
    ears(other)?;
    let pairs = reference
        .over_time
        .width
        .iter()
        .zip(&other.over_time.width)
        .map(|(one, two)| Some(((*one)?, (*two)?)));
    largest(pairs, reference.over_time.frame_seconds)
}

/// The step whose two values differ most, and when it starts.
fn largest(pairs: impl Iterator<Item = Option<(f64, f64)>>, step_seconds: f64) -> Option<Moment> {
    pairs
        .enumerate()
        .filter_map(|(index, pair)| {
            let (one, two) = pair?;
            Some(Moment {
                seconds: index as f64 * step_seconds,
                values: Difference::of(one, two),
            })
        })
        .max_by(|a, b| {
            a.values
                .difference
                .abs()
                .total_cmp(&b.values.difference.abs())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 0 to 99 in order: L10 is the value exceeded 10 % of the time, the 90 %
    /// quantile, 89.1 by linear interpolation; L50 49.5.
    #[test]
    fn exceeded_counts_from_the_top() {
        let values: Vec<f64> = (0..100).map(f64::from).collect();
        assert!((exceeded(&values, 10.0).unwrap() - 89.1).abs() < 1e-9);
        assert!((exceeded(&values, 50.0).unwrap() - 49.5).abs() < 1e-9);
    }

    #[test]
    fn the_largest_difference_and_when() {
        let pairs = [Some((1.0, 1.0)), None, Some((2.0, -1.0)), Some((0.0, 2.0))];
        let found = largest(pairs.into_iter(), 0.1).unwrap();
        assert!((found.seconds - 0.2).abs() < 1e-12);
        assert_eq!(found.values.difference, -3.0);
    }
}
