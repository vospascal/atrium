//! What a plan hands on: what becomes due as it moves on, and what the audio
//! thread is then told.

use atrium_core::commands::SynthParam;
use atrium_core::source::Utterance;

use super::voice::Planned;

/// How far ahead of the audio clock what is due is handed to the audio
/// thread (s). Longer than a planner's wake-up and an audio buffer together,
/// so nothing is late; short, so a change is heard within half a second.
/// The live planner and the one driven in step for a file use the same.
pub(crate) const LEAD_SECONDS: f64 = 0.4;

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

impl Due {
    /// What the audio thread is told for this, with its moment turned from
    /// scene time into the audio clock's samples. `moved_by` is scene time
    /// minus the audio clock: 0 until the scene is moved.
    pub(crate) fn scheduled(self, sample_rate: f32, moved_by: f64) -> Scheduled {
        match self {
            Due::Utter(slot, planned) => Scheduled::Utter {
                slot: slot as u16,
                utterance: Utterance {
                    take: planned.take,
                    at_sample: ((planned.at - moved_by) * sample_rate as f64)
                        .round()
                        .max(0.0) as u64,
                    gain: planned.gain,
                    rate: planned.rate,
                    skip_seconds: planned.skip as f32,
                },
            },
            Due::Cancel(slot) => Scheduled::Cancel { slot: slot as u16 },
            Due::Set(slot, param, value) => Scheduled::Set {
                slot: slot as u16,
                param,
                value,
            },
            Due::Air {
                wind,
                daylight,
                wind_from,
            } => Scheduled::Air {
                wind,
                daylight,
                wind_from,
            },
        }
    }
}
