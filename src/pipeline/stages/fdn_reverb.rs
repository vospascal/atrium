//! FDN (Feedback Delay Network) late reverb.
//!
//! Based on Jot & Chaigne (1991). 8 parallel delay lines coupled through an
//! 8×8 Hadamard mixing matrix, with one-pole lowpass damping per line for
//! frequency-dependent decay (highs fade faster than lows).
//!
//! Chain order: Direct (0–3ms) → EarlyReflections (3–50ms) → FdnReverb (50ms+)

use crate::pipeline::mix_stage::{MixContext, MixStage};
use crate::pipeline::room_acoustics;
use crate::pipeline::stages::soft_clip;

const NUM_LINES: usize = 8;
const MAX_OUT: usize = 8;
const BUF_SIZE: usize = 512;
const BUF_MASK: usize = BUF_SIZE - 1;
const PRE_DELAY_BUF_SIZE: usize = 2048;
const PRE_DELAY_BUF_MASK: usize = PRE_DELAY_BUF_SIZE - 1;
const PRE_DELAY_SECONDS: f32 = 0.020;
const BASE_DELAYS: [usize; NUM_LINES] = [241, 307, 353, 389, 421, 433, 461, 499];
const BASE_SAMPLE_RATE: f32 = 48000.0;

#[derive(Clone, Copy)]
struct DampingFilter {
    k: f32,
    p: f32,
    state: f32,
}

impl DampingFilter {
    fn new() -> Self {
        Self {
            k: 0.0,
            p: 0.0,
            state: 0.0,
        }
    }

    #[inline(always)]
    fn process(&mut self, input: f32) -> f32 {
        let output = self.k * input + self.p * self.state;
        self.state = output;
        output
    }
}

/// Post-mix FDN late reverb stage.
///
/// Fully physics-based gain staging:
/// - **Send**: d²/(d²+d_c²) per source (bounded reverberant energy fraction)
/// - **Decay**: Sabine RT60 → Jot per-line damping gains
/// - **Output normalization**: derived from steady-state energy buildup of the
///   8-line feedback network (1/RMS of per-line 1/√(1-g²)), so the d/d_c send
///   law directly controls perceived wet level without an arbitrary wet knob.
///
/// High-frequency RT60 is set to 0.5× the low-frequency RT60, modeling
/// the natural air absorption and surface absorption that causes highs
/// to decay faster than lows in real rooms.
pub struct FdnReverbStage {
    delay_buffers: Box<[[f32; BUF_SIZE]; NUM_LINES]>,
    write_pos: usize,
    delays: [usize; NUM_LINES],
    damping: [DampingFilter; NUM_LINES],
    pre_delay_buf: Box<[f32; PRE_DELAY_BUF_SIZE]>,
    pre_delay_write_pos: usize,
    pre_delay_samples: usize,
    /// Structural output normalization derived from Jot damping gains.
    /// Compensates for the FDN's steady-state energy accumulation so that
    /// the d/d_c send law directly controls the perceived wet level.
    output_normalization: f32,
    initialized: bool,
}

/// High-frequency RT60 ratio relative to low-frequency RT60.
/// 0.5 means highs decay twice as fast as lows — a typical ratio
/// for rooms with moderate air absorption and surface absorption.
const RT60_HIGH_RATIO: f32 = 0.5;

impl Default for FdnReverbStage {
    fn default() -> Self {
        Self::new()
    }
}

impl FdnReverbStage {
    pub fn new() -> Self {
        Self {
            delay_buffers: Box::new([[0.0; BUF_SIZE]; NUM_LINES]),
            write_pos: 0,
            delays: [0; NUM_LINES],
            damping: [DampingFilter::new(); NUM_LINES],
            pre_delay_buf: Box::new([0.0; PRE_DELAY_BUF_SIZE]),
            pre_delay_write_pos: 0,
            pre_delay_samples: 0,
            output_normalization: 1.0,
            initialized: false,
        }
    }

    fn compute_delays(&mut self, sample_rate: f32) {
        let scale = sample_rate / BASE_SAMPLE_RATE;
        for (i, &base_delay) in BASE_DELAYS.iter().enumerate() {
            let scaled = ((base_delay as f32) * scale) as usize;
            self.delays[i] = scaled.clamp(1, BUF_SIZE - 1);
        }
        self.pre_delay_samples =
            ((PRE_DELAY_SECONDS * sample_rate) as usize).min(PRE_DELAY_BUF_SIZE - 1);
    }

    /// Compute per-line one-pole damping filters for frequency-dependent decay,
    /// and derive structural output normalization from the loop gains.
    ///
    /// Each filter has gain `k` and pole `p` derived from the RT60 at DC and Nyquist:
    ///   g_dc  = 10^(-3m / (RT60_low  × fs))   — gain per sample at 0 Hz
    ///   g_nyq = 10^(-3m / (RT60_high × fs))   — gain per sample at Nyquist
    ///   k = 2·g_dc·g_nyq / (g_dc + g_nyq)     — filter gain (harmonic mean)
    ///   p = (g_dc - g_nyq) / (g_dc + g_nyq)   — pole location
    ///
    /// Output normalization uses g_eff = √((g_dc² + g_nyq²) / 2) per line —
    /// the RMS of the actual endpoint loop gains, not the filter coefficient `k`.
    /// Steady-state energy buildup per line is 1/(1 - g_eff²); RMS-averaged across
    /// lines (Hadamard is orthonormal, so lines don't add coherently).
    ///
    /// Reference: Jot, "Digital Delay Networks for Designing Artificial Reverberators",
    /// AES 90th Convention (1991), §3.2; extended in Jot's PhD thesis (1992), Ch. 4.
    fn compute_damping(&mut self, sample_rate: f32, rt60_low: f32, rt60_high: f32) {
        let mut sum_norm_sq = 0.0f32;

        for i in 0..NUM_LINES {
            let m = self.delays[i] as f32;
            let g_dc = 10.0_f32.powf(-3.0 * m / (rt60_low * sample_rate));
            let g_nyq = 10.0_f32.powf(-3.0 * m / (rt60_high * sample_rate));
            let sum = g_dc + g_nyq;
            if sum < f32::EPSILON {
                self.damping[i].k = 0.0;
                self.damping[i].p = 0.0;
            } else {
                self.damping[i].k = 2.0 * g_dc * g_nyq / sum;
                self.damping[i].p = (g_dc - g_nyq) / sum;
            }
            self.damping[i].state = 0.0;

            // Effective broadband loop gain: RMS of DC and Nyquist gains.
            // This is the correct quantity for energy buildup estimation —
            // unlike k (harmonic mean), g_eff represents the actual per-sample
            // loop gain averaged across the spectrum.
            let g_eff_sq = (g_dc * g_dc + g_nyq * g_nyq) * 0.5;
            let norm_sq = if g_eff_sq < 0.9999 {
                1.0 / (1.0 - g_eff_sq)
            } else {
                10000.0
            };
            sum_norm_sq += norm_sq;
        }

        // Network-level gain: RMS across lines (Hadamard is orthonormal).
        let rms_gain = (sum_norm_sq / NUM_LINES as f32).sqrt();
        self.output_normalization = (1.0 / rms_gain).clamp(0.01, 1.0);
    }

    #[inline(always)]
    fn hadamard_8(v: &mut [f32; NUM_LINES]) {
        for i in (0..8).step_by(2) {
            let a = v[i];
            let b = v[i + 1];
            v[i] = a + b;
            v[i + 1] = a - b;
        }
        for i in (0..8).step_by(4) {
            let (a0, a1) = (v[i], v[i + 1]);
            let (b0, b1) = (v[i + 2], v[i + 3]);
            v[i] = a0 + b0;
            v[i + 1] = a1 + b1;
            v[i + 2] = a0 - b0;
            v[i + 3] = a1 - b1;
        }
        let (a0, a1, a2, a3) = (v[0], v[1], v[2], v[3]);
        let (b0, b1, b2, b3) = (v[4], v[5], v[6], v[7]);
        v[0] = a0 + b0;
        v[1] = a1 + b1;
        v[2] = a2 + b2;
        v[3] = a3 + b3;
        v[4] = a0 - b0;
        v[5] = a1 - b1;
        v[6] = a2 - b2;
        v[7] = a3 - b3;
        let scale = 1.0 / (NUM_LINES as f32).sqrt();
        for x in v.iter_mut() {
            *x *= scale;
        }
    }

    #[allow(clippy::needless_range_loop)]
    #[inline(always)]
    fn process_fdn_sample(&mut self, mono_in: f32, out_channels: usize) -> [f32; MAX_OUT] {
        let mut output = [0.0f32; MAX_OUT];

        let mut taps = [0.0_f32; NUM_LINES];
        for i in 0..NUM_LINES {
            let read_pos = (self.write_pos + BUF_SIZE - self.delays[i]) & BUF_MASK;
            taps[i] = self.delay_buffers[i][read_pos];
        }

        for i in 0..NUM_LINES {
            taps[i] = self.damping[i].process(taps[i]);
        }

        let ch_count = out_channels.clamp(1, MAX_OUT);
        for i in 0..NUM_LINES {
            output[i % ch_count] += taps[i];
        }
        let lines_per_ch = NUM_LINES.div_ceil(ch_count) as f32;
        for ch in 0..ch_count {
            output[ch] /= lines_per_ch;
        }

        Self::hadamard_8(&mut taps);

        let input_scale = 1.0 / (NUM_LINES as f32).sqrt();
        let scaled_input = mono_in * input_scale;
        for i in 0..NUM_LINES {
            self.delay_buffers[i][self.write_pos] = (taps[i] + scaled_input).clamp(-4.0, 4.0);
        }

        self.write_pos = (self.write_pos + 1) & BUF_MASK;
        output
    }
}

impl MixStage for FdnReverbStage {
    fn init(&mut self, ctx: &MixContext) {
        self.compute_delays(ctx.sample_rate);

        // Derive RT60 from room geometry via Sabine's equation.
        let (volume, surface_area) = room_acoustics::room_geometry(ctx.room_min, ctx.room_max);
        let rt60_low = room_acoustics::sabine_rt60(volume, surface_area, ctx.wall_reflectivity);
        let rt60_high = rt60_low * RT60_HIGH_RATIO;

        self.compute_damping(ctx.sample_rate, rt60_low, rt60_high);
        self.initialized = true;
    }

    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        if !self.initialized {
            return;
        }

        let channels = ctx.channels;
        let render_channels = ctx.render_channels;
        let num_frames = buffer.len() / channels;

        for frame in 0..num_frames {
            let base = frame * channels;

            // Read mono input from the reverb send buffer (pre-weighted by d/d_c)
            // if available, otherwise fall back to mono-summing the main buffer.
            // Reverb send is a mono bus on channel 0 for the generic FDN path.
            let mono_in = if let Some(reverb_input) = ctx.reverb_input {
                reverb_input[base]
            } else {
                let mut mono_sum = 0.0f32;
                let mut active_count = 0u32;
                for ch in 0..render_channels {
                    if ctx.layout.is_channel_active(ch) {
                        mono_sum += buffer[base + ch];
                        active_count += 1;
                    }
                }
                if active_count == 0 {
                    continue;
                }
                mono_sum / active_count as f32
            };

            // Pre-delay
            self.pre_delay_buf[self.pre_delay_write_pos] = mono_in;
            let read_pos = (self.pre_delay_write_pos + PRE_DELAY_BUF_SIZE - self.pre_delay_samples)
                & PRE_DELAY_BUF_MASK;
            let delayed_in = self.pre_delay_buf[read_pos];
            self.pre_delay_write_pos = (self.pre_delay_write_pos + 1) & PRE_DELAY_BUF_MASK;

            // FDN → only active channels within render range
            let wet = self.process_fdn_sample(delayed_in, render_channels);

            for ch in 0..render_channels {
                if ctx.layout.is_channel_active(ch) {
                    buffer[base + ch] =
                        soft_clip(buffer[base + ch] + wet[ch] * self.output_normalization);
                }
            }
        }
    }

    fn reset(&mut self) {
        for line in self.delay_buffers.iter_mut() {
            line.fill(0.0);
        }
        self.pre_delay_buf.fill(0.0);
        self.write_pos = 0;
        self.pre_delay_write_pos = 0;
        for d in &mut self.damping {
            d.state = 0.0;
        }
    }

    fn name(&self) -> &str {
        "fdn_reverb"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::atmosphere::AtmosphericParams;
    use atrium_core::listener::Listener;
    use atrium_core::speaker::SpeakerLayout;
    use atrium_core::types::Vec3;

    fn make_ctx(channels: usize, _render_channels: usize) -> (SpeakerLayout, Listener) {
        let layout = if channels >= 6 {
            SpeakerLayout::surround_5_1(
                Vec3::new(0.0, 4.0, 0.0),
                Vec3::new(6.0, 4.0, 0.0),
                Vec3::new(3.0, 4.0, 0.0),
                Vec3::new(0.0, 0.0, 0.0),
                Vec3::new(6.0, 0.0, 0.0),
            )
        } else {
            SpeakerLayout::stereo(Vec3::new(-1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0))
        };
        let listener = Listener::new(Vec3::ZERO, 0.0);
        (layout, listener)
    }

    const TEST_ATMOSPHERE: AtmosphericParams = AtmosphericParams {
        temperature_c: 20.0,
        humidity_pct: 50.0,
        pressure_kpa: 101.325,
    };

    fn mix_ctx<'a>(
        layout: &'a SpeakerLayout,
        listener: &'a Listener,
        channels: usize,
        render_channels: usize,
    ) -> MixContext<'a> {
        MixContext {
            listener,
            layout,
            sample_rate: 48000.0,
            channels,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
            render_channels,
            reverb_input: None,
            wall_reflectivity: 0.9,
            atmosphere: &TEST_ATMOSPHERE,
        }
    }

    /// FDN with render_channels < channels must not touch channels beyond render_channels.
    #[test]
    fn render_channels_limits_wet_output() {
        let channels = 6;
        let render_channels = 2; // HRTF on 5.1 device
        let (layout, listener) = make_ctx(channels, render_channels);
        let ctx = mix_ctx(&layout, &listener, channels, render_channels);

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        let frames = 2048;
        let mut buffer = vec![0.0f32; frames * channels];

        // Put signal only in channels 0 and 1 (stereo HRTF output)
        for frame in 0..frames {
            buffer[frame * channels] = 0.5;
            buffer[frame * channels + 1] = 0.5;
        }

        fdn.process(&mut buffer, &ctx);

        // Channels 2-5 must remain exactly zero
        for frame in 0..frames {
            for ch in 2..channels {
                assert_eq!(
                    buffer[frame * channels + ch], 0.0,
                    "channel {ch} at frame {frame} should be zero when render_channels={render_channels}"
                );
            }
        }

        // Channels 0-1 should have been modified (dry + wet)
        let energy_ch0: f32 = buffer.iter().step_by(channels).map(|s| s * s).sum();
        let energy_ch1: f32 = buffer.iter().skip(1).step_by(channels).map(|s| s * s).sum();
        assert!(energy_ch0 > 0.0, "channel 0 should have signal");
        assert!(energy_ch1 > 0.0, "channel 1 should have signal");
    }

    /// When render_channels == channels, all channels get wet signal (normal behavior).
    #[test]
    fn full_channels_all_get_wet() {
        let channels = 6;
        let render_channels = 6;
        let (layout, listener) = make_ctx(channels, render_channels);
        let ctx = mix_ctx(&layout, &listener, channels, render_channels);

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        let frames = 2048;
        let mut buffer = vec![0.0f32; frames * channels];

        // Put signal in all channels
        for sample in buffer.iter_mut() {
            *sample = 0.5;
        }

        fdn.process(&mut buffer, &ctx);

        // All channels should have been processed (not just passthrough)
        for ch in 0..channels {
            let max_abs = (0..frames)
                .map(|f| buffer[f * channels + ch].abs())
                .fold(0.0f32, f32::max);
            assert!(
                max_abs > 0.0,
                "channel {ch} should have signal with render_channels={render_channels}"
            );
        }
    }

    /// FDN only sums mono from render_channels, ignoring higher channels.
    #[test]
    fn mono_sum_only_from_render_channels() {
        let channels = 6;
        let render_channels = 2;
        let (layout, listener) = make_ctx(channels, render_channels);
        let ctx = mix_ctx(&layout, &listener, channels, render_channels);

        let mut fdn_a = FdnReverbStage::new();
        let mut fdn_b = FdnReverbStage::new();
        fdn_a.init(&ctx);
        fdn_b.init(&ctx);

        let frames = 1024;

        // Buffer A: signal in channels 0-1, noise in channels 2-5
        let mut buffer_a = vec![0.0f32; frames * channels];
        for frame in 0..frames {
            buffer_a[frame * channels] = 0.5;
            buffer_a[frame * channels + 1] = 0.5;
            for ch in 2..channels {
                buffer_a[frame * channels + ch] = 99.0; // should be ignored
            }
        }

        // Buffer B: same signal in channels 0-1, zeros in 2-5
        let mut buffer_b = vec![0.0f32; frames * channels];
        for frame in 0..frames {
            buffer_b[frame * channels] = 0.5;
            buffer_b[frame * channels + 1] = 0.5;
        }

        fdn_a.process(&mut buffer_a, &ctx);
        fdn_b.process(&mut buffer_b, &ctx);

        // Channels 0-1 should be identical regardless of what's in channels 2-5
        for frame in 0..frames {
            for ch in 0..2 {
                let a = buffer_a[frame * channels + ch];
                let b = buffer_b[frame * channels + ch];
                assert!(
                    (a - b).abs() < 1e-6,
                    "ch{ch} frame {frame}: content beyond render_channels should not affect output ({a} vs {b})"
                );
            }
        }
    }

    /// Quad channel mode on 5.1 hardware: FDN only writes to active channels [0,1,4,5].
    /// Channels 2 (C) and 3 (LFE) must stay silent despite render_channels=6.
    #[test]
    fn active_mask_respects_quad_on_5_1() {
        let channels = 6;
        let render_channels = 6;
        let (mut layout, listener) = make_ctx(channels, render_channels);
        // Quad channel mode: only FL(0), FR(1), RL(4), RR(5) active
        layout.set_active_channels(&[0, 1, 4, 5]);
        let ctx = mix_ctx(&layout, &listener, channels, render_channels);

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        let frames = 2048;
        let mut buffer = vec![0.0f32; frames * channels];

        // Signal in active channels only
        for frame in 0..frames {
            buffer[frame * channels] = 0.5; // FL
            buffer[frame * channels + 1] = 0.5; // FR
            buffer[frame * channels + 4] = 0.5; // RL
            buffer[frame * channels + 5] = 0.5; // RR
        }

        fdn.process(&mut buffer, &ctx);

        // Channels 2 (C) and 3 (LFE) must remain zero
        for frame in 0..frames {
            for ch in [2, 3] {
                assert_eq!(
                    buffer[frame * channels + ch],
                    0.0,
                    "channel {ch} at frame {frame} should be zero in quad mode"
                );
            }
        }

        // Active channels should have signal
        for ch in [0, 1, 4, 5] {
            let has_signal = (0..frames).any(|f| buffer[f * channels + ch].abs() > 1e-10);
            assert!(has_signal, "active channel {ch} should have signal");
        }
    }

    /// Silent input produces silent output (no self-oscillation).
    #[test]
    fn silent_input_silent_output() {
        let channels = 6;
        let render_channels = 2;
        let (layout, listener) = make_ctx(channels, render_channels);
        let ctx = mix_ctx(&layout, &listener, channels, render_channels);

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        let frames = 512;
        let mut buffer = vec![0.0f32; frames * channels];
        fdn.process(&mut buffer, &ctx);

        let max = buffer.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max < 1e-10, "silent input should produce silent output");
    }

    /// When reverb_input is provided (DBAP/VBAP/HRTF mono bus on ch0),
    /// FDN reads ch0 directly and produces wet output on active channels.
    #[test]
    fn reverb_input_mono_bus_produces_wet_on_active_channels() {
        let channels = 6;
        let render_channels = 6;
        let (mut layout, listener) = make_ctx(channels, render_channels);
        // Quad mask: FL(0), FR(1), RL(4), RR(5) active; C(2), LFE(3) masked.
        layout.set_active_channels(&[0, 1, 4, 5]);

        let frames = 2048;
        let mut reverb_input = vec![0.0f32; frames * channels];
        // Write mono signal to ch0 only (mimics renderer mono bus).
        for frame in 0..frames {
            reverb_input[frame * channels] = 0.5;
        }

        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
            render_channels,
            reverb_input: Some(&reverb_input),
            wall_reflectivity: 0.9,
            atmosphere: &AtmosphericParams::default(),
        };

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        let mut buffer = vec![0.0f32; frames * channels];
        fdn.process(&mut buffer, &ctx);

        // Active channels (0, 1, 4, 5) should have wet signal.
        let active_energy: f32 = (0..frames)
            .map(|f| {
                let base = f * channels;
                [0, 1, 4, 5]
                    .iter()
                    .map(|&ch| buffer[base + ch].powi(2))
                    .sum::<f32>()
            })
            .sum();
        assert!(
            active_energy > 0.01,
            "active channels should have wet signal"
        );

        // Masked channels (2, 3) should remain silent.
        let masked_energy: f32 = (0..frames)
            .map(|f| {
                let base = f * channels;
                buffer[base + 2].powi(2) + buffer[base + 3].powi(2)
            })
            .sum();
        assert!(
            masked_energy < 1e-10,
            "masked channels should stay silent, got energy {masked_energy}"
        );
    }

    /// Output normalization adapts to room reflectivity: reflective rooms get
    /// heavier attenuation (lower output_normalization) because the Jot feedback
    /// gains are higher, causing more steady-state energy buildup in the network.
    #[test]
    fn output_normalization_scales_with_reflectivity() {
        let channels = 2;
        let render_channels = 2;
        let (layout, listener) = make_ctx(channels, render_channels);

        // Dead room: low reflectivity → short RT60 → low Jot gains → normalization near 1.0
        let ctx_dead = MixContext {
            wall_reflectivity: 0.3,
            ..mix_ctx(&layout, &listener, channels, render_channels)
        };
        let mut fdn_dead = FdnReverbStage::new();
        fdn_dead.init(&ctx_dead);

        // Reflective room: high reflectivity → long RT60 → high Jot gains → lower normalization
        let ctx_live = MixContext {
            wall_reflectivity: 0.95,
            ..mix_ctx(&layout, &listener, channels, render_channels)
        };
        let mut fdn_live = FdnReverbStage::new();
        fdn_live.init(&ctx_live);

        assert!(
            fdn_dead.output_normalization > fdn_live.output_normalization,
            "dead room norm ({}) should exceed live room norm ({})",
            fdn_dead.output_normalization,
            fdn_live.output_normalization
        );
        // Dead room should need minimal correction (near 1.0)
        assert!(
            fdn_dead.output_normalization > 0.5,
            "dead room norm should be near 1.0, got {}",
            fdn_dead.output_normalization
        );
        // Live room needs substantial attenuation
        assert!(
            fdn_live.output_normalization < 0.3,
            "live room norm should be well below 1.0, got {}",
            fdn_live.output_normalization
        );
    }

    /// Verify the actual output normalization for the default atrium room (6×4×3m, r=0.9).
    /// This is the generic FDN used by VBAP, HRTF, and DBAP modes.
    #[test]
    fn output_normalization_atrium_room() {
        let channels = 6;
        let render_channels = 6;
        let (layout, listener) = make_ctx(channels, render_channels);

        // Actual atrium: 6×4×3m room, wall_reflectivity=0.9
        let ctx = MixContext {
            room_min: Vec3::new(0.0, 0.0, 0.0),
            room_max: Vec3::new(6.0, 4.0, 3.0),
            wall_reflectivity: 0.9,
            ..mix_ctx(&layout, &listener, channels, render_channels)
        };

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        // With g_eff (RMS of g_dc, g_nyq), the 6×4×3m room at r=0.9
        // should produce normalization around 0.37 (significant attenuation
        // because RT60≈1.07s creates substantial feedback energy buildup).
        eprintln!(
            "Generic FDN output_normalization = {:.4} (atrium 6x4x3, r=0.9)",
            fdn.output_normalization
        );
        assert!(
            (fdn.output_normalization - 0.37).abs() < 0.05,
            "atrium room norm should be ~0.37, got {}",
            fdn.output_normalization
        );
    }

    /// Sweep wall_reflectivity and RT60 high-frequency ratio to show how each
    /// affects normalization and actual wet tail energy.
    ///
    /// Feeds a single impulse through the FDN and measures:
    /// - output_normalization (structural gain compensation)
    /// - RT60 low (from Sabine)
    /// - wet tail RMS (actual energy in the reverb tail after the impulse)
    #[test]
    fn parameter_sweep_reflectivity_and_hf_ratio() {
        use crate::pipeline::room_acoustics;

        let channels = 2;
        let render_channels = 2;
        let (layout, listener) = make_ctx(channels, render_channels);
        let sample_rate = 48000.0;

        let room_min = Vec3::new(0.0, 0.0, 0.0);
        let room_max = Vec3::new(6.0, 4.0, 3.0);
        let (volume, surface_area) = room_acoustics::room_geometry(room_min, room_max);

        let reflectivities = [0.70, 0.75, 0.80, 0.85, 0.90, 0.95];
        let hf_ratios = [0.3, 0.4, 0.5, 0.6];

        eprintln!();
        eprintln!("═══ Parameter sweep: 6×4×3m atrium ═══");
        eprintln!(
            "{:>6}  {:>8}  {:>8}  {:>8}  {:>12}",
            "refl", "hf_ratio", "RT60_lo", "norm", "tail_rms_dB"
        );
        eprintln!("{}", "─".repeat(50));

        for &refl in &reflectivities {
            let rt60_low = room_acoustics::sabine_rt60(volume, surface_area, refl);

            for &hf_ratio in &hf_ratios {
                let rt60_high = rt60_low * hf_ratio;

                let mut fdn = FdnReverbStage::new();
                fdn.compute_delays(sample_rate);
                fdn.compute_damping(sample_rate, rt60_low, rt60_high);
                fdn.initialized = true;

                // Feed a single impulse and measure the tail energy.
                // Use reverb_input to control exactly what goes in (mono bus on ch0).
                let frames = 48000; // 1 second
                let mut buffer = vec![0.0f32; frames * channels];
                let mut reverb_input = vec![0.0f32; frames * channels];
                reverb_input[0] = 1.0; // single impulse on ch0

                let ctx = MixContext {
                    room_min,
                    room_max,
                    wall_reflectivity: refl,
                    reverb_input: Some(&reverb_input),
                    ..mix_ctx(&layout, &listener, channels, render_channels)
                };

                fdn.process(&mut buffer, &ctx);

                // Measure wet tail RMS (skip first 50ms to avoid the direct impulse).
                let skip_samples = (0.05 * sample_rate) as usize;
                let tail_energy: f32 = buffer[skip_samples * channels..]
                    .iter()
                    .step_by(channels) // ch0 only
                    .map(|s| s * s)
                    .sum();
                let tail_samples = (frames - skip_samples) as f32;
                let tail_rms = (tail_energy / tail_samples).sqrt();
                let tail_rms_db = if tail_rms > 1e-10 {
                    20.0 * tail_rms.log10()
                } else {
                    -100.0
                };

                eprintln!(
                    "{:>6.2}  {:>8.1}  {:>8.3}  {:>8.4}  {:>12.1}",
                    refl, hf_ratio, rt60_low, fdn.output_normalization, tail_rms_db
                );
            }
        }
        eprintln!();
    }

    /// Break down the energy budget between early reflections and FDN late tail
    /// for the default atrium room. Shows which stage dominates the perceived echo.
    #[test]
    fn energy_breakdown_early_vs_late() {
        use crate::pipeline::path::{PathResolver, PathSet, ResolveContext};
        use crate::pipeline::path_resolvers::ImageSourceResolver;
        use crate::pipeline::room_acoustics;

        let channels = 2;
        let render_channels = 2;
        let (layout, listener) = make_ctx(channels, render_channels);
        let sample_rate = 48000.0;

        let room_min = Vec3::new(0.0, 0.0, 0.0);
        let room_max = Vec3::new(6.0, 4.0, 3.0);
        let (volume, surface_area) = room_acoustics::room_geometry(room_min, room_max);

        let listener_pos = Vec3::new(3.0, 2.0, 0.0); // center of room
        let source_positions = [
            ("djembe (center, 1.5m orbit)", Vec3::new(4.5, 2.0, 0.0)),
            ("campfire (corner)", Vec3::new(1.0, 1.0, 0.0)),
            ("purring (front-right)", Vec3::new(5.0, 3.0, 0.0)),
        ];

        let reflectivities = [0.75, 0.90];

        for &refl in &reflectivities {
            let rt60 = room_acoustics::sabine_rt60(volume, surface_area, refl);
            let d_c = room_acoustics::critical_distance(volume, rt60, 1.0);

            eprintln!();
            eprintln!("═══ Energy breakdown: r={refl}, RT60={rt60:.3}s, d_c={d_c:.3}m ═══");
            eprintln!(
                "{:>25}  {:>6}  {:>10}  {:>12}  {:>12}  {:>10}",
                "source", "dist", "send (d/dc)", "refl_energy", "fdn_tail_dB", "ratio E/L"
            );
            eprintln!("{}", "─".repeat(85));

            let resolver = ImageSourceResolver::new(refl);

            for (name, source_pos) in &source_positions {
                let dist = source_pos.distance_to(listener_pos);
                let send = room_acoustics::reverb_send(dist, d_c);

                // ── Early reflections: path resolver gives us the gains ──
                let mut paths = PathSet::new();
                let resolve_ctx = ResolveContext {
                    source_pos: *source_pos,
                    target_pos: listener_pos,
                    room_min,
                    room_max,
                    barriers: &[],
                    atmosphere: &AtmosphericParams::default(),
                };
                resolver.resolve(&resolve_ctx, &mut paths);

                let mut early_energy = 0.0f32;
                for path in paths.as_slice() {
                    if path.kind == crate::pipeline::path::PathKind::Reflection {
                        // Each reflection has gain = reflectivity / image_dist
                        early_energy += path.gain * path.gain;
                    }
                }

                // ── FDN late tail: feed impulse × send, measure tail ──
                let rt60_high = rt60 * RT60_HIGH_RATIO;
                let mut fdn = FdnReverbStage::new();
                fdn.compute_delays(sample_rate);
                fdn.compute_damping(sample_rate, rt60, rt60_high);
                fdn.initialized = true;

                let frames = 48000; // 1 second
                let mut buffer = vec![0.0f32; frames * channels];
                let mut reverb_input = vec![0.0f32; frames * channels];
                reverb_input[0] = send; // impulse scaled by d/d_c send law

                let ctx = MixContext {
                    room_min,
                    room_max,
                    wall_reflectivity: refl,
                    reverb_input: Some(&reverb_input),
                    ..mix_ctx(&layout, &listener, channels, render_channels)
                };

                fdn.process(&mut buffer, &ctx);

                // Measure FDN tail energy (skip first 50ms)
                let skip = (0.05 * sample_rate) as usize;
                let tail_energy: f32 = buffer[skip * channels..]
                    .iter()
                    .step_by(channels)
                    .map(|s| s * s)
                    .sum();
                let tail_rms = (tail_energy / (frames - skip) as f32).sqrt();
                let tail_db = if tail_rms > 1e-10 {
                    20.0 * tail_rms.log10()
                } else {
                    -100.0
                };

                let ratio = if tail_energy > 1e-20 {
                    early_energy / tail_energy
                } else {
                    f32::INFINITY
                };

                eprintln!(
                    "{:>25}  {:>6.2}  {:>10.4}  {:>12.6}  {:>12.1}  {:>10.1}",
                    name, dist, send, early_energy, tail_db, ratio
                );
            }
        }
        eprintln!();
    }
}
