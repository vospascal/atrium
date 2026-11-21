//! The scene's clock: pausing it, moving it, and what the plan has decided
//! for the time ahead.

use std::sync::atomic::Ordering;

use atrium_core::commands::Command;

use super::{lock, Engine};
use crate::api::{EngineError, PlannedTake};

/// How far ahead `planned` looks at most (s).
const MAX_PLANNED_SECONDS: f32 = 60.0;

#[uniffi::export]
impl Engine {
    /// Pause the scene, or play it on. Paused, the session's clock stops:
    /// every sound, the plan and the weather hold where they are, and resume
    /// from there. It fades out and in, so nothing clicks.
    pub fn set_paused(&self, paused: bool) -> Result<(), EngineError> {
        lock(&self.commands)
            .push(Command::SetPaused { paused })
            .map_err(|_| EngineError::CommandQueueFull)?;
        self.paused.store(paused, Ordering::Relaxed);
        Ok(())
    }

    /// Whether the scene is paused.
    pub fn paused(&self) -> bool {
        self.paused.load(Ordering::Relaxed)
    }

    /// Seconds into the scene: what a timeline's playhead shows. It stands
    /// still while the scene is paused, and jumps when it is moved.
    pub fn scene_time(&self) -> f32 {
        self.planner.now() as f32
    }

    /// Move the scene to `seconds`, as a playhead is dragged along its
    /// timeline: every sound and the weather are as they would be there, a
    /// take under way starts partway, and it plays on from that moment.
    pub fn seek(&self, seconds: f32) {
        self.planner.seek(seconds as f64);
    }

    /// The takes the plan has decided for the next `seconds`, at most a
    /// minute ahead.
    pub fn planned(&self, seconds: f32) -> Vec<PlannedTake> {
        self.planner
            .upcoming(seconds.clamp(0.0, MAX_PLANNED_SECONDS) as f64)
            .into_iter()
            .map(|(slot, planned)| PlannedTake {
                slot: slot as u16,
                take: planned.take,
                starts_in: planned.at as f32,
                seconds: planned.seconds as f32,
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Band, Story, Weather};
    use atrium::config::SceneConfig;
    use std::time::{Duration, Instant};

    /// The session's plan reaches the audio thread: birds whose takes are
    /// decided ahead are heard uttering them, and a rest made short is heard
    /// at once. At zero volume:
    /// `cargo test -p atrium-ffi -- --ignored --test-threads=1`.
    #[test]
    #[ignore = "opens the default output device"]
    fn planned_takes_are_heard_and_a_shorter_rest_at_once() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine = Engine::new("scenes/meadow-birds.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");
        let blackbird = engine
            .scene()
            .sources
            .iter()
            .find(|source| source.id == "blackbird")
            .expect("a blackbird")
            .slot;
        engine
            .set_source_gap(blackbird, 0.1, 0.2)
            .expect("a short rest");
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut heard = std::collections::BTreeSet::new();
        while Instant::now() < deadline {
            if let Some(telemetry) = engine.latest_telemetry() {
                for source in &telemetry.sources {
                    if source.slot == blackbird {
                        heard.extend(source.take);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !heard.is_empty(),
            "the blackbird was planned but never heard"
        );

        // Paused, the scene's clock stands still: what is planned does not
        // come closer. Scrubbing the weather moves its story at once.
        engine.set_paused(true).expect("pause");
        std::thread::sleep(Duration::from_millis(300));
        let before = engine.planned(30.0).first().map(|take| take.starts_in);
        std::thread::sleep(Duration::from_millis(500));
        let after = engine.planned(30.0).first().map(|take| take.starts_in);
        assert!(engine.paused());
        assert_eq!(before, after, "a paused plan stands still");
        let paused_at = engine.scene_time();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            engine.scene_time(),
            paused_at,
            "the scene's time stands still"
        );
        engine.set_paused(false).expect("play");
        engine.seek(1234.0);
        assert!(
            (engine.scene_time() - 1234.0).abs() < 0.5,
            "moved to 1234 s"
        );
        std::thread::sleep(Duration::from_millis(300));
        assert!(engine.scene_time() > 1234.0, "and plays on from there");
        engine.set_weather_speed(60.0);
        assert_eq!(engine.weather_speed(), 60.0);

        // What is planned is there to read, and the weather can be changed and
        // saved; conditions are known for the past and the future alike.
        assert!(engine
            .planned(30.0)
            .iter()
            .any(|take| take.slot == blackbird));
        engine
            .set_weather(Weather {
                story: Story::PassingStorm,
                wind: Band {
                    low: 0.1,
                    high: 0.3,
                },
                rain: Band {
                    low: 0.0,
                    high: 0.0,
                },
                storm: Band {
                    low: 0.0,
                    high: 0.0,
                },
                daylight: Band {
                    low: 1.0,
                    high: 1.0,
                },
            })
            .expect("weather");
        assert_eq!(engine.weather().story, Story::PassingStorm);
        let hour = engine.conditions(-3600.0, 3600.0, 721);
        assert_eq!(hour.len(), 721);
        assert!(hour.iter().all(|now| (0.0..=1.0).contains(&now.wind)));
        let saved =
            std::env::temp_dir().join(format!("atrium-weather-{}.yaml", std::process::id()));
        let saved = saved.to_str().expect("utf-8").to_owned();
        let _ = std::fs::remove_file(&saved);
        engine
            .save_scene(saved.clone(), String::new())
            .expect("saves");
        let written = SceneConfig::load(&saved).expect("reads");
        let weather = written.weather.clone().expect("the weather is saved");
        assert_eq!(weather.story, "passing storm");
        assert_eq!(weather.wind, atrium::config::Band::Between([0.1, 0.3]));
        written.build().expect("the saved scene builds");
        let _ = std::fs::remove_file(&saved);
    }
}
