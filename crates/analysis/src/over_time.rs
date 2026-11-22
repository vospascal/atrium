//! The file over time: the series the summary's strip and the picture draw.

use crate::report::{OverTime, Segment};
use crate::signal::Calibration;
use crate::space::{PairOverTime, FRAME_SECONDS};
use crate::weighting::LAF_STEP_SECONDS;

/// The strip cuts the file into this many equal parts.
pub(crate) const SEGMENTS: usize = 20;

/// What the series are made from.
pub(crate) struct Series<'a> {
    pub(crate) calibration: Calibration,
    pub(crate) sample_rate: u32,
    /// The channels' A-weighted powers combined, sample by sample.
    pub(crate) a_weighted_power: &'a [f64],
    /// LAF, the largest per step, as power: all channels, then each.
    pub(crate) laf_overall: &'a [f64],
    pub(crate) laf_channels: Vec<&'a [f64]>,
    pub(crate) onset_seconds: &'a [f64],
    pub(crate) pair: Option<&'a PairOverTime>,
}

pub(crate) fn over_time(series: &Series) -> OverTime {
    let level = |power: f64| series.calibration.level(power);
    let length = series.a_weighted_power.len();
    let rate = series.sample_rate as f64;
    let (width, ild_decibels) = match series.pair {
        Some(pair) => (pair.width.clone(), pair.ild_decibels.clone()),
        None => (Vec::new(), Vec::new()),
    };
    let strip = (0..SEGMENTS)
        .map(|segment| {
            let start = segment * length / SEGMENTS;
            let end = ((segment + 1) * length / SEGMENTS).max(start);
            let start_seconds = start as f64 / rate;
            let end_seconds = end as f64 / rate;
            let laeq = level(crate::statistics::mean(
                &series.a_weighted_power[start..end],
            ));
            let onsets = series
                .onset_seconds
                .iter()
                .filter(|time| **time >= start_seconds && **time < end_seconds)
                .count();
            let widths: Vec<f64> = width
                .iter()
                .enumerate()
                .filter(|(frame, _)| {
                    let centre = (*frame as f64 + 0.5) * FRAME_SECONDS;
                    centre >= start_seconds && centre < end_seconds
                })
                .filter_map(|(_, value)| *value)
                .collect();
            Segment {
                start_seconds,
                end_seconds,
                laeq,
                onsets,
                width: (!widths.is_empty()).then(|| crate::statistics::mean(&widths)),
            }
        })
        .collect();
    OverTime {
        laf_step_seconds: LAF_STEP_SECONDS,
        laf_overall: series
            .laf_overall
            .iter()
            .map(|power| level(*power))
            .collect(),
        laf_channels: series
            .laf_channels
            .iter()
            .map(|channel| channel.iter().map(|power| level(*power)).collect())
            .collect(),
        frame_seconds: FRAME_SECONDS,
        width,
        ild_decibels,
        strip,
    }
}
