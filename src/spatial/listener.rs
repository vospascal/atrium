use crate::spatial::directivity::DirectivityPattern;
use crate::world::types::Vec3;

/// Hearing cone for the listener (models forward-facing ears).
/// Uses the same DirectivityPattern as sources but applied to reception.
#[derive(Clone, Copy, Debug)]
pub struct HearingCone {
    pub pattern: DirectivityPattern,
}

impl Default for HearingCone {
    fn default() -> Self {
        // Matches the TypeScript spatial project (DirectionalEmissionProcessor):
        //   coneInnerAngle:  30°  → 15° half-angle
        //   coneOuterAngle:  90°  → 45° half-angle
        //   coneOuterGain:   0.3
        Self {
            pattern: DirectivityPattern::Cone {
                inner: 15.0_f32.to_radians(),
                outer: 45.0_f32.to_radians(),
                outer_gain: 0.3,
            },
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct Listener {
    pub position: Vec3,
    /// Yaw in radians. 0 = facing +X, π/2 = facing +Y.
    pub yaw: f32,
    pub hearing_cone: HearingCone,
}

impl Listener {
    pub fn new(position: Vec3, yaw: f32) -> Self {
        Self {
            position,
            yaw,
            hearing_cone: HearingCone::default(),
        }
    }

    /// Unit vector of the listener's forward direction (derived from yaw).
    pub fn forward(&self) -> Vec3 {
        Vec3::new(self.yaw.cos(), self.yaw.sin(), 0.0)
    }

    /// Compute the hearing cone gain for a source at the given position.
    pub fn hearing_gain(&self, source_pos: Vec3) -> f32 {
        let to_source = source_pos - self.position;
        if to_source.length() < 1e-6 {
            return 1.0;
        }
        let to_source = to_source.normalize();
        let forward = self.forward();
        let cos_angle = forward.dot(to_source).clamp(-1.0, 1.0);
        let angle = cos_angle.acos();
        self.hearing_cone.pattern.gain_at_angle(angle)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn forward_at_zero_yaw_is_plus_x() {
        let l = Listener::new(Vec3::ZERO, 0.0);
        let f = l.forward();
        assert!((f.x - 1.0).abs() < 1e-6);
        assert!(f.y.abs() < 1e-6);
    }

    #[test]
    fn forward_at_90_degrees_is_plus_y() {
        let l = Listener::new(Vec3::ZERO, FRAC_PI_2);
        let f = l.forward();
        assert!(f.x.abs() < 1e-5);
        assert!((f.y - 1.0).abs() < 1e-5);
    }

    #[test]
    fn hearing_gain_forward_is_full() {
        let l = Listener::new(Vec3::ZERO, 0.0); // facing +X
        let source_ahead = Vec3::new(5.0, 0.0, 0.0);
        let g = l.hearing_gain(source_ahead);
        assert!((g - 1.0).abs() < 1e-5, "expected 1.0, got {}", g);
    }

    #[test]
    fn hearing_gain_behind_is_attenuated() {
        let l = Listener::new(Vec3::ZERO, 0.0); // facing +X
        let source_behind = Vec3::new(-5.0, 0.0, 0.0);
        let g = l.hearing_gain(source_behind);
        // Default hearing cone outer_gain = 0.3
        assert!((g - 0.3).abs() < 1e-5, "expected 0.3, got {}", g);
    }

    #[test]
    fn hearing_gain_at_side_is_intermediate() {
        let l = Listener::new(Vec3::ZERO, 0.0); // facing +X
        let source_side = Vec3::new(0.0, 5.0, 0.0); // 90° off forward
        let g = l.hearing_gain(source_side);
        // 90° (π/2) is beyond the 45° half-angle outer cone → should be outer_gain
        assert!((g - 0.3).abs() < 1e-5, "expected 0.3, got {}", g);
    }
}
