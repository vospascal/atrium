//! The generator lab: one sound worked on against a reference recording, at
//! the same loudness, against its own previous version, with its settings
//! under the listener's hands while it plays (`docs/generator-lab-plan.md`).
//! Stage 1 tunes the soft wind.
//!
//! - `candidate`: the reference, the previous version and the working one
//! - `mix`: the working generator's five layers, their faders, and undo
//! - `settings`: which settings move the gusts and which only colour them
//! - `run`: a seeded run, offline, its waveform, its moves and its state at
//!   a cue
//! - `controls` and `voice`: the three candidates playing in step on the
//!   audio thread, switched with a fade, recalled at every wrap
//! - `matching` and `trims`: levels measured at the output tap, and when a
//!   trim goes stale
//! - `session`: the session as a file, to reopen
//!
//! Everything but `voice` and `controls` is decided off the audio thread and
//! can be tested without a device.

mod candidate;
mod controls;
mod matching;
mod mix;
mod run;
mod session;
mod settings;
mod trims;
mod voice;

#[cfg(test)]
mod tests;

pub use candidate::Candidate;
pub use controls::LabControls;
pub use matching::{in_first_half, LevelMatch, MatchOutcome, Reading, SOUNDING_SECONDS};
pub use mix::{Fader, Layer, LayerMix, MixHistory};
pub use run::{run, state_at, GeneratorRun, MoveStart, Waveform};
pub use session::{
    session_path, ReferencePassage, Session, SessionCandidate, SessionCandidates, SessionNote,
};
pub use settings::{setting_named, CandidateSettings, Setting, SettingKind, SETTINGS};
pub use trims::{Edit, Trims};
pub use voice::{placed_like, LabVoice, VoiceParts};
