//! What a plan hands on: what becomes due as it moves on, and what the audio
//! thread is then told.

use atrium_core::commands::SynthParam;
use atrium_core::source::Utterance;

use super::voice::Planned;

/// What the plan hands the audio thread for the voice in a slot.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Scheduled {
    /// Utter a take at a set moment.
    Utter { slot: u16, utterance: Utterance },
    /// Change one of a generator's settings now, because a condition moved.
    Set {
        slot: u16,
        param: SynthParam,
        value: f32,
    },
    /// Forget every appointment: the scene was moved to another moment.
    Cancel { slot: u16 },
}

/// What becomes due when the plan moves on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Due {
    Utter(usize, Planned),
    Set(usize, SynthParam, f32),
    /// Forget what the voice was told: the scene moved to another moment.
    Cancel(usize),
}
