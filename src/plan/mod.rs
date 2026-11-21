//! The session: what every voice will do, decided ahead of time.
//!
//! The audio thread does not decide when a voice sounds or what it says. A
//! [`Plan`] decides that from the scene's seed and each entity's id, ahead of
//! the clock, the way a video player buffers. A [`Planner`] runs it on its own
//! thread and hands the audio thread only what is due in the next fraction of
//! a second, so a change is heard almost at once and nothing planned further
//! ahead has to be taken back.
//!
//! The plan holds decisions, not sound. Every device that plays the same plan
//! hears the same take at the same moment, each from its own position.
//!
//! What is decided is a pure function of the rhythms, the seed and the time
//! window, so it is tested without a sound device.

mod clock;
mod rhythm;
mod runner;
mod voice;

pub use clock::Clock;
pub use rhythm::{Rests, Rhythm, TakeRhythm};
pub use runner::Planner;
pub use voice::Planned;

use voice::VoicePlan;

use atrium_core::source::Utterance;
use atrium_core::telemetry::MAX_SOURCES;

/// An utterance for the voice in `slot`, as the audio thread receives it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Scheduled {
    pub slot: u16,
    pub utterance: Utterance,
}

/// What every voice with takes and rests will do. A voice that loops or a
/// generator is not in it: it sounds all the time and needs no plan.
#[derive(Clone, Debug)]
pub struct Plan {
    seed: u64,
    voices: Vec<Option<VoicePlan>>,
}

impl Plan {
    /// An empty plan for a scene whose choices all follow from `seed`.
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            voices: vec![None; MAX_SOURCES],
        }
    }

    /// The seed every choice follows from.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    /// Plan the voice in `slot` from `now` (s), for the entity `id`. Its
    /// choices follow from the scene's seed and the id, not the slot, so a
    /// sound keeps its way of singing when the voices around it change.
    pub fn add_voice(&mut self, slot: usize, id: &str, rhythm: Rhythm, now: f64) {
        if rhythm.takes.is_empty() {
            return;
        }
        if let Some(voice) = self.voices.get_mut(slot) {
            *voice = Some(VoicePlan::new(rhythm, voice_seed(self.seed, id), now));
        }
    }

    /// Stop planning for the voice in `slot`.
    pub fn remove_voice(&mut self, slot: usize) {
        if let Some(voice) = self.voices.get_mut(slot) {
            *voice = None;
        }
    }

    /// Whether the voice in `slot` is planned.
    pub fn has_voice(&self, slot: usize) -> bool {
        self.voices.get(slot).is_some_and(Option::is_some)
    }

    /// Rest between `shortest` and `longest` seconds in `slot` from `now`.
    pub fn set_gap(&mut self, slot: usize, now: f64, shortest: f32, longest: f32) {
        if let Some(Some(voice)) = self.voices.get_mut(slot) {
            voice.set_gap(now, shortest, longest);
        }
    }

    /// Hold one take `fewest` to `most` times running in `slot`.
    pub fn set_bout(&mut self, slot: usize, fewest: u32, most: u32) {
        if let Some(Some(voice)) = self.voices.get_mut(slot) {
            voice.set_bout(fewest, most);
        }
    }

    /// Choose take `take` in `slot` with `chance` next to the others.
    pub fn set_chance(&mut self, slot: usize, take: usize, chance: f32) {
        if let Some(Some(voice)) = self.voices.get_mut(slot) {
            voice.set_chance(take, chance);
        }
    }

    /// Decide every utterance that starts before `until` (s), in time order
    /// per voice, and move past them: they are now the past of the plan.
    pub fn take_due(&mut self, until: f64) -> Vec<(usize, Planned)> {
        let mut due = Vec::new();
        for (slot, voice) in self.voices.iter_mut().enumerate() {
            if let Some(voice) = voice {
                while voice.next_at() < until {
                    due.push((slot, voice.next()));
                }
            }
        }
        due
    }

    /// What is planned from now until `until` (s), without deciding it for
    /// real: the same utterances [`Plan::take_due`] will give, for a screen
    /// to draw ahead of time.
    pub fn upcoming(&self, until: f64) -> Vec<(usize, Planned)> {
        self.clone().take_due(until)
    }
}

/// The seed for one entity's voice: the scene's seed and the entity's id
/// mixed by FNV-1a, so neighbours do not share choices.
fn voice_seed(scene: u64, id: &str) -> u64 {
    let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ scene;
    for byte in id.bytes() {
        hash ^= byte as u64;
        hash = hash.wrapping_mul(0x0100_0000_01b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn birdsong() -> Rhythm {
        Rhythm {
            takes: (0..4)
                .map(|_| TakeRhythm {
                    seconds: 1.5,
                    chance: 1.0,
                })
                .collect(),
            rests: Rests::plain(2.0, 6.0),
        }
    }

    /// A voice's choices follow its entity, not its slot: the same id plans
    /// the same in any voice, and two ids plan differently.
    #[test]
    fn a_voice_plans_by_its_entity_not_its_slot() {
        let mut plan = Plan::new(42);
        plan.add_voice(0, "blackbird", birdsong(), 0.0);
        plan.add_voice(5, "blackbird", birdsong(), 0.0);
        plan.add_voice(6, "robin", birdsong(), 0.0);
        let due = plan.take_due(120.0);
        let of = |slot| {
            due.iter()
                .filter(|(own, _)| *own == slot)
                .map(|(_, planned)| *planned)
                .collect::<Vec<_>>()
        };
        assert_eq!(of(0), of(5));
        assert_ne!(of(0), of(6));
        assert!(of(0).len() > 10);
    }

    /// Another seed is another session: every voice decides differently.
    #[test]
    fn another_seed_plans_another_session() {
        let first = |seed| {
            let mut plan = Plan::new(seed);
            plan.add_voice(0, "blackbird", birdsong(), 0.0);
            plan.take_due(60.0)
        };
        assert_eq!(first(1), first(1));
        assert_ne!(first(1), first(2));
    }

    /// What is shown ahead of time is what is then played, and showing it
    /// decides nothing.
    #[test]
    fn what_is_shown_ahead_is_what_plays() {
        let mut plan = Plan::new(7);
        plan.add_voice(3, "robin", birdsong(), 0.0);
        let shown = plan.upcoming(45.0);
        assert_eq!(plan.upcoming(45.0), shown, "showing it changes nothing");
        let played: Vec<_> = (1..=45)
            .flat_map(|second| plan.take_due(second as f64))
            .collect();
        assert_eq!(played, shown);
    }

    /// A removed voice is no longer planned, and a voice without takes is
    /// never planned at all.
    #[test]
    fn only_voices_with_takes_are_planned() {
        let mut plan = Plan::new(1);
        plan.add_voice(0, "robin", birdsong(), 0.0);
        plan.add_voice(
            1,
            "nothing",
            Rhythm {
                takes: Vec::new(),
                rests: Rests::plain(1.0, 2.0),
            },
            0.0,
        );
        assert!(plan.has_voice(0) && !plan.has_voice(1));
        plan.remove_voice(0);
        assert!(plan.take_due(100.0).is_empty());
    }
}
