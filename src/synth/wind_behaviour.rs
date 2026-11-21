//! What the wind does now and then, as a repertoire.
//!
//! Real wind is mostly steady air that drifts slowly, with a gust, a long
//! swell or a lull every so often, not a gust every few seconds. So, like a
//! bird with its songs, the wind has a repertoire: the things it can do, each
//! with how often it is chosen, how strong and how long it is, and a rest of
//! steady wind between two of them. Everything is drawn from the seed, so a
//! session plays the same wind again.

use std::f32::consts::PI;

use super::noise::Rng;

/// One kind of thing the wind does.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindMove {
    /// A quick push: up in about a fifth of its length, then easing off.
    Gust,
    /// A slow rise and fall, even on both sides.
    Swell,
    /// The air drops away below the steady wind for a while.
    Lull,
}

/// One entry of the repertoire.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WindEvent {
    pub kind: WindMove,
    /// How often it is chosen next to the others: 2 twice as often as 1.
    pub chance: f32,
    /// How strong, lowest and highest, from 0 to 1 of what the wind can add
    /// (or, for a lull, take away).
    pub strength: (f32, f32),
    /// How long, shortest and longest (s).
    pub seconds: (f32, f32),
}

/// The repertoire and the steady wind between its moves.
#[derive(Clone, Debug, PartialEq)]
pub struct WindBehaviour {
    /// Steady wind between two moves, shortest and longest (s).
    pub rest: (f32, f32),
    pub events: Vec<WindEvent>,
}

/// Plays a behaviour: rests, then a move, then a rest again.
pub(crate) struct WindMoves {
    behaviour: WindBehaviour,
    rng: Rng,
    /// The move under way, or none while resting.
    playing: Option<(WindEvent, f32)>,
    position: usize,
    length: usize,
    /// Nothing has been played yet: the first rest starts partway.
    first: bool,
}

impl WindMoves {
    pub(crate) fn new(behaviour: WindBehaviour, seed: u64) -> Self {
        Self {
            behaviour,
            rng: Rng::new(seed),
            playing: None,
            position: 0,
            length: 0,
            first: true,
        }
    }

    /// The push of the wind now, per sample: above 0 adds to the steady wind
    /// (a gust or swell, up to 1), below 0 takes from it (a lull, down to −1).
    #[inline(always)]
    pub(crate) fn next(&mut self, sample_rate: f32) -> f32 {
        if self.first {
            // Start somewhere inside a rest, so a scene does not open on a gust.
            self.first = false;
            self.start_rest(sample_rate);
            self.position = (self.rng.next_f32() * self.length as f32) as usize;
        } else if self.position >= self.length {
            match self.playing {
                Some(_) => self.start_rest(sample_rate),
                None => self.start_move(sample_rate),
            }
        }
        let along = self.position as f32 / self.length.max(1) as f32;
        self.position += 1;
        let Some((event, strength)) = self.playing else {
            return 0.0;
        };
        let shape = match event.kind {
            WindMove::Gust => gust_shape(along),
            WindMove::Swell | WindMove::Lull => 0.5 - 0.5 * (2.0 * PI * along).cos(),
        };
        let sign = if event.kind == WindMove::Lull {
            -1.0
        } else {
            1.0
        };
        sign * strength * shape
    }

    fn start_rest(&mut self, sample_rate: f32) {
        self.playing = None;
        let (shortest, longest) = self.behaviour.rest;
        self.position = 0;
        self.length = (self.between(shortest, longest) * sample_rate).max(1.0) as usize;
    }

    fn start_move(&mut self, sample_rate: f32) {
        let total: f32 = self
            .behaviour
            .events
            .iter()
            .map(|event| event.chance.max(0.0))
            .sum();
        if total <= 0.0 {
            return self.start_rest(sample_rate);
        }
        let mut pick = self.rng.next_f32() * total;
        let mut chosen = self.behaviour.events[0];
        for event in &self.behaviour.events {
            pick -= event.chance.max(0.0);
            if pick <= 0.0 {
                chosen = *event;
                break;
            }
        }
        let strength = self
            .between(chosen.strength.0, chosen.strength.1)
            .clamp(0.0, 1.0);
        let seconds = self.between(chosen.seconds.0, chosen.seconds.1).max(0.05);
        self.playing = Some((chosen, strength));
        self.position = 0;
        self.length = (seconds * sample_rate).max(1.0) as usize;
    }

    fn between(&mut self, low: f32, high: f32) -> f32 {
        let (low, high) = (low.min(high), low.max(high));
        low + self.rng.next_f32() * (high - low)
    }
}

/// A gust's shape along its length, 0 to 1 and back: a quick rise over the
/// first fifth, then a long easing off.
fn gust_shape(along: f32) -> f32 {
    const PEAK: f32 = 0.2;
    if along < PEAK {
        0.5 - 0.5 * (PI * along / PEAK).cos()
    } else {
        0.5 + 0.5 * (PI * (along - PEAK) / (1.0 - PEAK)).cos()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn breeze() -> WindBehaviour {
        WindBehaviour {
            rest: (40.0, 120.0),
            events: vec![
                WindEvent {
                    kind: WindMove::Gust,
                    chance: 3.0,
                    strength: (0.3, 0.8),
                    seconds: (3.0, 8.0),
                },
                WindEvent {
                    kind: WindMove::Swell,
                    chance: 2.0,
                    strength: (0.2, 0.5),
                    seconds: (10.0, 25.0),
                },
                WindEvent {
                    kind: WindMove::Lull,
                    chance: 1.0,
                    strength: (0.3, 0.6),
                    seconds: (8.0, 20.0),
                },
            ],
        }
    }

    /// Over an hour the wind is steady most of the time, with a move every
    /// minute or two, and every kind in the repertoire is heard.
    #[test]
    fn moves_come_now_and_then_with_steady_wind_between() {
        let mut moves = WindMoves::new(breeze(), 3);
        let rate = 100.0;
        let (mut busy, mut starts, mut was) = (0usize, 0usize, false);
        let (mut up, mut down) = (false, false);
        for _ in 0..(3600.0 * rate) as usize {
            let push = moves.next(rate);
            let now = push.abs() > 1e-3;
            busy += now as usize;
            starts += (now && !was) as usize;
            was = now;
            up |= push > 0.1;
            down |= push < -0.1;
        }
        let share = busy as f32 / (3600.0 * rate);
        assert!(share < 0.25, "moving {share} of the time");
        assert!((20..=60).contains(&starts), "{starts} moves in an hour");
        assert!(up && down, "gusts or swells, and lulls, are both heard");
    }

    #[test]
    fn a_gust_rises_quickly_and_eases_off_slowly() {
        assert_eq!(gust_shape(0.0), 0.0);
        assert!((gust_shape(0.2) - 1.0).abs() < 1e-6);
        assert!(gust_shape(0.1) > 0.4 && gust_shape(0.6) > 0.4);
        assert!(gust_shape(1.0).abs() < 1e-6);
    }

    #[test]
    fn the_same_seed_plays_the_same_wind() {
        let mut first = WindMoves::new(breeze(), 9);
        let mut second = WindMoves::new(breeze(), 9);
        for _ in 0..100_000 {
            assert_eq!(first.next(50.0), second.next(50.0));
        }
    }
}
