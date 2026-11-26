//! Events on top of the chords: pings (struck notes, each on a note of the
//! chord playing, ringing out) and swooshes (a soft swell of noise in a
//! band, now and then). After Astral Ambience "Calm" (docs/ambient-recipes.md):
//! pings of more than one kind, lower and higher, each ringing out for
//! seconds, one now and then and maybe another some seconds later; a swoosh
//! every 15–40 s, rising over 1.5–5.5 s and fading over 1.5–5 s, in about
//! 1.5–4 kHz.
//!
//! Decided here from a seed, like the chords; the engine's voice picks the
//! note from the chord sounding at the ping's time and makes the sound.

use serde::Deserialize;

use crate::note;
use crate::random::Random;
use crate::span::{Span, SpanDef};
use crate::RecipeError;

/// One kind of ping; a recipe may have several (`pings: [{...}, {...}]`).
#[derive(Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct PingsDef {
    /// The lowest and highest note a ping may sound (`[E4, D5]`): the
    /// chord's note is moved by octaves into it.
    #[serde(default)]
    range: Option<[String; 2]>,
    /// From one run to the next (s).
    #[serde(default)]
    every: Option<SpanDef>,
    /// How many pings a run has.
    #[serde(default)]
    run: Option<SpanDef>,
    /// From one ping of a run to the next (s).
    #[serde(default)]
    gap: Option<SpanDef>,
    /// How long a ping takes to fall 20 dB (s).
    #[serde(default)]
    ring: Option<SpanDef>,
    /// Its peak against one chord note at full (dB).
    #[serde(default)]
    level: Option<f64>,
}

#[derive(Deserialize, Clone, Debug, PartialEq)]
pub(crate) struct SwooshDef {
    #[serde(default)]
    every: Option<SpanDef>,
    #[serde(default)]
    rise: Option<SpanDef>,
    #[serde(default)]
    fall: Option<SpanDef>,
    /// The band its noise fills: `1500..4000 Hz`.
    #[serde(default)]
    band: Option<SpanDef>,
    /// Its peak against one chord note at full (dB).
    #[serde(default)]
    level: Option<f64>,
}

/// One kind of ping, ready to plan.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Pings {
    /// Its place in the recipe's list: each kind plans from its own seed.
    kind: usize,
    pub low_hertz: f64,
    pub high_hertz: f64,
    pub every: Span,
    pub run: Span,
    pub gap: Span,
    pub ring: Span,
    pub level_db: f64,
}

/// Swooshes, ready to plan.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct Swoosh {
    pub every: Span,
    pub rise: Span,
    pub fall: Span,
    pub low_hertz: f64,
    pub high_hertz: f64,
    pub level_db: f64,
}

fn span(
    def: &Option<SpanDef>,
    field: &str,
    unit: &str,
    default: (f64, f64),
) -> Result<Span, RecipeError> {
    let span = match def {
        Some(def) => def.resolve(field, unit, 0.0)?,
        None => Span {
            low: default.0,
            high: default.1,
        },
    };
    if span.low <= 0.0 {
        return Err(RecipeError::new(
            field,
            format!("must be more than 0 {unit}"),
        ));
    }
    Ok(span)
}

fn level(value: Option<f64>, field: &str, default: f64) -> Result<f64, RecipeError> {
    let level = value.unwrap_or(default);
    if !level.is_finite() || level > 12.0 {
        return Err(RecipeError::new(field, "is a level in dB, at most +12"));
    }
    Ok(level)
}

impl PingsDef {
    /// The `kind`th in the recipe's list of pings.
    pub(crate) fn resolve(&self, a4: f64, kind: usize) -> Result<Pings, RecipeError> {
        let field = |name: &str| format!("pings[{kind}].{name}");
        let (low_hertz, high_hertz) = match &self.range {
            Some([low, high]) => (
                note::frequency(low, a4, &format!("{}[0]", field("range")))?,
                note::frequency(high, a4, &format!("{}[1]", field("range")))?,
            ),
            None => (
                note::frequency("E4", a4, &field("range"))?,
                note::frequency("D5", a4, &field("range"))?,
            ),
        };
        if low_hertz >= high_hertz {
            return Err(RecipeError::new(
                field("range"),
                "runs from a low note to a higher one",
            ));
        }
        let run = span(&self.run, &field("run"), "", (1.0, 1.0))?;
        if run.low < 1.0 {
            return Err(RecipeError::new(field("run"), "is at least 1 ping"));
        }
        Ok(Pings {
            kind,
            low_hertz,
            high_hertz,
            every: span(&self.every, &field("every"), "s", (5.0, 12.0))?,
            run,
            gap: span(&self.gap, &field("gap"), "s", (1.0, 3.0))?,
            ring: span(&self.ring, &field("ring"), "s", (1.0, 3.0))?,
            level_db: level(self.level, &field("level"), -6.0)?,
        })
    }
}

impl SwooshDef {
    pub(crate) fn resolve(&self) -> Result<Swoosh, RecipeError> {
        let band = span(&self.band, "swoosh.band", "Hz", (1500.0, 4000.0))?;
        if band.low < 20.0 || band.low >= band.high {
            return Err(RecipeError::new(
                "swoosh.band",
                "runs from at least 20 Hz up to a higher frequency, such as `1500..4000 Hz`",
            ));
        }
        Ok(Swoosh {
            every: span(&self.every, "swoosh.every", "s", (15.0, 40.0))?,
            rise: span(&self.rise, "swoosh.rise", "s", (1.5, 5.5))?,
            fall: span(&self.fall, "swoosh.fall", "s", (1.5, 5.0))?,
            low_hertz: band.low,
            high_hertz: band.high,
            level_db: level(self.level, "swoosh.level", -20.0)?,
        })
    }
}

/// One ping.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct PingEvent {
    /// When it strikes (s from the start).
    pub at: f64,
    /// Which of the sounding chord's notes, as a share from 0 to 1 of them.
    pub choice: f64,
    /// How long it takes to fall 20 dB (s).
    pub ring: f64,
}

/// One kind of ping's times, run by run. Allocation-free.
#[derive(Clone, Debug)]
pub struct PingPlanner {
    pings: Pings,
    seed: u64,
    random: Random,
    at: f64,
    left_in_run: u32,
}

impl PingPlanner {
    pub fn new(pings: &Pings, seed: u64) -> Self {
        // Each kind its own stream, so adding a kind leaves the others as
        // they were.
        let seed =
            seed ^ 0x0050_494E_4753 ^ (pings.kind as u64 + 1).wrapping_mul(0x9E37_79B9_7F4A_7C15);
        let mut planner = Self {
            pings: *pings,
            seed,
            random: Random::new(seed),
            at: 0.0,
            left_in_run: 0,
        };
        planner.restart();
        planner
    }

    pub fn restart(&mut self) {
        self.random = Random::new(self.seed);
        // The first run comes after one gap between runs, not at once.
        self.at = self.pings.every.draw(&mut self.random);
        self.left_in_run = 0;
    }
}

impl Iterator for PingPlanner {
    type Item = PingEvent;

    fn next(&mut self) -> Option<PingEvent> {
        if self.left_in_run == 0 {
            self.left_in_run = self.pings.run.draw(&mut self.random).round().max(1.0) as u32;
        }
        let event = PingEvent {
            at: self.at,
            choice: self.random.unit(),
            ring: self.pings.ring.draw(&mut self.random),
        };
        self.left_in_run -= 1;
        self.at += if self.left_in_run > 0 {
            self.pings.gap.draw(&mut self.random)
        } else {
            self.pings.every.draw(&mut self.random)
        };
        Some(event)
    }
}

/// One swoosh.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct SwooshEvent {
    /// When it begins to rise (s from the start).
    pub at: f64,
    pub rise: f64,
    pub fall: f64,
}

/// The swooshes' times. Allocation-free.
#[derive(Clone, Debug)]
pub struct SwooshPlanner {
    swoosh: Swoosh,
    seed: u64,
    random: Random,
    at: f64,
}

impl SwooshPlanner {
    pub fn new(swoosh: &Swoosh, seed: u64) -> Self {
        let mut planner = Self {
            swoosh: *swoosh,
            seed,
            random: Random::new(seed ^ 0x5357_4F4F_5348),
            at: 0.0,
        };
        planner.restart();
        planner
    }

    pub fn restart(&mut self) {
        self.random = Random::new(self.seed ^ 0x5357_4F4F_5348);
        self.at = self.swoosh.every.draw(&mut self.random);
    }
}

impl Iterator for SwooshPlanner {
    type Item = SwooshEvent;

    fn next(&mut self) -> Option<SwooshEvent> {
        let event = SwooshEvent {
            at: self.at,
            rise: self.swoosh.rise.draw(&mut self.random),
            fall: self.swoosh.fall.draw(&mut self.random),
        };
        // The next begins after this one is over, and a gap drawn after it.
        self.at += event.rise + event.fall + self.swoosh.every.draw(&mut self.random);
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pings(yaml: &str) -> Pings {
        serde_yaml::from_str::<PingsDef>(yaml)
            .expect("parses")
            .resolve(440.0, 0)
            .expect("valid")
    }

    /// Runs of 1–6 pings 0.1–0.7 s apart, a run every 4–12 s.
    #[test]
    fn pings_come_in_runs() {
        let plan = pings("{ every: 4..12 s, run: 1..6, gap: 0.1..0.7 s, ring: 0.2..0.6 s }");
        let events: Vec<PingEvent> = PingPlanner::new(&plan, 3).take(400).collect();
        let mut run = 1;
        let mut runs = Vec::new();
        for pair in events.windows(2) {
            let gap = pair[1].at - pair[0].at;
            assert!(
                (0.1..=0.7).contains(&gap) || (4.0..=12.0).contains(&gap),
                "{gap}"
            );
            if gap <= 0.7 {
                run += 1;
            } else {
                runs.push(run);
                run = 1;
            }
            assert!((0.2..=0.6).contains(&pair[0].ring));
            assert!((0.0..1.0).contains(&pair[0].choice));
        }
        assert!(runs.iter().all(|count| (1..=6).contains(count)), "{runs:?}");
        assert!(runs.iter().any(|count| *count > 1) && runs.contains(&1));
        assert!((4.0..=12.0).contains(&events[0].at), "the first run waits");
    }

    #[test]
    fn the_same_seed_gives_the_same_pings_and_swooshes() {
        let plan = pings("{}");
        let first: Vec<PingEvent> = PingPlanner::new(&plan, 5).take(20).collect();
        assert_eq!(
            first,
            PingPlanner::new(&plan, 5).take(20).collect::<Vec<_>>()
        );
        let swoosh = serde_yaml::from_str::<SwooshDef>("{}")
            .unwrap()
            .resolve()
            .unwrap();
        let mut planner = SwooshPlanner::new(&swoosh, 5);
        let swooshes: Vec<SwooshEvent> = planner.by_ref().take(5).collect();
        planner.restart();
        assert_eq!(swooshes, planner.take(5).collect::<Vec<_>>());
    }

    /// A swoosh every 15–40 s after the last has faded, rising 1.5–5.5 s and
    /// fading 1.5–5 s, in 1500–4000 Hz by default.
    #[test]
    fn swooshes_come_now_and_then() {
        let swoosh = serde_yaml::from_str::<SwooshDef>("{}")
            .unwrap()
            .resolve()
            .unwrap();
        assert_eq!((swoosh.low_hertz, swoosh.high_hertz), (1500.0, 4000.0));
        let events: Vec<SwooshEvent> = SwooshPlanner::new(&swoosh, 7).take(50).collect();
        for pair in events.windows(2) {
            let gap = pair[1].at - (pair[0].at + pair[0].rise + pair[0].fall);
            assert!((15.0..=40.0).contains(&gap), "{gap}");
            assert!((1.5..=5.5).contains(&pair[0].rise) && (1.5..=5.0).contains(&pair[0].fall));
        }
    }

    /// By default a ping comes alone, every 5–12 s, and rings 1–3 s to fall
    /// 20 dB: "ping, and some seconds later maybe again".
    #[test]
    fn a_ping_comes_alone_and_rings_out() {
        let events: Vec<PingEvent> = PingPlanner::new(&pings("{}"), 2).take(200).collect();
        for pair in events.windows(2) {
            let gap = pair[1].at - pair[0].at;
            assert!((5.0..=12.0).contains(&gap), "{gap}");
            assert!((1.0..=3.0).contains(&pair[0].ring));
        }
    }

    /// Two kinds with the same settings and seed still ping at different
    /// times: each plans from its own stream.
    #[test]
    fn each_kind_of_ping_has_its_own_times() {
        let def = serde_yaml::from_str::<PingsDef>("{}").unwrap();
        let first = def.resolve(440.0, 0).unwrap();
        let second = def.resolve(440.0, 1).unwrap();
        let times = |pings: &Pings| -> Vec<f64> {
            PingPlanner::new(pings, 9)
                .take(10)
                .map(|event| event.at)
                .collect()
        };
        assert_ne!(times(&first), times(&second));
    }

    #[test]
    fn mistakes_are_named() {
        let error = serde_yaml::from_str::<PingsDef>("{ range: [D5, E4] }")
            .unwrap()
            .resolve(440.0, 1)
            .unwrap_err();
        assert_eq!(error.field, "pings[1].range");
        let error = serde_yaml::from_str::<SwooshDef>("{ band: 4000..1500 Hz }")
            .unwrap()
            .resolve()
            .unwrap_err();
        assert_eq!(error.field, "swoosh.band");
        let error = serde_yaml::from_str::<PingsDef>("{ run: 0 }")
            .unwrap()
            .resolve(440.0, 0)
            .unwrap_err();
        assert_eq!(error.field, "pings[0].run");
    }
}
