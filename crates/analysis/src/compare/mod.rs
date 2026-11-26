//! The comparison: two or more reports side by side, where the others
//! differ from the first. It reports differences; only listening judges
//! whether a version is better.
//!
//! - `comparison`: the types the comparison JSON holds
//! - one file per group: `level`, `colour`, `texture`, `events`, `space`,
//!   `movement`, `over_time`
//! - `summary`: the one-screen text

mod colour;
mod comparison;
mod events;
mod level;
mod movement;
mod over_time;
mod space;
mod summary;
mod texture;

#[cfg(test)]
mod tests;

pub use comparison::{
    AbsoluteLevels, BandDifference, BeatLists, ClassDifference, ColourDifferences, Comparison,
    Difference, Differences, EventDifferences, FileFacts, LevelDifferences, Moment,
    MovementDifferences, OctaveDifference, OffsetDifference, OverTimeDifferences,
    PartialDifference, PartialsDifferences, PercentileDifference, Place, PulseDifferences,
    RateDifference, Ratio, SlowBandDifference, SlowDifferences, SpaceDifferences, SwellDifferences,
    TextureClass, TextureDifferences,
};
pub use summary::comparison_summary;

pub(crate) use colour::relative_band_levels;
pub(crate) use over_time::laf_exceeded;
pub(crate) use texture::{correlation_at_offset, modulation_spectrum};

use crate::report::Report;
use crate::signal::Signal;
use comparison::COMPARISON_VERSION;

/// One file to compare: its report and what it is.
#[derive(Clone, Debug)]
pub struct Compared<'a> {
    /// Short, for the summary's columns and the picture's legend.
    pub name: String,
    /// The file and how it was made, in one line.
    pub heading: String,
    pub report: &'a Report,
    /// The file's first and second half, each analysed apart: how far two
    /// stretches of the same sound lie apart, which sets the texture's
    /// scatter floor. Read for the reference only, which says what the sound
    /// is; None for a report read from its JSON.
    pub halves: Option<[&'a Report; 2]>,
    /// What fixes when things happen in the file: for a render, its scene,
    /// seed and stretch. Two files with the same timeline line up moment by
    /// moment; None for a recording, which lines up with nothing.
    pub timeline: Option<String>,
    /// The samples, for the picture's spectrogram; None for a report read
    /// from its JSON.
    pub signal: Option<Signal<'a>>,
}

impl Compared<'_> {
    fn facts(&self) -> FileFacts {
        let signal = &self.report.signal;
        FileFacts {
            name: self.name.clone(),
            heading: self.heading.clone(),
            channels: signal.channel_names.len(),
            sample_rate: signal.sample_rate,
            seconds: signal.seconds,
            unit: self.report.unit,
        }
    }
}

/// Where each of `others` differs from `reference`. Pure: the same reports
/// give the same comparison.
pub fn compare(reference: &Compared, others: &[Compared]) -> Comparison {
    Comparison {
        comparison_version: COMPARISON_VERSION,
        reference: reference.facts(),
        others: others
            .iter()
            .map(|other| differences(reference, other))
            .collect(),
    }
}

fn differences(reference: &Compared, other: &Compared) -> Differences {
    let lines_up = lines_up(reference, other);
    let (first, second) = (reference.report, other.report);
    Differences {
        file: other.facts(),
        lines_up,
        level: level::compare(first, second),
        colour: colour::compare(first, second),
        texture: texture::compare(reference, other),
        events: events::compare(first, second),
        space: space::compare(first, second),
        movement: movement::compare(first, second),
        over_time: over_time::compare(first, second, lines_up),
    }
}

/// Two files line up when both say what fixes their timing and say the same,
/// and they are as long as each other.
fn lines_up(reference: &Compared, other: &Compared) -> bool {
    reference.timeline.is_some()
        && reference.timeline == other.timeline
        && (reference.report.signal.seconds - other.report.signal.seconds).abs() < 1e-6
}

/// The colours that tell the files apart, in the picture and by name in the
/// summary: the reference black, the others Tableau 10's blue, orange,
/// green, purple and brown (matplotlib's default cycle, without its red,
/// which the analyser's picture uses for the right ear), repeating after
/// six.
const FILE_COLOURS: [(&str, [u8; 3]); 6] = [
    ("black", [0, 0, 0]),
    ("blue", [31, 119, 180]),
    ("orange", [255, 127, 14]),
    ("green", [44, 160, 44]),
    ("purple", [148, 103, 189]),
    ("brown", [140, 86, 75]),
];

/// The name and colour of the file at `index`: 0 the reference, 1 the first
/// other, and so on.
pub(crate) fn file_colour(index: usize) -> (&'static str, [u8; 3]) {
    if index == 0 {
        FILE_COLOURS[0]
    } else {
        FILE_COLOURS[1 + (index - 1) % (FILE_COLOURS.len() - 1)]
    }
}

/// Whether both files' levels are in dB SPL.
pub(crate) fn both_calibrated(reference: &Report, other: &Report) -> bool {
    reference.signal.full_scale_db_spl.is_some() && other.signal.full_scale_db_spl.is_some()
}
