// Panning & spatialization.
//
// References for future expansion:
//   - HRTF binaural: hrtf crate (https://github.com/mrDIMAS/hrtf), fyrox-sound HRTF,
//     web-audio-api-rs PannerNode (IRCAM LISTEN HRIR database)
//   - 5.1 surround: VBAP panning (Pulkki 1997), see cubeb-rs for multichannel device routing
//   - Ambisonics: encode sources → FOA (W,Y,Z,X) → decode to 5.1 or binaural
//
// See REFERENCES.md for full list.

use std::f32::consts::PI;

use crate::listener::Listener;
use crate::types::Vec3;

/// Stereo gain pair (left, right).
#[derive(Clone, Copy, Debug)]
pub struct StereoGains {
    pub left: f32,
    pub right: f32,
}

/// W3C Web Audio API distance models.
/// See: https://www.w3.org/TR/webaudio/#distance-attenuation
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum DistanceModelType {
    /// gain = 1 - rolloff * (distance - refDistance) / (maxDistance - refDistance)
    /// Straight-line falloff. Good for volumetric sources (rain, wind).
    Linear,
    /// gain = refDistance / (refDistance + rolloff * (distance - refDistance))
    /// Realistic inverse-square-like falloff. Standard for point sources.
    Inverse,
    /// gain = pow(distance / refDistance, -rolloff)
    /// Steeper falloff than inverse. For complex propagation patterns.
    Exponential,
}

/// Compute distance-based gain attenuation between two points.
///
/// Implements all three W3C Web Audio API distance models:
///   - Linear: straight-line falloff from refDistance to maxDistance
///   - Inverse: 1/r-like falloff (most common for point sources)
///   - Exponential: power-law falloff (steeper than inverse)
///
/// - `ref_distance`: distance at which gain = 1.0 (no attenuation).
/// - `max_distance`: beyond this, gain stays constant.
/// - `rolloff`: how quickly sound fades with distance.
pub fn distance_gain_at(
    from: Vec3,
    to: Vec3,
    ref_distance: f32,
    max_distance: f32,
    rolloff: f32,
) -> f32 {
    distance_gain_at_model(
        from,
        to,
        ref_distance,
        max_distance,
        rolloff,
        DistanceModelType::Inverse,
    )
}

/// Minimum effective distance (meters) to prevent division by zero in near field.
const MIN_DISTANCE: f32 = 0.01;

/// Maximum near-field gain (+20 dB). Prevents runaway amplification when a
/// source is extremely close to the listener. At 0.1m with ref_distance=1.0,
/// the inverse model gives gain=10.0 naturally; this cap only triggers closer.
const MAX_NEAR_FIELD_GAIN: f32 = 10.0;

/// Compute distance-based gain using a specific distance model.
///
/// Unlike the W3C Web Audio spec (which clamps distance to ref_distance),
/// this allows near-field gain > 1.0 for sources closer than ref_distance.
/// A source at 0.3m with ref_distance=1.0 gets +10.5 dB of near-field boost,
/// matching real-world inverse square law behavior.
pub fn distance_gain_at_model(
    from: Vec3,
    to: Vec3,
    ref_distance: f32,
    max_distance: f32,
    rolloff: f32,
    model: DistanceModelType,
) -> f32 {
    let dist = from.distance_to(to);
    // Floor at MIN_DISTANCE to prevent div-by-zero; cap at max_distance.
    let effective = dist.max(MIN_DISTANCE).min(max_distance);

    let gain = match model {
        DistanceModelType::Linear => {
            let range = max_distance - ref_distance;
            if range <= 0.0 {
                1.0
            } else {
                1.0 - rolloff * (effective - ref_distance) / range
            }
        }
        DistanceModelType::Inverse => {
            let denom = ref_distance + rolloff * (effective - ref_distance);
            if denom <= 0.0 {
                1.0
            } else {
                ref_distance / denom
            }
        }
        DistanceModelType::Exponential => {
            if ref_distance <= 0.0 {
                1.0
            } else {
                (effective / ref_distance).powf(-rolloff)
            }
        }
    };
    gain.clamp(0.0, MAX_NEAR_FIELD_GAIN)
}

/// Compute distance-based gain from listener to source.
/// Convenience wrapper around [`distance_gain_at`].
pub fn distance_gain(
    listener: &Listener,
    source_position: Vec3,
    ref_distance: f32,
    max_distance: f32,
    rolloff: f32,
) -> f32 {
    distance_gain_at(
        listener.position,
        source_position,
        ref_distance,
        max_distance,
        rolloff,
    )
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

    #[test]
    fn distance_at_ref_is_unity() {
        let l = listener_at_origin();
        // Source at exactly ref_distance → gain = 1.0
        let g = distance_gain(&l, Vec3::new(1.0, 0.0, 0.0), 1.0, 10.0, 1.0);
        assert!((g - 1.0).abs() < 0.001, "gain={g}");
    }

    #[test]
    fn near_field_gain_boost() {
        let l = listener_at_origin();
        // Source at 0.3m with ref_distance=1.0 → gain = 1.0/0.3 ≈ 3.33 (+10.5 dB)
        let g = distance_gain(&l, Vec3::new(0.3, 0.0, 0.0), 1.0, 10.0, 1.0);
        let expected = 1.0 / 0.3; // inverse: ref / (ref + rolloff*(d - ref)) = 1/(1+1*(0.3-1)) = 1/0.3
        assert!(
            (g - expected).abs() < 0.01,
            "near-field gain at 0.3m should be ~{expected:.2}, got {g:.4}"
        );
        assert!(g > 1.0, "near-field gain should exceed 1.0");
    }

    #[test]
    fn near_field_gain_capped() {
        let l = listener_at_origin();
        // Source extremely close (0.01m) → gain would be 100 but capped at 10.0
        let g = distance_gain(&l, Vec3::new(0.01, 0.0, 0.0), 1.0, 10.0, 1.0);
        assert!(
            (g - 10.0).abs() < 0.01,
            "near-field gain should cap at 10.0, got {g}"
        );
    }

    #[test]
    fn distance_far_away_is_quiet() {
        let l = listener_at_origin();
        // Source at 10m with ref=1.0, rolloff=1.0 → gain = 1/(1+1*9) = 0.1
        let g = distance_gain(&l, Vec3::new(10.0, 0.0, 0.0), 1.0, 10.0, 1.0);
        assert!((g - 0.1).abs() < 0.001, "gain={g}");
    }

    #[test]
    fn near_field_6db_per_halving() {
        let l = listener_at_origin();
        // Inverse square: halving distance → +6 dB (2x gain)
        let g_1m = distance_gain(&l, Vec3::new(1.0, 0.0, 0.0), 1.0, 10.0, 1.0);
        let g_half = distance_gain(&l, Vec3::new(0.5, 0.0, 0.0), 1.0, 10.0, 1.0);
        let db_diff = 20.0 * (g_half / g_1m).log10();
        assert!(
            (db_diff - 6.0).abs() < 0.5,
            "halving distance should add ~6 dB, got {db_diff:.1}"
        );
    }

    #[test]
    fn distance_beyond_max_clamps() {
        let l = listener_at_origin();
        // Source at 20m but max=10m → same gain as at 10m
        let g_at_max = distance_gain(&l, Vec3::new(10.0, 0.0, 0.0), 1.0, 10.0, 1.0);
        let g_beyond = distance_gain(&l, Vec3::new(20.0, 0.0, 0.0), 1.0, 10.0, 1.0);
        assert!((g_at_max - g_beyond).abs() < 0.001);
    }
}
