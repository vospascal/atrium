//! Concrete `PathEffect` implementations.
//!
//! - `AirAbsorptionEffect`: ISO 9613-1 frequency-dependent atmospheric absorption.
//! - `DistanceAttenuationEffect`: distance-based gain (inverse, linear, exponential).

use atrium_core::panner::DistanceModelType;

use crate::audio::distance::DistanceModel;
use crate::pipeline::path::{PathEffect, PathEffectContext};
use crate::pipeline::stages::air_absorption::AirAbsorptionFilter;

// ─────────────────────────────────────────────────────────────────────────────
// AirAbsorptionEffect
// ─────────────────────────────────────────────────────────────────────────────

/// Per-path air absorption using ISO 9613-1 lowpass filter.
///
/// Reuses `AirAbsorptionFilter` — the same biquad + hysteresis logic used by
/// the existing SourceStage and PathStage implementations.
pub struct AirAbsorptionEffect {
    inner: AirAbsorptionFilter,
}

impl AirAbsorptionEffect {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            inner: AirAbsorptionFilter::new(sample_rate),
        }
    }
}

impl PathEffect for AirAbsorptionEffect {
    fn update(&mut self, ctx: &PathEffectContext) {
        self.inner.update(ctx.path.distance, ctx.atmosphere);
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        self.inner.process(sample)
    }

    fn name(&self) -> &str {
        "air_absorption"
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// DistanceAttenuationEffect
// ─────────────────────────────────────────────────────────────────────────────

/// Per-path distance attenuation. Computes a broadband gain from path distance.
///
/// Uses the same distance model formulas as `distance_gain_at_model` in
/// atrium-core, but operates on `path.distance` directly instead of two positions.
pub struct DistanceAttenuationEffect {
    gain: f32,
    distance_model: DistanceModel,
}

impl DistanceAttenuationEffect {
    pub fn new(distance_model: DistanceModel) -> Self {
        Self {
            gain: 1.0,
            distance_model,
        }
    }
}

impl PathEffect for DistanceAttenuationEffect {
    fn update(&mut self, ctx: &PathEffectContext) {
        let dm = &self.distance_model;
        let clamped = ctx.path.distance.clamp(dm.ref_distance, dm.max_distance);

        self.gain = match dm.model {
            DistanceModelType::Linear => {
                let range = dm.max_distance - dm.ref_distance;
                if range <= 0.0 {
                    1.0
                } else {
                    1.0 - dm.rolloff * (clamped - dm.ref_distance) / range
                }
            }
            DistanceModelType::Inverse => {
                let denom = dm.ref_distance + dm.rolloff * (clamped - dm.ref_distance);
                if denom <= 0.0 {
                    1.0
                } else {
                    dm.ref_distance / denom
                }
            }
            DistanceModelType::Exponential => {
                if dm.ref_distance <= 0.0 {
                    1.0
                } else {
                    (clamped / dm.ref_distance).powf(-dm.rolloff)
                }
            }
        }
        .clamp(0.0, 1.0);
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        sample * self.gain
    }

    fn name(&self) -> &str {
        "distance_attenuation"
    }

    fn reset(&mut self) {
        self.gain = 1.0;
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::atmosphere::AtmosphericParams;
    use crate::audio::propagation::GroundProperties;
    use crate::pipeline::path::{PathContribution, PathKind};
    use atrium_core::types::Vec3;

    fn make_ctx(distance: f32) -> (PathContribution, AtmosphericParams, GroundProperties) {
        let path = PathContribution {
            kind: PathKind::Direct,
            direction: Vec3::new(1.0, 0.0, 0.0),
            distance,
            delay_seconds: 0.0,
            gain: 1.0,
        };
        (
            path,
            AtmosphericParams::default(),
            GroundProperties::default(),
        )
    }

    // ── AirAbsorptionEffect ─────────────────────────────────────────────

    #[test]
    fn air_absorption_transparent_at_zero_distance() {
        let (path, atmo, ground) = make_ctx(0.0);
        let mut effect = AirAbsorptionEffect::new(48000.0);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
        };
        effect.update(&ctx);
        // Feed DC (1.0) until the biquad settles — a 20kHz lowpass passes DC
        let mut out = 0.0;
        for _ in 0..200 {
            out = effect.process_sample(1.0);
        }
        assert!(
            (out - 1.0).abs() < 0.01,
            "expected near-transparent at 0m after settling, got {out}"
        );
    }

    #[test]
    fn air_absorption_attenuates_at_distance() {
        let (path_near, atmo, ground) = make_ctx(2.0);
        let (path_far, _, _) = make_ctx(50.0);
        let mut effect_near = AirAbsorptionEffect::new(48000.0);
        let mut effect_far = AirAbsorptionEffect::new(48000.0);

        let ctx_near = PathEffectContext {
            path: &path_near,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
        };
        let ctx_far = PathEffectContext {
            path: &path_far,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
        };

        effect_near.update(&ctx_near);
        effect_far.update(&ctx_far);

        // Process a burst of samples to let the filter settle
        let mut out_near = 0.0;
        let mut out_far = 0.0;
        for _ in 0..1000 {
            out_near = effect_near.process_sample(1.0);
            out_far = effect_far.process_sample(1.0);
        }
        // Far distance should attenuate more (lower cutoff = more HF loss)
        assert!(
            out_near >= out_far,
            "near ({out_near}) should >= far ({out_far})"
        );
    }

    // ── DistanceAttenuationEffect ───────────────────────────────────────

    #[test]
    fn distance_atten_unity_at_ref_distance() {
        let dm = DistanceModel::default(); // ref=0.3, max=20, rolloff=1, inverse
        let (path, atmo, ground) = make_ctx(dm.ref_distance);
        let mut effect = DistanceAttenuationEffect::new(dm);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
        };
        effect.update(&ctx);
        let out = effect.process_sample(1.0);
        assert!(
            (out - 1.0).abs() < 1e-6,
            "expected 1.0 at ref distance, got {out}"
        );
    }

    #[test]
    fn distance_atten_decreases_with_distance() {
        let dm = DistanceModel::default();
        let (path_near, atmo, ground) = make_ctx(1.0);
        let (path_far, _, _) = make_ctx(10.0);
        let mut effect = DistanceAttenuationEffect::new(dm);

        let ctx_near = PathEffectContext {
            path: &path_near,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
        };
        effect.update(&ctx_near);
        let gain_near = effect.process_sample(1.0);

        let ctx_far = PathEffectContext {
            path: &path_far,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
        };
        effect.update(&ctx_far);
        let gain_far = effect.process_sample(1.0);

        assert!(
            gain_near > gain_far,
            "near gain ({gain_near}) should > far gain ({gain_far})"
        );
    }

    #[test]
    fn distance_atten_linear_model() {
        let dm = DistanceModel {
            ref_distance: 1.0,
            max_distance: 10.0,
            rolloff: 1.0,
            model: DistanceModelType::Linear,
        };
        let (path_mid, atmo, ground) = make_ctx(5.5);
        let mut effect = DistanceAttenuationEffect::new(dm);
        let ctx = PathEffectContext {
            path: &path_mid,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
        };
        effect.update(&ctx);
        let gain = effect.process_sample(1.0);
        // Linear: 1.0 - 1.0 * (5.5 - 1.0) / (10.0 - 1.0) = 1.0 - 4.5/9.0 = 0.5
        assert!(
            (gain - 0.5).abs() < 1e-6,
            "expected 0.5 at midpoint, got {gain}"
        );
    }

    #[test]
    fn distance_atten_exponential_model() {
        let dm = DistanceModel {
            ref_distance: 1.0,
            max_distance: 100.0,
            rolloff: 2.0,
            model: DistanceModelType::Exponential,
        };
        let (path, atmo, ground) = make_ctx(2.0);
        let mut effect = DistanceAttenuationEffect::new(dm);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
        };
        effect.update(&ctx);
        let gain = effect.process_sample(1.0);
        // Exponential: (2.0 / 1.0)^(-2.0) = 0.25
        assert!((gain - 0.25).abs() < 1e-6, "expected 0.25, got {gain}");
    }

    #[test]
    fn distance_atten_reset_restores_unity() {
        let dm = DistanceModel::default();
        let (path, atmo, ground) = make_ctx(10.0);
        let mut effect = DistanceAttenuationEffect::new(dm);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
        };
        effect.update(&ctx);
        assert!(effect.process_sample(1.0) < 1.0);
        effect.reset();
        assert!((effect.process_sample(1.0) - 1.0).abs() < 1e-6);
    }
}
