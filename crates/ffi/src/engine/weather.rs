//! The scene's weather: its story, the band of each condition, and the
//! conditions at any moment.

use atrium::config::{SceneConfig, SceneKind, WeatherConfig};
use atrium::plan::Condition;

use super::{lock, rounded, Engine};
use crate::api::{Band, Conditions, EngineError, Weather};

/// The most moments one `conditions` call returns.
const MAX_CONDITION_STEPS: u32 = 2048;

#[uniffi::export]
impl Engine {
    /// The scene's weather: its story and the band of each condition.
    /// Nothing for a listening session, which runs no weather.
    pub fn weather(&self) -> Option<Weather> {
        let weather = self.planner().weather()?;
        let band = |condition| {
            let (low, high) = weather.band(condition);
            Band { low, high }
        };
        Some(Weather {
            story: weather.story.into(),
            wind: band(Condition::Wind),
            rain: band(Condition::Rain),
            storm: band(Condition::Storm),
            daylight: band(Condition::Daylight),
            wind_from: weather.wind_from(),
        })
    }

    /// Let `weather` move the conditions from now on, and keep it with the
    /// scene. Every sound that follows a condition follows the new story;
    /// what is already handed to the audio thread still plays. A listening
    /// session has no weather and refuses it.
    pub fn set_weather(&self, weather: Weather) -> Result<(), EngineError> {
        let bands = [weather.wind, weather.rain, weather.storm, weather.daylight];
        if bands
            .iter()
            .any(|band| !(band.low.is_finite() && band.high.is_finite()))
            || !weather.wind_from.is_finite()
        {
            return Ok(());
        }
        let story: atrium::plan::Story = weather.story.into();
        let in_file = |band: Band| {
            let (low, high) = (band.low.clamp(0.0, 1.0), band.high.clamp(0.0, 1.0));
            let (low, high) = (rounded(low.min(high), 100.0), rounded(low.max(high), 100.0));
            if low == high {
                atrium::config::Band::At(low)
            } else {
                atrium::config::Band::Between([low, high])
            }
        };
        let weather = WeatherConfig {
            story: story.name().to_string(),
            wind: in_file(weather.wind),
            rain: in_file(weather.rain),
            storm: in_file(weather.storm),
            daylight: in_file(weather.daylight),
            wind_from: rounded(weather.wind_from.rem_euclid(360.0), 1.0) % 360.0,
        };
        let mut scene = lock(&self.scene);
        if scene.document.kind == SceneKind::Listening {
            return Err(EngineError::Scene(
                "a listening session has no weather".to_string(),
            ));
        }
        let before = scene.document.weather.replace(weather);
        let played = self.play_weather(&scene.document);
        if played.is_err() {
            scene.document.weather = before;
        }
        played
    }

    /// The conditions from `from` to `to` seconds around now (negative is
    /// the past), at `steps` evenly spaced moments. The weather is a pure
    /// function of time, so the past is what was, and the future what will be
    /// unless the weather is changed.
    pub fn conditions(&self, from: f32, to: f32, steps: u32) -> Vec<Conditions> {
        let steps = steps.clamp(1, MAX_CONDITION_STEPS);
        (0..steps)
            .map(|step| {
                let along = if steps == 1 {
                    0.0
                } else {
                    step as f32 / (steps - 1) as f32
                };
                self.planner()
                    .conditions((from + (to - from) * along) as f64)
                    .into()
            })
            .collect()
    }

    /// Let the weather run `speed` times as fast as the scene: 60 runs an hour
    /// of weather in a minute, a time-lapse, while every sound still plays
    /// at its own pace.
    pub fn set_weather_speed(&self, speed: f32) {
        self.planner().set_weather_speed(speed as f64);
    }

    /// How many times as fast as the scene the weather runs: 1 until
    /// `set_weather_speed` changes it.
    pub fn weather_speed(&self) -> f32 {
        self.planner().weather_speed() as f32
    }
}

impl Engine {
    /// Let the weather `document` runs move the conditions from now on: its
    /// story, a calm day for a nature scene without one, and none for a
    /// listening session.
    pub(super) fn play_weather(&self, document: &SceneConfig) -> Result<(), EngineError> {
        let weather = document
            .weather_story(self.planner().seed())
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        self.planner().set_weather(weather);
        Ok(())
    }
}
