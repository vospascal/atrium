//! One report for a recording or a render: level, colour, texture, events
//! and space, each from a published definition, as numbers (`Report`, JSON
//! for the comparison tool), as a one-screen text (`summary`) and as a
//! picture (`picture`). The same measures on a stream, a column every 50 ms
//! while a scene plays (`Live`).
//!
//! Pure functions on samples: no engine type, no file. The measures cannot
//! depend on the engine they measure, and each is tested against a signal
//! whose answer is known by hand. The design record is the crate README.

mod analyse;
mod bands;
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
pub use level::power_while_sounding;
pub use live::{Column, Live};
pub use picture::picture;
pub use report::{
    BandLevel, Binaural, BinauralBand, Colour, Distribution, Events, Level, LevelUnit, Levels,
    ModulationCorrelation, OverTime, Report, Segment, SignalFacts, Space, Subband,
    SubbandCorrelation, Texture,
};
pub use signal::{Settings, Signal};
pub use summary::summary;
