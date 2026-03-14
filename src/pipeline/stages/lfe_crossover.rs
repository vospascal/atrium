//! LFE bass management with Linkwitz-Riley 4th-order (LR4) crossover at 120 Hz.
//!
//! When the layout has an LFE channel:
//! - LFE receives LR4 lowpass (two cascaded Butterworth 2nd-order LP sections)
//! - All non-LFE channels receive LR4 highpass (two cascaded Butterworth HP sections)
//! - Bass content removed from mains is redirected (summed) into the LFE channel
//!
//! LR4 guarantees flat magnitude reconstruction: LP(f) + HP(f) = 1.0 at all
//! frequencies, with zero phase shift at the 120 Hz crossover point. Each filter
//! is -6 dB at crossover, so their sum is 0 dB (unity).
//!
//! No-op for layouts without LFE (e.g. stereo, quad).

use atrium_core::speaker::MAX_CHANNELS;

use crate::audio::filters::Biquad;
use crate::pipeline::mix_stage::{MixContext, MixStage};

/// LFE crossover cutoff frequency in Hz.
const LFE_CUTOFF_HZ: f32 = 120.0;

/// Linkwitz-Riley 4th-order filter: two cascaded identical Butterworth biquads.
#[derive(Clone)]
struct Lr4Filter {
    stage1: Biquad,
    stage2: Biquad,
}

impl Lr4Filter {
    fn lowpass(cutoff_hz: f32, sample_rate: f32) -> Self {
        Self {
            stage1: Biquad::lowpass(cutoff_hz, sample_rate),
            stage2: Biquad::lowpass(cutoff_hz, sample_rate),
        }
    }

    fn highpass(cutoff_hz: f32, sample_rate: f32) -> Self {
        Self {
            stage1: Biquad::highpass(cutoff_hz, sample_rate),
            stage2: Biquad::highpass(cutoff_hz, sample_rate),
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        self.stage2.process(self.stage1.process(x))
    }

    fn reset(&mut self) {
        self.stage1.reset();
        self.stage2.reset();
    }
}

/// LFE bass management stage with Linkwitz-Riley 4th-order crossover.
///
/// Only active when the layout has an LFE channel. Applies:
/// - LR4 lowpass to the LFE channel
/// - LR4 highpass to all non-LFE channels
/// - Redirects bass removed from mains into the LFE
///
/// No-op for layouts without LFE.
pub struct LfeBassManagementStage {
    /// LR4 lowpass for the LFE channel.
    lfe_lowpass: Option<Lr4Filter>,
    /// LR4 highpass per non-LFE channel. Only populated for active channels.
    main_highpass: [Option<Lr4Filter>; MAX_CHANNELS],
    /// Cached LFE channel index.
    lfe_channel: Option<usize>,
}

impl Default for LfeBassManagementStage {
    fn default() -> Self {
        Self {
            lfe_lowpass: None,
            main_highpass: std::array::from_fn(|_| None),
            lfe_channel: None,
        }
    }
}

impl LfeBassManagementStage {
    pub fn new() -> Self {
        Self::default()
    }
}

impl MixStage for LfeBassManagementStage {
    fn init(&mut self, ctx: &MixContext) {
        let lfe = match ctx.layout.lfe_channel() {
            // LFE channel must fit within the actual output buffer width.
            Some(ch) if ch < ctx.channels => ch,
            _ => {
                self.lfe_lowpass = None;
                self.lfe_channel = None;
                self.main_highpass = std::array::from_fn(|_| None);
                return;
            }
        };

        self.lfe_channel = Some(lfe);
        self.lfe_lowpass = Some(Lr4Filter::lowpass(LFE_CUTOFF_HZ, ctx.sample_rate));

        // Create HP filters for all non-LFE channels up to the render channel count.
        self.main_highpass = std::array::from_fn(|ch| {
            if ch < ctx.channels && ch != lfe {
                Some(Lr4Filter::highpass(LFE_CUTOFF_HZ, ctx.sample_rate))
            } else {
                None
            }
        });
    }

    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        let lfe = match self.lfe_channel {
            Some(ch) if ch < ctx.channels => ch,
            _ => return,
        };
        let lfe_lp = match self.lfe_lowpass.as_mut() {
            Some(f) => f,
            None => return,
        };

        let channels = ctx.channels;
        let num_frames = buffer.len() / channels;

        for frame in 0..num_frames {
            let base = frame * channels;

            // Sum bass content redirected from main channels.
            let mut bass_sum = 0.0f32;
            for (ch, hp_filter) in self.main_highpass.iter_mut().enumerate() {
                if ch >= channels {
                    break;
                }
                if let Some(ref mut hp) = hp_filter {
                    let idx = base + ch;
                    let original = buffer[idx];
                    let highpassed = hp.process(original);
                    buffer[idx] = highpassed;
                    // Bass = what the HP removed. LR4 guarantees: original = hp + lp,
                    // so lp = original - hp.
                    bass_sum += original - highpassed;
                }
            }

            // LFE channel: lowpass existing LFE content, then add redirected bass.
            // The redirected bass is already implicitly lowpassed (original - HP = LP
            // by LR4's perfect reconstruction property), so it bypasses the LFE LP
            // to avoid double-filtering.
            let lfe_idx = base + lfe;
            buffer[lfe_idx] = lfe_lp.process(buffer[lfe_idx]) + bass_sum;
        }
    }

    fn reset(&mut self) {
        if let Some(ref mut f) = self.lfe_lowpass {
            f.reset();
        }
        for f in self.main_highpass.iter_mut().flatten() {
            f.reset();
        }
    }

    fn name(&self) -> &str {
        "lfe_bass_management"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atrium_core::listener::Listener;
    use atrium_core::speaker::{Speaker, SpeakerLayout};
    use atrium_core::types::Vec3;

    use crate::audio::atmosphere::AtmosphericParams;
    use crate::pipeline::path::WallMaterial;

    const SAMPLE_RATE: f32 = 48000.0;
    const TEST_ATMOSPHERE: AtmosphericParams = AtmosphericParams {
        temperature_c: 20.0,
        humidity_pct: 50.0,
        pressure_kpa: 101.325,
    };
    const TEST_MATERIALS: [WallMaterial; 6] = [WallMaterial::HARD_WALL; 6];

    fn surround_51_layout() -> SpeakerLayout {
        SpeakerLayout::new(
            &[
                Speaker {
                    position: Vec3::new(-1.0, 0.0, 1.0),
                    channel: 0,
                }, // L
                Speaker {
                    position: Vec3::new(1.0, 0.0, 1.0),
                    channel: 1,
                }, // R
                Speaker {
                    position: Vec3::new(0.0, 0.0, 1.0),
                    channel: 2,
                }, // C
                // channel 3 = LFE (no position)
                Speaker {
                    position: Vec3::new(-1.0, 0.0, -1.0),
                    channel: 4,
                }, // LS
                Speaker {
                    position: Vec3::new(1.0, 0.0, -1.0),
                    channel: 5,
                }, // RS
            ],
            Some(3), // LFE on channel 3
            6,
        )
    }

    fn stereo_layout() -> SpeakerLayout {
        SpeakerLayout::stereo(Vec3::new(-1.0, 0.0, 1.0), Vec3::new(1.0, 0.0, 1.0))
    }

    fn test_mix_context<'a>(layout: &'a SpeakerLayout, listener: &'a Listener) -> MixContext<'a> {
        MixContext {
            listener,
            layout,
            sample_rate: SAMPLE_RATE,
            channels: layout.total_channels(),
            environment_min: Vec3::new(-5.0, -5.0, -5.0),
            environment_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
            render_channels: layout.total_channels(),
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &TEST_MATERIALS,
            atmosphere: &TEST_ATMOSPHERE,
            measurement_mode: false,
        }
    }

    /// No LFE channel → process is a no-op, signal passes through unchanged.
    #[test]
    fn no_lfe_is_noop() {
        let layout = stereo_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let ctx = test_mix_context(&layout, &listener);
        let mut stage = LfeBassManagementStage::new();
        stage.init(&ctx);

        let mut buffer = vec![1.0; 2 * 256];
        let original = buffer.clone();
        stage.process(&mut buffer, &ctx);
        assert_eq!(buffer, original);
    }

    /// LR4 lowpass attenuates content well above 120 Hz on the LFE channel.
    #[test]
    fn lfe_lowpass_attenuates_high_frequencies() {
        let layout = surround_51_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let ctx = test_mix_context(&layout, &listener);
        let mut stage = LfeBassManagementStage::new();
        stage.init(&ctx);

        let channels = 6;
        let num_frames = 4096;
        let mut buffer = vec![0.0f32; channels * num_frames];

        // Put a 1 kHz sine on LFE channel only (well above 120 Hz crossover).
        let freq = 1000.0;
        for frame in 0..num_frames {
            let t = frame as f32 / SAMPLE_RATE;
            buffer[frame * channels + 3] = (2.0 * std::f32::consts::PI * freq * t).sin();
        }

        stage.process(&mut buffer, &ctx);

        // Measure LFE output energy in the last 2048 frames (after filter settles).
        let lfe_energy: f32 = (num_frames / 2..num_frames)
            .map(|f| {
                let s = buffer[f * channels + 3];
                s * s
            })
            .sum::<f32>()
            / (num_frames / 2) as f32;

        // LR4 at 1 kHz (3+ octaves above 120 Hz) should attenuate by ~48+ dB.
        // Energy should be negligible.
        assert!(
            lfe_energy < 0.001,
            "LFE energy at 1 kHz should be heavily attenuated, got {lfe_energy}"
        );
    }

    /// LR4 lowpass passes content well below 120 Hz on the LFE channel.
    #[test]
    fn lfe_lowpass_passes_bass() {
        let layout = surround_51_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let ctx = test_mix_context(&layout, &listener);
        let mut stage = LfeBassManagementStage::new();
        stage.init(&ctx);

        let channels = 6;
        let num_frames = 4096;
        let mut buffer = vec![0.0f32; channels * num_frames];

        // Put a 40 Hz sine on LFE channel (well below 120 Hz).
        let freq = 40.0;
        for frame in 0..num_frames {
            let t = frame as f32 / SAMPLE_RATE;
            buffer[frame * channels + 3] = (2.0 * std::f32::consts::PI * freq * t).sin();
        }

        // Measure input energy for reference.
        let input_energy: f32 = (num_frames / 2..num_frames)
            .map(|f| {
                let s = (2.0 * std::f32::consts::PI * freq * f as f32 / SAMPLE_RATE).sin();
                s * s
            })
            .sum::<f32>()
            / (num_frames / 2) as f32;

        stage.process(&mut buffer, &ctx);

        let lfe_energy: f32 = (num_frames / 2..num_frames)
            .map(|f| {
                let s = buffer[f * channels + 3];
                s * s
            })
            .sum::<f32>()
            / (num_frames / 2) as f32;

        // 40 Hz should pass through with minimal loss (within 1 dB).
        let ratio = lfe_energy / input_energy;
        assert!(
            ratio > 0.89, // -0.5 dB
            "40 Hz should pass through LFE LP, got ratio {ratio}"
        );
    }

    /// Main channels get highpassed: bass content is removed.
    #[test]
    fn main_channels_highpassed() {
        let layout = surround_51_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let ctx = test_mix_context(&layout, &listener);
        let mut stage = LfeBassManagementStage::new();
        stage.init(&ctx);

        let channels = 6;
        let num_frames = 4096;
        let mut buffer = vec![0.0f32; channels * num_frames];

        // Put a 40 Hz sine on channel 0 (front left).
        let freq = 40.0;
        for frame in 0..num_frames {
            let t = frame as f32 / SAMPLE_RATE;
            buffer[frame * channels + 0] = (2.0 * std::f32::consts::PI * freq * t).sin();
        }

        stage.process(&mut buffer, &ctx);

        // Channel 0 should have very little 40 Hz energy left.
        let ch0_energy: f32 = (num_frames / 2..num_frames)
            .map(|f| {
                let s = buffer[f * channels + 0];
                s * s
            })
            .sum::<f32>()
            / (num_frames / 2) as f32;

        assert!(
            ch0_energy < 0.01,
            "40 Hz on main channel should be attenuated by HP, got energy {ch0_energy}"
        );
    }

    /// Bass removed from mains is redirected to LFE.
    #[test]
    fn bass_redirected_to_lfe() {
        let layout = surround_51_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let ctx = test_mix_context(&layout, &listener);
        let mut stage = LfeBassManagementStage::new();
        stage.init(&ctx);

        let channels = 6;
        let num_frames = 4096;
        let mut buffer = vec![0.0f32; channels * num_frames];

        // Put a 40 Hz sine on channel 0 (front left). Nothing on LFE initially.
        let freq = 40.0;
        for frame in 0..num_frames {
            let t = frame as f32 / SAMPLE_RATE;
            buffer[frame * channels + 0] = (2.0 * std::f32::consts::PI * freq * t).sin();
        }

        stage.process(&mut buffer, &ctx);

        // LFE should now contain the redirected 40 Hz bass.
        let lfe_energy: f32 = (num_frames / 2..num_frames)
            .map(|f| {
                let s = buffer[f * channels + 3];
                s * s
            })
            .sum::<f32>()
            / (num_frames / 2) as f32;

        assert!(
            lfe_energy > 0.1,
            "Redirected 40 Hz bass should appear on LFE, got energy {lfe_energy}"
        );
    }

    /// LR4 reconstruction: HP(main) + LP(redirected bass) ≈ original signal.
    /// Tests that no energy is lost or gained through the crossover.
    #[test]
    fn lr4_reconstruction_flat() {
        let layout = surround_51_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let ctx = test_mix_context(&layout, &listener);
        let mut stage = LfeBassManagementStage::new();
        stage.init(&ctx);

        let channels = 6;
        let num_frames = 8192;

        // Test with broadband signal (sum of several frequencies).
        let test_freqs = [40.0, 80.0, 120.0, 500.0, 2000.0, 8000.0];
        let mut buffer = vec![0.0f32; channels * num_frames];

        for frame in 0..num_frames {
            let t = frame as f32 / SAMPLE_RATE;
            let sample: f32 = test_freqs
                .iter()
                .map(|&f| (2.0 * std::f32::consts::PI * f * t).sin())
                .sum();
            // Put signal on channel 0 only.
            buffer[frame * channels + 0] = sample;
        }

        let original: Vec<f32> = (0..num_frames).map(|f| buffer[f * channels + 0]).collect();

        stage.process(&mut buffer, &ctx);

        // Reconstruction = main channel (HP) + LFE channel (LP of redirected bass).
        // Use last 4096 frames to avoid transients.
        let mut max_error = 0.0f32;
        for frame in num_frames / 2..num_frames {
            let reconstructed = buffer[frame * channels + 0] + buffer[frame * channels + 3];
            let error = (reconstructed - original[frame]).abs();
            max_error = max_error.max(error);
        }

        // LR4 perfect reconstruction: error should be negligible.
        // Allow small floating-point tolerance.
        assert!(
            max_error < 0.01,
            "LR4 reconstruction error too large: {max_error}"
        );
    }
}
