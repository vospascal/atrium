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
        self.scene_seconds_at(self.clock.samples())
    }

    /// The scene time of the audio clock's sample `clock_samples`, as the
    /// scene is moved now: what is measured of a block is placed where it
    /// sounded on the timeline.
    pub fn scene_seconds_at(&self, clock_samples: u64) -> f64 {
        self.clock.seconds_at(clock_samples) + f64::from_bits(self.offset.load(Ordering::Acquire))
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

    /// The weather story moving the conditions; `None` for a session
    /// without weather.
    pub fn weather(&self) -> Option<Weather> {
        lock(&self.plan).weather().cloned()
    }

    /// Let `weather` move the conditions from now on; `None` holds them at
    /// their defaults and stills the air.
    pub fn set_weather(&self, weather: Option<Weather>) {
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

    /// Carry on where `earlier` stands, as a scene built again in another
    /// place does: at the same moment of the scene, the weather at the same
    /// point of its story and running as fast. With the same seed, every
    /// voice and every condition is then what it would have been.
    pub fn carry_on_from(&self, earlier: &Planner) {
        let now = earlier.now();
        lock(&self.plan).keep_weather_clock_of(&lock(&earlier.plan));
        self.seek(now);
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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::Story;

    const RATE: f32 = 48_000.0;

    /// A planner for a passing storm from `seed`, on a clock of its own.
    fn planner(seed: u64) -> Planner {
        let mut plan = Plan::new(seed);
        plan.set_weather(Some(Weather::new(
            Story::PassingStorm,
            [(0.1, 0.6), (0.0, 0.8), (0.0, 1.0), (0.3, 1.0)],
            seed,
        )));
        let (audio, _) = rtrb::RingBuffer::new(16);
        Planner::start(plan, Arc::new(Clock::default()), audio)
    }

    /// A scene built again in another place carries on where it stood: the
    /// same moment, and the weather at the same point of its story, running
    /// as fast. A seek alone would put a time-lapsed story elsewhere.
    #[test]
    fn a_planner_carries_on_where_the_earlier_one_stood() {
        let earlier = planner(7);
        earlier.clock.advance((90.0 * RATE) as u64, RATE);
        earlier.set_weather_speed(60.0);
        earlier.clock.advance((30.0 * RATE) as u64, RATE);
        earlier.seek(150.0);

        let later = planner(7);
        later.carry_on_from(&earlier);
        assert_eq!(later.now(), earlier.now());
        assert_eq!(later.weather_speed(), 60.0);
        for ahead in [0.0, 10.0, 600.0] {
            assert_eq!(later.conditions(ahead), earlier.conditions(ahead));
        }

        let sought = planner(7);
        sought.seek(earlier.now());
        assert_ne!(sought.conditions(0.0), earlier.conditions(0.0));
    }
}
