//! B-format coefficients and the FOA encoder (SN3D/AmbiX, max-rE weighted).

/// Max-rE weight for FOA: cos(π / (2·order + 2)) where order = 1.
/// Narrows the panning function for better perceptual localization.
pub const MAX_RE_WEIGHT: f32 = std::f32::consts::FRAC_1_SQRT_2; // cos(45°) = 1/√2

/// SN3D normalization for zeroth-order (W channel): 1/√2.
pub(super) const W_NORM: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// FOA B-format coefficients (ACN ordering, SN3D normalization).
///
/// ACN 0 = W (omnidirectional pressure)
/// ACN 1 = Y (left-right, cos θ · sin φ)
/// ACN 2 = Z (up-down, sin θ)
/// ACN 3 = X (front-back, cos θ · cos φ)
#[derive(Clone, Copy, Debug)]
pub struct BFormat {
    pub w: f32,
    pub y: f32,
    pub z: f32,
    pub x: f32,
}

/// Encode a source at listener-relative azimuth and elevation (radians) with gain `g`.
///
/// Azimuth convention: 0 = front (+X), π/2 = left (+Y), π = rear (-X).
/// Elevation convention: 0 = horizon, π/2 = overhead, -π/2 = below.
/// Max-rE weighting is applied to Y, Z, and X channels.
pub fn foa_encode(azimuth: f32, elevation: f32, gain: f32) -> BFormat {
    let cos_el = elevation.cos();
    BFormat {
        w: gain * W_NORM,
        y: gain * cos_el * azimuth.sin() * MAX_RE_WEIGHT,
        z: gain * elevation.sin() * MAX_RE_WEIGHT,
        x: gain * cos_el * azimuth.cos() * MAX_RE_WEIGHT,
    }
}

#[cfg(test)]
mod tests {
    use super::super::test_support::assert_close;
    use super::*;
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    #[test]
    fn foa_encode_front() {
        let b = foa_encode(0.0, 0.0, 1.0);
        assert!((b.w - W_NORM).abs() < 1e-6, "W should be 1/√2");
        assert!(b.y.abs() < 1e-6, "Y should be 0 for front source");
        assert!(b.z.abs() < 1e-6, "Z should be 0 at horizon");
        assert!(
            (b.x - MAX_RE_WEIGHT).abs() < 1e-6,
            "X should be max-rE weight"
        );
    }

    #[test]
    fn foa_encode_left() {
        let b = foa_encode(FRAC_PI_2, 0.0, 1.0);
        assert!((b.w - W_NORM).abs() < 1e-6);
        assert!(
            (b.y - MAX_RE_WEIGHT).abs() < 1e-6,
            "Y should be max-rE for left"
        );
        assert!(b.z.abs() < 1e-6, "Z should be 0 at horizon");
        assert!(b.x.abs() < 1e-6, "X should be ~0 for left");
    }

    #[test]
    fn foa_encode_rear() {
        let b = foa_encode(PI, 0.0, 1.0);
        assert!((b.w - W_NORM).abs() < 1e-6);
        assert!(b.y.abs() < 1e-5, "Y should be ~0 for rear");
        assert!(b.z.abs() < 1e-6, "Z should be 0 at horizon");
        assert!(
            (b.x + MAX_RE_WEIGHT).abs() < 1e-6,
            "X should be -max-rE for rear"
        );
    }

    #[test]
    fn foa_encode_zero_gain() {
        let b = foa_encode(1.0, 0.5, 0.0);
        assert!(b.w.abs() < 1e-10);
        assert!(b.y.abs() < 1e-10);
        assert!(b.z.abs() < 1e-10);
        assert!(b.x.abs() < 1e-10);
    }

    #[test]
    fn foa_encode_overhead() {
        let b = foa_encode(0.0, FRAC_PI_2, 1.0);
        assert!((b.w - W_NORM).abs() < 1e-6, "W unchanged by elevation");
        assert!(b.y.abs() < 1e-6, "Y should be ~0 overhead");
        assert!(
            (b.z - MAX_RE_WEIGHT).abs() < 1e-6,
            "Z should be max-rE overhead"
        );
        assert!(b.x.abs() < 1e-6, "X should be ~0 overhead");
    }

    #[test]
    fn foa_encode_below() {
        let b = foa_encode(0.0, -FRAC_PI_2, 1.0);
        assert!((b.w - W_NORM).abs() < 1e-6);
        assert!(
            (b.z + MAX_RE_WEIGHT).abs() < 1e-6,
            "Z should be -max-rE below"
        );
        assert!(b.x.abs() < 1e-6);
    }

    #[test]
    fn foa_encode_elevated_45() {
        // At 45° elevation, cos(45°) = sin(45°) → X and Z should be equal
        let b = foa_encode(0.0, FRAC_PI_4, 1.0);
        assert!(
            (b.x - b.z).abs() < 1e-6,
            "at 45° elevation, X={} and Z={} should be equal",
            b.x,
            b.z
        );
        assert!(b.y.abs() < 1e-6, "Y should be 0 at azimuth 0");
    }

    // ─────────────────────────────────────────────────────────────────────
    // Audit 2026-11: hand-derived values and invariants
    // ─────────────────────────────────────────────────────────────────────

    /// Unweighted first-order basis (max-rE removed) in ACN order W, Y, Z, X.
    fn unweighted_basis(azimuth: f32, elevation: f32) -> [f64; 4] {
        let bformat = foa_encode(azimuth, elevation, 1.0);
        [
            bformat.w as f64,
            (bformat.y / MAX_RE_WEIGHT) as f64,
            (bformat.z / MAX_RE_WEIGHT) as f64,
            (bformat.x / MAX_RE_WEIGHT) as f64,
        ]
    }

    #[test]
    fn spherical_harmonic_gram_matrix_is_fuma_w_with_sn3d_first_order() {
        // ∫ Yₙ Yₘ dΩ / 4π over the sphere (midpoint quadrature, cos-weighted).
        // SN3D first order: ‖Y‖² = ‖Z‖² = ‖X‖² = 1/3 (e.g. ∫cos²θ sin²φ dΩ/4π = 1/3).
        // W here is 1/√2 (FuMa scaling), so ‖W‖² = 1/2 — not SN3D/AmbiX's 1.
        // Off-diagonal terms vanish (orthogonality). Encoder and decoders share
        // W_NORM, so the convention is self-consistent inside the engine.
        let elevation_steps = 180;
        let azimuth_steps = 360;
        let mut gram = [[0.0f64; 4]; 4];
        let mut total_weight = 0.0f64;
        for elevation_index in 0..elevation_steps {
            let elevation = -std::f64::consts::FRAC_PI_2
                + (elevation_index as f64 + 0.5) * std::f64::consts::PI / elevation_steps as f64;
            let weight = elevation.cos();
            for azimuth_index in 0..azimuth_steps {
                let azimuth =
                    (azimuth_index as f64 + 0.5) * std::f64::consts::TAU / azimuth_steps as f64;
                let basis = unweighted_basis(azimuth as f32, elevation as f32);
                for row in 0..4 {
                    for column in 0..4 {
                        gram[row][column] += weight * basis[row] * basis[column];
                    }
                }
                total_weight += weight;
            }
        }
        let expected_diagonal = [0.5, 1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0];
        for (row, gram_row) in gram.iter().enumerate() {
            for (column, sum) in gram_row.iter().enumerate() {
                let value = sum / total_weight;
                let expected = if row == column {
                    expected_diagonal[row]
                } else {
                    0.0
                };
                assert!(
                    (value - expected).abs() < 1e-4,
                    "Gram[{row}][{column}] = {value}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn circular_harmonic_gram_matrix_on_horizon_is_uniform_one_half() {
        // On the horizontal circle the (W, Y, X) basis with W = 1/√2 is
        // orthogonal with equal norms: mean(W²) = 1/2, mean(sin²φ) = mean(cos²φ)
        // = 1/2. Equal norms are why EPAD energy is exactly azimuth-independent.
        let point_count = 36;
        let mut gram = [[0.0f64; 3]; 3];
        for index in 0..point_count {
            let azimuth = index as f32 * std::f32::consts::TAU / point_count as f32;
            let basis = unweighted_basis(azimuth, 0.0);
            let horizontal = [basis[0], basis[1], basis[3]];
            for row in 0..3 {
                for column in 0..3 {
                    gram[row][column] += horizontal[row] * horizontal[column];
                }
            }
        }
        for (row, gram_row) in gram.iter().enumerate() {
            for (column, value) in gram_row.iter().enumerate() {
                let mean = value / point_count as f64;
                let expected = if row == column { 0.5 } else { 0.0 };
                assert!(
                    (mean - expected).abs() < 1e-6,
                    "circular Gram[{row}][{column}] = {mean}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn max_re_weight_is_the_2d_first_order_optimum() {
        // 2D max-rE: a₁ = cos(π/(2N+2)) = cos(π/4) = 1/√2 for N = 1.
        // For a 2D panning function g(γ) = 1 + 2a·cos γ, |rE| = 2a/(1+2a²),
        // maximised at a = 1/√2 where |rE| = 1/√2.
        assert_close(
            MAX_RE_WEIGHT,
            (std::f32::consts::PI / 4.0).cos(),
            1e-7,
            "max-rE weight",
        );
        let energy_vector_magnitude = |weight: f32| 2.0 * weight / (1.0 + 2.0 * weight * weight);
        let at_optimum = energy_vector_magnitude(MAX_RE_WEIGHT);
        assert_close(at_optimum, std::f32::consts::FRAC_1_SQRT_2, 1e-6, "|rE|");
        assert!(energy_vector_magnitude(MAX_RE_WEIGHT - 0.02) < at_optimum);
        assert!(energy_vector_magnitude(MAX_RE_WEIGHT + 0.02) < at_optimum);
    }
}
