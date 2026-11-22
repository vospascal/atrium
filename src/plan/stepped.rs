//! A plan driven in step with the audio, for a scene rendered to a file.
//!
//! Live, a [`Planner`](super::Planner) runs beside the audio on its own
//! thread: it wakes every 50 ms of the wall clock and hands over what is due
//! within [`LEAD_SECONDS`] of the audio clock. Rendered faster than real time,
//! that thread would fall behind the audio; takes would arrive late or not at
//! all, differently on every run. Offline the same [`Plan`] is driven here
//! instead, between blocks: before each one, everything due up to the block's
//! end plus the same lead is taken, converted as the live planner converts it
//! ([`Due::scheduled`]) and pushed into the same ring the audio thread reads.
//!
//! What a voice utters, and when, depends only on the seed and the voices,
//! never on when the plan is looked at, so the utterances are exactly the live
//! planner's. What does depend on when it looks are the air and the settings
//! of generators that follow the weather: those are sent as they stand at
//! that moment plus the lead, here at every block instead of at every wake.

use super::due::{Due, LEAD_SECONDS};
use super::{Plan, Scheduled};

pub(crate) struct SteppedPlanner {
    plan: Plan,
    /// Scene time minus the audio clock (s): where in the scene the render
    /// started.
    moved_by: f64,
    audio: rtrb::Producer<Scheduled>,
}

impl SteppedPlanner {
    /// Drive `plan` into `audio` from `start` seconds into the scene, the
    /// weather running `weather_speed` times as fast as the session from the
    /// scene's beginning, so a render from `start` hears the weather a render
    /// from 0 would hear there.
    pub(crate) fn new(
        mut plan: Plan,
        audio: rtrb::Producer<Scheduled>,
        start: f64,
        weather_speed: f64,
    ) -> Self {
        plan.set_weather_speed(0.0, weather_speed);
        if start > 0.0 {
            // As the editor's timeline does: every voice as it would be
            // there, a take under way starting partway.
            plan.seek(start);
        }
        Self {
            plan,
            moved_by: start,
            audio,
        }
    }

    /// Hand over everything due before `audio_seconds` on the audio clock,
    /// plus the lead. Called before each block with the block's end.
    pub(crate) fn hand_over(&mut self, audio_seconds: f64, sample_rate: f32) {
        let due: Vec<Due> = self
            .plan
            .take_due(audio_seconds + self.moved_by + LEAD_SECONDS);
        for due in due {
            // The audio thread drains the ring every block and a block's
            // worth of plan is a few dozen entries, so it never fills; were
            // it full, the render would silently lose a take.
            self.audio
                .push(due.scheduled(sample_rate, self.moved_by))
                .expect("the plan ring is drained every block");
        }
    }
}

#[cfg(test)]
mod tests;
