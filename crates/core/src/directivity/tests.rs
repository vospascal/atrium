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
