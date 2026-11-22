//! The generator lab's records as Kotlin sees them, and their conversion
//! from the lab's own types.

use atrium::lab::{Candidate, Fader, Layer, MoveStart, Setting, SettingKind, Waveform};
use atrium::synth::wind_behaviour::WindMove;

/// One of the three things the lab switches between.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LabCandidate {
    /// The recording the generator is tuned towards.
    Reference,
    /// The baseline for this round.
    Previous,
    /// The generator with this round's changes, live.
    Working,
}

impl From<LabCandidate> for Candidate {
    fn from(candidate: LabCandidate) -> Self {
        match candidate {
            LabCandidate::Reference => Candidate::Reference,
            LabCandidate::Previous => Candidate::Previous,
            LabCandidate::Working => Candidate::Working,
        }
    }
}

impl From<Candidate> for LabCandidate {
    fn from(candidate: Candidate) -> Self {
        match candidate {
            Candidate::Reference => LabCandidate::Reference,
            Candidate::Previous => LabCandidate::Previous,
            Candidate::Working => LabCandidate::Working,
        }
    }
}

/// One of the soft wind's five layers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LabLayer {
    Low,
    Body,
    Mid,
    Presence,
    Air,
}

impl From<LabLayer> for Layer {
    fn from(layer: LabLayer) -> Self {
        match layer {
            LabLayer::Low => Layer::Low,
            LabLayer::Body => Layer::Body,
            LabLayer::Mid => Layer::Mid,
            LabLayer::Presence => Layer::Presence,
            LabLayer::Air => Layer::Air,
        }
    }
}

impl From<Layer> for LabLayer {
    fn from(layer: Layer) -> Self {
        match layer {
            Layer::Low => LabLayer::Low,
            Layer::Body => LabLayer::Body,
            Layer::Mid => LabLayer::Mid,
            Layer::Presence => LabLayer::Presence,
            Layer::Air => LabLayer::Air,
        }
    }
}

/// One layer's strip in the working generator's mixer.
#[derive(Debug, Clone, Copy, PartialEq, uniffi::Record)]
pub struct LabFader {
    pub layer: LabLayer,
    /// Relative to the layer as the generator gives it (dB).
    pub level_db: f32,
    pub solo: bool,
    pub mute: bool,
}

impl LabFader {
    pub(super) fn of(layer: Layer, fader: Fader) -> Self {
        Self {
            layer: layer.into(),
            level_db: fader.level_db,
            solo: fader.solo,
            mute: fader.mute,
        }
    }

    pub(super) fn fader(&self) -> Fader {
        Fader {
            level_db: self.level_db,
            solo: self.solo,
            mute: self.mute,
        }
    }
}

/// A waveform to draw: the lowest and highest sample in each of equal
/// stretches.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LabWaveform {
    /// How long the whole is (s).
    pub seconds: f32,
    pub minima: Vec<f32>,
    pub maxima: Vec<f32>,
}

impl From<&Waveform> for LabWaveform {
    fn from(waveform: &Waveform) -> Self {
        Self {
            seconds: waveform.seconds as f32,
            minima: waveform.minima.clone(),
            maxima: waveform.maxima.clone(),
        }
    }
}

/// The reference recording, opened.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LabRecording {
    pub file: String,
    pub waveform: LabWaveform,
}

/// Where one of the generator's moves begins: a place its markers snap to.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LabMove {
    /// From the start of the run (s).
    pub seconds: f32,
    /// `gust`, `swell` or `lull`.
    pub kind: String,
}

impl From<&MoveStart> for LabMove {
    fn from(start: &MoveStart) -> Self {
        Self {
            seconds: start.seconds as f32,
            kind: match start.kind {
                WindMove::Gust => "gust",
                WindMove::Swell => "swell",
                WindMove::Lull => "lull",
            }
            .to_owned(),
        }
    }
}

/// A seeded run of the working generator, made offline.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LabRun {
    pub seed: u32,
    pub waveform: LabWaveform,
    pub moves: Vec<LabMove>,
}

/// What a reopened session brought back to draw.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LabReopened {
    pub recording: Option<LabRecording>,
    pub run: Option<LabRun>,
}

/// The passage the loop plays: where it is in the recording, and where in
/// the seeded run the generators start. Both last as long as the recording's
/// stretch.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LabPassage {
    /// Such as "Gust 1".
    pub name: String,
    pub reference_from: f32,
    pub reference_to: f32,
    pub generator_cue: f32,
}

/// What changing a setting does to the gusts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum LabSettingKind {
    /// The same gusts, heard at once.
    LevelOrColour,
    /// Different gusts: the marked one no longer lines up.
    Timing,
}

/// A generator setting as the Behaviour panel draws it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LabSetting {
    pub name: String,
    pub label: String,
    pub kind: LabSettingKind,
    pub low: f32,
    pub high: f32,
    /// The working generator's.
    pub value: f32,
    pub description: String,
}

impl LabSetting {
    pub(super) fn of(setting: &Setting, value: f32) -> Self {
        Self {
            name: setting.name().to_owned(),
            label: setting.label.to_owned(),
            kind: match setting.kind {
                SettingKind::LevelOrColour => LabSettingKind::LevelOrColour,
                SettingKind::Timing => LabSettingKind::Timing,
            },
            low: setting.low,
            high: setting.high,
            value,
            description: setting.description.to_owned(),
        }
    }
}

/// A candidate's trim and what the last match measured of it.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LabTrim {
    pub candidate: LabCandidate,
    /// What it plays at (dB); 0 for the reference.
    pub trim_db: f32,
    /// Whether a match set it.
    pub matched: bool,
    /// Held through an edit that may have changed its level.
    pub rematch_needed: bool,
    /// How far it measured from the reference in the last match, at the trim
    /// it was measured at (dB).
    pub difference_db: Option<f32>,
    /// How far apart the two halves of the passage measured (dB).
    pub halves_apart_db: Option<f32>,
}

/// An observation pinned to the moment it was heard.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LabNote {
    /// Into the passage (s).
    pub at_seconds: f32,
    pub heard: LabCandidate,
    /// The same moment in the recording or the seeded run (s).
    pub moment_seconds: f32,
    pub text: String,
}

/// The lab as it is now, for drawing every frame.
#[derive(Debug, Clone, PartialEq, uniffi::Record)]
pub struct LabState {
    /// The sound being tuned, as its preset names it.
    pub sound: String,
    pub question: String,
    /// The reference recording's file, once one is open.
    pub recording: Option<String>,
    pub passage: Option<LabPassage>,
    pub seed: u32,
    pub playing: bool,
    pub looping: bool,
    pub run_freely: bool,
    /// The lab's own playhead: into the passage (s), and how long it is.
    pub playhead_seconds: f32,
    pub loop_seconds: f32,
    /// The candidate asked for, and the one heard alone (none mid-switch).
    pub asked: LabCandidate,
    pub heard: Option<LabCandidate>,
    pub trims: Vec<LabTrim>,
    pub hold_level_match: bool,
    /// While levels are being matched: the candidate measured, and how far
    /// along the match is, 0 to 1.
    pub measuring: Option<LabCandidate>,
    pub match_progress: f32,
    /// The working generator's strips, lowest layer first.
    pub mix: Vec<LabFader>,
    pub can_undo_mix: bool,
    /// The settings the Behaviour panel shows.
    pub behaviour: Vec<LabSetting>,
    /// Whether the working generator's gusts come where the previous one's
    /// do; after a timing change, the marked gust no longer lines up.
    pub lines_up: bool,
    pub notes: Vec<LabNote>,
    pub verdict: String,
    /// Whether a saved session waits to be reopened.
    pub saved_session: bool,
}
