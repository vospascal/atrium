//! The comparison: where other files differ from a reference, as the
//! comparison JSON holds it. Plain data with public fields;
//! `comparison_version` is raised when a field changes meaning or goes away.
//!
//! Every difference is the other file's value minus the reference's, so a
//! positive number means "more in the other file".

use serde::{Deserialize, Serialize};

use crate::report::{LevelUnit, Pulse};

/// The layout of [`Comparison`] this crate writes.
pub(crate) const COMPARISON_VERSION: u32 = 2;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Comparison {
    pub comparison_version: u32,
    pub reference: FileFacts,
    /// One entry per other file, in the order given.
    pub others: Vec<Differences>,
}

/// What a compared file is.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FileFacts {
    /// Short: the summary's columns and the picture's legend.
    pub name: String,
    /// The file and how it was made, in one line.
    pub heading: String,
    pub channels: usize,
    pub sample_rate: u32,
    pub seconds: f64,
    /// dB SPL when the file's calibration is known, else dBFS.
    pub unit: LevelUnit,
}

/// A value of the reference, the other file's, and the other's minus the
/// reference's.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Difference {
    pub reference: f64,
    pub other: f64,
    pub difference: f64,
}

impl Difference {
    pub(crate) fn of(reference: f64, other: f64) -> Self {
        Self {
            reference,
            other,
            difference: other - reference,
        }
    }

    /// A difference of two values either of which may be missing.
    pub(crate) fn of_both(reference: Option<f64>, other: Option<f64>) -> Option<Self> {
        Some(Self::of(reference?, other?))
    }
}

/// Where one other file differs from the reference.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Differences {
    pub file: FileFacts,
    /// Whether the two are on one timeline (renders of the same scene and
    /// seed, over the same stretch), so their series compare moment by
    /// moment.
    pub lines_up: bool,
    pub level: LevelDifferences,
    pub colour: ColourDifferences,
    pub texture: TextureDifferences,
    pub events: EventDifferences,
    /// Both files two channels only.
    pub space: Option<SpaceDifferences>,
    pub movement: MovementDifferences,
    pub over_time: OverTimeDifferences,
}

/// Level as shapes (dB differences of differences, the same in dBFS and in
/// dB SPL), and absolute level when both files are calibrated.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct LevelDifferences {
    /// L10 − L90 (dB): how eventful.
    pub l10_minus_l90: Difference,
    /// LAFmax over the level while sounding (dB): how far the loudest
    /// moment stands out.
    pub lafmax_over_while_sounding: Difference,
    /// Peak over Leq (dB).
    pub crest_decibels: Difference,
    /// LCeq − LAeq (dB): the low end.
    pub lc_minus_la: Difference,
    /// None unless both files carry a calibration: a recording's dBFS says
    /// nothing of its sound pressure.
    pub absolute: Option<AbsoluteLevels>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AbsoluteLevels {
    /// In dB SPL.
    pub laeq: Difference,
    pub leq: Difference,
    /// Whether the difference is beyond 1 dB, the just-noticeable
    /// difference ISO 3382-1 gives for level.
    pub laeq_noticeable: bool,
    pub leq_noticeable: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ColourDifferences {
    /// The third octaves both files have, each relative to its own file's
    /// Leq (dB), floored at −100 dB.
    pub bands: Vec<BandDifference>,
    /// The RMS of the band differences from 50 Hz to 16 kHz (dB).
    pub rms_difference_decibels: f64,
    /// The slope of a line fitted to each file's band levels over log
    /// frequency, 100 Hz to 10 kHz (dB per octave): white noise +3, pink 0.
    pub tilt_decibels_per_octave: Difference,
    pub centroid_hertz: Ratio,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct BandDifference {
    /// The nominal centre (Hz).
    pub centre_hertz: f64,
    pub decibels: Difference,
}

/// Two values and the other's over the reference's.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Ratio {
    pub reference: f64,
    pub other: f64,
    /// None when the reference is zero.
    pub ratio: Option<f64>,
}

impl Ratio {
    pub(crate) fn of(reference: f64, other: f64) -> Self {
        Self {
            reference,
            other,
            ratio: (reference != 0.0).then(|| other / reference),
        }
    }
}

/// McDermott and Simoncelli's (2011) statistic classes, each compared by the
/// paper's own measure of how far one set of statistics is from another.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct TextureDifferences {
    /// In the paper's order: M1, M2, M3, M4, C, modulation power, C1.
    pub classes: Vec<ClassDifference>,
    /// The overall level difference (dB, other minus reference, unweighted
    /// Leq in full-scale terms) taken out of the other file's envelope means
    /// before M1 is compared: the means follow the level, the colour
    /// comparison is relative to Leq, and so is this.
    pub level_removed_decibels: f64,
    /// Modulation power per filter, mean over the sounding subbands, as a
    /// fraction of the envelope's variance.
    pub modulation_spectrum: Vec<RateDifference>,
    /// C, the mean over every pair of subbands this far apart.
    pub correlation_by_offset: Vec<OffsetDifference>,
    /// C1 per octave modulation band, the mean over neighbouring subbands.
    pub modulation_correlation_by_band: Vec<RateDifference>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ClassDifference {
    pub class: TextureClass,
    /// How many statistics the class holds.
    pub statistics: usize,
    /// 10·log10(Σ s² / Σ (s − s′)²) over the class's statistics, s the
    /// reference's (dB). None when the statistics are identical: no error.
    pub snr_decibels: Option<f64>,
    /// At or above 20 dB, or identical: what the paper took as converged.
    pub converged: bool,
    /// The SNR two takes of the reference's sound reach by chance, estimated
    /// from its first half against its second (dB; see the crate README).
    /// Statistics near zero (C, C1, M3 of wide subbands) scatter, and two
    /// takes of one noise reach only 5 to 20 dB in them. None when the
    /// reference's halves were not analysed (a report read from its JSON).
    pub scatter_floor_decibels: Option<f64>,
    /// Whether the SNR is at or above the scatter floor, less 3 dB for the
    /// floor's own uncertainty: the files differ no more than two stretches
    /// of the same sound would.
    pub within_scatter: bool,
    /// Where the error is largest, largest first, at most three.
    pub largest: Vec<Place>,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TextureClass {
    /// M1, the envelope mean, after the level difference is removed.
    EnvelopeMean,
    /// M2, variance over the squared mean.
    Variance,
    /// M3.
    Skewness,
    /// M4.
    Kurtosis,
    /// C, the correlation of subband envelopes.
    Correlation,
    /// Modulation power per subband and filter.
    ModulationPower,
    /// C1, the correlation of neighbouring subbands' modulation bands.
    ModulationCorrelation,
}

impl TextureClass {
    pub(crate) const ALL: [Self; 7] = [
        Self::EnvelopeMean,
        Self::Variance,
        Self::Skewness,
        Self::Kurtosis,
        Self::Correlation,
        Self::ModulationPower,
        Self::ModulationCorrelation,
    ];

    /// Its name as the paper writes it.
    pub(crate) fn symbol(self) -> &'static str {
        match self {
            Self::EnvelopeMean => "M1",
            Self::Variance => "M2",
            Self::Skewness => "M3",
            Self::Kurtosis => "M4",
            Self::Correlation => "C",
            Self::ModulationPower => "mod. power",
            Self::ModulationCorrelation => "C1",
        }
    }
}

/// One place a class's error lies: a subband, a pair of subbands or a
/// modulation rate.
#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Place {
    /// The subband's centre (Hz) for M1 to M4; the lower subband of the pair
    /// for C; the modulation rate for modulation power and C1.
    pub hertz: f64,
    /// For C: the upper subband's centre.
    pub partner_hertz: Option<f64>,
    /// The statistic, or for a modulation rate its mean over the subbands
    /// (modulation power) or over the pairs (C1).
    pub values: Difference,
    /// This place's part of the class's summed squared error, 0 to 1.
    pub share_of_error: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct RateDifference {
    pub modulation_hertz: f64,
    pub values: Difference,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OffsetDifference {
    /// k − j.
    pub offset: usize,
    pub values: Difference,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct EventDifferences {
    pub onsets_per_minute: Difference,
    /// The other's rate over the reference's; None when the reference has
    /// no onsets.
    pub onsets_ratio: Option<f64>,
    /// The intervals between onsets (s); None unless both have two or more
    /// onsets.
    pub interval_median_seconds: Option<Difference>,
    pub interval_p10_seconds: Option<Difference>,
    pub interval_p90_seconds: Option<Difference>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SpaceDifferences {
    /// The octave bands both files have.
    pub octave_bands: Vec<OctaveDifference>,
    /// 1 − IACC over 500 Hz, 1 and 2 kHz.
    pub width: Option<Difference>,
    /// Whether the width difference is beyond 0.075, the just-noticeable
    /// difference of IACC (Cox, Davies and Lam 1993).
    pub width_noticeable: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct OctaveDifference {
    pub centre_hertz: f64,
    pub iacc: Option<Difference>,
    pub envelope_coherence: Option<Difference>,
    /// The ILD's standard deviation over 100 ms frames (dB).
    pub ild_spread_decibels: Option<Difference>,
}

/// The series over time, compared as distributions (two recordings do not
/// line up), and moment by moment when the two files line up.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct OverTimeDifferences {
    /// Whether the LAF values are each relative to its own file's L50 (dB),
    /// as they are unless both files are calibrated (dB SPL then).
    pub laf_relative_to_l50: bool,
    /// LN, the LAF exceeded N % of the time, N from 95 down to 5, from the
    /// report's 25 ms LAF series.
    pub laf_exceeded: Vec<PercentileDifference>,
    /// The width per 100 ms frame exceeded 90, 50 and 10 % of the time;
    /// empty unless both files have a pair.
    pub width_exceeded: Vec<PercentileDifference>,
    /// When the two line up: where the LAF differs most, and the width.
    pub largest_laf: Option<Moment>,
    pub largest_width: Option<Moment>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct PercentileDifference {
    /// N: the value is exceeded N % of the time.
    pub exceeded_percent: f64,
    pub values: Difference,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct Moment {
    /// The start of the step or frame (s).
    pub seconds: f64,
    pub values: Difference,
}

/// How the tones and noise move, compared. Two files' partials seldom sit
/// at the same frequencies, so they are compared as a whole (medians over
/// each file's partials) and, where both have a partial at the same pitch,
/// one to one.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct MovementDifferences {
    /// The share of the power in the partials, 0 to 1.
    pub tonal_share: Difference,
    pub partials: PartialsDifferences,
    /// Partials within 3 % of each other's frequency, nearest first, lowest
    /// reference frequency first.
    pub matched_partials: Vec<PartialDifference>,
    /// Each file's binaural beats (Hz, right minus left).
    pub binaural_beats: BeatLists,
    pub slow: SlowDifferences,
    pub swells: SwellDifferences,
    pub pulse: PulseDifferences,
}

/// Medians over each file's partials. None when either file has none.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartialsDifferences {
    pub count: Difference,
    pub lines_median: Option<Difference>,
    /// The −20 dB width (Hz).
    pub width_median_hertz: Option<Difference>,
    pub level_p10_p90_median_decibels: Option<Difference>,
    pub pitch_standard_deviation_median_cents: Option<Difference>,
    /// How many partials are detuned between the ears: the phase between
    /// them turns at 0.3 Hz or more, varying by less than half that from one
    /// 10 s stretch to the next. None unless both files have an ear pair.
    pub detuned: Option<Difference>,
}

/// One partial in both files.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PartialDifference {
    pub frequency_hertz: Difference,
    /// Its power relative to its own file's (dB).
    pub relative_decibels: Difference,
    pub width_hertz: Difference,
    pub level_p10_p90_decibels: Option<Difference>,
    pub pitch_standard_deviation_cents: Option<Difference>,
    /// Left minus right (Hz), when both files have an ear pair.
    pub rotation_hertz: Option<Difference>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct BeatLists {
    pub reference: Vec<f64>,
    pub other: Vec<f64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlowDifferences {
    /// The octaves both files have.
    pub octave_bands: Vec<SlowBandDifference>,
    pub broadband_standard_deviation_decibels: Difference,
    pub broadband_modulation_slope: Option<Difference>,
    pub neighbour_correlation: Option<Difference>,
    pub all_pairs_correlation: Option<Difference>,
    pub centroid_standard_deviation_octaves: Difference,
    pub centroid_level_correlation: Option<Difference>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize, Deserialize)]
pub struct SlowBandDifference {
    pub centre_hertz: f64,
    pub standard_deviation_decibels: Difference,
    pub modulation_slope: Option<Difference>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct SwellDifferences {
    /// Swells a minute: files of different lengths compare.
    pub per_minute: Difference,
    pub rise_median_seconds: Option<Difference>,
    pub decay_median_seconds: Option<Difference>,
    pub rate_of_change_skewness: Option<Difference>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct PulseDifferences {
    pub reference: Option<Pulse>,
    pub other: Option<Pulse>,
    /// None unless both files pulse.
    pub rate_hertz: Option<Difference>,
    pub depth: Option<Difference>,
    pub rise_seconds: Option<Difference>,
    pub decay_seconds: Option<Difference>,
}
