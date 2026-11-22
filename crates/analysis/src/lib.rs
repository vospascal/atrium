//! One report for a recording or a render: level, colour, texture, events
//! and space, each from a published definition, as numbers (`Report`, JSON
//! for the comparison tool), as a one-screen text (`summary`) and as a
//! picture (`picture`). Reports side by side, where the others differ from a
//! reference (`compare`, `comparison_summary`, `comparison_picture`). The
//! same measures on a stream, a column every 50 ms while a scene plays
//! (`Live`).
//!
//! Pure functions on samples: no engine type, no file. The measures cannot
//! depend on the engine they measure, and each is tested against a signal
//! whose answer is known by hand. The design record is the crate README.

mod analyse;
mod bands;
mod compare;
mod envelope;
mod frequency_rows;
mod level;
mod live;
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
    compare, comparison_summary, AbsoluteLevels, BandDifference, ClassDifference,
    ColourDifferences, Compared, Comparison, Difference, Differences, EventDifferences, FileFacts,
    LevelDifferences, Moment, OctaveDifference, OffsetDifference, OverTimeDifferences,
    PercentileDifference, Place, RateDifference, Ratio, SpaceDifferences, TextureClass,
    TextureDifferences,
};
pub use level::power_while_sounding;
pub use live::{Column, Live};
pub use picture::{comparison_picture, picture};
pub use report::{
    BandLevel, Binaural, BinauralBand, Colour, Distribution, Events, Level, LevelUnit, Levels,
    ModulationCorrelation, OverTime, Report, Segment, SignalFacts, Space, Subband,
    SubbandCorrelation, Texture, REPORT_VERSION,
};
pub use signal::{Settings, Signal};
pub use summary::summary;
