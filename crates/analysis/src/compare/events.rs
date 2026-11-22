//! Events: how often things start, and how far apart.

use super::comparison::{Difference, EventDifferences};
use crate::report::Report;

pub(super) fn compare(reference: &Report, other: &Report) -> EventDifferences {
    let (first, second) = (&reference.events, &other.events);
    EventDifferences {
        onsets_per_minute: Difference::of(first.onsets_per_minute, second.onsets_per_minute),
        onsets_ratio: (first.onsets_per_minute > 0.0)
            .then(|| second.onsets_per_minute / first.onsets_per_minute),
        interval_median_seconds: Difference::of_both(
            first.interval_median_seconds,
            second.interval_median_seconds,
        ),
        interval_p10_seconds: Difference::of_both(
            first.interval_p10_seconds,
            second.interval_p10_seconds,
        ),
        interval_p90_seconds: Difference::of_both(
            first.interval_p90_seconds,
            second.interval_p90_seconds,
        ),
    }
}
