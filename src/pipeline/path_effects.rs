//! Concrete `PathEffect` implementations.
//!
//! - `AirAbsorptionEffect`: ISO 9613-1 frequency-dependent atmospheric absorption.
//! - `DistanceAttenuationEffect`: distance-based gain (inverse, linear, exponential).
//! - `PropagationDelayEffect`: fractional-delay line for reflection timing.

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
/// the SourceStage and WorldLockedRenderer.
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
// PropagationDelayEffect
// ─────────────────────────────────────────────────────────────────────────────

/// Fractional-delay line for propagation delay (reflections, diffraction).
///
/// Uses a circular buffer with linear interpolation between adjacent samples,
/// following the pattern from `delay_comp.rs:118-126`. The delay is derived
/// from `path.delay_seconds * sample_rate` each buffer.
///
/// Buffer capacity is 8192 samples (~170ms at 48kHz), enough for room-scale
/// first-order reflections. Delays beyond capacity are clamped.
pub struct PropagationDelayEffect {
    buffer: Box<[f32; Self::CAPACITY]>,
    write_pos: usize,
    delay_samples: f32,
    sample_rate: f32,
}

impl PropagationDelayEffect {
    const CAPACITY: usize = 8192;
    const MASK: usize = Self::CAPACITY - 1;

    pub fn new(sample_rate: f32) -> Self {
        Self {
            buffer: Box::new([0.0; Self::CAPACITY]),
            write_pos: 0,
            delay_samples: 0.0,
            sample_rate,
        }
    }
}

impl PathEffect for PropagationDelayEffect {
    fn update(&mut self, ctx: &PathEffectContext) {
        self.sample_rate = ctx.sample_rate;
        self.delay_samples = ctx.path.delay_seconds * ctx.sample_rate;
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        let wp = self.write_pos;
        self.buffer[wp] = sample;
        self.write_pos = (wp + 1) & Self::MASK;

        if self.delay_samples < 0.5 {
            return sample;
        }

        let delay_clamped = self.delay_samples.min((Self::CAPACITY - 2) as f32);
        let delay_int = delay_clamped as usize;
        let frac = delay_clamped - delay_int as f32;

        let idx0 = (wp + Self::CAPACITY - delay_int) & Self::MASK;
        let idx1 = (wp + Self::CAPACITY - delay_int - 1) & Self::MASK;

        let s0 = self.buffer[idx0];
        let s1 = self.buffer[idx1];
        s0 + (s1 - s0) * frac
    }

    fn name(&self) -> &str {
        "propagation_delay"
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
        self.delay_samples = 0.0;
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

    // ── PropagationDelayEffect ──────────────────────────────────────────

    fn make_delay_ctx(
        delay_seconds: f32,
    ) -> (PathContribution, AtmosphericParams, GroundProperties) {
        let path = PathContribution {
            kind: PathKind::Reflection,
            direction: Vec3::new(-1.0, 0.0, 0.0),
            distance: 10.0,
            delay_seconds,
            gain: 0.9,
        };
        (
            path,
            AtmosphericParams::default(),
            GroundProperties::default(),
        )
    }

    #[test]
    fn propagation_delay_zero_is_passthrough() {
        let (path, atmo, ground) = make_delay_ctx(0.0);
        let mut effect = PropagationDelayEffect::new(48000.0);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate: 48000.0,
        };
        effect.update(&ctx);

        // With zero delay, output should equal input immediately
        let out = effect.process_sample(0.5);
        assert!(
            (out - 0.5).abs() < 1e-6,
            "zero delay should pass through, got {out}"
        );
    }

    #[test]
    fn propagation_delay_integer_samples() {
        let sample_rate = 48000.0;
        let delay_samples = 10.0;
        let delay_seconds = delay_samples / sample_rate;

        let (path, atmo, ground) = make_delay_ctx(delay_seconds);
        let mut effect = PropagationDelayEffect::new(sample_rate);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate,
        };
        effect.update(&ctx);

        // Feed an impulse at sample 0, then silence
        let _ = effect.process_sample(1.0);
        for _ in 1..10 {
            let out = effect.process_sample(0.0);
            assert!(out.abs() < 1e-6, "should be silent before delay, got {out}");
        }
        // At sample 10 (after 10 samples of delay), the impulse should appear
        let out = effect.process_sample(0.0);
        assert!(
            (out - 1.0).abs() < 1e-6,
            "impulse should appear at delay offset, got {out}"
        );
    }

    #[test]
    fn propagation_delay_fractional_interpolates() {
        let sample_rate = 48000.0;
        let delay_samples = 5.5;
        let delay_seconds = delay_samples / sample_rate;

        let (path, atmo, ground) = make_delay_ctx(delay_seconds);
        let mut effect = PropagationDelayEffect::new(sample_rate);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate,
        };
        effect.update(&ctx);

        // Feed an impulse, then silence
        let _ = effect.process_sample(1.0);
        for _ in 1..5 {
            let _ = effect.process_sample(0.0);
        }
        // At sample 5 (integer part of 5.5): should get partial impulse
        let out5 = effect.process_sample(0.0);
        // At sample 6: should get the complementary part
        let out6 = effect.process_sample(0.0);

        // Linear interpolation: sample 5 gets 0.5, sample 6 gets 0.5
        assert!(
            (out5 - 0.5).abs() < 1e-6,
            "fractional delay at int part: expected 0.5, got {out5}"
        );
        assert!(
            (out6 - 0.5).abs() < 1e-6,
            "fractional delay at int+1: expected 0.5, got {out6}"
        );
    }

    #[test]
    fn propagation_delay_reset_clears_buffer() {
        let sample_rate = 48000.0;
        let delay_seconds = 10.0 / sample_rate;

        let (path, atmo, ground) = make_delay_ctx(delay_seconds);
        let mut effect = PropagationDelayEffect::new(sample_rate);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate,
        };
        effect.update(&ctx);

        // Fill buffer with signal
        for _ in 0..20 {
            effect.process_sample(1.0);
        }

        // Reset should clear everything
        effect.reset();
        effect.update(&ctx);

        // After reset, feeding silence should produce silence (no stale data)
        for _ in 0..20 {
            let out = effect.process_sample(0.0);
            assert!(out.abs() < 1e-6, "after reset, should be silent, got {out}");
        }
    }

    #[test]
    fn propagation_delay_preserves_signal_energy() {
        let sample_rate = 48000.0;
        let delay_seconds = 100.0 / sample_rate;

        let (path, atmo, ground) = make_delay_ctx(delay_seconds);
        let mut effect = PropagationDelayEffect::new(sample_rate);
        let ctx = PathEffectContext {
            path: &path,
            atmosphere: &atmo,
            ground: &ground,
            sample_rate,
        };
        effect.update(&ctx);

        // Feed N samples of DC, then enough silence for the delay to drain
        let n = 200;
        let mut total_out = 0.0;
        for _ in 0..n {
            total_out += effect.process_sample(1.0);
        }
        for _ in 0..200 {
            total_out += effect.process_sample(0.0);
        }

        // Total energy out should equal total energy in (delay is lossless)
        assert!(
            (total_out - n as f32).abs() < 1e-3,
            "delay should preserve energy: expected {n}, got {total_out}"
        );
    }
}
