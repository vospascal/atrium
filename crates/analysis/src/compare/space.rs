//! Space: for two two-channel files, how alike the ears are per octave and
//! how wide the whole is. A 5.1 file's L/R pair is two loudspeakers, not two
//! ears; against a pair of ears its IACC would compare different things.

use super::comparison::{Difference, OctaveDifference, SpaceDifferences};
use crate::report::{Binaural, Report};

/// The just-noticeable difference of IACC (Cox, Davies and Lam 1993), and so
/// of width, which is 1 − IACC.
pub(crate) const WIDTH_NOTICEABLE: f64 = 0.075;

pub(super) fn compare(reference: &Report, other: &Report) -> Option<SpaceDifferences> {
    let first = ears(reference)?;
    let second = ears(other)?;
    let octave_bands = first
        .octave_bands
        .iter()
        .filter_map(|one| {
            let two = second
                .octave_bands
                .iter()
                .find(|two| two.centre_hertz == one.centre_hertz)?;
            Some(OctaveDifference {
                centre_hertz: one.centre_hertz,
                iacc: Difference::of_both(one.iacc, two.iacc),
                envelope_coherence: Difference::of_both(
                    one.envelope_coherence,
                    two.envelope_coherence,
                ),
                ild_spread_decibels: Difference::of_both(
                    one.ild_standard_deviation_decibels,
                    two.ild_standard_deviation_decibels,
                ),
            })
        })
        .collect();
    let width = Difference::of_both(first.width, second.width);
    Some(SpaceDifferences {
        octave_bands,
        width,
        width_noticeable: width.is_some_and(|width| width.difference.abs() > WIDTH_NOTICEABLE),
    })
}

/// The pair of a two-channel file.
pub(crate) fn ears(report: &Report) -> Option<&Binaural> {
    if report.signal.channel_names.len() != 2 {
        return None;
    }
    report.space.as_ref()?.pair.as_ref()
}
