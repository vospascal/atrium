//! Recipes: what a piece is made of, as data, and the decisions it leaves to
//! chance, made from a seed. A recipe names its chords, how one follows
//! another, how long each holds and how it swells in and out, and the voice
//! that plays them (docs/recipe-design.md). [`RecipeDef`] is the recipe as a
//! preset writes it; [`Recipe`] is the same, checked, with every note in
//! hertz; a [`Planner`] turns a recipe and a seed into the chords' times.
//!
//! No audio, no engine type, no file. The engine's voice plays the chords;
//! this crate only decides what plays when, so the decision is testable
//! without a device and the same seed always gives the same piece.

mod api;
mod events;
mod note;
mod planner;
mod random;
mod span;

pub use api::{Chord, Recipe, RecipeDef, Voice, MOST_NOTES, MOST_VOICES};
pub use events::{PingEvent, PingPlanner, Pings, Swoosh, SwooshEvent, SwooshPlanner};
pub use planner::{ChordEvent, Planner};
pub use span::Span;

use std::fmt;

/// Why a recipe cannot be played: which field, and what is wrong with it.
#[derive(Clone, Debug, PartialEq, Eq)]
#[non_exhaustive]
pub struct RecipeError {
    /// Where it is, as the preset writes it: `chords[1].notes[2]`, `hold.D`.
    pub field: String,
    pub message: String,
}

impl RecipeError {
    pub(crate) fn new(field: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            field: field.into(),
            message: message.into(),
        }
    }
}

impl fmt::Display for RecipeError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "{}: {}", self.field, self.message)
    }
}

impl std::error::Error for RecipeError {}
