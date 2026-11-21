//! B-format rotation about the vertical axis.

use super::encode::BFormat;

/// Rotate B-format around the Z (vertical) axis by `angle` radians.
///
/// Standard SN3D/AmbiX Z-rotation for FOA (Zotter & Frank 2019, §3.2):
///
/// ```text
/// [Y']   [ cos(α)  0  -sin(α) ] [Y]
/// [Z'] = [   0     1     0    ] [Z]
/// [X']   [ sin(α)  0   cos(α) ] [X]
/// ```
///
/// W (order 0) is omnidirectional → invariant under rotation.
/// Z is the vertical axis → invariant under yaw rotation.
/// Y and X encode sin(φ) and cos(φ) → rotate as a 2D vector.
/// This is an energy-preserving orthogonal transform (det R = 1).
///
/// **Why only Z rotation?** The ears are offset horizontally (left/right), so the
/// relevant parallax is in the azimuthal plane. Pitch/roll ear offsets are negligible
/// for a stationary listener. This keeps the operation to 2 multiplies + 2 adds.
pub fn foa_rotate_z(b: &BFormat, angle: f32) -> BFormat {
    let c = angle.cos();
    let s = angle.sin();
    BFormat {
        w: b.w,
        y: b.y * c - b.x * s,
        z: b.z,
        x: b.y * s + b.x * c,
    }
}

#[cfg(test)]
mod tests {
    use super::super::encode::{foa_encode, MAX_RE_WEIGHT};
    use super::super::test_support::assert_close;
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn foa_rotate_z_identity() {
        let b = foa_encode(0.0, 0.0, 1.0);
        let rotated = foa_rotate_z(&b, 0.0);
        assert!((rotated.w - b.w).abs() < 1e-6);
        assert!((rotated.y - b.y).abs() < 1e-6);
        assert!((rotated.z - b.z).abs() < 1e-6);
        assert!((rotated.x - b.x).abs() < 1e-6);
    }

    #[test]
    fn foa_rotate_z_90_front_becomes_right() {
        // Front source (azimuth=0): X positive, Y zero.
        // foa_rotate_z turns the *head* by +angle (the field by −angle), so a
        // +90° turn puts a front source on the listener's right:
        //   Y' = Y·cos α − X·sin α = 0 − a·1 = −a  (negative Y = right).
        let b = foa_encode(0.0, 0.0, 1.0);
        let rotated = foa_rotate_z(&b, FRAC_PI_2);
        assert!((rotated.w - b.w).abs() < 1e-6, "W unchanged by rotation");
        assert!(
            (rotated.z - b.z).abs() < 1e-6,
            "Z unchanged by yaw rotation"
        );
        assert!(
            (rotated.y + MAX_RE_WEIGHT).abs() < 1e-6,
            "front source after +90° head turn should sit at the right (Y = −a): Y={}",
            rotated.y
        );
        assert!(
            rotated.x.abs() < 1e-5,
            "X should be ~0 after 90° rotation: X={}",
            rotated.x
        );
    }

    #[test]
    fn foa_rotate_z_preserves_energy() {
        let b = foa_encode(0.7, 0.3, 1.0); // arbitrary direction
        let energy_before = b.w * b.w + b.y * b.y + b.z * b.z + b.x * b.x;
        for deg in (0..360).step_by(30) {
            let angle = (deg as f32).to_radians();
            let r = foa_rotate_z(&b, angle);
            let energy_after = r.w * r.w + r.y * r.y + r.z * r.z + r.x * r.x;
            assert!(
                (energy_before - energy_after).abs() < 1e-5,
                "rotation should preserve energy at {}°: before={:.4}, after={:.4}",
                deg,
                energy_before,
                energy_after
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // Audit 2026-11: hand-derived values and invariants
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn foa_rotate_z_maps_source_azimuth_phi_to_phi_minus_angle() {
        // Y' = Y cos α − X sin α = a·sin(φ − α), X' = Y sin α + X cos α = a·cos(φ − α):
        // a head turn by +α. W and Z are invariant.
        for &source_degrees in &[0.0_f32, 30.0, 90.0, 170.0, -120.0] {
            for &angle_degrees in &[0.0_f32, 45.0, 90.0, -72.0, 200.0] {
                let source = source_degrees.to_radians();
                let angle = angle_degrees.to_radians();
                let rotated = foa_rotate_z(&foa_encode(source, 0.4, 0.8), angle);
                let expected = foa_encode(source - angle, 0.4, 0.8);
                for (actual, wanted) in [
                    (rotated.w, expected.w),
                    (rotated.y, expected.y),
                    (rotated.z, expected.z),
                    (rotated.x, expected.x),
                ] {
                    assert_close(actual, wanted, 1e-5, "rotated component");
                }
            }
        }
    }
}
