//! The report: every measure of one file, as the comparison tool will read
//! it. Plain data with public fields; `report_version` is raised when a field
//! changes meaning or goes away.
//!
//! Levels are in the report's `unit`: dB SPL when the file's calibration is
//! known, otherwise dBFS, where a signal of RMS 1.0 is 0 dBFS (so a
//! full-scale sine is −3.01 dBFS). Differences of levels are plain dB.

use serde::{Deserialize, Serialize};

/// The layout of [`Report`] this crate writes. A report JSON with another
/// `report_version` is not this layout, and `analyse --compare` refuses it.
pub const REPORT_VERSION: u32 = 2;

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
    pub movement: Movement,
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

/// How the file's tones and noise move: its partials and how each one's
/// level, pitch and interaural phase move, the slow swells of each octave,
/// their shape, and a periodic pulse if there is one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Movement {
    /// The share of the power (20 Hz to Nyquist) in the partials' bands, 0 to 1.
    pub tonal_share: f64,
    /// The strongest partials, strongest first, at most 12.
    pub partials: Vec<Partial>,
    /// Pairs of partials heard one in each ear: a binaural beat. Empty
    /// without an ear pair.
    pub binaural_beats: Vec<BinauralBeat>,
    pub slow: SlowMovement,
    pub swells: Swells,
    /// None when the envelope has no periodic pulse from 1 to 50 Hz.
    pub pulse: Option<Pulse>,
}

/// A spectral line standing at least 10 dB over its surroundings, with the
/// lines within 2 Hz and 20 dB of it that dip 6 dB apart: one tone, a
/// cluster of detuned voices, or a tone and its vibrato sidebands.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Partial {
    /// The strongest line (Hz), from 16 s spectra.
    pub frequency_hertz: f64,
    /// The band the partial fills (Hz): from 0.75 to 2 Hz past its outer
    /// lines, to where the spectrum falls 20 dB under its strongest line. A
    /// steady tone is 1.5 Hz wide; a wandering one a hump of a few hertz; a
    /// dense ensemble or a narrow noise band reaches the 2 Hz limit.
    pub low_hertz: f64,
    pub high_hertz: f64,
    pub lines: usize,
    /// The median gap between neighbouring lines (Hz); None for one line.
    pub line_spacing_hertz: Option<f64>,
    /// The cluster's power relative to the whole file's (dB, so re Leq).
    pub relative_decibels: f64,
    /// The strongest line over the median of the spectrum around it (dB).
    pub prominence_decibels: f64,
    /// How far the cluster's level moves over the file: its 90th percentile
    /// minus its 10th (dB), on a 0.25 s smoothed level. None for a file too
    /// short to say.
    pub level_p10_p90_decibels: Option<f64>,
    /// How its frequency wanders: the standard deviation of the
    /// instantaneous frequency, 0.25 s smoothed, around its median (cents).
    pub pitch_standard_deviation_cents: Option<f64>,
    pub pitch_p10_p90_cents: Option<f64>,
    /// None without an ear pair.
    pub interaural: Option<PartialInteraural>,
}

/// The partial at the two ears.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartialInteraural {
    /// Right minus left (dB).
    pub level_difference_decibels: f64,
    /// The rate the phase between the ears turns at: the left ear's
    /// frequency minus the right's (Hz), within ±5 Hz.
    pub rotation_hertz: f64,
    /// How much that rate varies between 10 s stretches: the power-weighted
    /// standard deviation of their rates (Hz). Small beside the rate for a
    /// binaural detune (one ear's voice steadily higher); as large as the
    /// rate or larger for voices that wander apart at the two ears. None for
    /// a file with fewer than two stretches.
    pub rotation_spread_hertz: Option<f64>,
}

/// A partial mostly in the left ear and one mostly in the right, within
/// 45 Hz: what a binaural beat is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BinauralBeat {
    pub left_hertz: f64,
    pub right_hertz: f64,
    /// Right minus left (Hz).
    pub beat_hertz: f64,
}

/// The level of each octave on 1 s frames every 0.5 s: how much it moves,
/// how its movement is spread over 0.01–0.5 Hz, and whether the bands move
/// together.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlowMovement {
    pub octave_bands: Vec<SlowBand>,
    /// The whole spectrum's level, 20 Hz to Nyquist.
    pub broadband_standard_deviation_decibels: f64,
    pub broadband_modulation_slope: Option<f64>,
    /// The mean correlation of neighbouring octaves' levels, and of all pairs.
    pub neighbour_correlation: Option<f64>,
    pub all_pairs_correlation: Option<f64>,
    /// The spectral centroid per frame: its spread (octaves) and its
    /// correlation with the broadband level (+ louder is brighter).
    pub centroid_standard_deviation_octaves: f64,
    pub centroid_level_correlation: Option<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlowBand {
    pub centre_hertz: f64,
    pub standard_deviation_decibels: f64,
    /// The slope of the level's power spectrum against modulation rate,
    /// 0.01 (or two cycles of the file) to 0.5 Hz, log-log: 0 for movement
    /// spread evenly over the rates, −1 for 1/f, −2 for a random walk. None
    /// for a file too short to give four third-octave rate bands.
    pub modulation_slope: Option<f64>,
}

/// Slow swells of the broadband level, 2 s smoothed, rising and falling at
/// least 3 dB.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Swells {
    pub count: usize,
    /// From the trough before to the peak, and from the peak to the trough
    /// after (s), medians over the swells.
    pub rise_median_seconds: Option<f64>,
    pub decay_median_seconds: Option<f64>,
    /// The skewness of the level's rate of change: + for quick rises and
    /// slow falls (damped), − for slow rises and quick falls (ramped).
    pub rate_of_change_skewness: Option<f64>,
}

/// A periodic pulse in the envelope: the average cycle at its rate.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Pulse {
    pub rate_hertz: f64,
    /// The rate's modulation power over the median from 1 to 50 Hz (dB).
    pub prominence_decibels: f64,
    /// (max − min) / (max + min) of the average cycle's amplitude.
    pub depth: f64,
    /// 10 to 90 % of the way up, and 90 to 10 % down (s).
    pub rise_seconds: f64,
    pub decay_seconds: f64,
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
