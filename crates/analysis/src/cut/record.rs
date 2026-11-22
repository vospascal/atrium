//! What a cut holds, as cut.json writes it: every decision with the values
//! behind it, and where each take lies in the recording. Plain data with
//! public fields; `cut_version` is raised when a field changes meaning or
//! goes away.
//!
//! Levels are dB re full scale, where a signal of RMS 1.0 is 0 dB, as the
//! analyser counts them; a recording's level says nothing of its SPL.

use serde::Serialize;

use crate::signal::Signal;

/// The layout of [`Cut`] this crate writes.
pub(crate) const CUT_VERSION: u32 = 1;

/// What to cut with: everything else is fixed and recorded in [`Thresholds`].
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct CutSettings {
    /// Find the events between these frequencies (Hz) instead of in the
    /// third octaves where they stand furthest above the bed.
    pub band: Option<(f64, f64)>,
}

/// A recording cut into takes.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Cut {
    pub cut_version: u32,
    pub sample_rate: u32,
    pub channels: usize,
    pub seconds: f64,
    pub thresholds: Thresholds,
    pub band: DetectionBand,
    /// The band level and its local bed over time.
    pub level: LevelSeries,
    /// Every event found, before any are joined.
    pub events: Vec<Event>,
    pub criterion: BoutCriterion,
    /// The takes kept, in the order they lie in the recording.
    pub takes: Vec<Take>,
    pub dropped: Vec<Dropped>,
    pub gaps: Gaps,
    pub types: Types,
    pub bouts: Bouts,
}

/// The fixed values every decision was made with, so a cut.json says what
/// made it.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Thresholds {
    pub bed_percentile: f64,
    pub bed_window_seconds: f64,
    pub start_above_bed_decibels: f64,
    pub end_above_bed_decibels: f64,
    pub edge_above_bed_decibels: f64,
    pub longest_attack_seconds: f64,
    pub longest_decay_seconds: f64,
    pub fade_seconds: f64,
    pub shortest_take_seconds: f64,
    pub flag_below_snr_decibels: f64,
    /// Bands whose L1 − L90 lies within this of the largest are detected in.
    pub band_within_decibels: f64,
    /// Average similarity at or above which takes are one type. Provisional:
    /// set on synthetic songs, not yet on an annotated recording.
    pub type_similarity: f64,
}

/// Where the events were found.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct DetectionBand {
    /// The lowest and highest frequency detected in (Hz).
    pub low_hertz: f64,
    pub high_hertz: f64,
    /// The stretches of frequency detected in, `[low, high]` Hz each; one
    /// unless the chosen third octaves are apart.
    pub ranges: Vec<[f64; 2]>,
    /// The ranges' widths together (Hz).
    pub bandwidth_hertz: f64,
    /// Whether `--band` gave it; otherwise chosen from `bands`.
    pub given: bool,
    /// How long each level reading averages (s): long enough for ten degrees
    /// of freedom in the band, so noise alone never reaches the start
    /// threshold.
    pub frame_seconds: f64,
    /// Every third octave measured, when the band was chosen.
    pub bands: Vec<BandSpread>,
}

/// How eventful one third octave is.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct BandSpread {
    pub nominal_hertz: f64,
    pub low_hertz: f64,
    pub high_hertz: f64,
    /// L1 − L90 of its level (dB): how far its loudest 1 % stands above its bed.
    pub l1_minus_l90: f64,
    pub chosen: bool,
}

/// The band level and its local bed, one value every `step_seconds` from
/// the start: the level the largest in the step, the bed at the step's
/// start.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct LevelSeries {
    pub step_seconds: f64,
    pub band_decibels: Vec<f64>,
    pub bed_decibels: Vec<f64>,
}

/// One stretch where the band level rose above the start threshold, until
/// it fell below the end threshold.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Event {
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// The highest the band level stood above the bed in it (dB).
    pub peak_above_bed_decibels: f64,
}

/// Whether two gaps between events are one take or a rest.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct BoutCriterion {
    /// The gaps between consecutive events (s), end to start.
    pub gap_seconds: Vec<f64>,
    /// One Poisson process fitted to the gaps; None without gaps.
    pub one: Option<OneProcess>,
    /// Two fitted together; None with fewer than three gaps.
    pub two: Option<TwoProcesses>,
    pub decision: CriterionDecision,
    /// Gaps shorter than this join two events into one take; None when
    /// nothing is joined.
    pub criterion_seconds: Option<f64>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct OneProcess {
    /// Events a second.
    pub rate: f64,
    pub log_likelihood: f64,
    /// Schwarz's Bayesian information criterion: lower fits better for what
    /// it costs.
    pub bic: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct TwoProcesses {
    /// The gaps inside a take.
    pub fast: Process,
    /// The rests between takes.
    pub slow: Process,
    pub log_likelihood: f64,
    pub bic: f64,
    /// Whether the fitted distribution of log gaps has two peaks, a dip
    /// between the two kinds of gap.
    pub two_peaks: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Process {
    /// Events a second.
    pub rate: f64,
    /// The share of the gaps it draws.
    pub share: f64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CriterionDecision {
    /// Fewer than three gaps: nothing to fit.
    TooFewGaps,
    /// One process fits as well as two for what two cost.
    OneProcess,
    /// Two fit better, but the log gaps have one peak: not two kinds.
    OnePeak,
    /// Two kinds of gap, and the criterion between them.
    TwoProcesses,
}

/// A take: a stretch of the recording written to its own file.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Take {
    /// 1, 2, …: the take's file is take-NN.wav.
    pub number: usize,
    /// Its type and its number in the type: "A 1".
    pub name: String,
    #[serde(rename = "type")]
    pub type_name: String,
    /// The file's first frame in the signal cut and the frame after its
    /// last; its fades lie inside. Times are from the signal's start too (for
    /// a stretch of a recording, the stretch's).
    pub first_frame: usize,
    pub end_frame: usize,
    pub fade_frames: usize,
    /// The file's stretch of the recording (s).
    pub start_seconds: f64,
    pub end_seconds: f64,
    /// From the first event's start to the last event's end (s).
    pub sounding_seconds: [f64; 2],
    /// How many events it joins.
    pub events: usize,
    pub duration_seconds: f64,
    /// While it sounds (`power_while_sounding` of the channels' mean, as the
    /// engine measures a take), dB re full scale.
    pub level_decibels: f64,
    pub relative_to_loudest_decibels: f64,
    /// Its band level while it sounds over the local bed (dB).
    pub snr_decibels: f64,
    pub centroid_hertz: f64,
    /// SNR below the flag threshold: its bed will gate in and out with it.
    pub flagged: bool,
}

impl Take {
    /// Its samples from `signal`, the recording it was cut from: interleaved
    /// as the recording, raised-cosine fades at both ends.
    pub fn samples(&self, signal: &Signal) -> Vec<f32> {
        super::edges::faded(signal, self.first_frame, self.end_frame, self.fade_frames)
    }
}

/// A take that was not kept, and why.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Dropped {
    pub start_seconds: f64,
    pub end_seconds: f64,
    pub reason: DropReason,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum DropReason {
    /// Shorter than the shortest take: a click, not a call.
    Shorter,
    /// Already sounding when the recording starts.
    CutOffAtStart,
    /// Still sounding when the recording ends.
    CutOffAtEnd,
    Clips,
}

/// The rests between takes, from the end of one file to the start of the
/// next, as the engine's gap runs. Takes dropped for being cut off or
/// clipping still count: the animal rested around them.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Gaps {
    pub seconds: Vec<f64>,
    pub p10: Option<f64>,
    pub median: Option<f64>,
    pub p90: Option<f64>,
}

/// The takes grouped by shape.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Types {
    /// Similarity of every pair of kept takes, in take order: the largest
    /// normalised cross-correlation of their log-mel spectrograms over a
    /// time shift. 1 on the diagonal.
    pub similarity: Vec<Vec<f64>>,
    /// Each type's name and its takes' numbers.
    pub groups: Vec<TypeGroup>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct TypeGroup {
    pub name: String,
    pub takes: Vec<usize>,
}

/// Runs of one type in the recording.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Bouts {
    pub runs: Vec<Run>,
    /// The shortest and longest run; None without takes.
    pub shortest: Option<usize>,
    pub longest: Option<usize>,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Run {
    #[serde(rename = "type")]
    pub type_name: String,
    /// The first take's number.
    pub first_take: usize,
    pub takes: usize,
}
