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
mod conditions;
mod rhythm;
mod runner;
mod story;
mod voice;

pub use clock::Clock;
pub use conditions::{Condition, Conditions, Follow, Target};
pub use rhythm::{Rests, Rhythm, TakeRhythm};
pub use runner::Planner;
pub use story::{Story, Weather};
pub use voice::Planned;

use voice::VoicePlan;

use atrium_core::commands::SynthParam;
use atrium_core::source::Utterance;
use atrium_core::telemetry::MAX_SOURCES;

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
}

/// What becomes due when the plan moves on.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Due {
    Utter(usize, Planned),
    Set(usize, SynthParam, f32),
}

/// A planned voice with takes, and what it follows.
#[derive(Clone, Debug)]
struct Voice {
    plan: VoicePlan,
    follows: Vec<Follow>,
}

/// A generator that follows conditions, and the settings last sent to it.
#[derive(Clone, Debug)]
struct Follower {
    follows: Vec<Follow>,
    sent: Vec<(SynthParam, f32)>,
}

/// What every sound will do: when each voice with takes and rests sounds,
/// what it says, and how every sound follows the place's conditions as the
/// weather story moves them. A sound that loops and follows nothing is not
/// in it: it sounds all the time and needs no plan.
#[derive(Clone, Debug)]
pub struct Plan {
    seed: u64,
    weather: Weather,
    voices: Vec<Option<Voice>>,
    followers: Vec<Option<Follower>>,
}

impl Plan {
    /// An empty plan for a scene whose choices all follow from `seed`, under
    /// a calm, held day until a weather story is set.
    pub fn new(seed: u64) -> Self {
        Self {
            seed,
            weather: Weather::calm(seed),
            voices: vec![None; MAX_SOURCES],
            followers: vec![None; MAX_SOURCES],
        }
    }

    /// The seed every choice follows from.
    pub fn seed(&self) -> u64 {
        self.seed
    }

    pub fn weather(&self) -> &Weather {
        &self.weather
    }

    /// Let `weather` move the conditions from now on. What is already handed
    /// over still plays; everything after follows the new story.
    pub fn set_weather(&mut self, weather: Weather) {
        self.weather = weather;
        for follower in self.followers.iter_mut().flatten() {
            follower.sent.clear();
        }
    }

    /// The conditions at `seconds`.
    pub fn conditions_at(&self, seconds: f64) -> Conditions {
        self.weather.at(seconds)
    }

    /// Plan the voice in `slot` from `now` (s), for the entity `id`, following
    /// `follows`. Its choices follow from the scene's seed and the id, not
    /// the slot, so a sound keeps its way of singing when the voices around
    /// it change. A generator, which has no rhythm, is planned only for the
    /// settings it follows.
    pub fn add_voice(
        &mut self,
        slot: usize,
        id: &str,
        rhythm: Option<Rhythm>,
        follows: Vec<Follow>,
        now: f64,
    ) {
        self.remove_voice(slot);
        if slot >= MAX_SOURCES {
            return;
        }
        // A generator with takes, such as thunder, is both: its takes are
        // planned, and its own settings follow too.
        if follows
            .iter()
            .any(|follow| matches!(follow.target, Target::Setting(_)))
        {
            self.followers[slot] = Some(Follower {
                follows: follows.clone(),
                sent: Vec::new(),
            });
        }
        if let Some(rhythm) = rhythm.filter(|rhythm| !rhythm.takes.is_empty()) {
            self.voices[slot] = Some(Voice {
                plan: VoicePlan::new(rhythm, voice_seed(self.seed, id), now),
                follows,
            });
        }
    }

    /// Stop planning for the voice in `slot`.
    pub fn remove_voice(&mut self, slot: usize) {
        if slot < MAX_SOURCES {
            self.voices[slot] = None;
            self.followers[slot] = None;
        }
    }

    /// Whether the voice in `slot` is planned.
    pub fn has_voice(&self, slot: usize) -> bool {
        slot < MAX_SOURCES && (self.voices[slot].is_some() || self.followers[slot].is_some())
    }

    /// Rest between `shortest` and `longest` seconds in `slot` from `now`.
    pub fn set_gap(&mut self, slot: usize, now: f64, shortest: f32, longest: f32) {
        if let Some(Some(voice)) = self.voices.get_mut(slot) {
            voice.plan.set_gap(now, shortest, longest);
        }
    }

    /// Hold one take `fewest` to `most` times running in `slot`.
    pub fn set_bout(&mut self, slot: usize, fewest: u32, most: u32) {
        if let Some(Some(voice)) = self.voices.get_mut(slot) {
            voice.plan.set_bout(fewest, most);
        }
    }

    /// Choose take `take` in `slot` with `chance` next to the others.
    pub fn set_chance(&mut self, slot: usize, take: usize, chance: f32) {
        if let Some(Some(voice)) = self.voices.get_mut(slot) {
            voice.plan.set_chance(take, chance);
        }
    }

    /// Decide everything due before `until` (s) and move past it: every
    /// utterance that starts by then, under the conditions at its moment,
    /// and every generator setting that has moved by then.
    pub fn take_due(&mut self, until: f64) -> Vec<Due> {
        let mut due = Vec::new();
        for (slot, voice) in self.voices.iter_mut().enumerate() {
            let Some(voice) = voice else { continue };
            while voice.plan.next_at() < until {
                let conditions = self.weather.at(voice.plan.next_at());
                let activity = conditions::activity(&voice.follows, &conditions);
                let level = conditions::level_db(&voice.follows, &conditions);
                if let Some(planned) = voice.plan.next_under(activity, level) {
                    due.push(Due::Utter(slot, planned));
                }
            }
        }
        let conditions = self.weather.at(until);
        for (slot, follower) in self.followers.iter_mut().enumerate() {
            let Some(follower) = follower else { continue };
            for follow in &follower.follows {
                let Target::Setting(param) = follow.target else {
                    continue;
                };
                let value = follow.value(&conditions);
                let last = follower.sent.iter_mut().find(|(sent, _)| *sent == param);
                match last {
                    Some((_, previous))
                        if (value - *previous).abs() <= SETTING_STEP * previous.abs().max(1.0) => {}
                    Some((_, previous)) => {
                        *previous = value;
                        due.push(Due::Set(slot, param, value));
                    }
                    None => {
                        follower.sent.push((param, value));
                        due.push(Due::Set(slot, param, value));
                    }
                }
            }
        }
        due
    }

    /// What is planned from now until `until` (s), without deciding it for
    /// real: the same utterances [`Plan::take_due`] will give, for a screen
    /// to draw ahead of time.
    pub fn upcoming(&self, until: f64) -> Vec<(usize, Planned)> {
        self.clone()
            .take_due(until)
            .into_iter()
            .filter_map(|due| match due {
                Due::Utter(slot, planned) => Some((slot, planned)),
                Due::Set(..) => None,
            })
            .collect()
    }
}

/// How far a followed setting moves before it is sent again: a hundredth of
/// its size, so a slow drift is not a stream of tiny changes.
const SETTING_STEP: f32 = 0.01;

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
        plan.add_voice(0, "blackbird", Some(birdsong()), Vec::new(), 0.0);
        plan.add_voice(5, "blackbird", Some(birdsong()), Vec::new(), 0.0);
        plan.add_voice(6, "robin", Some(birdsong()), Vec::new(), 0.0);
        let due = plan.upcoming(120.0);
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
            plan.add_voice(0, "blackbird", Some(birdsong()), Vec::new(), 0.0);
            plan.upcoming(60.0)
        };
        assert_eq!(first(1), first(1));
        assert_ne!(first(1), first(2));
    }

    /// What is shown ahead of time is what is then played, and showing it
    /// decides nothing.
    #[test]
    fn what_is_shown_ahead_is_what_plays() {
        let mut plan = Plan::new(7);
        plan.add_voice(3, "robin", Some(birdsong()), Vec::new(), 0.0);
        let shown = plan.upcoming(45.0);
        assert_eq!(plan.upcoming(45.0), shown, "showing it changes nothing");
        let played: Vec<_> = (1..=45)
            .flat_map(|second| plan.take_due(second as f64))
            .filter_map(|due| match due {
                Due::Utter(slot, planned) => Some((slot, planned)),
                Due::Set(..) => None,
            })
            .collect();
        assert_eq!(played, shown);
    }

    /// A removed voice is no longer planned, and a voice without takes is
    /// never planned at all.
    #[test]
    fn only_voices_with_takes_are_planned() {
        let mut plan = Plan::new(1);
        plan.add_voice(0, "robin", Some(birdsong()), Vec::new(), 0.0);
        plan.add_voice(
            1,
            "nothing",
            Some(Rhythm {
                takes: Vec::new(),
                rests: Rests::plain(1.0, 2.0),
            }),
            Vec::new(),
            0.0,
        );
        assert!(plan.has_voice(0) && !plan.has_voice(1));
        plan.remove_voice(0);
        assert!(plan.take_due(100.0).is_empty());
    }

    fn rainy_bird() -> Vec<Follow> {
        vec![Follow::named("rain", "activity", 1.0, 0.0, true, false).expect("rain")]
    }

    /// A bird that stops in the rain sings through a dry spell and falls
    /// silent while it pours, decided ahead from the weather story.
    #[test]
    fn a_bird_that_follows_the_rain_falls_silent_while_it_pours() {
        let mut plan = Plan::new(9);
        let mut bands = [(0.05, 0.2), (0.0, 0.0), (0.0, 0.0), (1.0, 1.0)];
        bands[Condition::Daylight as usize] = (1.0, 1.0);
        plan.set_weather(Weather::new(Story::PassingStorm, bands, 9));
        plan.add_voice(0, "robin", Some(birdsong()), rainy_bird(), 0.0);
        let weather = plan.weather().clone();
        let sung = plan.upcoming(3.0 * 3600.0);
        let rain_at = |planned: &Planned| weather.at(planned.at).get(Condition::Rain);
        let pouring = sung
            .iter()
            .filter(|(_, planned)| rain_at(planned) >= 0.999)
            .count();
        let wet = sung
            .iter()
            .filter(|(_, planned)| rain_at(planned) > 0.5)
            .count() as f64;
        let dry = sung
            .iter()
            .filter(|(_, planned)| rain_at(planned) < 0.01)
            .count() as f64;
        let seconds = |test: &dyn Fn(f32) -> bool| {
            (0..3 * 3600)
                .filter(|second| test(weather.at(*second as f64).get(Condition::Rain)))
                .count() as f64
        };
        let (wet_seconds, dry_seconds) =
            (seconds(&|rain| rain > 0.5), seconds(&|rain| rain < 0.01));
        assert!(dry > 100.0, "only {dry} phrases in dry weather");
        assert_eq!(pouring, 0, "it sang in the downpour");
        assert!(
            wet / wet_seconds < 0.3 * dry / dry_seconds,
            "the rain hardly quietened it"
        );
    }

    /// A generator follows with its own settings: rain intensity moves with
    /// the rain, sent when it has moved and not every time the plan looks.
    #[test]
    fn a_generator_setting_follows_its_condition() {
        let mut plan = Plan::new(2);
        plan.set_weather(Weather::new(
            Story::PassingStorm,
            [(0.1, 0.1), (0.0, 0.0), (0.0, 0.0), (1.0, 1.0)],
            2,
        ));
        let rain = vec![Follow::named("rain", "intensity", 0.0, 1.0, false, true).expect("rain")];
        plan.add_voice(4, "rain", None, rain, 0.0);
        let mut sent = Vec::new();
        for tenth in 1..(3 * 3600 * 10) {
            for due in plan.take_due(tenth as f64 / 10.0) {
                if let Due::Set(slot, param, value) = due {
                    assert_eq!((slot, param), (4, SynthParam::Intensity));
                    sent.push(value);
                }
            }
        }
        assert!(
            sent.iter().any(|value| *value > 0.9),
            "it never rained hard"
        );
        assert!(
            sent.len() < 3 * 3600 * 10 / 10,
            "{} changes sent; most looks should send nothing",
            sent.len()
        );
        assert_eq!(sent[0], 0.0, "the first send is the dry start");
    }
}
