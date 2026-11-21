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
    /// The air of the place has changed: how hard the wind blows and how
    /// bright the day is, each 0 to 1, and the compass bearing the wind
    /// comes from (degrees).
    Air {
        wind: f32,
        daylight: f32,
        wind_from: f32,
    },
}

/// What becomes due when the plan moves on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Due {
    Utter(usize, Planned),
    Set(usize, SynthParam, f32),
    /// Forget what the voice was told: the scene moved to another moment.
    Cancel(usize),
    /// The air of the place: wind and daylight, 0 to 1, and the bearing the
    /// wind comes from (degrees).
    Air {
        wind: f32,
        daylight: f32,
        wind_from: f32,
    },
}
