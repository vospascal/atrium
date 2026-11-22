//! Level: its shape always, its absolute value only when both files are
//! calibrated.

use super::both_calibrated;
use super::comparison::{AbsoluteLevels, Difference, LevelDifferences};
use crate::report::Report;

/// The just-noticeable difference of level, ISO 3382-1 Table A.1 (dB).
pub(crate) const LEVEL_NOTICEABLE_DECIBELS: f64 = 1.0;

pub(super) fn compare(reference: &Report, other: &Report) -> LevelDifferences {
    let (first, second) = (&reference.level.overall, &other.level.overall);
    let absolute = both_calibrated(reference, other).then(|| {
        let laeq = Difference::of(first.laeq, second.laeq);
        let leq = Difference::of(first.leq, second.leq);
        AbsoluteLevels {
            laeq,
            leq,
            laeq_noticeable: laeq.difference.abs() > LEVEL_NOTICEABLE_DECIBELS,
            leq_noticeable: leq.difference.abs() > LEVEL_NOTICEABLE_DECIBELS,
        }
    });
    LevelDifferences {
        l10_minus_l90: Difference::of(first.l10 - first.l90, second.l10 - second.l90),
        lafmax_over_while_sounding: Difference::of(
            first.lafmax - first.while_sounding,
            second.lafmax - second.while_sounding,
        ),
        crest_decibels: Difference::of(first.crest_decibels, second.crest_decibels),
        lc_minus_la: Difference::of(first.lceq - first.laeq, second.lceq - second.laeq),
        absolute,
    }
}
