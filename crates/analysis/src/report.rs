//! The report: every measure of one file, as the comparison tool will read
//! it. Plain data with public fields; `report_version` is raised when a field
//! changes meaning or goes away.
//!
//! Levels are in the report's `unit`: dB SPL when the file's calibration is
//! known, otherwise dBFS, where a signal of RMS 1.0 is 0 dBFS (so a
//! full-scale sine is −3.01 dBFS). Differences of levels are plain dB.

use serde::{Deserialize, Serialize};

/// The layout of [`Report`] this crate writes.
pub(crate) const REPORT_VERSION: u32 = 1;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Report {
    pub report_version: u32,
    pub signal: SignalFacts,
    pub unit: LevelUnit,
    pub level: Level,
    pub colour: Colour,
    pub texture: Texture,
    pub events: Events,
    /// None for one channel.
    pub space: Option<Space>,
    pub over_time: OverTime,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SignalFacts {
    pub channel_names: Vec<String>,
    pub sample_rate: u32,
    pub seconds: f64,
    /// dB SPL of RMS 1.0, when known; levels are dB SPL then.
    pub full_scale_db_spl: Option<f64>,
    /// How the channels make the overall level: `single`, `ears_mean` (two
    /// channels: the mean of the ears' powers) or `room_sum` (three or more:
    /// the sum of the speakers' powers).
    pub combination: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub enum LevelUnit {
    #[serde(rename = "dB SPL")]
    SoundPressureLevel,
    #[serde(rename = "dBFS")]
    FullScale,
}

impl LevelUnit {
    pub(crate) fn symbol(self) -> &'static str {
        match self {
            Self::SoundPressureLevel => "dB SPL",
            Self::FullScale => "dBFS",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Level {
    /// The channels together, combined as `signal.combination` says.
    pub overall: Levels,
    pub channels: Vec<Levels>,
    /// The gate below which a 400 ms block is silence for `while_sounding`,
    /// in the report's unit: −70 dBFS (BS.1770) without calibration, 0 dB
    /// SPL with it.
    pub absolute_gate: f64,
}

/// The levels of one channel or of all together, in the report's unit.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Levels {
    /// The channel's name, or `overall`.
    pub channel: String,
    /// Unweighted equivalent level.
    pub leq: f64,
    /// A-weighted (IEC 61672-1).
    pub laeq: f64,
    /// C-weighted; LCeq − LAeq is the low-frequency content.
    pub lceq: f64,
    /// Unweighted, over the 400 ms blocks that pass BS.1770's gates.
    pub while_sounding: f64,
    /// The largest A-weighted FAST (125 ms) level.
    pub lafmax: f64,
    /// LAF exceeded 10, 50 and 90 % of the time (ISO 1996-1).
    pub l10: f64,
    pub l50: f64,
    pub l90: f64,
    /// The largest sample, as a level.
    pub peak: f64,
    /// Peak over Leq (dB).
    pub crest_decibels: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Colour {
    /// IEC 61260-1 third octaves, 25 Hz to 20 kHz, those below Nyquist.
    pub third_octaves: Vec<BandLevel>,
    /// Power-weighted mean frequency, 20 Hz to 20 kHz (Hz).
    pub centroid_hertz: f64,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BandLevel {
    /// The nominal centre (Hz).
    pub centre_hertz: f64,
    /// The band's long-term level, in the report's unit.
    pub level: f64,
    /// The band's level relative to the overall Leq (dB).
    pub relative_decibels: f64,
}

/// McDermott and Simoncelli's (2011) texture statistics, on 32 cochlear
/// subband envelopes compressed by 0.3 and sampled at 400 Hz. For two or
/// more channels each statistic is the power-weighted mean of the channels'.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Texture {
    pub subbands: Vec<Subband>,
    /// The centres of the 20 modulation filters (Hz), 0.5 to 200, Q = 2.
    pub modulation_centres_hertz: Vec<f64>,
    /// The cross-band envelope correlation C at subband offsets 1, 2, 3, 5,
    /// 8, 11, 16 and 21.
    pub correlations: Vec<SubbandCorrelation>,
    /// C1 per octave modulation band, 3.125 to 100 Hz (Q = √2).
    pub c1: Vec<ModulationCorrelation>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Subband {
    /// Where the filter passes anything and where it is centred (Hz).
    pub low_hertz: f64,
    pub centre_hertz: f64,
    pub high_hertz: f64,
    /// M1: the compressed envelope's mean (full scale = 1).
    pub mean: f64,
    /// M2: variance over the squared mean.
    pub variance_over_mean_squared: f64,
    /// M3.
    pub skewness: f64,
    /// M4, not excess: a Gaussian's is 3.
    pub kurtosis: f64,
    /// Power in each modulation filter over the envelope's variance.
    pub modulation_power: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SubbandCorrelation {
    /// k − j.
    pub offset: usize,
    /// C between subband j and j + offset, for every j that has a partner.
    pub values: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ModulationCorrelation {
    pub modulation_centre_hertz: f64,
    /// C1 between subbands j and j + 1.
    pub neighbours: Vec<f64>,
    /// C1 between subbands j and j + 2.
    pub second_neighbours: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Events {
    /// Onsets as librosa's `onset_detect` finds them: spectral flux on a log
    /// mel spectrogram, peak-picked.
    pub onset_seconds: Vec<f64>,
    pub onsets_per_minute: f64,
    /// The intervals between onsets (s); None with fewer than two onsets.
    pub interval_median_seconds: Option<f64>,
    pub interval_p10_seconds: Option<f64>,
    pub interval_p90_seconds: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Space {
    /// The ears, or the L/R pair of a speaker layout.
    pub pair: Option<Binaural>,
    /// Front against surround, for a layout with Ls and Rs.
    pub distribution: Option<Distribution>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Binaural {
    pub left: String,
    pub right: String,
    /// Octave bands 125 Hz to 8 kHz.
    pub octave_bands: Vec<BinauralBand>,
    /// 1 − IACC averaged over 500 Hz, 1 kHz and 2 kHz (Hidaka et al. 1995).
    pub width: Option<f64>,
    /// The lag of the largest interaural correlation, 88 Hz to 1.4 kHz, over
    /// the 100 ms frames whose IACC there exceeds 0.5 (ms; positive when
    /// the left ear leads, a source on the left).
    pub itd_median_milliseconds: Option<f64>,
    pub itd_interquartile_milliseconds: Option<f64>,
    /// The frames the ITD is taken over, and all frames.
    pub itd_frames: usize,
    pub frames: usize,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BinauralBand {
    pub centre_hertz: f64,
    /// ISO 3382-1: the largest |normalised cross-correlation| within ±1 ms,
    /// over the whole file. None for a silent band.
    pub iacc: Option<f64>,
    /// The median of the same over 100 ms frames.
    pub iacc_median_of_frames: Option<f64>,
    /// The correlation of the two ears' band envelopes (Hilbert, 400 Hz,
    /// uncompressed) over the whole file: whether the band's level moves
    /// together at both ears, which is what is heard above 1.5 kHz.
    pub envelope_coherence: Option<f64>,
    /// Right minus left (dB), mean and standard deviation over 100 ms frames.
    pub ild_mean_decibels: Option<f64>,
    pub ild_standard_deviation_decibels: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Distribution {
    /// L, C and R together, in the report's unit.
    pub front: f64,
    /// Ls and Rs together.
    pub surround: f64,
    pub front_minus_surround_decibels: f64,
}

/// The file over time: what the summary's strip and the picture draw.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverTime {
    /// LAF, the largest in each step, in the report's unit.
    pub laf_step_seconds: f64,
    pub laf_overall: Vec<f64>,
    pub laf_channels: Vec<Vec<f64>>,
    /// Width and broadband ILD (right minus left, dB) per 100 ms frame of
    /// the pair; empty without one, None for a silent frame.
    pub frame_seconds: f64,
    pub width: Vec<Option<f64>>,
    pub ild_decibels: Vec<Option<f64>>,
    /// The file in twenty equal parts.
    pub strip: Vec<Segment>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Segment {
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// In the report's unit.
    pub laeq: f64,
    pub onsets: usize,
    /// The mean width of the segment's frames, for a pair.
    pub width: Option<f64>,
}
