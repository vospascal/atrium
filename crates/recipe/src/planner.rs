//! When each chord comes, how long it holds and how it swells: decided ahead,
//! from the recipe and a seed. The same seed gives the same piece; a new seed
//! a new performance of it.
//!
//! A chord's `hold` is the time from its arrival to the next chord's: the
//! next begins to swell in when this one begins to swell out, so the two
//! overlap for as long as the fades last and the change has no edge. The
//! first chord is already sounding (no swell in): the piece begins in the
//! middle of itself, and its level from the first second is its level.

use crate::api::{Change, Recipe};
use crate::random::Random;
use crate::span::Span;

/// One chord's turn.
#[derive(Clone, Copy, Debug, PartialEq)]
#[non_exhaustive]
pub struct ChordEvent {
    /// Which chord, as an index into [`Recipe::chords`].
    pub chord: usize,
    /// When it begins to swell in (s from the start).
    pub at: f64,
    /// Until the next chord arrives (s); this one then swells out.
    pub hold: f64,
    pub swell_in: f64,
    pub swell_out: f64,
}

/// Decides the chords' turns one after another. Allocation-free after
/// [`Planner::new`], so an audio thread may ask it for the next turn.
#[derive(Clone, Debug)]
pub struct Planner {
    change: Change,
    hold: Vec<Span>,
    swell_in: Span,
    swell_out: Span,
    start: usize,
    seed: u64,
    random: Random,
    chord: usize,
    at: f64,
    first: bool,
}

impl Planner {
    pub fn new(recipe: &Recipe, seed: u64) -> Self {
        Self {
            change: recipe.change.clone(),
            hold: recipe.hold.clone(),
            swell_in: recipe.swell_in,
            swell_out: recipe.swell_out,
            start: recipe.start,
            seed,
            random: Random::new(seed),
            chord: recipe.start,
            at: 0.0,
            first: true,
        }
    }

    /// Back to the start of the same performance.
    pub fn restart(&mut self) {
        self.random = Random::new(self.seed);
        self.chord = self.start;
        self.at = 0.0;
        self.first = true;
    }

    fn following(&mut self, chord: usize) -> usize {
        match &self.change {
            Change::InOrder => (chord + 1) % self.hold.len(),
            Change::Table(rows) => {
                let draw = self.random.unit();
                let row = &rows[chord];
                row.iter()
                    .find(|(_, running)| draw < *running)
                    .or(row.last())
                    .map_or(chord, |(next, _)| *next)
            }
        }
    }
}

impl Iterator for Planner {
    type Item = ChordEvent;

    /// The next chord's turn; it never ends.
    fn next(&mut self) -> Option<ChordEvent> {
        let chord = self.chord;
        let hold = self.hold[chord].draw(&mut self.random);
        let swell_in = if self.first {
            0.0
        } else {
            self.swell_in.draw(&mut self.random).min(hold)
        };
        let swell_out = self.swell_out.draw(&mut self.random);
        let event = ChordEvent {
            chord,
            at: self.at,
            hold,
            swell_in,
            swell_out,
        };
        self.at += hold;
        self.chord = self.following(chord);
        self.first = false;
        Some(event)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::RecipeDef;

    fn recipe(yaml: &str) -> Recipe {
        serde_yaml::from_str::<RecipeDef>(yaml)
            .expect("parses")
            .resolve()
            .expect("valid")
    }

    const ROCKING: &str = "
chords:
  - { name: D, notes: [D3] }
  - { name: C, notes: [C3] }
change: alternate
hold: 12..16 s
swell: { in: 0.5..6 s, out: 5..24 s }
";

    #[test]
    fn alternate_rocks_and_every_turn_follows_the_last() {
        let turns: Vec<ChordEvent> = Planner::new(&recipe(ROCKING), 1).take(20).collect();
        for (index, turn) in turns.iter().enumerate() {
            assert_eq!(turn.chord, index % 2);
            assert!((12.0..=16.0).contains(&turn.hold), "{turn:?}");
            assert!((5.0..=24.0).contains(&turn.swell_out), "{turn:?}");
            if index > 0 {
                let before = &turns[index - 1];
                assert!((turn.at - (before.at + before.hold)).abs() < 1e-9);
                assert!((0.5..=6.0).contains(&turn.swell_in), "{turn:?}");
            }
        }
        assert_eq!(turns[0].at, 0.0);
        assert_eq!(turns[0].swell_in, 0.0, "the piece begins already sounding");
    }

    #[test]
    fn the_same_seed_is_the_same_piece_and_a_restart_replays_it() {
        let rocking = recipe(ROCKING);
        let first: Vec<ChordEvent> = Planner::new(&rocking, 5).take(10).collect();
        let again: Vec<ChordEvent> = Planner::new(&rocking, 5).take(10).collect();
        assert_eq!(first, again);
        let other: Vec<ChordEvent> = Planner::new(&rocking, 6).take(10).collect();
        assert_ne!(first, other);
        let mut planner = Planner::new(&rocking, 5);
        planner.by_ref().take(7).for_each(drop);
        planner.restart();
        assert_eq!(planner.take(10).collect::<Vec<_>>(), first);
    }

    /// From D, C 80 % and Am7 20 %; Am7 always back to D; C always to D. Over
    /// 5000 turns from D the share going to C is 0.8 within 0.02.
    #[test]
    fn a_table_of_chances_is_followed() {
        let table = recipe(
            "
chords:
  - { name: D, notes: [D3] }
  - { name: C, notes: [C3] }
  - { name: Am7, notes: [A2] }
change: { D: { C: 0.8, Am7: 0.2 }, C: { D: 1 }, Am7: { D: 1 } }
",
        );
        let turns: Vec<usize> = Planner::new(&table, 9)
            .take(10_000)
            .map(|turn| turn.chord)
            .collect();
        let (mut from_d, mut to_c) = (0, 0);
        for pair in turns.windows(2) {
            assert!(
                pair[0] == 0 || pair[1] == 0,
                "C and Am7 always go back to D"
            );
            if pair[0] == 0 {
                from_d += 1;
                if pair[1] == 1 {
                    to_c += 1;
                }
            }
        }
        let share = to_c as f64 / from_d as f64;
        assert!((share - 0.8).abs() < 0.02, "{share}");
    }

    #[test]
    fn a_swell_in_never_outlasts_its_hold() {
        let long = recipe(
            "
chords: [{ name: D, notes: [D3] }, { name: C, notes: [C3] }]
hold: 2 s
swell: { in: 10 s, out: 1 s }
",
        );
        for turn in Planner::new(&long, 2).take(10).skip(1) {
            assert_eq!(turn.swell_in, 2.0);
        }
    }
}
