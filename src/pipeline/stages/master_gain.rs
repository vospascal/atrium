//! Applies runtime master gain from AudioScene.
//!
//! Stateless — reads `ctx.master_gain` each buffer. AudioScene is the
//! single source of truth; this stage just multiplies and soft-clips.
//! In measurement mode, bypasses soft clipping to preserve linear signal
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
    use crate::pipeline::path::WallMaterial;
    use atrium_core::listener::Listener;
    use atrium_core::speaker::SpeakerLayout;
    use atrium_core::types::Vec3;

    const TEST_MATERIALS: [WallMaterial; 6] = [WallMaterial::HARD_WALL; 6];

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
        let atmosphere = AtmosphericParams::default();
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &TEST_MATERIALS,
            atmosphere: &atmosphere,
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
        let atmosphere = AtmosphericParams::default();
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &TEST_MATERIALS,
            atmosphere: &atmosphere,
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
        let atmosphere = AtmosphericParams::default();
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &TEST_MATERIALS,
            atmosphere: &atmosphere,
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
        let atmosphere = AtmosphericParams::default();
        let base_ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &TEST_MATERIALS,
            atmosphere: &atmosphere,
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
}
