//! Local player movement-sound playback.
//!
//! The renderer deliberately has no platform/audio dependency. This small
//! application-layer module owns the native output stream and is fed only the
//! distance the grounded character actually travelled after collision resolve.
//! That keeps flying, jumping into a wall, and free-fall silent.

use std::io::{BufReader, Cursor};

use rodio::{Decoder, OutputStream, OutputStreamHandle, Sink};
use voxel_sound::{MovementCue, MovementSounds};

/// Metres travelled between footfalls at the normal walking pace.
const FOOTSTEP_INTERVAL_METERS: f32 = 1.85;
/// Do not turn an unusually long frame into an audible machine-gun burst.
const MAX_FOOTSTEPS_PER_FRAME: u8 = 2;
/// The supplied recording is mastered louder than the rest of the sparse game
/// mix, so leave headroom for future ambience and effects.
const FOOTSTEP_VOLUME: f32 = 0.35;
const JUMP_VOLUME: f32 = 0.45;
const LANDING_VOLUME: f32 = 0.45;

/// Sound-worthy changes in the grounded state, decided independently of audio
/// output so collision semantics remain testable without a sound device.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(crate) enum GroundTransition {
    Jump,
    Landing,
}

/// Decide whether one character simulation step has produced a jump or landing
/// sound. Walking off an edge is intentionally silent; it is a jump only when
/// upward input caused a grounded body to leave the ground.
pub(crate) fn ground_transition_sound(
    was_grounded: bool,
    is_grounded: bool,
    jump_pressed: bool,
) -> Option<GroundTransition> {
    if was_grounded && !is_grounded && jump_pressed {
        Some(GroundTransition::Jump)
    } else if !was_grounded && is_grounded {
        Some(GroundTransition::Landing)
    } else {
        None
    }
}

/// Pure distance-based cadence. Keeping timing policy separate from playback
/// makes the movement-to-sound rule testable without an audio device.
#[derive(Debug, Default)]
struct FootstepCadence {
    distance_since_footstep: f32,
    was_moving: bool,
}

impl FootstepCadence {
    fn reset(&mut self) {
        *self = Self::default();
    }

    /// Report how many footfalls movement in this frame should produce.
    fn advance(&mut self, horizontal_distance: f32, grounded: bool) -> u8 {
        if !grounded || !horizontal_distance.is_finite() || horizontal_distance <= 0.0 {
            self.was_moving = false;
            return 0;
        }

        // A walk feels responsive only if its first footfall is immediate; every
        // following one is paced by actual movement, not keyboard repeat rate.
        if !self.was_moving {
            self.was_moving = true;
            self.distance_since_footstep = 0.0;
            return 1;
        }

        self.distance_since_footstep += horizontal_distance;
        let count = (self.distance_since_footstep / FOOTSTEP_INTERVAL_METERS).floor() as u8;
        let count = count.min(MAX_FOOTSTEPS_PER_FRAME);
        self.distance_since_footstep -= count as f32 * FOOTSTEP_INTERVAL_METERS;
        count
    }
}

/// Best-effort native playback for player movement sounds.
///
/// No sound device is not a reason the renderer cannot start (CI, remote
/// desktops, and headless sessions are all valid), so setup and each footfall
/// fail closed after a single diagnostic rather than panicking.
pub(crate) struct FootstepAudio {
    _stream: Option<OutputStream>,
    handle: Option<OutputStreamHandle>,
    cadence: FootstepCadence,
    movement_sounds: MovementSounds,
}

impl FootstepAudio {
    pub(crate) fn new() -> Self {
        match OutputStream::try_default() {
            Ok((stream, handle)) => Self {
                _stream: Some(stream),
                handle: Some(handle),
                cadence: FootstepCadence::default(),
                movement_sounds: MovementSounds::default(),
            },
            Err(error) => {
                eprintln!("movement sounds disabled: could not open audio output: {error}");
                Self {
                    _stream: None,
                    handle: None,
                    cadence: FootstepCadence::default(),
                    movement_sounds: MovementSounds::default(),
                }
            }
        }
    }

    pub(crate) fn reset(&mut self) {
        self.cadence.reset();
    }

    /// Feed post-collision movement, in world metres.
    pub(crate) fn update(&mut self, horizontal_distance: f32, grounded: bool) {
        let footfalls = self.cadence.advance(horizontal_distance, grounded);
        for _ in 0..footfalls {
            self.play(MovementCue::Footstep);
        }
    }

    /// Play an already-resolved grounded-state transition.
    pub(crate) fn play_ground_transition(&mut self, transition: GroundTransition) {
        self.play(match transition {
            GroundTransition::Jump => MovementCue::Jump,
            GroundTransition::Landing => MovementCue::Landing,
        });
    }

    fn play(&mut self, sound: MovementCue) {
        let bytes = self.movement_sounds.next_sample(sound);
        let Some(handle) = self.handle.as_ref() else {
            return;
        };

        let decoder = match Decoder::new(BufReader::new(Cursor::new(bytes))) {
            Ok(decoder) => decoder,
            Err(error) => {
                eprintln!("movement sounds disabled: could not decode bundled sound: {error}");
                self.disable();
                return;
            }
        };
        match Sink::try_new(handle) {
            Ok(sink) => {
                sink.set_volume(match sound {
                    MovementCue::Footstep => FOOTSTEP_VOLUME,
                    MovementCue::Jump => JUMP_VOLUME,
                    MovementCue::Landing => LANDING_VOLUME,
                });
                sink.append(decoder);
                // Each footfall has its own sink so samples may overlap naturally.
                sink.detach();
            }
            Err(error) => {
                eprintln!("movement sounds disabled: audio output was lost: {error}");
                self.disable();
            }
        }
    }

    fn disable(&mut self) {
        self.handle = None;
        self._stream = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn first_grounded_movement_has_an_immediate_footfall() {
        let mut cadence = FootstepCadence::default();
        assert_eq!(cadence.advance(0.01, true), 1);
    }

    #[test]
    fn cadence_is_driven_by_distance_after_the_first_footfall() {
        let mut cadence = FootstepCadence::default();
        assert_eq!(cadence.advance(0.1, true), 1);
        assert_eq!(cadence.advance(FOOTSTEP_INTERVAL_METERS * 0.99, true), 0);
        assert_eq!(cadence.advance(FOOTSTEP_INTERVAL_METERS * 0.01, true), 1);
    }

    #[test]
    fn airborne_and_stationary_frames_are_silent() {
        let mut cadence = FootstepCadence::default();
        assert_eq!(cadence.advance(1.0, false), 0);
        assert_eq!(cadence.advance(0.0, true), 0);
    }

    #[test]
    fn ground_transitions_distinguish_jumps_from_walk_offs_and_landings() {
        assert_eq!(
            ground_transition_sound(true, false, true),
            Some(GroundTransition::Jump)
        );
        assert_eq!(ground_transition_sound(true, false, false), None);
        assert_eq!(
            ground_transition_sound(false, true, false),
            Some(GroundTransition::Landing)
        );
        assert_eq!(ground_transition_sound(true, true, true), None);
    }
}
