use crate::directivity::DirectivityPattern;
use crate::telemetry::SourceSignal;
use crate::types::Vec3;

/// How a source occupies acoustic space.
///
/// This is deliberately separate from the signal generator. A bird sample and
/// a procedural aeolian whistle are both [`Object`](EmitterKind::Object)
/// emitters, while wind and steady rain beds are [`Field`](EmitterKind::Field)
/// emitters regardless of how their samples are produced.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum EmitterKind {
    /// Localized radiator: distance, direction, propagation paths, reflections,
    /// and room reverb all apply.
    #[default]
    Object,
    /// Environmental sound field: level is independent of a single source
    /// position and the signal bypasses point-source propagation and room send.
    Field,
}

impl EmitterKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Object => "object",
            Self::Field => "field",
        }
    }
}

/// One take to utter at a set moment: what a plan asks of a voice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Utterance {
    /// Which take, counted from 0.
    pub take: u8,
    /// When, in output samples on the session's clock. A moment already past
    /// is kept at once.
    pub at_sample: u64,
    /// How much louder or softer than the take itself (linear).
    pub gain: f32,
    /// How much faster or slower, and so higher or lower (1 is as recorded).
    pub rate: f32,
}

/// Trait for anything that generates audio samples and has a position.
/// Implementations must be Send (moved to audio thread) and must not allocate.
pub trait SoundSource: Send {
    /// Generate the next mono sample.
    fn next_sample(&mut self, sample_rate: f32) -> f32;

    /// Latest short-time levels, if this source meters its rendered signal.
    fn signal(&self) -> Option<SourceSignal> {
        None
    }

    /// Which of its takes the source is uttering, counted from 0, or `None`
    /// while it rests between two. A source that never rests, such as a
    /// loop or a generator, is always on take 0.
    fn sounding_take(&self) -> Option<u8> {
        Some(0)
    }

    /// Current world-space position.
    fn position(&self) -> Vec3;

    /// Displacement from the source's anchor caused by its own motion.
    ///
    /// A generator that flies by itself (the wisp) reports where it currently is
    /// relative to where it was placed; the scene wrapper adds this to the
    /// anchor position. Stationary sources stay at zero.
    fn motion_offset(&self) -> Vec3 {
        Vec3::ZERO
    }
    /// Spatial interpretation of this source. Existing sources remain point-like
    /// objects unless they explicitly opt into field rendering.
    fn emitter_kind(&self) -> EmitterKind {
        EmitterKind::Object
    }

    /// Whether this source is still producing audio.
    fn is_active(&self) -> bool {
        true
    }

    /// Advance time-varying state (orbit, etc.). Called once per buffer.
    fn tick(&mut self, dt: f32);

    /// Unit vector of the source's forward/facing direction.
    /// Default: +X (irrelevant for omnidirectional sources).
    fn orientation(&self) -> Vec3 {
        Vec3::new(1.0, 0.0, 0.0)
    }

    /// The source's directivity emission pattern. Default: omnidirectional.
    fn directivity(&self) -> DirectivityPattern {
        DirectivityPattern::OMNI
    }

    /// Whether this source is muted (silent but still ticking).
    fn is_muted(&self) -> bool {
        false
    }

    /// Mute or unmute this source.
    fn set_muted(&mut self, _muted: bool) {}

    /// Reposition this source. Interpretation depends on the source type
    /// (e.g. sets orbit center for orbiting sources, direct position for static).
    fn set_position(&mut self, _position: Vec3) {}

    /// Source spread for MDAP (0.0 = point source, 1.0 = full hemisphere).
    /// Controls how many phantom directions VBAP evaluates to widen the image.
    fn spread(&self) -> f32 {
        0.0
    }

    /// Set the source spread for MDAP.
    fn set_spread(&mut self, _spread: f32) {}

    /// Set the source's reference SPL (dB at 1 m), recomputing its base
    /// amplitude in place, and return the new base amplitude. Lets loudness
    /// change live without a scene rebuild (the amplitude is `k · 10^(spl/20)`
    /// for a per-source constant `k`, so no audio re-decode is needed).
    /// Returns 0.0 for sources that don't model SPL.
    fn set_reference_spl(&mut self, _spl: f32) -> f32 {
        0.0
    }

    /// Set the source's directivity emission pattern.
    fn set_directivity(&mut self, _pattern: DirectivityPattern) {}

    /// Turn the source to face `yaw` (radians, 0 = +X, π/2 = +Y). Default:
    /// no-op, for sources that have no facing to turn.
    fn set_facing(&mut self, _yaw: f32) {}

    /// The session's clock at the start of the next block, in output
    /// samples since the scene started. Called before every block, so a
    /// source added later counts from the same moment as the others.
    /// Default: no-op, for sources that keep no appointments.
    fn set_clock(&mut self, _sample: u64) {}

    /// Utter a take when the clock reaches `utterance.at_sample`. A plan made
    /// on another thread decides what and when; the source only keeps the
    /// appointment. Default: no-op, for sources that have no takes.
    fn schedule(&mut self, _utterance: Utterance) {}

    /// Set a procedural-synth parameter live. Sources that don't model the
    /// given parameter ignore it. Default: no-op (sample players etc.).
    fn set_synth_param(&mut self, _param: crate::commands::SynthParam, _value: f32) {}

    /// Set orbit speed (radians/sec). 0 = paused.
    fn set_orbit_speed(&mut self, _speed: f32) {}

    /// Set orbit radius (meters).
    fn set_orbit_radius(&mut self, _radius: f32) {}

    /// Set orbit angle (radians).
    fn set_orbit_angle(&mut self, _angle: f32) {}

    /// Orbit center position (for orbiting sources).
    fn orbit_center(&self) -> Vec3 {
        self.position()
    }

    /// Orbit radius (meters). 0 = stationary.
    fn orbit_radius(&self) -> f32 {
        0.0
    }

    /// Per-source reference distance (meters) at which gain = 1.0.
    /// Derived from SPL: louder sources project further.
    /// Default: 1.0 m (IEC 61672 standard measurement distance).
    fn ref_distance(&self) -> f32 {
        1.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    struct StillSource;

    impl SoundSource for StillSource {
        fn next_sample(&mut self, _sample_rate: f32) -> f32 {
            0.0
        }
        fn position(&self) -> Vec3 {
            Vec3::new(1.0, 2.0, 0.5)
        }
        fn tick(&mut self, _dt: f32) {}
    }

    /// The defaults every source inherits are physically neutral: a point
    /// object with a unit-length facing, omni radiation (Q = 1), the 1 m
    /// IEC 61672 reference distance, no spread, and no self-motion.
    #[test]
    fn default_source_is_neutral_point_object() {
        let source = StillSource;
        assert_eq!(source.emitter_kind(), EmitterKind::Object);
        assert!((source.orientation().length() - 1.0).abs() < 1e-6);
        assert_eq!(
            crate::directivity::directivity_factor(&source.directivity()),
            1.0
        );
        assert_eq!(source.ref_distance(), 1.0);
        assert_eq!(source.spread(), 0.0);
        assert_eq!(source.motion_offset().length(), 0.0);
        assert_eq!(source.orbit_center().distance_to(source.position()), 0.0);
    }
}
