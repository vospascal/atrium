//! What moves the conditions: a weather story.
//!
//! A story is a pure function of time and seed, so the conditions an hour
//! from now are known now, every device on a session agrees on them, and the
//! plan can decide a bird's silence in the rain before the rain is heard.

use std::f64::consts::TAU;

use super::conditions::{Condition, Conditions};

/// How the conditions move.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Story {
    /// They stay where they are set: the middle of each band.
    Held,
    /// They wander inside their bands, slowly and never regularly.
    Drifting,
    /// They drift, and now and then a storm passes: stillness, a gust front,
    /// a downpour with close thunder, easing, dripping, and back. Every storm
    /// and every quiet spell between has its own length.
    PassingStorm,
}

impl Story {
    pub fn name(self) -> &'static str {
        match self {
            Story::Held => "held",
            Story::Drifting => "drifting",
            Story::PassingStorm => "passing storm",
        }
    }

    pub fn named(name: &str) -> Option<Self> {
        [Story::Held, Story::Drifting, Story::PassingStorm]
            .into_iter()
            .find(|story| story.name() == name)
    }
}

/// A story with the bands it keeps the conditions in.
#[derive(Clone, Debug, PartialEq)]
pub struct Weather {
    pub story: Story,
    /// Lowest and highest of each condition, in [`Condition::ALL`] order.
    pub bands: [(f32, f32); Condition::ALL.len()],
    seed: u64,
}

/// Shortest and longest quiet spell between two storms (s).
const QUIET_SECONDS: (f64, f64) = (8.0 * 60.0, 25.0 * 60.0);
/// The first quiet spell, before the first storm: short, so a scene with a
/// passing storm shows what it is within minutes (s).
const FIRST_QUIET_SECONDS: (f64, f64) = (60.0, 180.0);
/// Shortest and longest storm, from first gust to the last drip (s).
const STORM_SECONDS: (f64, f64) = (14.0 * 60.0, 30.0 * 60.0);

impl Weather {
    pub fn new(story: Story, bands: [(f32, f32); Condition::ALL.len()], seed: u64) -> Self {
        let bands = bands.map(|(low, high)| {
            let (low, high) = (low.clamp(0.0, 1.0), high.clamp(0.0, 1.0));
            (low.min(high), low.max(high))
        });
        Self { story, bands, seed }
    }

    /// A still, dry, bright day: nothing moves.
    pub fn calm(seed: u64) -> Self {
        let mut bands = [(0.0, 0.0); Condition::ALL.len()];
        bands[Condition::Daylight as usize] = (1.0, 1.0);
        Self::new(Story::Held, bands, seed)
    }

    pub fn band(&self, condition: Condition) -> (f32, f32) {
        self.bands[condition as usize]
    }

    /// Every condition at `seconds` on the session's clock.
    pub fn at(&self, seconds: f64) -> Conditions {
        let mut conditions = Conditions::default();
        for condition in Condition::ALL {
            let (low, high) = self.band(condition);
            let value = match self.story {
                Story::Held => (low + high) / 2.0,
                Story::Drifting | Story::PassingStorm => {
                    low + (high - low) * self.drift(condition, seconds)
                }
            };
            conditions.set(condition, value);
        }
        if self.story == Story::PassingStorm {
            if let Some(phase) = self.storm_phase(seconds) {
                storm(&mut conditions, phase);
            }
        }
        conditions
    }

    /// A slow wander from 0 to 1: three swells with periods of minutes that
    /// never line up, each with its own phase from the seed.
    fn drift(&self, condition: Condition, seconds: f64) -> f32 {
        let mut random = Random::new(self.seed ^ (condition as u64 + 1).wrapping_mul(0x9E37_79B9));
        let mut sum = 0.0;
        for period_minutes in [3.1, 7.3, 13.7] {
            let phase = random.unit() * TAU;
            sum += (seconds / (period_minutes * 60.0) * TAU + phase).sin();
        }
        ((sum / 3.0 + 1.0) / 2.0).clamp(0.0, 1.0) as f32
    }

    /// How far through a storm `seconds` is, 0 to 1, or `None` in a quiet
    /// spell. Spells and storms follow each other with lengths from the seed.
    fn storm_phase(&self, seconds: f64) -> Option<f32> {
        let mut random = Random::new(self.seed ^ 0x0057_07A1);
        let mut start = 0.0;
        let mut first = true;
        loop {
            let spell = if first {
                FIRST_QUIET_SECONDS
            } else {
                QUIET_SECONDS
            };
            first = false;
            let quiet = between(spell, random.unit());
            let length = between(STORM_SECONDS, random.unit());
            let storm_start = start + quiet;
            if seconds < storm_start {
                return None;
            }
            if seconds < storm_start + length {
                return Some(((seconds - storm_start) / length) as f32);
            }
            start = storm_start + length;
        }
    }
}

/// A storm's arc laid over the drifting conditions, `phase` from 0 to 1:
/// a gust front, the downpour with the storm at its height, easing, and the
/// dripping after. Daylight dims under the clouds.
fn storm(conditions: &mut Conditions, phase: f32) {
    let rise = |from: f32, to: f32| smooth((phase - from) / (to - from));
    let gusts = rise(0.0, 0.2) * (1.0 - rise(0.6, 0.9));
    let rain = rise(0.15, 0.35) * (1.0 - rise(0.55, 0.95));
    let storm = rise(0.25, 0.4) * (1.0 - rise(0.5, 0.7));
    let lift = |condition: Condition, by: f32, conditions: &mut Conditions| {
        let now = conditions.get(condition);
        conditions.set(condition, now + (1.0 - now) * by);
    };
    lift(Condition::Wind, 0.85 * gusts, conditions);
    lift(Condition::Rain, rain, conditions);
    lift(Condition::Storm, storm, conditions);
    let daylight = conditions.get(Condition::Daylight);
    conditions.set(
        Condition::Daylight,
        daylight * (1.0 - 0.6 * rain.max(gusts)),
    );
}

/// 0 below 0, 1 above 1, an S between.
fn smooth(along: f32) -> f32 {
    let along = along.clamp(0.0, 1.0);
    along * along * (3.0 - 2.0 * along)
}

fn between((low, high): (f64, f64), unit: f64) -> f64 {
    low + (high - low) * unit
}

/// A small seeded generator, the same xorshift the voices use.
struct Random(u64);

impl Random {
    fn new(seed: u64) -> Self {
        Self(seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1)
    }

    fn unit(&mut self) -> f64 {
        self.0 ^= self.0 >> 12;
        self.0 ^= self.0 << 25;
        self.0 ^= self.0 >> 27;
        (self.0.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 11) as f64 / (1u64 << 53) as f64
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn bands(
        wind: (f32, f32),
        rain: (f32, f32),
        storm: (f32, f32),
        daylight: (f32, f32),
    ) -> [(f32, f32); 4] {
        [wind, rain, storm, daylight]
    }

    #[test]
    fn held_weather_stays_in_the_middle_of_its_bands() {
        let weather = Weather::new(
            Story::Held,
            bands((0.2, 0.4), (0.0, 0.0), (0.0, 0.0), (1.0, 1.0)),
            1,
        );
        for seconds in [0.0, 600.0, 7200.0] {
            let now = weather.at(seconds);
            assert!((now.get(Condition::Wind) - 0.3).abs() < 1e-6);
            assert_eq!(now.get(Condition::Daylight), 1.0);
        }
    }

    /// Drifting stays inside its band, moves slowly, uses most of the band
    /// over an hour, and two seeds drift differently.
    #[test]
    fn drifting_weather_wanders_slowly_inside_its_band() {
        let weather = Weather::new(
            Story::Drifting,
            bands((0.1, 0.5), (0.0, 0.0), (0.0, 0.0), (1.0, 1.0)),
            3,
        );
        let wind: Vec<f32> = (0..3600)
            .map(|second| weather.at(second as f64).get(Condition::Wind))
            .collect();
        assert!(wind.iter().all(|value| (0.1..=0.5).contains(value)));
        assert!(
            wind.windows(2)
                .all(|pair| (pair[1] - pair[0]).abs() < 0.005),
            "no jumps from one second to the next"
        );
        let (low, high) = wind.iter().fold((1f32, 0f32), |(low, high), value| {
            (low.min(*value), high.max(*value))
        });
        assert!(high - low > 0.2, "it uses the band: {low}..{high}");
        let other = Weather::new(Story::Drifting, weather.bands, 4);
        assert_ne!(other.at(900.0), weather.at(900.0));
    }

    /// A passing storm comes, rains, thunders and goes: over three hours
    /// there are storms with dry spells between, rain falls only in them,
    /// the storm peaks inside the rain, and every storm is a different length.
    #[test]
    fn a_passing_storm_comes_and_goes() {
        let weather = Weather::new(
            Story::PassingStorm,
            bands((0.05, 0.2), (0.0, 0.0), (0.0, 0.0), (1.0, 1.0)),
            9,
        );
        let samples: Vec<Conditions> = (0..3 * 3600)
            .map(|second| weather.at(second as f64))
            .collect();
        let raining: Vec<bool> = samples
            .iter()
            .map(|now| now.get(Condition::Rain) > 0.5)
            .collect();
        let showers = raining
            .windows(2)
            .filter(|pair| !pair[0] && pair[1])
            .count();
        assert!(showers >= 3, "only {showers} storms in three hours");
        let lengths: Vec<usize> = raining
            .split(|wet| !wet)
            .map(<[bool]>::len)
            .filter(|length| *length > 0)
            .collect();
        assert!(
            lengths.windows(2).any(|pair| pair[0] != pair[1]),
            "every storm the same length: {lengths:?}"
        );
        for now in &samples {
            if now.get(Condition::Storm) > 0.5 {
                assert!(
                    now.get(Condition::Rain) > 0.3,
                    "thunder comes with the rain"
                );
                assert!(
                    now.get(Condition::Daylight) < 0.9,
                    "it is darker under a storm"
                );
            }
        }
        assert!(
            samples[..60]
                .iter()
                .all(|now| now.get(Condition::Rain) == 0.0),
            "it starts quiet"
        );
    }

    #[test]
    fn a_story_is_named_as_a_file_names_it() {
        assert_eq!(Story::named("passing storm"), Some(Story::PassingStorm));
        assert_eq!(Story::named("drifting"), Some(Story::Drifting));
        assert_eq!(Story::named("sunny"), None);
    }
}
