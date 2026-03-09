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
/// Fallback pre-delay when room geometry is degenerate (volume ≈ 0 or surface area ≈ 0).
const PRE_DELAY_FALLBACK_SECONDS: f32 = 0.020;
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
/// High-frequency decay is derived from per-wall material absorption
/// coefficients via Sabine's equation at 500 Hz (low) and 4 kHz (high),
/// so rooms with absorptive surfaces (carpet, acoustic tile) naturally
/// produce faster HF decay than hard-walled rooms.
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

/// Sabine RT60 band indices for FDN damping.
/// Band 2 = 500 Hz (low-frequency reference), band 5 = 4 kHz (HF decay).
const RT60_LOW_BAND: usize = 2;
const RT60_HIGH_BAND: usize = 5;

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
    }

    /// Compute pre-delay from mean free path time (average time between wall
    /// bounces in a diffuse field). Falls back to 20ms for degenerate rooms.
    ///
    /// Reference: Kuttruff, "Room Acoustics" (5th ed., 2009).
    fn compute_pre_delay(
        &mut self,
        sample_rate: f32,
        volume: f32,
        surface_area: f32,
        speed_of_sound: f32,
    ) {
        let pre_delay_seconds = if volume < 1e-6 || surface_area < 1e-6 {
            PRE_DELAY_FALLBACK_SECONDS
        } else {
            room_acoustics::mean_free_path_time(volume, surface_area, speed_of_sound)
        };

        // Clamp to [5ms, buffer capacity] for safety.
        let max_seconds = (PRE_DELAY_BUF_SIZE - 1) as f32 / sample_rate;
        let clamped = pre_delay_seconds.clamp(0.005, max_seconds);
        self.pre_delay_samples = (clamped * sample_rate) as usize;
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

        // Derive per-band RT60 from room geometry + wall materials via Sabine's equation.
        // RT60 at 500 Hz (band 2) sets the overall decay; RT60 at 4 kHz (band 5)
        // controls how much faster highs decay — driven by actual material absorption.
        let (volume, surface_area) = room_acoustics::room_geometry(ctx.room_min, ctx.room_max);
        let wall_areas = room_acoustics::wall_surface_areas(ctx.room_min, ctx.room_max);
        let rt60_low = room_acoustics::sabine_rt60_at_band(
            volume,
            &wall_areas,
            ctx.wall_materials,
            ctx.atmosphere,
            RT60_LOW_BAND,
        );
        let rt60_high = room_acoustics::sabine_rt60_at_band(
            volume,
            &wall_areas,
            ctx.wall_materials,
            ctx.atmosphere,
            RT60_HIGH_BAND,
        );

        // Pre-delay from mean free path: average time between wall bounces.
        let speed_of_sound = ctx.atmosphere.speed_of_sound();
        self.compute_pre_delay(ctx.sample_rate, volume, surface_area, speed_of_sound);

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

    use crate::pipeline::path::WallMaterial;

    const TEST_MATERIALS: [WallMaterial; 6] = [WallMaterial::HARD_WALL; 6];

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
            wall_materials: &TEST_MATERIALS,
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
            wall_materials: &TEST_MATERIALS,
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

    /// Output normalization adapts to wall materials: absorptive rooms get
    /// less attenuation (higher output_normalization) because the Jot feedback
    /// gains are lower, causing less steady-state energy buildup in the network.
    #[test]
    fn output_normalization_scales_with_materials() {
        let channels = 2;
        let render_channels = 2;
        let (layout, listener) = make_ctx(channels, render_channels);

        // Dead room: acoustic tile on all walls → high absorption → short RT60
        let tile_materials: [WallMaterial; 6] =
            std::array::from_fn(|_| WallMaterial::ceiling_tile());
        let ctx_dead = MixContext {
            wall_materials: &tile_materials,
            ..mix_ctx(&layout, &listener, channels, render_channels)
        };
        let mut fdn_dead = FdnReverbStage::new();
        fdn_dead.init(&ctx_dead);

        // Live room: hard walls → low absorption → long RT60
        let ctx_live = mix_ctx(&layout, &listener, channels, render_channels);
        let mut fdn_live = FdnReverbStage::new();
        fdn_live.init(&ctx_live);

        assert!(
            fdn_dead.output_normalization > fdn_live.output_normalization,
            "dead room norm ({}) should exceed live room norm ({})",
            fdn_dead.output_normalization,
            fdn_live.output_normalization
        );
    }

    /// Verify the actual output normalization for the default atrium room (6×4×3m, hard walls).
    /// This is the generic FDN used by VBAP, HRTF, and DBAP modes.
    #[test]
    fn output_normalization_atrium_room() {
        let channels = 6;
        let render_channels = 6;
        let (layout, listener) = make_ctx(channels, render_channels);

        // Actual atrium: 6×4×3m room, hard walls
        let ctx = MixContext {
            room_min: Vec3::new(0.0, 0.0, 0.0),
            room_max: Vec3::new(6.0, 4.0, 3.0),
            ..mix_ctx(&layout, &listener, channels, render_channels)
        };

        let mut fdn = FdnReverbStage::new();
        fdn.init(&ctx);

        // Hard walls produce long RT60 → high feedback gains → low output normalization.
        // The exact value depends on material-derived RT60 at 500 Hz and 4 kHz bands.
        eprintln!(
            "Generic FDN output_normalization = {:.4} (atrium 6x4x3, hard walls)",
            fdn.output_normalization
        );
        assert!(
            fdn.output_normalization > 0.05 && fdn.output_normalization < 0.5,
            "atrium room norm should be between 0.05 and 0.5, got {}",
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
                fdn.compute_pre_delay(sample_rate, volume, surface_area, 343.42);
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
                // Use band-specific RT60 from wall materials (hard walls for this test).
                let wall_areas = room_acoustics::wall_surface_areas(room_min, room_max);
                let atmosphere = AtmosphericParams::default();
                let rt60_high = room_acoustics::sabine_rt60_at_band(
                    volume,
                    &wall_areas,
                    &TEST_MATERIALS,
                    &atmosphere,
                    RT60_HIGH_BAND,
                );
                let mut fdn = FdnReverbStage::new();
                fdn.compute_delays(sample_rate);
                fdn.compute_pre_delay(sample_rate, volume, surface_area, 343.42);
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

    // ── Pre-delay from room geometry (Phase 4A) ──────────────────────────

    #[test]
    fn pre_delay_10m_cube() {
        // 10m cube: V=1000, S=600 → MFP = 4×1000/600 = 6.667m
        // t = 6.667 / 343.42 ≈ 19.4ms → 932 samples at 48kHz
        let mut fdn = FdnReverbStage::new();
        fdn.compute_pre_delay(48000.0, 1000.0, 600.0, 343.42);
        let expected_ms = 19.4;
        let actual_ms = fdn.pre_delay_samples as f32 / 48.0;
        assert!(
            (actual_ms - expected_ms).abs() < 1.0,
            "10m cube pre-delay should be ~{expected_ms}ms, got {actual_ms:.1}ms ({} samples)",
            fdn.pre_delay_samples
        );
    }

    #[test]
    fn pre_delay_small_room() {
        // 6×4×3m atrium: V=72, S=108 → MFP = 4×72/108 = 2.667m
        // t = 2.667 / 343.42 ≈ 7.8ms → 373 samples at 48kHz
        let mut fdn = FdnReverbStage::new();
        let (volume, surface_area) =
            room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(6.0, 4.0, 3.0));
        fdn.compute_pre_delay(48000.0, volume, surface_area, 343.42);
        let expected_ms = 7.8;
        let actual_ms = fdn.pre_delay_samples as f32 / 48.0;
        assert!(
            (actual_ms - expected_ms).abs() < 1.0,
            "6×4×3m room pre-delay should be ~{expected_ms}ms, got {actual_ms:.1}ms ({} samples)",
            fdn.pre_delay_samples
        );
    }

    #[test]
    fn pre_delay_varies_with_room_size() {
        let mut fdn_small = FdnReverbStage::new();
        let mut fdn_large = FdnReverbStage::new();

        // Small room: 3×3×3m
        let (vol_s, sa_s) = room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(3.0, 3.0, 3.0));
        fdn_small.compute_pre_delay(48000.0, vol_s, sa_s, 343.42);

        // Large room: 20×15×10m
        let (vol_l, sa_l) = room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(20.0, 15.0, 10.0));
        fdn_large.compute_pre_delay(48000.0, vol_l, sa_l, 343.42);

        assert!(
            fdn_large.pre_delay_samples > fdn_small.pre_delay_samples,
            "larger room should have longer pre-delay: {} vs {} samples",
            fdn_large.pre_delay_samples,
            fdn_small.pre_delay_samples
        );
    }

    #[test]
    fn pre_delay_degenerate_room_uses_fallback() {
        let mut fdn = FdnReverbStage::new();
        // Zero volume room → should use fallback (20ms)
        fdn.compute_pre_delay(48000.0, 0.0, 0.0, 343.42);
        let expected_samples = (PRE_DELAY_FALLBACK_SECONDS * 48000.0) as usize;
        assert_eq!(
            fdn.pre_delay_samples, expected_samples,
            "degenerate room should use fallback pre-delay"
        );
    }

    // ── HF decay from materials (Phase 4B) ────────────────────────────

    #[test]
    fn hard_walls_rt60_ratio_near_unity() {
        // Hard walls have nearly uniform absorption across bands (0.02–0.05),
        // so RT60 at 500 Hz and 4 kHz should be similar (ratio near 1.0).
        let room_min = Vec3::ZERO;
        let room_max = Vec3::new(6.0, 4.0, 3.0);
        let (volume, _) = room_acoustics::room_geometry(room_min, room_max);
        let wall_areas = room_acoustics::wall_surface_areas(room_min, room_max);
        let atmosphere = AtmosphericParams::default();

        let rt60_low = room_acoustics::sabine_rt60_at_band(
            volume,
            &wall_areas,
            &TEST_MATERIALS,
            &atmosphere,
            RT60_LOW_BAND,
        );
        let rt60_high = room_acoustics::sabine_rt60_at_band(
            volume,
            &wall_areas,
            &TEST_MATERIALS,
            &atmosphere,
            RT60_HIGH_BAND,
        );

        let ratio = rt60_high / rt60_low;
        // Hard walls: alpha at 500 Hz = 0.03, at 4 kHz = 0.05 + air absorption.
        // Ratio should be between 0.5 and 1.0 (close to 1 but air absorption pulls HF down).
        assert!(
            ratio > 0.4 && ratio < 1.0,
            "hard walls RT60 ratio should be near 1.0, got {ratio:.3} (low={rt60_low:.3}s, high={rt60_high:.3}s)"
        );
    }

    #[test]
    fn carpet_walls_have_lower_hf_ratio() {
        // Carpet has much higher absorption at 4 kHz (0.40) vs 500 Hz (0.08),
        // so RT60_high / RT60_low should be significantly lower than hard walls.
        let room_min = Vec3::ZERO;
        let room_max = Vec3::new(6.0, 4.0, 3.0);
        let (volume, _) = room_acoustics::room_geometry(room_min, room_max);
        let wall_areas = room_acoustics::wall_surface_areas(room_min, room_max);
        let atmosphere = AtmosphericParams::default();

        let carpet_materials: [WallMaterial; 6] = std::array::from_fn(|_| WallMaterial::carpet());

        let rt60_low = room_acoustics::sabine_rt60_at_band(
            volume,
            &wall_areas,
            &carpet_materials,
            &atmosphere,
            RT60_LOW_BAND,
        );
        let rt60_high = room_acoustics::sabine_rt60_at_band(
            volume,
            &wall_areas,
            &carpet_materials,
            &atmosphere,
            RT60_HIGH_BAND,
        );

        let ratio = rt60_high / rt60_low;
        assert!(
            ratio < 0.5,
            "carpet walls should have RT60_high/RT60_low < 0.5, got {ratio:.3} (low={rt60_low:.3}s, high={rt60_high:.3}s)"
        );
    }

    #[test]
    fn fdn_init_uses_material_derived_damping() {
        // Verify that FDN init produces different damping for different materials.
        let room_min = Vec3::ZERO;
        let room_max = Vec3::new(6.0, 4.0, 3.0);

        let hard_materials = [WallMaterial::HARD_WALL; 6];
        let carpet_materials: [WallMaterial; 6] = std::array::from_fn(|_| WallMaterial::carpet());

        let layout = SpeakerLayout::new(&[], None, 2);
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
        let atmosphere = AtmosphericParams::default();

        let mut fdn_hard = FdnReverbStage::new();
        let ctx_hard = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            room_min,
            room_max,
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &hard_materials,
            atmosphere: &atmosphere,
        };
        fdn_hard.init(&ctx_hard);

        let mut fdn_carpet = FdnReverbStage::new();
        let ctx_carpet = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            room_min,
            room_max,
            master_gain: 1.0,
            render_channels: 2,
            reverb_input: None,
            wall_reflectivity: 0.9,
            wall_materials: &carpet_materials,
            atmosphere: &atmosphere,
        };
        fdn_carpet.init(&ctx_carpet);

        // Hard walls should have higher output normalization (less damping, more buildup)
        // than carpet walls (more damping, less buildup).
        assert!(
            fdn_hard.output_normalization < fdn_carpet.output_normalization,
            "hard walls should have lower output_norm (more energy buildup) than carpet: hard={}, carpet={}",
            fdn_hard.output_normalization, fdn_carpet.output_normalization
        );
    }

    #[test]
    fn pre_delay_clamped_minimum() {
        let mut fdn = FdnReverbStage::new();
        // Very tiny room: 0.1×0.1×0.1m → MFP ≈ 0.067m → t ≈ 0.19ms
        // Should clamp to 5ms minimum
        let (volume, surface_area) =
            room_acoustics::room_geometry(Vec3::ZERO, Vec3::new(0.1, 0.1, 0.1));
        fdn.compute_pre_delay(48000.0, volume, surface_area, 343.42);
        let min_samples = (0.005 * 48000.0) as usize;
        assert!(
            fdn.pre_delay_samples >= min_samples,
            "pre-delay should be at least 5ms ({min_samples} samples), got {}",
            fdn.pre_delay_samples
        );
    }
}
