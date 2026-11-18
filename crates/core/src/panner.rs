// Panning & spatialization.
//
// References for future expansion:
//   - HRTF binaural: hrtf crate (https://github.com/mrDIMAS/hrtf), fyrox-sound HRTF,
//     web-audio-api-rs PannerNode (IRCAM LISTEN HRIR database)
//   - 5.1 surround: VBAP panning (Pulkki 1997), see cubeb-rs for multichannel device routing
//   - Ambisonics: encode sources → FOA (W,Y,Z,X) → decode to 5.1 or binaural
//
// See REFERENCES.md for full list.

use crate::types::Vec3;

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
///
/// For every model and every `rolloff ≥ 0` the gain is monotonic
/// non-increasing in distance and saturates at `MAX_NEAR_FIELD_GAIN` (+20 dB)
/// where the law diverges. The inverse law with `rolloff > 1` diverges at
/// d = ref·(1 − 1/rolloff), where its denominator reaches zero; everything
/// closer than that radius is held at the cap rather than falling back to
/// unity, so a source moving closer never gets quieter.
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
            if ref_distance <= 0.0 {
                // Degenerate configuration: no reference distance, no attenuation
                // (same convention as the exponential model).
                1.0
            } else {
                let denominator = ref_distance + rolloff * (effective - ref_distance);
                if denominator <= 0.0 {
                    // Inside the divergence radius ref·(1 − 1/rolloff): the law
                    // has gone through +∞, so hold the near-field ceiling.
                    MAX_NEAR_FIELD_GAIN
                } else {
                    ref_distance / denominator
                }
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inverse_model_partial_rolloff() {
        // Inverse, ref 1, rolloff 0.5, d = 3: 1 / (1 + 0.5·(3 − 1)) = 0.5.
        let gain = distance_gain_at_model(
            Vec3::ZERO,
            Vec3::new(0.0, 3.0, 0.0),
            1.0,
            20.0,
            0.5,
            DistanceModelType::Inverse,
        );
        assert!((gain - 0.5).abs() < 1e-6, "gain={gain}");
    }

    #[test]
    fn linear_model_hand_values() {
        // Linear, ref 1, max 11, rolloff 1: gain = 1 − (d − 1)/10.
        let linear = |distance: f32| {
            distance_gain_at_model(
                Vec3::ZERO,
                Vec3::new(distance, 0.0, 0.0),
                1.0,
                11.0,
                1.0,
                DistanceModelType::Linear,
            )
        };
        assert!((linear(6.0) - 0.5).abs() < 1e-6); // halfway → 0.5
        assert!(linear(11.0).abs() < 1e-6); // at max → silent
        assert!(linear(30.0).abs() < 1e-6); // beyond max → clamped to max → silent
                                            // Near field (no ref clamp, deliberate): d = 0.5 → 1 + 0.5/10 = 1.05.
        assert!((linear(0.5) - 1.05).abs() < 1e-6);
    }

    #[test]
    fn exponential_model_hand_values() {
        // Exponential: (d/ref)^(−rolloff), ref 1.
        let exponential = |distance: f32, rolloff: f32| {
            distance_gain_at_model(
                Vec3::ZERO,
                Vec3::new(distance, 0.0, 0.0),
                1.0,
                20.0,
                rolloff,
                DistanceModelType::Exponential,
            )
        };
        assert!((exponential(2.0, 1.0) - 0.5).abs() < 1e-6); // 2^−1
        assert!((exponential(2.0, 2.0) - 0.25).abs() < 1e-6); // 2^−2
        assert!((exponential(4.0, 0.5) - 0.5).abs() < 1e-6); // 4^−0.5
                                                             // Near field: 0.5^−1 = 2 (+6 dB), same as the inverse model at rolloff 1.
        assert!((exponential(0.5, 1.0) - 2.0).abs() < 1e-5);
    }

    #[test]
    fn zero_distance_is_finite_and_capped_for_every_model() {
        // Source exactly on the listener: MIN_DISTANCE floor + 10× (+20 dB) cap,
        // never inf/NaN.
        for model in [
            DistanceModelType::Inverse,
            DistanceModelType::Exponential,
            DistanceModelType::Linear,
        ] {
            let gain = distance_gain_at_model(Vec3::ZERO, Vec3::ZERO, 1.0, 20.0, 1.0, model);
            assert!(gain.is_finite(), "{model:?} gain={gain}");
            assert!(gain <= MAX_NEAR_FIELD_GAIN, "{model:?} gain={gain}");
        }
    }

    /// With rolloff > 1 the inverse denominator `ref + rolloff·(d − ref)`
    /// crosses zero at d = ref·(1 − 1/rolloff). ref 1, rolloff 2: d = 0.6 →
    /// 1/0.2 = 5 (+14 dB); d = 0.4 → denominator −0.2, inside the divergence
    /// radius 0.5 → held at the 10× cap. The old guard returned 1.0 there, so
    /// a source moving closer got 14 dB quieter.
    /// `rolloff` is user-configurable in the scene YAML and unvalidated.
    #[test]
    fn inverse_model_near_field_is_monotonic_for_rolloff_above_one() {
        let inverse = |distance: f32| {
            distance_gain_at_model(
                Vec3::ZERO,
                Vec3::new(distance, 0.0, 0.0),
                1.0,
                20.0,
                2.0,
                DistanceModelType::Inverse,
            )
        };
        let gain_at_0_6 = inverse(0.6);
        let gain_at_0_4 = inverse(0.4);
        assert!(
            (gain_at_0_6 - 5.0).abs() < 1e-4,
            "gain at 0.6 m = {gain_at_0_6}"
        );
        assert!(
            gain_at_0_4 >= gain_at_0_6,
            "closer source must not be quieter: 0.4 m → {gain_at_0_4}, 0.6 m → {gain_at_0_6}"
        );
    }

    /// Every model, every rolloff ≥ 0: sweeping the source from 0 to past
    /// max_distance never makes the gain rise, the gain stays in
    /// [0, MAX_NEAR_FIELD_GAIN], and a law that diverges (inverse rolloff > 1,
    /// exponential near zero) ends at the cap, not below it.
    #[test]
    fn every_model_is_monotonic_non_increasing_in_distance() {
        for model in [
            DistanceModelType::Inverse,
            DistanceModelType::Exponential,
            DistanceModelType::Linear,
        ] {
            for rolloff in [0.0_f32, 0.5, 1.0, 1.5, 2.0, 4.0, 10.0] {
                let mut previous_gain = f32::INFINITY;
                for step in 0..=3000 {
                    let distance = step as f32 * 0.01;
                    let gain = distance_gain_at_model(
                        Vec3::ZERO,
                        Vec3::new(distance, 0.0, 0.0),
                        1.0,
                        20.0,
                        rolloff,
                        model,
                    );
                    assert!(
                        (0.0..=MAX_NEAR_FIELD_GAIN).contains(&gain),
                        "{model:?} rolloff {rolloff} d {distance}: gain {gain}"
                    );
                    assert!(
                        gain <= previous_gain,
                        "{model:?} rolloff {rolloff}: gain rose to {gain} at {distance} m \
                         from {previous_gain}"
                    );
                    previous_gain = gain;
                }
            }
        }
        // Inverse rolloff 4: divergence radius 1·(1 − 1/4) = 0.75 m, so 0.5 m
        // is held at the cap.
        let inside_divergence = distance_gain_at_model(
            Vec3::ZERO,
            Vec3::new(0.5, 0.0, 0.0),
            1.0,
            20.0,
            4.0,
            DistanceModelType::Inverse,
        );
        assert_eq!(inside_divergence, MAX_NEAR_FIELD_GAIN);
    }
}
