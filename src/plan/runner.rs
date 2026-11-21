//! Running a plan on its own thread, next to the audio.

use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use atrium_core::source::Utterance;

use super::{Clock, Plan, Planned, Rhythm, Scheduled};

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
                        for (slot, planned) in due {
                            // A full ring only happens when the audio thread has
                            // stopped; that utterance is then not heard.
                            let _ = audio.push(scheduled(slot, planned, rate));
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

    /// Plan the voice in `slot` for entity `id` from now; nothing for a sound
    /// without rests.
    pub fn add_voice(&self, slot: usize, id: &str, rhythm: Option<Rhythm>) {
        let now = self.now();
        let mut plan = lock(&self.plan);
        match rhythm {
            Some(rhythm) => plan.add_voice(slot, id, rhythm, now),
            None => plan.remove_voice(slot),
        }
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

fn scheduled(slot: usize, planned: Planned, sample_rate: f32) -> Scheduled {
    Scheduled {
        slot: slot as u16,
        utterance: Utterance {
            take: planned.take,
            at_sample: (planned.at * sample_rate as f64).round().max(0.0) as u64,
            gain: planned.gain,
            rate: planned.rate,
        },
    }
}

/// A poisoned lock only means a call panicked mid-change; the plan is still
/// usable.
fn lock(plan: &Mutex<Plan>) -> MutexGuard<'_, Plan> {
    plan.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
