use crate::directivity::DirectivityPattern;
use crate::types::Vec3;

/// Trait for anything that generates audio samples and has a position.
/// Implementations must be Send (moved to audio thread) and must not allocate.
pub trait SoundSource: Send {
    /// Generate the next mono sample.
    fn next_sample(&mut self, sample_rate: f32) -> f32;

    /// Current world-space position.
    fn position(&self) -> Vec3;

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
