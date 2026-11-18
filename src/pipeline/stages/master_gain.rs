//! Applies runtime master gain from AudioScene.
//!
//! Stateless — reads `ctx.master_gain` each buffer. AudioScene is the
//! single source of truth; this stage just multiplies and soft-clips
//! (the soft clip also maps NaN → 0 and ±Inf → ±1.0, so no non-finite
//! sample reaches the device). In measurement mode, bypasses soft clipping to preserve linear signal
//! levels, applying only NaN/Inf sanitization and a ±100.0 stability ceiling.

use crate::pipeline::mix_stage::{MixContext, MixStage};
use crate::pipeline::stages::{sanitize_finite, soft_clip};

#[derive(Default)]
pub struct MasterGainStage;

impl MixStage for MasterGainStage {
    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        let gain = ctx.master_gain;
        if ctx.measurement_mode {
            for sample in buffer.iter_mut() {
                *sample = sanitize_finite(*sample * gain);
            }
        } else {
            for sample in buffer.iter_mut() {
                *sample = soft_clip(*sample * gain);
            }
        }
    }

    fn name(&self) -> &str {
        "master_gain"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::atmosphere::AtmosphericParams;
    use crate::audio::propagation::GroundProperties;
    use crate::pipeline::environment::Environment;
    use crate::pipeline::wall_material::WallMaterial;
    use atrium_core::listener::Listener;
    use atrium_core::speaker::SpeakerLayout;
    use atrium_core::types::Vec3;

    fn test_environment() -> Environment {
        Environment {
            min: Vec3::new(-5.0, -5.0, -5.0),
            max: Vec3::new(5.0, 5.0, 5.0),
            wall_materials: [WallMaterial::HARD_WALL; 6],
            ground: GroundProperties::hard(),
            atmosphere: AtmosphericParams::default(),
        }
    }

    fn make_ctx() -> (SpeakerLayout, Listener) {
        let layout = SpeakerLayout::stereo(Vec3::new(-1.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0));
        let listener = Listener::new(Vec3::ZERO, 0.0);
        (layout, listener)
    }

    /// In normal mode, signals above the soft-clip knee (0.9) are compressed
    /// toward ±1.0. A sample of 2.0 should come out well below 2.0.
    #[test]
    fn normal_mode_soft_clips() {
        let (layout, listener) = make_ctx();
        let environment = test_environment();
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            environment: &environment,
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            measurement_mode: false,
        };
        let mut buffer = vec![2.0f32; 4];
        let mut stage = MasterGainStage;
        stage.process(&mut buffer, &ctx);
        for &s in &buffer {
            assert!(s < 1.0, "soft-clipped output should be < 1.0, got {s}");
            assert!(s > 0.9, "should still be above knee, got {s}");
        }
    }

    /// In measurement mode, signals pass through linearly (no soft clipping),
    /// limited only by the ±100.0 stability ceiling.
    #[test]
    fn measurement_mode_passes_through_unclipped() {
        let (layout, listener) = make_ctx();
        let environment = test_environment();
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            environment: &environment,
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            measurement_mode: true,
        };
        let mut buffer = vec![2.0f32, -3.0, 0.5, -0.1];
        let mut stage = MasterGainStage;
        stage.process(&mut buffer, &ctx);
        // Values within stability ceiling pass through exactly
        assert_eq!(buffer[0], 2.0);
        assert_eq!(buffer[1], -3.0);
        assert_eq!(buffer[2], 0.5);
        assert_eq!(buffer[3], -0.1);
    }

    /// Measurement mode still enforces the ±100.0 stability ceiling and
    /// sanitizes NaN/Inf to prevent DAC damage.
    #[test]
    fn measurement_mode_sanitizes_extreme_values() {
        let (layout, listener) = make_ctx();
        let environment = test_environment();
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            environment: &environment,
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            measurement_mode: true,
        };
        let mut buffer = vec![200.0, -500.0, f32::NAN, f32::INFINITY];
        let mut stage = MasterGainStage;
        stage.process(&mut buffer, &ctx);
        assert_eq!(buffer[0], 100.0, "clamped to +100");
        assert_eq!(buffer[1], -100.0, "clamped to -100");
        assert_eq!(buffer[2], 0.0, "NaN → 0");
        assert_eq!(buffer[3], 0.0, "Inf → 0");
    }

    /// Energy scaling is linear in measurement mode: doubling the gain
    /// doubles the output (within the stability ceiling).
    #[test]
    fn measurement_mode_linear_energy_scaling() {
        let (layout, listener) = make_ctx();
        let environment = test_environment();
        let base_ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            environment: &environment,
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            measurement_mode: true,
        };
        let input = 0.4f32;

        // Gain = 1.0
        let mut buf1 = vec![input; 2];
        let mut stage = MasterGainStage;
        stage.process(&mut buf1, &base_ctx);

        // Gain = 2.0
        let ctx2 = MixContext {
            master_gain: 2.0,
            ..base_ctx
        };
        let mut buf2 = vec![input; 2];
        stage.process(&mut buf2, &ctx2);

        let ratio = buf2[0] / buf1[0];
        assert!(
            (ratio - 2.0).abs() < 1e-6,
            "doubling gain should double output, ratio = {ratio}"
        );
    }

    // ── soft_clip curve ─────────────────────────────────────────────────────

    /// Run `MasterGainStage` over `input` and return the output.
    fn run_stage(input: &[f32], master_gain: f32, measurement_mode: bool) -> Vec<f32> {
        let (layout, listener) = make_ctx();
        let environment = test_environment();
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            environment: &environment,
            master_gain,
            render_channels: 2,
            reverb_input: None,
            measurement_mode,
        };
        let mut buffer = input.to_vec();
        MasterGainStage.process(&mut buffer, &ctx);
        buffer
    }

    /// Below the 0.9 knee the curve is the identity, bit for bit.
    #[test]
    fn soft_clip_is_identity_below_knee() {
        for step in -900..=900 {
            let x = step as f32 / 1000.0;
            assert_eq!(soft_clip(x), x, "x = {x}");
        }
    }

    /// Closed form above the knee: 0.9 + 0.1·e/(1+e), e = (x−0.9)/0.1.
    /// x = 1.0 → e = 1 → 0.95; x = 1.9 → e = 10 → 0.9 + 1/11.
    #[test]
    fn soft_clip_matches_closed_form_above_knee() {
        assert!((soft_clip(1.0) - 0.95).abs() < 1e-6);
        assert!((soft_clip(1.9) - (0.9 + 0.1 * 10.0 / 11.0)).abs() < 1e-6);
        assert!((soft_clip(-1.0) + 0.95).abs() < 1e-6);
        assert!((soft_clip(-1.9) + (0.9 + 0.1 * 10.0 / 11.0)).abs() < 1e-6);
    }

    /// Continuity and unity slope (C1) at both knees: no step and no kink,
    /// so the transition from linear to compressed is click-free.
    #[test]
    fn soft_clip_is_continuous_with_unity_slope_at_knee() {
        let epsilon = 1e-3f32;
        for sign in [1.0f32, -1.0] {
            let knee = 0.9 * sign;
            assert!((soft_clip(knee) - knee).abs() < 1e-7, "value at knee");
            let just_above = knee + sign * epsilon;
            let just_below = knee - sign * epsilon;
            let slope_outside = (soft_clip(just_above) - soft_clip(knee)) / (sign * epsilon);
            let slope_inside = (soft_clip(knee) - soft_clip(just_below)) / (sign * epsilon);
            assert!(
                (slope_inside - 1.0).abs() < 1e-3,
                "inside slope {slope_inside}"
            );
            // Analytic slope just outside is 1/(1+e)², e = 0.01 → 0.980.
            assert!(
                (slope_outside - 1.0).abs() < 0.03,
                "outside slope {slope_outside}"
            );
            assert!(slope_outside < 1.0, "compression must start at the knee");
        }
        // Unity slope at the origin.
        assert!((soft_clip(1e-4) / 1e-4 - 1.0).abs() < 1e-6);
    }

    /// Output never exceeds ±1.0 for any finite input up to 1e30, is odd, and is
    /// monotonic non-decreasing, so loud peaks can never overshoot full scale.
    #[test]
    fn soft_clip_is_bounded_odd_and_monotonic() {
        let mut previous = soft_clip(-1.0e30);
        let mut x = -1.0e30f32;
        // Log-spaced sweep through both signs, plus a dense linear sweep.
        let mut magnitudes: Vec<f32> = Vec::new();
        let mut magnitude = 1.0e-6f32;
        while magnitude < 1.0e30 {
            magnitudes.push(magnitude);
            magnitude *= 1.01;
        }
        for step in 0..=40_000 {
            magnitudes.push(step as f32 * 0.001);
        }
        magnitudes.sort_by(|a, b| a.partial_cmp(b).unwrap());
        let mut inputs: Vec<f32> = magnitudes.iter().rev().map(|m| -m).collect();
        inputs.extend(magnitudes.iter().copied());
        for &input in &inputs {
            let output = soft_clip(input);
            assert!(
                output.abs() <= 1.0,
                "soft_clip({input}) = {output} exceeds 1.0"
            );
            assert_eq!(soft_clip(-input), -output, "oddness at {input}");
            // One f32 ulp near 1.0 (6e-8) of rounding noise is tolerated: far above
            // the knee e/(1+e) is evaluated within a few ulp of 1.
            assert!(
                output >= previous - 1.2e-7,
                "not monotonic at {input}: {output} < {previous}"
            );
            previous = output;
            x = input;
        }
        assert!(x > 0.0);
        assert_eq!(soft_clip(1.0e30), 1.0);
        assert_eq!(soft_clip(-1.0e30), -1.0);
    }

    /// Strictly increasing (invertible, no flat spot) through the knee region
    /// where the curve is still resolvable in f32.
    #[test]
    fn soft_clip_is_strictly_increasing_near_knee() {
        let mut previous = soft_clip(0.8);
        let mut input = 0.8f32;
        while input < 3.0 {
            input += 0.001;
            let output = soft_clip(input);
            assert!(output > previous, "flat at {input}");
            previous = output;
        }
    }

    // ── gain staging ────────────────────────────────────────────────────────

    /// Linear region: output = gain × input, and dB conversion is 20·log10
    /// (2× = +6.0206 dB, 0.5× = −6.0206 dB, 0.1× = −20 dB).
    #[test]
    fn gain_scales_linearly_below_the_knee() {
        let output = run_stage(&[0.4, -0.4, 0.2, 0.0], 2.0, false);
        assert_eq!(output, vec![0.8, -0.8, 0.4, 0.0]);
        let ratio_db = 20.0 * (output[0] / 0.4).log10();
        assert!((ratio_db - 6.0206).abs() < 1e-3);
        let output = run_stage(&[0.5], 0.1, false);
        assert!((20.0 * (output[0] / 0.5).log10() + 20.0).abs() < 1e-4);
        // Zero gain mutes, negative gain inverts polarity.
        assert_eq!(run_stage(&[0.7, -0.7], 0.0, false), vec![0.0, 0.0]);
        assert_eq!(run_stage(&[0.3], -1.0, false), vec![-0.3]);
    }

    /// Gain is applied *before* the clip: a 0.6 input at gain 2 enters the
    /// knee region (1.2 → ≈ 0.9 + 0.1·3/4 = 0.975) rather than being clipped
    /// at 0.6 and then scaled.
    #[test]
    fn gain_is_applied_before_clipping() {
        let output = run_stage(&[0.6], 2.0, false);
        let expected = 0.9 + 0.1 * (0.3 / 0.1) / (1.0 + 0.3 / 0.1);
        assert!(
            (output[0] - expected).abs() < 1e-6,
            "{} vs {expected}",
            output[0]
        );
    }

    /// In normal mode no sample can exceed ±1.0 for any finite gain-scaled input.
    #[test]
    fn normal_mode_output_never_exceeds_full_scale() {
        let input: Vec<f32> = (0..2000)
            .map(|index| (index as f32 - 1000.0) * 0.37)
            .collect();
        for master_gain in [0.5f32, 1.0, 4.0, 100.0] {
            let output = run_stage(&input, master_gain, false);
            assert!(output.iter().all(|sample| sample.abs() <= 1.0));
        }
    }

    /// Measurement mode applies the gain linearly up to the ±100 ceiling.
    #[test]
    fn measurement_mode_ceiling_applies_after_gain() {
        let output = run_stage(&[60.0, -60.0, 1.0], 2.0, true);
        assert_eq!(output, vec![100.0, -100.0, 2.0]);
    }

    /// Normal (default) mode must not forward NaN/Inf to the output device.
    /// `soft_clip(+Inf)` used to evaluate Inf/(1+Inf) = NaN and `soft_clip(NaN)`
    /// returned NaN, so a single non-finite sample upstream (or an overflow of
    /// `sample * gain`) reached the DAC. Now NaN → 0 and ±Inf → ±1.0.
    #[test]
    fn normal_mode_must_not_emit_non_finite_samples() {
        let output = run_stage(
            &[f32::NAN, f32::INFINITY, f32::NEG_INFINITY, 3.0e38],
            1.0,
            false,
        );
        assert!(
            output
                .iter()
                .all(|sample| sample.is_finite() && sample.abs() <= 1.0),
            "non-finite output: {output:?}"
        );
        // NaN → silence; ±Inf and overflowing finite input → the ±1.0 ceiling.
        assert_eq!(output, vec![0.0, 1.0, -1.0, 1.0]);
        // A finite sample that only overflows after the gain is applied.
        assert_eq!(run_stage(&[-3.0e30], 1.0e10, false), vec![-1.0]);
    }
}
