//! LFE bass management with Linkwitz-Riley 4th-order (LR4) crossover at 120 Hz.
//!
//! When the layout has an LFE channel:
//! - LFE receives LR4 lowpass (two cascaded Butterworth 2nd-order LP sections)
//! - All non-LFE channels receive LR4 highpass (two cascaded Butterworth HP sections)
//! - Bass content removed from mains is redirected (summed) into the LFE channel
//!
//! LR4's reconstruction property: LP(f) + HP(f) is an *allpass* — flat
//! magnitude at all frequencies, with LP and HP mutually in phase. Each branch
//! is -6 dB at crossover, so their sum is 0 dB. Note LP + HP ≠ 1 as a transfer
//! function (the sum carries the allpass phase), which is why the bass redirect
//! uses an explicit parallel lowpass rather than `original - highpassed`.
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
    /// Parallel LR4 lowpass per non-LFE channel, for the bass redirect.
    ///
    /// The redirected bass must be a *true* LR4 lowpass of each main channel.
    /// `original - highpassed` is NOT that: LP4 + HP4 sums to an allpass, not
    /// to unity, so `1 - HP4` equals `LP4 + (1 - allpass)` — a term that peaks
    /// at 1.5× at the crossover (vs the correct 0.5×) and leaks midrange into
    /// the LFE at only −9 dB @ 1 kHz.
    main_lowpass: [Option<Lr4Filter>; MAX_CHANNELS],
    /// Cached LFE channel index.
    lfe_channel: Option<usize>,
}

impl Default for LfeBassManagementStage {
    fn default() -> Self {
        Self {
            lfe_lowpass: None,
            main_highpass: std::array::from_fn(|_| None),
            main_lowpass: std::array::from_fn(|_| None),
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
                self.main_lowpass = std::array::from_fn(|_| None);
                return;
            }
        };

        self.lfe_channel = Some(lfe);
        self.lfe_lowpass = Some(Lr4Filter::lowpass(LFE_CUTOFF_HZ, ctx.sample_rate));

        // Create HP + parallel LP filters for all non-LFE channels.
        self.main_highpass = std::array::from_fn(|ch| {
            if ch < ctx.channels && ch != lfe {
                Some(Lr4Filter::highpass(LFE_CUTOFF_HZ, ctx.sample_rate))
            } else {
                None
            }
        });
        self.main_lowpass = std::array::from_fn(|ch| {
            if ch < ctx.channels && ch != lfe {
                Some(Lr4Filter::lowpass(LFE_CUTOFF_HZ, ctx.sample_rate))
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

            // Sum bass content redirected from main channels: a true parallel
            // LR4 lowpass per channel. All LR4 branches share the same phase
            // response, so the redirected bass from multiple mains (and the
            // lowpassed LFE input) sums coherently.
            let mut bass_sum = 0.0f32;
            for ch in 0..channels.min(MAX_CHANNELS) {
                let (Some(hp), Some(lp)) =
                    (&mut self.main_highpass[ch], &mut self.main_lowpass[ch])
                else {
                    continue;
                };
                let idx = base + ch;
                let original = buffer[idx];
                buffer[idx] = hp.process(original);
                bass_sum += lp.process(original);
            }

            // LFE channel: lowpass existing LFE content, then add redirected bass
            // (already LR4-lowpassed above — it bypasses the LFE LP to avoid
            // double-filtering).
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
        for f in self.main_lowpass.iter_mut().flatten() {
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

    /// Steady-state amplitude of a sine on the given channel over the last
    /// half of the buffer (RMS × √2).
    fn steady_state_amplitude(buffer: &[f32], channels: usize, ch: usize) -> f32 {
        let num_frames = buffer.len() / channels;
        let start = num_frames / 2;
        let mean_square: f32 = (start..num_frames)
            .map(|f| {
                let s = buffer[f * channels + ch];
                s * s
            })
            .sum::<f32>()
            / (num_frames - start) as f32;
        (2.0 * mean_square).sqrt()
    }

    /// Run a single-frequency sine (amplitude 1.0) on channel 0 through the
    /// stage and return (main_ch0_amplitude, lfe_amplitude) at steady state.
    fn run_sine_through_crossover(freq: f32) -> (f32, f32) {
        let layout = surround_51_layout();
        let listener = Listener::new(Vec3::ZERO, 0.0);
        let ctx = test_mix_context(&layout, &listener);
        let mut stage = LfeBassManagementStage::new();
        stage.init(&ctx);

        let channels = 6;
        let num_frames = 16384;
        let mut buffer = vec![0.0f32; channels * num_frames];
        for frame in 0..num_frames {
            let t = frame as f32 / SAMPLE_RATE;
            buffer[frame * channels] = (2.0 * std::f32::consts::PI * freq * t).sin();
        }
        stage.process(&mut buffer, &ctx);

        (
            steady_state_amplitude(&buffer, channels, 0),
            steady_state_amplitude(&buffer, channels, 3),
        )
    }

    /// LR4 reconstruction: |HP(f)| + |LP(f)| contributions must sum to flat
    /// magnitude at every frequency. LR4's property is an *allpass* sum —
    /// flat magnitude with phase rotation — so this is asserted per frequency
    /// on steady-state sine amplitude, NOT as time-domain waveform identity.
    /// (The old formulation `redirect = original − HP` made the time-domain
    /// identity hold tautologically while injecting +9.5 dB excess bass at
    /// the crossover.)
    #[test]
    fn crossover_sum_is_magnitude_flat_per_frequency() {
        for freq in [40.0, 80.0, 120.0, 240.0, 1000.0, 4000.0] {
            let (main, lfe) = run_sine_through_crossover(freq);
            // LP and HP branches are phase-coherent (that's the point of LR4),
            // so amplitudes of ch0 (HP) + ch3 (LP redirect) add in phase.
            let total = main + lfe;
            assert!(
                (total - 1.0).abs() < 0.06,
                "{freq} Hz: |HP| + |LP| = {total:.3}, expected ~1.0 (main={main:.3}, lfe={lfe:.3})"
            );
        }
    }

    /// At the 120 Hz crossover each LR4 branch is exactly −6 dB (0.5×).
    /// The redirected bass must arrive at 0.5×, not the 1.5× produced by the
    /// old `original − highpassed` formulation.
    #[test]
    fn redirected_bass_at_crossover_is_minus_6_db() {
        let (main, lfe) = run_sine_through_crossover(120.0);
        assert!(
            (lfe - 0.5).abs() < 0.03,
            "120 Hz redirected bass should be ~0.5× (−6 dB), got {lfe:.3}"
        );
        assert!(
            (main - 0.5).abs() < 0.03,
            "120 Hz highpassed main should be ~0.5× (−6 dB), got {main:.3}"
        );
    }

    /// Midrange must not leak into the subwoofer. A true LR4 lowpass is
    /// ~−73 dB at 1 kHz; the old formulation leaked 1 kHz into the LFE at
    /// only −9.4 dB (0.34×).
    #[test]
    fn midrange_does_not_leak_into_lfe() {
        let (_, lfe) = run_sine_through_crossover(1000.0);
        assert!(
            lfe < 0.01,
            "1 kHz content on a main channel should stay out of the LFE, got {lfe:.4}"
        );
    }
}
