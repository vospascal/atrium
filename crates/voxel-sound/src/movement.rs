//! The current generated island is grass-topped, so it starts with grass walk
//! samples. `build.rs` validates `assets/sounds.json` and generates the private
//! embedded asset table. A later ground-material resolver can add more
//! selections here without making the application name individual asset files.

/// A local-player movement sound requested by gameplay.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MovementCue {
    Footstep,
    Jump,
    Landing,
}

/// Deterministic sample selection for local-player movement.
///
/// The round-robin sequence avoids a repeated-sample click without requiring
/// runtime randomness or a mutable global audio catalog.
#[derive(Debug, Default)]
pub struct MovementSounds {
    next_footstep: usize,
    next_jump: usize,
    next_landing: usize,
}

impl MovementSounds {
    /// Return the next bundled sample for one semantic movement cue.
    pub fn next_sample(&mut self, cue: MovementCue) -> &'static [u8] {
        match cue {
            MovementCue::Footstep => next(&DEFAULT_FOOTSTEP, &mut self.next_footstep),
            MovementCue::Jump => next(&DEFAULT_JUMP, &mut self.next_jump),
            MovementCue::Landing => next(&DEFAULT_LANDING, &mut self.next_landing),
        }
    }
}

fn next(samples: &[&'static [u8]], index: &mut usize) -> &'static [u8] {
    let sample = samples[*index % samples.len()];
    *index = index.wrapping_add(1);
    sample
}

include!(concat!(env!("OUT_DIR"), "/movement_assets.rs"));

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_movement_cue_resolves_to_a_bundled_sound() {
        let mut sounds = MovementSounds::default();
        for cue in [
            MovementCue::Footstep,
            MovementCue::Jump,
            MovementCue::Landing,
        ] {
            assert!(!sounds.next_sample(cue).is_empty());
        }
    }

    #[test]
    fn each_sample_set_rotates_before_repeating() {
        let mut sounds = MovementSounds::default();
        let first = sounds.next_sample(MovementCue::Footstep).as_ptr();
        let second = sounds.next_sample(MovementCue::Footstep).as_ptr();
        assert_ne!(first, second);
    }
}
