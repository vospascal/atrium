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

    /// A beam of any width with some sound behind it, the way a sound's
    /// shape is set: `gain = behind + (1 − behind) · ((1 + cos θ) / 2)^k`.
    /// `k` puts the point where the gain is halfway between front and back
    /// at `half_width`. A half width of π is omni; π/2 with nothing behind is
    /// exactly a cardioid. Build it with [`DirectivityPattern::beam`].
    Beam {
        /// Half the beam's width (radians).
        half_width: f32,
        /// Gain straight behind, 0–1.
        behind: f32,
        /// The exponent that gives `half_width`.
        sharpness: f32,
    },
}

/// A level this far below the front, or lower, is silence behind (dB).
pub const SILENT_BEHIND_DB: f32 = -60.0;

impl DirectivityPattern {
    pub const OMNI: DirectivityPattern = DirectivityPattern::Omni;

    pub fn cardioid() -> Self {
        DirectivityPattern::Polar { alpha: 0.5 }
    }

    pub fn supercardioid() -> Self {
        DirectivityPattern::Polar { alpha: 0.37 }
    }

    /// A beam `width_degrees` wide, from a few degrees up to 360 (all round),
    /// with the sound straight behind it `behind_db` below the front: 0 dB
    /// as loud, [`SILENT_BEHIND_DB`] or lower silent. The width is where the
    /// level is halfway between front and back.
    pub fn beam(width_degrees: f32, behind_db: f32) -> Self {
        let half_width = (width_degrees.clamp(1.0, 360.0) / 2.0).to_radians();
        let behind = if behind_db <= SILENT_BEHIND_DB {
            0.0
        } else {
            10f32.powf(behind_db.min(0.0) / 20.0)
        };
        // ((1 + cos w) / 2)^k = 1/2 at the half width w.
        let at_edge = (1.0 + half_width.cos()) / 2.0;
        let sharpness = if at_edge <= 1e-6 {
            0.0
        } else {
            0.5f32.ln() / at_edge.ln()
        };
        DirectivityPattern::Beam {
            half_width,
            behind,
            sharpness,
        }
    }

    /// Return the polar alpha coefficient (1.0 for omni, 0.5 for cardioid, etc.).
    /// Used for serialization to the browser UI.
    pub fn alpha(&self) -> f32 {
        match self {
            DirectivityPattern::Omni => 1.0,
            DirectivityPattern::Polar { alpha } => *alpha,
            DirectivityPattern::Cone { .. } => 1.0,
            // A polar pattern's alpha is its gain at 90°.
            DirectivityPattern::Beam { .. } => self.gain_at_angle(std::f32::consts::FRAC_PI_2),
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
            DirectivityPattern::Beam {
                behind, sharpness, ..
            } => {
                let lobe = ((1.0 + angle.cos()) / 2.0).max(0.0).powf(*sharpness);
                behind + (1.0 - behind) * lobe
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

    /// A beam is as loud as the front ahead, halfway between front and back
    /// at the edge of its width, and as loud as `behind` straight behind.
    #[test]
    fn a_beam_has_the_width_and_the_level_behind_it_was_given() {
        for (width, behind_db) in [(60.0, -60.0), (120.0, -12.0), (200.0, -6.0), (300.0, -20.0)] {
            let beam = DirectivityPattern::beam(width, behind_db);
            let behind = if behind_db <= SILENT_BEHIND_DB {
                0.0
            } else {
                10f32.powf(behind_db / 20.0)
            };
            assert!(
                (beam.gain_at_angle(0.0) - 1.0).abs() < 1e-5,
                "{width}°: front"
            );
            assert!(
                (beam.gain_at_angle(PI) - behind).abs() < 1e-4,
                "{width}°: behind"
            );
            let edge = beam.gain_at_angle((width / 2.0_f32).to_radians());
            assert!(
                (edge - (1.0 + behind) / 2.0).abs() < 1e-3,
                "{width}°: halfway at the edge, got {edge}"
            );
        }
    }

    /// The named shapes are points on the same range: all round is omni, and
    /// 180° with nothing behind is exactly a cardioid.
    #[test]
    fn omni_and_cardioid_are_beams_too() {
        let all_round = DirectivityPattern::beam(360.0, -60.0);
        for angle in [0.0, FRAC_PI_4, FRAC_PI_2, PI] {
            assert!((all_round.gain_at_angle(angle) - 1.0).abs() < 1e-6);
        }
        let half = DirectivityPattern::beam(180.0, -60.0);
        let cardioid = DirectivityPattern::cardioid();
        for angle in [0.0, 0.5, FRAC_PI_4, FRAC_PI_2, 2.0, PI] {
            assert!((half.gain_at_angle(angle) - cardioid.gain_at_angle(angle)).abs() < 1e-5);
        }
        assert!((directivity_factor(&half) - 3.0).abs() < 0.05);
        assert!((half.alpha() - 0.5).abs() < 1e-5);
    }

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

    // ── Directivity factor against closed forms and the sphere ──────────

    /// Closed form for g = α + (1 − α)·cos θ, with u = cos θ:
    ///   Q = 2 / ∫₋₁¹ g² du.
    /// For α ≥ 0.5 there is no rear lobe: ∫ = 2α² + 2(1 − α)²/3, so
    ///   Q = 1 / (α² + (1 − α)²/3)   (omni 1, cardioid 3).
    /// For α < 0.5 `gain_at_angle` clamps the negative rear lobe to 0, so the
    /// integral only runs from u₀ = −α/(1 − α): ∫ = 1/(3(1 − α)), Q = 6(1 − α).
    fn clamped_polar_directivity_factor(alpha: f32) -> f32 {
        if alpha >= 0.5 {
            1.0 / (alpha * alpha + (1.0 - alpha) * (1.0 - alpha) / 3.0)
        } else {
            6.0 * (1.0 - alpha)
        }
    }

    /// Simpson's rule must reproduce the closed form across the polar family.
    /// Pinned: omni 1, cardioid 3 (DI 4.77 dB), α = 0.7 → 1.923,
    /// supercardioid (α = 0.37) → 3.78, α = 0.25 → 4.5, α = 0 → 6.
    #[test]
    fn directivity_factor_matches_closed_form() {
        for (alpha, expected) in [
            (1.0f32, 1.0f32),
            (0.7, 1.923),
            (0.5, 3.0),
            (0.37, 3.78),
            (0.25, 4.5),
            (0.0, 6.0),
        ] {
            let closed_form = clamped_polar_directivity_factor(alpha);
            assert!(
                (closed_form - expected).abs() < 1e-3,
                "{alpha}: {closed_form}"
            );
            let simpson = directivity_factor(&DirectivityPattern::Polar { alpha });
            assert!(
                (simpson - expected).abs() / expected < 2e-3,
                "α = {alpha}: Simpson {simpson}, closed form {expected}"
            );
        }
        let cardioid_index_db = 10.0 * directivity_factor(&DirectivityPattern::cardioid()).log10();
        assert!(
            (cardioid_index_db - 4.771).abs() < 0.01,
            "{cardioid_index_db}"
        );
    }

    /// Q = 4π / ∮ g² dΩ, integrated over the whole sphere with the 3-D
    /// `directivity_gain` (midpoint rule, 360 × 720 cells) and an arbitrary
    /// tilted facing. Agreeing with the 1-D `directivity_factor` checks both
    /// the axisymmetric reduction and the angle convention of the 3-D path.
    #[test]
    fn directivity_factor_matches_sphere_integration() {
        let facing = Vec3::new(1.0, 2.0, -0.5).normalize();
        let patterns = [
            DirectivityPattern::cardioid(),
            DirectivityPattern::supercardioid(),
            DirectivityPattern::Polar { alpha: 0.25 },
            DirectivityPattern::Cone {
                inner: FRAC_PI_4,
                outer: FRAC_PI_2,
                outer_gain: 0.2,
            },
        ];
        const POLAR_STEPS: usize = 360;
        const AZIMUTH_STEPS: usize = 720;
        let polar_step = PI / POLAR_STEPS as f32;
        let azimuth_step = 2.0 * PI / AZIMUTH_STEPS as f32;
        for pattern in patterns {
            let mut energy = 0.0f64;
            for polar_index in 0..POLAR_STEPS {
                let polar = (polar_index as f32 + 0.5) * polar_step;
                for azimuth_index in 0..AZIMUTH_STEPS {
                    let azimuth = (azimuth_index as f32 + 0.5) * azimuth_step;
                    let direction = Vec3::new(
                        polar.sin() * azimuth.cos(),
                        polar.sin() * azimuth.sin(),
                        polar.cos(),
                    );
                    let gain = directivity_gain(Vec3::ZERO, facing, direction, &pattern);
                    energy += (gain * gain * polar.sin() * polar_step * azimuth_step) as f64;
                }
            }
            let sphere_factor = (4.0 * std::f64::consts::PI / energy) as f32;
            let simpson_factor = directivity_factor(&pattern);
            assert!(
                (sphere_factor - simpson_factor).abs() / simpson_factor < 5e-3,
                "{pattern:?}: sphere {sphere_factor}, Simpson {simpson_factor}"
            );
        }
    }

    /// The clamp makes the engine's α < 0.5 patterns differ from the textbook
    /// polar patterns, whose rear lobe radiates with inverted polarity:
    /// textbook supercardioid (α = 0.37) Q = 1/(0.37² + 0.63²/3) = 3.715,
    /// hypercardioid (α = 0.25) Q = 4.0, figure-8 (α = 0) Q = 3.0.
    /// Pinned so a change to the clamp is a conscious decision.
    #[test]
    fn rear_lobe_clamp_departs_from_textbook_factor() {
        let supercardioid = directivity_factor(&DirectivityPattern::supercardioid());
        assert!(supercardioid > 3.715 && (supercardioid - 3.78).abs() < 0.01);
        let hypercardioid = directivity_factor(&DirectivityPattern::Polar { alpha: 0.25 });
        assert!((hypercardioid - 4.5).abs() < 0.01, "{hypercardioid}");
        // Hypercardioid at θ = π: 0.25 − 0.75 = −0.5 before the clamp.
        assert_eq!(
            DirectivityPattern::Polar { alpha: 0.25 }.gain_at_angle(PI),
            0.0
        );
    }

    /// Cone with outer_gain 0 and inner = outer = π/2 is a hemisphere: Q = 2
    /// (half the sphere at unit gain).
    #[test]
    fn hemisphere_cone_has_factor_two() {
        let hemisphere = DirectivityPattern::Cone {
            inner: FRAC_PI_2,
            outer: FRAC_PI_2,
            outer_gain: 0.0,
        };
        let factor = directivity_factor(&hemisphere);
        assert!((factor - 2.0).abs() < 0.05, "{factor}");
    }

    /// Gain depends only on the angle to the facing axis: angles are symmetric
    /// (±θ give the same gain) and rotating the target about the facing axis
    /// changes nothing. Cardioid at 60° = 0.5 + 0.5·cos 60° = 0.75.
    #[test]
    fn directivity_gain_is_axisymmetric() {
        let pattern = DirectivityPattern::cardioid();
        assert_eq!(pattern.gain_at_angle(1.0), pattern.gain_at_angle(-1.0));
        let facing = Vec3::new(1.0, 0.0, 0.0);
        let sixty_degrees = std::f32::consts::FRAC_PI_3;
        for roll in [0.0f32, 1.0, 2.5, 4.0] {
            let target = Vec3::new(
                sixty_degrees.cos(),
                sixty_degrees.sin() * roll.cos(),
                sixty_degrees.sin() * roll.sin(),
            ) * 3.0;
            let gain = directivity_gain(Vec3::ZERO, facing, target, &pattern);
            assert!((gain - 0.75).abs() < 1e-5, "roll {roll}: {gain}");
        }
    }

    /// Target coincident with the entity is degenerate: unity, never NaN.
    #[test]
    fn directivity_gain_at_zero_distance_is_unity() {
        let gain = directivity_gain(
            Vec3::new(1.0, 1.0, 1.0),
            Vec3::new(1.0, 0.0, 0.0),
            Vec3::new(1.0, 1.0, 1.0),
            &DirectivityPattern::cardioid(),
        );
        assert_eq!(gain, 1.0);
    }
}
