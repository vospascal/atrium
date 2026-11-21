//! Running a plan on its own thread, next to the audio.

use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use atrium_core::source::Utterance;

use super::{Clock, Conditions, Due, Follow, Plan, Planned, Rhythm, Scheduled, Weather};

/// How far ahead of the clock utterances are handed to the audio thread (s).
/// Longer than a wake-up and an audio buffer together, so nothing is late;
/// short, so a change is heard within half a second.
const LEAD_SECONDS: f64 = 0.4;
/// How often the planner looks at the clock.
const WAKE_EVERY: Duration = Duration::from_millis(50);

/// The plan of a playing scene, kept on its own thread. Changes go through
/// here; dropping it stops the thread.
pub struct Planner {
    plan: Arc<Mutex<Plan>>,
    clock: Arc<Clock>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Planner {
    /// Run `plan` against `clock`, handing what is due to `audio`.
    pub fn start(plan: Plan, clock: Arc<Clock>, mut audio: rtrb::Producer<Scheduled>) -> Self {
        let plan = Arc::new(Mutex::new(plan));
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = {
            let plan = plan.clone();
            let clock = clock.clone();
            std::thread::Builder::new()
                .name("atrium-planner".into())
                .spawn(move || {
                    while let Err(mpsc::RecvTimeoutError::Timeout) =
                        stopped.recv_timeout(WAKE_EVERY)
                    {
                        let Some(rate) = clock.sample_rate() else {
                            continue;
                        };
                        let due = lock(&plan).take_due(clock.seconds() + LEAD_SECONDS);
                        for due in due {
                            // A full ring only happens when the audio thread has
                            // stopped; that change is then not heard.
                            let _ = audio.push(scheduled(due, rate));
                        }
                    }
                })
                .ok()
        };
        Self {
            plan,
            clock,
            stop: Some(stop),
            thread,
        }
    }

    /// Seconds on the session's clock.
    pub fn now(&self) -> f64 {
        self.clock.seconds()
    }

    /// The seed every choice follows from.
    pub fn seed(&self) -> u64 {
        lock(&self.plan).seed()
    }

    /// Plan the voice in `slot` for entity `id` from now, following
    /// `follows`; nothing for a sound without rests that follows nothing.
    pub fn add_voice(&self, slot: usize, id: &str, rhythm: Option<Rhythm>, follows: Vec<Follow>) {
        let now = self.now();
        lock(&self.plan).add_voice(slot, id, rhythm, follows, now);
    }

    /// The weather story moving the conditions.
    pub fn weather(&self) -> Weather {
        lock(&self.plan).weather().clone()
    }

    /// Let `weather` move the conditions from now on.
    pub fn set_weather(&self, weather: Weather) {
        lock(&self.plan).set_weather(weather);
    }

    /// The conditions `ahead` seconds from now (0 for now).
    pub fn conditions(&self, ahead: f64) -> Conditions {
        let now = self.now();
        lock(&self.plan).conditions_at(now + ahead)
    }

    pub fn remove_voice(&self, slot: usize) {
        lock(&self.plan).remove_voice(slot);
    }

    pub fn set_gap(&self, slot: usize, shortest: f32, longest: f32) {
        let now = self.now();
        lock(&self.plan).set_gap(slot, now, shortest, longest);
    }

    pub fn set_bout(&self, slot: usize, fewest: u32, most: u32) {
        lock(&self.plan).set_bout(slot, fewest, most);
    }

    pub fn set_chance(&self, slot: usize, take: usize, chance: f32) {
        lock(&self.plan).set_chance(slot, take, chance);
    }

    /// What is planned for the next `seconds`, with times relative to now.
    pub fn upcoming(&self, seconds: f64) -> Vec<(usize, Planned)> {
        let now = self.now();
        lock(&self.plan)
            .upcoming(now + seconds)
            .into_iter()
            .map(|(slot, planned)| {
                (
                    slot,
                    Planned {
                        at: planned.at - now,
                        ..planned
                    },
                )
            })
            .collect()
    }
}

impl Drop for Planner {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn scheduled(due: Due, sample_rate: f32) -> Scheduled {
    match due {
        Due::Utter(slot, planned) => Scheduled::Utter {
            slot: slot as u16,
            utterance: Utterance {
                take: planned.take,
                at_sample: (planned.at * sample_rate as f64).round().max(0.0) as u64,
                gain: planned.gain,
                rate: planned.rate,
            },
        },
        Due::Set(slot, param, value) => Scheduled::Set {
            slot: slot as u16,
            param,
            value,
        },
    }
}

/// A poisoned lock only means a call panicked mid-change; the plan is still
/// usable.
fn lock(plan: &Mutex<Plan>) -> MutexGuard<'_, Plan> {
    plan.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
