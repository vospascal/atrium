//! One report for a recording or a render: level, colour, texture, events,
//! movement and space, each from a published definition, as numbers (`Report`, JSON
//! for the comparison tool), as a one-screen text (`summary`) and as a
//! picture (`picture`). Reports side by side, where the others differ from a
//! reference (`compare`, `comparison_summary`, `comparison_picture`). A long
//! recording cut into its takes, rests, types and bouts (`cut`,
//! `cut_summary`, `cut_picture`). The same measures on a stream, a column
//! every 50 ms while a scene plays (`Live`).
//!
//! Pure functions on samples: no engine type, no file. The measures cannot
//! depend on the engine they measure, and each is tested against a signal
//! whose answer is known by hand. The design record is the crate README.

mod analyse;
mod bands;
mod compare;
mod cut;
mod envelope;
mod frequency_rows;
mod level;
mod live;
mod movement;
mod onsets;
mod over_time;
mod picture;
mod report;
mod signal;
mod space;
mod spectrum;
mod statistics;
mod summary;
mod texture;
mod weighting;

#[cfg(test)]
mod test_signals;

pub use analyse::analyse;
pub use compare::{
    compare, comparison_summary, AbsoluteLevels, BandDifference, BeatLists, ClassDifference,
    ColourDifferences, Compared, Comparison, Difference, Differences, EventDifferences, FileFacts,
    LevelDifferences, Moment, MovementDifferences, OctaveDifference, OffsetDifference,
    OverTimeDifferences, PartialDifference, PartialsDifferences, PercentileDifference, Place,
    PulseDifferences, RateDifference, Ratio, SlowBandDifference, SlowDifferences, SpaceDifferences,
    SwellDifferences, TextureClass, TextureDifferences,
};
pub use cut::{
    cut, cut_summary, BandSpread, BoutCriterion, Bouts, CriterionDecision, Cut, CutSettings,
    DetectionBand, DropReason, Dropped, Event, Gaps, LevelSeries, OneProcess, Process, Run, Take,
    Thresholds, TwoProcesses, TypeGroup, Types,
};
pub use level::power_while_sounding;
pub use live::{Column, Live};
pub use picture::{comparison_picture, cut_picture, picture};
pub use report::{
    BandLevel, Binaural, BinauralBand, BinauralBeat, Colour, Distribution, Events, Level,
    LevelUnit, Levels, ModulationCorrelation, Movement, OverTime, Partial, PartialInteraural,
    Pulse, Report, Segment, SignalFacts, SlowBand, SlowMovement, Space, Subband,
    SubbandCorrelation, Swells, Texture, REPORT_VERSION,
};
pub use signal::{Settings, Signal};
pub use summary::summary;
