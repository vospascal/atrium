//! Bilateral ambisonics decoder for stereo headphones.

use super::encode::BFormat;
use super::rotate::foa_rotate_z;

/// Bilateral ambisonics decoder for stereo headphone output.
///
/// Implements Bilateral Ambisonics (Ben-Hur et al. 2020): rotates B-format
/// by ±π/2 (90°) per ear, aligning each ear's ipsilateral hemisphere with
/// the "front" axis. The cardioid decode then naturally emphasizes sounds
/// on each ear's side.
///
/// Without HRTFs, we use forward-facing cardioid weights after rotation:
///   ear = W × √2 × 0.5 + X' × 0.5
/// (the √2 undoes the SN3D 1/√2 on W), where X' is the rotated forward axis
/// (was Y before the ±90° rotation).
///
/// Reference: Ben-Hur et al. 2020, "Binaural Reproduction Based on
/// Bilateral Ambisonics and Ear-Aligned HRTFs."
#[derive(Clone, Copy)]
pub struct BilateralDecoder;

impl BilateralDecoder {
    pub fn new() -> Self {
        Self
    }

    /// Decode B-format to stereo (left, right) for headphone output.
    pub fn decode_stereo(&self, bformat: &BFormat) -> (f32, f32) {
        let sqrt2 = std::f32::consts::SQRT_2;

        // ±90° rotation aligns each ear's ipsilateral direction with "front" (X axis).
        let b_left = foa_rotate_z(bformat, std::f32::consts::FRAC_PI_2);
        let b_right = foa_rotate_z(bformat, -std::f32::consts::FRAC_PI_2);

        // Forward-facing cardioid decode (X is primary after ±90° rotation):
        //   W × √2 × 0.5 — omnidirectional pressure
        //   X' × 0.5 — ipsilateral emphasis (was Y before rotation)
        let left = b_left.w * sqrt2 * 0.5 + b_left.x * 0.5;
        let right = b_right.w * sqrt2 * 0.5 + b_right.x * 0.5;
        (left, right)
    }
}

impl Default for BilateralDecoder {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::super::encode::foa_encode;
    use super::super::test_support::assert_close;
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn bilateral_front_source_balanced() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(0.0, 0.0, 1.0); // front center
        let (l, r) = bilateral.decode_stereo(&b);
        assert!(
            (l - r).abs() < 0.05,
            "bilateral front: L={l:.4} and R={r:.4} should be roughly equal"
        );
    }

    #[test]
    fn bilateral_left_source_louder_left() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(FRAC_PI_2, 0.0, 1.0); // left
        let (l, r) = bilateral.decode_stereo(&b);
        assert!(
            l > r,
            "bilateral left source: L={l:.4} should be louder than R={r:.4}"
        );
    }

    #[test]
    fn bilateral_right_source_louder_right() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(-FRAC_PI_2, 0.0, 1.0); // right
        let (l, r) = bilateral.decode_stereo(&b);
        assert!(
            r > l,
            "bilateral right source: R={r:.4} should be louder than L={l:.4}"
        );
    }

    #[test]
    fn bilateral_nonzero_output() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(0.5, 0.0, 1.0);
        let (l, r) = bilateral.decode_stereo(&b);
        assert!(l.abs() > 0.01, "left output should be nonzero: {l}");
        assert!(r.abs() > 0.01, "right output should be nonzero: {r}");
    }

    #[test]
    fn bilateral_strong_ild_at_90_deg() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(FRAC_PI_2, 0.0, 1.0); // source at 90° left
        let (l, r) = bilateral.decode_stereo(&b);
        let ild = (l - r).abs();
        assert!(ild > 0.5, "bilateral ILD at 90° too low: {ild:.4}");
    }

    // ─────────────────────────────────────────────────────────────────────
    // Audit 2026-11: hand-derived values and invariants
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn bilateral_hand_derived_ear_gains() {
        // Left ear  = ½(√2·W + X'_left)  with X'_left  = a·cos(φ − π/2) = a·sin φ·cos θ
        // Right ear = ½(√2·W + X'_right) with X'_right = −a·sin φ·cos θ
        // so L = ½(1 + a·sin φ·cos θ), R = ½(1 − a·sin φ·cos θ), L + R = 1.
        //   90° left: L = ½(1 + 0.707107) = 0.853553, R = 0.146447 → ILD 15.31 dB
        //   30° left: L = ½(1 + 0.353553) = 0.676777, R = 0.323223
        //   front / overhead: L = R = ½
        let bilateral = BilateralDecoder::new();
        let cases = [
            (FRAC_PI_2, 0.0, 0.853553, 0.146447),
            (30.0_f32.to_radians(), 0.0, 0.676777, 0.323223),
            (0.0, 0.0, 0.5, 0.5),
            (1.0, FRAC_PI_2, 0.5, 0.5),
        ];
        for (azimuth, elevation, expected_left, expected_right) in cases {
            let (left, right) = bilateral.decode_stereo(&foa_encode(azimuth, elevation, 1.0));
            assert_close(left, expected_left, 1e-5, "left ear");
            assert_close(right, expected_right, 1e-5, "right ear");
        }
        let (left, right) = bilateral.decode_stereo(&foa_encode(FRAC_PI_2, 0.0, 1.0));
        assert_close(20.0 * (left / right).log10(), 15.31, 0.01, "ILD at 90°");
    }

    #[test]
    fn bilateral_is_left_right_mirror_symmetric_with_constant_amplitude_sum() {
        let bilateral = BilateralDecoder::new();
        for degrees in (-180..=180).step_by(10) {
            let azimuth = (degrees as f32).to_radians();
            let (left, right) = bilateral.decode_stereo(&foa_encode(azimuth, 0.0, 1.0));
            let (mirror_left, mirror_right) =
                bilateral.decode_stereo(&foa_encode(-azimuth, 0.0, 1.0));
            assert_close(left, mirror_right, 1e-5, "mirror L↔R");
            assert_close(right, mirror_left, 1e-5, "mirror R↔L");
            assert_close(left + right, 1.0, 1e-5, "amplitude sum");
        }
    }

    #[test]
    fn bilateral_power_rises_1_76_db_from_front_to_side() {
        // L² + R² = ½(1 + a²·sin²φ) = ½ at the front and ¾ at ±90° (a² = ½):
        // 10·log10(1.5) = 1.761 dB louder at the sides. The decode is
        // amplitude-sum-preserving, not power-preserving; pinned so a change to
        // either is deliberate.
        let bilateral = BilateralDecoder::new();
        let power = |azimuth: f32| {
            let (left, right) = bilateral.decode_stereo(&foa_encode(azimuth, 0.0, 1.0));
            left * left + right * right
        };
        assert_close(power(0.0), 0.5, 1e-6, "front power");
        assert_close(power(FRAC_PI_2), 0.75, 1e-6, "side power");
        assert_close(
            10.0 * (power(FRAC_PI_2) / power(0.0)).log10(),
            1.761,
            1e-3,
            "front→side power rise",
        );
    }
}
