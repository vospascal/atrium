// Directivity patterns for sources and listeners.
//
// Both source emission and listener reception are directional. A directivity
// pattern maps an angle (from the entity's forward direction) to a gain (0–1).
// The same math applies to both ends:
//
//   perceived_energy = source_directivity(emission_angle)
//                    × path_transfer(distance, reflections, occlusion)
//                    × receiver_directivity(arrival_angle)
//
// See docs/directivity-and-energy-transfer.md for the full derivation.

use crate::types::Vec3;

/// A directivity pattern: a function from angle to gain.
#[derive(Clone, Copy, Debug)]
pub enum DirectivityPattern {
    /// Gain = 1.0 everywhere. Campfire, ambient, subwoofer.
    Omni,

    /// Three-parameter cone (WebAudio PannerNode semantics).
    /// `inner` and `outer` are half-angles in radians.
    /// Full gain inside inner, linear interpolation to outer_gain, floor beyond outer.
    Cone {
        inner: f32,
        outer: f32,
        outer_gain: f32,
    },

    /// Polar pattern: gain = alpha + (1 - alpha) * cos(θ).
    /// alpha=1.0 → omni, 0.5 → cardioid, 0.37 → supercardioid, 0.25 → hypercardioid.
    Polar { alpha: f32 },
}

impl DirectivityPattern {
    pub const OMNI: DirectivityPattern = DirectivityPattern::Omni;

    pub fn cardioid() -> Self {
        DirectivityPattern::Polar { alpha: 0.5 }
    }

    pub fn supercardioid() -> Self {
        DirectivityPattern::Polar { alpha: 0.37 }
    }

    /// Evaluate the pattern at a given angle (radians, 0 = forward, π = behind).
    pub fn gain_at_angle(&self, angle: f32) -> f32 {
        let angle = angle.abs();
        match self {
            DirectivityPattern::Omni => 1.0,
            DirectivityPattern::Cone {
                inner,
                outer,
                outer_gain,
            } => {
                if angle <= *inner {
                    1.0
                } else if angle >= *outer {
                    *outer_gain
                } else {
                    let t = (angle - inner) / (outer - inner);
                    1.0 + t * (outer_gain - 1.0)
                }
            }
            DirectivityPattern::Polar { alpha } => {
                // gain = alpha + (1 - alpha) * cos(theta)
                // Clamp to 0 — hypercardioid can go slightly negative at rear
                (alpha + (1.0 - alpha) * angle.cos()).max(0.0)
            }
        }
    }
}

/// Compute directivity gain from an entity (source or listener) toward a target point.
///
/// Works identically for source emission and listener reception:
/// - For sources: entity_pos = source position, entity_facing = source forward, target = listener
/// - For listeners: entity_pos = listener position, entity_facing = listener forward, target = source
pub fn directivity_gain(
    entity_pos: Vec3,
    entity_facing: Vec3,
    target_pos: Vec3,
    pattern: &DirectivityPattern,
) -> f32 {
    if matches!(pattern, DirectivityPattern::Omni) {
        return 1.0;
    }

    let to_target = target_pos - entity_pos;
    if to_target.length() < 1e-6 {
        return 1.0; // target at entity position — degenerate
    }
    let to_target = to_target.normalize();
    let cos_angle = entity_facing.normalize().dot(to_target).clamp(-1.0, 1.0);
    let angle = cos_angle.acos();
    pattern.gain_at_angle(angle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    #[test]
    fn omni_is_unity_everywhere() {
        let p = DirectivityPattern::Omni;
        assert_eq!(p.gain_at_angle(0.0), 1.0);
        assert_eq!(p.gain_at_angle(FRAC_PI_2), 1.0);
        assert_eq!(p.gain_at_angle(PI), 1.0);
    }

    #[test]
    fn cone_inside_inner_is_unity() {
        let p = DirectivityPattern::Cone {
            inner: FRAC_PI_4,
            outer: FRAC_PI_2,
            outer_gain: 0.3,
        };
        assert_eq!(p.gain_at_angle(0.0), 1.0);
        assert_eq!(p.gain_at_angle(FRAC_PI_4 * 0.5), 1.0);
    }

    #[test]
    fn cone_outside_outer_is_floor() {
        let p = DirectivityPattern::Cone {
            inner: FRAC_PI_4,
            outer: FRAC_PI_2,
            outer_gain: 0.3,
        };
        assert!((p.gain_at_angle(PI) - 0.3).abs() < 1e-6);
        assert!((p.gain_at_angle(FRAC_PI_2 + 0.1) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn cone_transition_interpolates() {
        let p = DirectivityPattern::Cone {
            inner: FRAC_PI_4,
            outer: FRAC_PI_2,
            outer_gain: 0.0,
        };
        // Midpoint of transition zone: (pi/4 + pi/2) / 2 = 3pi/8
        let mid = (FRAC_PI_4 + FRAC_PI_2) / 2.0;
        let gain = p.gain_at_angle(mid);
        assert!((gain - 0.5).abs() < 1e-5, "expected ~0.5, got {}", gain);
    }

    #[test]
    fn polar_cardioid_front_is_one() {
        let p = DirectivityPattern::cardioid();
        // At 0 degrees: 0.5 + 0.5 * cos(0) = 0.5 + 0.5 = 1.0
        assert!((p.gain_at_angle(0.0) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn polar_cardioid_back_is_zero() {
        let p = DirectivityPattern::cardioid();
        // At π: 0.5 + 0.5 * cos(π) = 0.5 - 0.5 = 0.0
        assert!(p.gain_at_angle(PI).abs() < 1e-6);
    }

    #[test]
    fn polar_cardioid_side_is_half() {
        let p = DirectivityPattern::cardioid();
        // At π/2: 0.5 + 0.5 * cos(π/2) = 0.5 + 0 = 0.5
        assert!((p.gain_at_angle(FRAC_PI_2) - 0.5).abs() < 1e-6);
    }

    #[test]
    fn directivity_gain_source_facing_target() {
        let source_pos = Vec3::new(0.0, 0.0, 0.0);
        let source_facing = Vec3::new(1.0, 0.0, 0.0);
        let target = Vec3::new(5.0, 0.0, 0.0);
        let p = DirectivityPattern::cardioid();
        let g = directivity_gain(source_pos, source_facing, target, &p);
        assert!((g - 1.0).abs() < 1e-5, "expected 1.0, got {}", g);
    }

    #[test]
    fn directivity_gain_source_facing_away() {
        let source_pos = Vec3::new(0.0, 0.0, 0.0);
        let source_facing = Vec3::new(-1.0, 0.0, 0.0); // facing away from target
        let target = Vec3::new(5.0, 0.0, 0.0);
        let p = DirectivityPattern::cardioid();
        let g = directivity_gain(source_pos, source_facing, target, &p);
        assert!(g < 0.01, "expected ~0, got {}", g);
    }

    #[test]
    fn directivity_gain_omni_shortcircuits() {
        let g = directivity_gain(
            Vec3::ZERO,
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(0.0, 5.0, 0.0),
            &DirectivityPattern::Omni,
        );
        assert_eq!(g, 1.0);
    }
}
