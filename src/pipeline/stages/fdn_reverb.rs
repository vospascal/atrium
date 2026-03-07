//! FDN (Feedback Delay Network) late reverb.
//!
//! Based on Jot & Chaigne (1991). 8 parallel delay lines coupled through an
//! 8×8 Hadamard mixing matrix, with one-pole lowpass damping per line for
//! frequency-dependent decay (highs fade faster than lows).
//!
//! Chain order: Direct (0–3ms) → EarlyReflections (3–50ms) → FdnReverb (50ms+)

use crate::pipeline::mix_stage::{MixContext, MixStage};
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
/// Mono-in, N-channel out. All input channels averaged to mono, processed
/// through 8-line FDN. Lines distributed round-robin across output channels.
pub struct FdnReverbStage {
    delay_buffers: Box<[[f32; BUF_SIZE]; NUM_LINES]>,
    write_pos: usize,
    delays: [usize; NUM_LINES],
    damping: [DampingFilter; NUM_LINES],
    pre_delay_buf: Box<[f32; PRE_DELAY_BUF_SIZE]>,
    pre_delay_write_pos: usize,
    pre_delay_samples: usize,
    wet_gain: f32,
    rt60_low: f32,
    rt60_high: f32,
    initialized: bool,
}

impl FdnReverbStage {
    pub fn new(wet_gain: f32, rt60_low: f32, rt60_high: f32) -> Self {
        Self {
            delay_buffers: Box::new([[0.0; BUF_SIZE]; NUM_LINES]),
            write_pos: 0,
            delays: [0; NUM_LINES],
            damping: [DampingFilter::new(); NUM_LINES],
            pre_delay_buf: Box::new([0.0; PRE_DELAY_BUF_SIZE]),
            pre_delay_write_pos: 0,
            pre_delay_samples: 0,
            wet_gain,
            rt60_low,
            rt60_high,
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

    /// Compute per-line one-pole damping filters for frequency-dependent decay.
    ///
    /// Each filter has gain `k` and pole `p` derived from the RT60 at DC and Nyquist:
    ///   g_dc  = 10^(-3m / (RT60_low  × fs))   — gain per sample at 0 Hz
    ///   g_nyq = 10^(-3m / (RT60_high × fs))   — gain per sample at Nyquist
    ///   k = 2·g_dc·g_nyq / (g_dc + g_nyq)     — filter gain (harmonic mean)
    ///   p = (g_dc - g_nyq) / (g_dc + g_nyq)   — pole location
    ///
    /// Reference: Jot, "Digital Delay Networks for Designing Artificial Reverberators",
    /// AES 90th Convention (1991), §3.2; extended in Jot's PhD thesis (1992), Ch. 4.
    fn compute_damping(&mut self, sample_rate: f32) {
        for i in 0..NUM_LINES {
            let m = self.delays[i] as f32;
            let g_dc = 10.0_f32.powf(-3.0 * m / (self.rt60_low * sample_rate));
            let g_nyq = 10.0_f32.powf(-3.0 * m / (self.rt60_high * sample_rate));
            let sum = g_dc + g_nyq;
            if sum < f32::EPSILON {
                self.damping[i].k = 0.0;
                self.damping[i].p = 0.0;
            } else {
                self.damping[i].k = 2.0 * g_dc * g_nyq / sum;
                self.damping[i].p = (g_dc - g_nyq) / sum;
            }
            self.damping[i].state = 0.0;
        }
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
        self.compute_damping(ctx.sample_rate);
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

            // Sum to mono from active channels only (respects non-contiguous masks like quad on 5.1)
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
            let mono_in = mono_sum / active_count as f32;

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
                    buffer[base + ch] = soft_clip(buffer[base + ch] + wet[ch] * self.wet_gain);
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
        }
    }

    /// FDN with render_channels < channels must not touch channels beyond render_channels.
    #[test]
    fn render_channels_limits_wet_output() {
        let channels = 6;
        let render_channels = 2; // HRTF on 5.1 device
        let (layout, listener) = make_ctx(channels, render_channels);
        let ctx = mix_ctx(&layout, &listener, channels, render_channels);

        let mut fdn = FdnReverbStage::new(0.3, 0.8, 0.3);
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

        let mut fdn = FdnReverbStage::new(0.3, 0.8, 0.3);
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

        let mut fdn_a = FdnReverbStage::new(0.3, 0.8, 0.3);
        let mut fdn_b = FdnReverbStage::new(0.3, 0.8, 0.3);
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

        let mut fdn = FdnReverbStage::new(0.3, 0.8, 0.3);
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

        let mut fdn = FdnReverbStage::new(0.3, 0.8, 0.3);
        fdn.init(&ctx);

        let frames = 512;
        let mut buffer = vec![0.0f32; frames * channels];
        fdn.process(&mut buffer, &ctx);

        let max = buffer.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(max < 1e-10, "silent input should produce silent output");
    }
}
