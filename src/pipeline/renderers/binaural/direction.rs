//! Where a path arrives from, in the two forms the renderer needs: SOFA
//! coordinates for the direct path's HRIR lookup, and an equal-power pan
//! for reflections.

use atrium_core::listener::Listener;
use atrium_core::types::Vec3;

/// Equal-power stereo gains from a lateral pan position.
/// `pan` ∈ [0, 1]: 0 = full left, 0.5 = center, 1 = full right.
/// Invariant: gain_left² + gain_right² = 1 for all pan values.
#[inline]
pub(super) fn equal_power_pan(pan: f32) -> (f32, f32) {
    let gain_left = (std::f32::consts::FRAC_PI_2 * pan).cos();
    let gain_right = (std::f32::consts::FRAC_PI_2 * (1.0 - pan)).cos();
    (gain_left, gain_right)
}

/// Convert source position to SOFA listener-relative coordinates.
/// NOTE: Only yaw rotation is applied. Pitch and roll are ignored — the
/// atrium installation assumes a horizontal listener plane.
pub(super) fn to_sofa_coords(source_pos: Vec3, listener: &Listener) -> (f32, f32, f32) {
    let d = source_pos - listener.position;
    let yaw = listener.yaw;
    let forward = d.x * yaw.cos() + d.y * yaw.sin();
    let right = d.x * yaw.sin() - d.y * yaw.cos();
    (forward, -right, d.z)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A reflection arriving from the listener's right must be louder in the
    /// right channel. This guards against the channel-swap bug where
    /// `gain_left` used `(1 - pan)` and `gain_right` used `pan` — mirroring
    /// every reflection to the wrong side.
    #[test]
    fn pan_right_is_louder_in_right_channel() {
        let (l, r) = equal_power_pan(1.0); // fully right
        assert!(
            l.abs() < 1e-6,
            "full-right pan should silence left, got {l}"
        );
        assert!(
            (r - 1.0).abs() < 1e-6,
            "full-right pan should be unity right, got {r}"
        );

        let (l, r) = equal_power_pan(0.75);
        assert!(r > l, "right-of-center pan must favor the right channel");
    }

    #[test]
    fn pan_left_is_louder_in_left_channel() {
        let (l, r) = equal_power_pan(0.0); // fully left
        assert!((l - 1.0).abs() < 1e-6);
        assert!(r.abs() < 1e-6);
    }

    #[test]
    fn pan_center_is_equal_power() {
        let (l, r) = equal_power_pan(0.5);
        assert!((l - r).abs() < 1e-6, "center pan should be symmetric");
        assert!(
            (l - std::f32::consts::FRAC_1_SQRT_2).abs() < 1e-6,
            "center pan should be -3 dB per channel"
        );
    }

    /// Energy preservation: L² + R² = 1 across the whole pan range.
    #[test]
    fn pan_preserves_power_everywhere() {
        for i in 0..=20 {
            let pan = i as f32 / 20.0;
            let (l, r) = equal_power_pan(pan);
            let power = l * l + r * r;
            assert!(
                (power - 1.0).abs() < 1e-5,
                "pan={pan}: power={power} should be 1.0"
            );
        }
    }
}
