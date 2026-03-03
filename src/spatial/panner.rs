// Panning & spatialization.
//
// References for future expansion:
//   - HRTF binaural: hrtf crate (https://github.com/mrDIMAS/hrtf), fyrox-sound HRTF,
//     web-audio-api-rs PannerNode (IRCAM LISTEN HRIR database)
//   - 5.1 surround: VBAP panning (Pulkki 1997), see cubeb-rs for multichannel device routing
//   - Ambisonics: encode sources → FOA (W,X,Y,Z) → decode to 5.1 or binaural
//
// See REFERENCES.md for full list.

use std::f32::consts::PI;

use crate::spatial::listener::Listener;
use crate::world::types::Vec3;

/// Stereo gain pair (left, right).
#[derive(Clone, Copy, Debug)]
pub struct StereoGains {
    pub left: f32,
    pub right: f32,
}

/// Compute equal-power stereo panning gains from a source position relative to a listener.
///
/// The panning law ensures constant perceived loudness as the source moves:
/// left² + right² ≈ 1.0 at all positions.
pub fn stereo_pan(listener: &Listener, source_position: Vec3) -> StereoGains {
    let d = source_position - listener.position;

    // Handle degenerate case: source exactly at listener
    if d.x * d.x + d.y * d.y < 1e-10 {
        return StereoGains {
            left: std::f32::consts::FRAC_1_SQRT_2,
            right: std::f32::consts::FRAC_1_SQRT_2,
        };
    }

    // Rotate direction into listener's local frame (undo listener yaw)
    let cos_y = listener.yaw.cos();
    let sin_y = listener.yaw.sin();
    let local_x = d.x * cos_y + d.y * sin_y; // forward
    let local_y = -d.x * sin_y + d.y * cos_y; // left

    // Azimuth: 0 = ahead, +π/2 = left, -π/2 = right
    let azimuth = local_y.atan2(local_x);

    // Map to pan: sin(azimuth) gives -1 (right) to +1 (left)
    let pan = azimuth.sin();

    // Equal-power gains
    let theta = (1.0 - pan) * PI / 4.0;
    StereoGains {
        left: theta.cos(),
        right: theta.sin(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn listener_at_origin() -> Listener {
        Listener::new(Vec3::new(0.0, 0.0, 0.0), 0.0)
    }

    #[test]
    fn source_ahead_is_center() {
        let g = stereo_pan(&listener_at_origin(), Vec3::new(1.0, 0.0, 0.0));
        assert!((g.left - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
        assert!((g.right - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
    }

    #[test]
    fn source_left() {
        let g = stereo_pan(&listener_at_origin(), Vec3::new(0.0, 1.0, 0.0));
        assert!(g.left > 0.95, "left={}", g.left);
        assert!(g.right < 0.05, "right={}", g.right);
    }

    #[test]
    fn source_right() {
        let g = stereo_pan(&listener_at_origin(), Vec3::new(0.0, -1.0, 0.0));
        assert!(g.left < 0.05, "left={}", g.left);
        assert!(g.right > 0.95, "right={}", g.right);
    }

    #[test]
    fn source_behind_is_center() {
        let g = stereo_pan(&listener_at_origin(), Vec3::new(-1.0, 0.0, 0.0));
        assert!((g.left - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
        assert!((g.right - std::f32::consts::FRAC_1_SQRT_2).abs() < 0.01);
    }

    #[test]
    fn constant_power() {
        // At any position, left² + right² should be ≈ 1.0
        let listener = listener_at_origin();
        for angle_deg in (0..360).step_by(15) {
            let angle = (angle_deg as f32).to_radians();
            let src = Vec3::new(angle.cos(), angle.sin(), 0.0);
            let g = stereo_pan(&listener, src);
            let power = g.left * g.left + g.right * g.right;
            assert!(
                (power - 1.0).abs() < 0.01,
                "angle={}° power={} (left={}, right={})",
                angle_deg,
                power,
                g.left,
                g.right
            );
        }
    }
}
