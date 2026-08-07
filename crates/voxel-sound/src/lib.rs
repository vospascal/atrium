//! Stable sound-selection contracts for the voxel game.
//!
//! Asset paths stay private to this crate. Callers choose a semantic movement
//! cue, so reorganising or extending the supplied catalog cannot leak through
//! the application boundary.

mod movement;

pub use movement::{MovementCue, MovementSounds};
