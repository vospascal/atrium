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

    /// Return the polar alpha coefficient (1.0 for omni, 0.5 for cardioid, etc.).
    /// Used for serialization to the browser UI.
    pub fn alpha(&self) -> f32 {
        match self {
            DirectivityPattern::Omni => 1.0,
            DirectivityPattern::Polar { alpha } => *alpha,
            DirectivityPattern::Cone { .. } => 1.0,
        }
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

/// Compute the directivity factor γ for a radiation pattern.
///
/// γ measures how focused the pattern is relative to omnidirectional:
///   γ = 2 / ∫₀^π g(θ)² sin(θ) dθ
///
/// where g(θ) is the pattern's gain at angle θ. The integral is evaluated
/// using composite Simpson's rule with 64 steps (sufficient for smooth patterns).
///
/// Known values:
///   - Omni → 1.0
///   - Cardioid (α=0.5) → 3.0
///   - Supercardioid (α=0.37) → ~3.7
///   - Hypercardioid (α=0.25) → ~4.0
///
/// Used in the critical distance formula: d_c = 0.057 × √(γ × V / RT60).
/// A higher γ means the source's direct sound dominates further from the source.
pub fn directivity_factor(pattern: &DirectivityPattern) -> f32 {
    if matches!(pattern, DirectivityPattern::Omni) {
        return 1.0;
    }

    // Composite Simpson's rule: ∫₀^π g(θ)² sin(θ) dθ
    const N: usize = 64; // must be even
    let h = std::f32::consts::PI / N as f32;
    let mut sum = 0.0f32;

    for i in 0..=N {
        let theta = i as f32 * h;
        let g = pattern.gain_at_angle(theta);
        let y = g * g * theta.sin();

        let weight = if i == 0 || i == N {
            1.0
        } else if i % 2 == 1 {
            4.0
        } else {
            2.0
        };
        sum += weight * y;
    }
    let integral = sum * h / 3.0;

    if integral < 1e-6 {
        return 1.0; // degenerate pattern — fallback to omni
    }
    2.0 / integral
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

    // ── Directivity factor tests ────────────────────────────────────────

    #[test]
    fn directivity_factor_omni_is_one() {
        let gamma = directivity_factor(&DirectivityPattern::Omni);
        assert!(
            (gamma - 1.0).abs() < 1e-4,
            "omni γ should be 1.0, got {gamma}"
        );
    }

    #[test]
    fn directivity_factor_cardioid_is_three() {
        // Cardioid: g(θ) = 0.5 + 0.5·cos(θ), known γ = 3.0
        let gamma = directivity_factor(&DirectivityPattern::cardioid());
        assert!(
            (gamma - 3.0).abs() < 0.05,
            "cardioid γ should be ~3.0, got {gamma}"
        );
    }

    #[test]
    fn directivity_factor_supercardioid_higher_than_cardioid() {
        let gamma_card = directivity_factor(&DirectivityPattern::cardioid());
        let gamma_super = directivity_factor(&DirectivityPattern::supercardioid());
        assert!(
            gamma_super > gamma_card,
            "supercardioid γ ({gamma_super}) should exceed cardioid γ ({gamma_card})"
        );
    }

    #[test]
    fn directivity_factor_cone_pattern() {
        // Tight cone: full gain in front, 0.1 elsewhere → highly directional
        let tight = DirectivityPattern::Cone {
            inner: FRAC_PI_4,
            outer: FRAC_PI_2,
            outer_gain: 0.1,
        };
        let gamma = directivity_factor(&tight);
        assert!(gamma > 2.0, "tight cone should have γ > 2.0, got {gamma}");
    }

    #[test]
    fn critical_distance_scales_with_sqrt_gamma() {
        // d_c = 0.057 × √(γ × V / RT60)
        // For same room, d_c_cardioid / d_c_omni = √(γ_cardioid / γ_omni) = √3
        let gamma_omni = directivity_factor(&DirectivityPattern::Omni);
        let gamma_card = directivity_factor(&DirectivityPattern::cardioid());
        let ratio = (gamma_card / gamma_omni).sqrt();
        let expected = 3.0_f32.sqrt(); // √3 ≈ 1.732
        assert!(
            (ratio - expected).abs() < 0.05,
            "d_c ratio should be √3 ≈ {expected:.3}, got {ratio:.3}"
        );
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
