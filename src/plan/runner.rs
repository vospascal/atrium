//! Running a plan on its own thread, next to the audio.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;
use std::time::Duration;

use super::due::LEAD_SECONDS;
use super::{Clock, Conditions, Follow, Plan, Planned, Rhythm, Scheduled, Weather};

/// How often the planner looks at the clock.
const WAKE_EVERY: Duration = Duration::from_millis(50);

/// The plan of a playing scene, kept on its own thread. Changes go through
/// here; dropping it stops the thread.
pub struct Planner {
    plan: Arc<Mutex<Plan>>,
    clock: Arc<Clock>,
    /// Scene time minus the audio clock (s, as `f64` bits): 0 until the scene
    /// is moved.
    offset: Arc<AtomicU64>,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl Planner {
    /// Run `plan` against `clock`, handing what is due to `audio`.
    pub fn start(plan: Plan, clock: Arc<Clock>, mut audio: rtrb::Producer<Scheduled>) -> Self {
        let plan = Arc::new(Mutex::new(plan));
        let offset = Arc::new(AtomicU64::new(0f64.to_bits()));
        let (stop, stopped) = mpsc::channel::<()>();
        let thread = {
            let plan = plan.clone();
            let clock = clock.clone();
            let offset = offset.clone();
            std::thread::Builder::new()
                .name("atrium-planner".into())
                .spawn(move || {
                    while let Err(mpsc::RecvTimeoutError::Timeout) =
                        stopped.recv_timeout(WAKE_EVERY)
                    {
                        let Some(rate) = clock.sample_rate() else {
                            continue;
                        };
                        let mut plan = lock(&plan);
                        let moved_by = f64::from_bits(offset.load(Ordering::Acquire));
                        let due = plan.take_due(clock.seconds() + moved_by + LEAD_SECONDS);
                        drop(plan);
                        for due in due {
                            // A full ring only happens when the audio thread has
                            // stopped; that change is then not heard.
                            let _ = audio.push(due.scheduled(rate, moved_by));
                        }
                    }
                })
                .ok()
        };
        Self {
            plan,
            clock,
            offset,
            stop: Some(stop),
            thread,
        }
    }

    /// Seconds into the scene: the audio clock, moved by any seek. It stands
    /// still while the scene is paused.
    pub fn now(&self) -> f64 {
        self.clock.seconds() + f64::from_bits(self.offset.load(Ordering::Acquire))
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

    /// Let the weather run `speed` times as fast as the session from now.
    pub fn set_weather_speed(&self, speed: f64) {
        let now = self.now();
        lock(&self.plan).set_weather_speed(now, speed);
    }

    pub fn weather_speed(&self) -> f64 {
        lock(&self.plan).weather_speed()
    }

    /// Move the scene to `seconds`, as a playhead is dragged along its
    /// timeline: every voice and the weather are as they would be there, and
    /// it plays on from that moment.
    pub fn seek(&self, seconds: f64) {
        if !seconds.is_finite() {
            return;
        }
        let to = seconds.max(0.0);
        let mut plan = lock(&self.plan);
        self.offset
            .store((to - self.clock.seconds()).to_bits(), Ordering::Release);
        plan.seek(to);
    }

    /// The conditions `ahead` seconds from now (0 for now).
    pub fn conditions(&self, ahead: f64) -> Conditions {
        let now = self.now();
        lock(&self.plan).conditions_at(now + ahead)
    }

    /// Plan the voice in `slot` to be heard on its own: it follows no
    /// condition, so the weather cannot silence it, and it starts at once.
    pub fn preview_voice(&self, slot: usize, rhythm: Option<Rhythm>) {
        let now = self.now();
        let mut plan = lock(&self.plan);
        plan.add_voice(slot, "preview", rhythm, Vec::new(), now);
        plan.start_now(slot, now);
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

/// A poisoned lock only means a call panicked mid-change; the plan is still
/// usable.
fn lock(plan: &Mutex<Plan>) -> MutexGuard<'_, Plan> {
    plan.lock().unwrap_or_else(|poisoned| poisoned.into_inner())
}
