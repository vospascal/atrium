//! Multi-Delay Spatial Ambisonics Effect (FOA).
//!
//! Adapted from Rudrich et al. 2016 (TMT16, Section 4.2, Fig. 4).
//! The original paper uses 36 feedback delay lines in a 5th-order SH domain.
//! This implementation faithfully adapts the architecture to 4-channel FOA
//! (W, Y, Z, X), accepting weaker spatial diffusion as a trade-off.
//!
//! Signal flow (per-sample, 4 FOA channels):
//!   B-format [W,Y,Z,X] → 4 feedback delay lines (100/167/233/300 ms)
//!     → feedback attenuation: 10^(-2.4/20) ≈ 0.759
//!     → one-pole LP (6 kHz) per channel
//!     → one-pole HP (200 Hz) per channel
//!     → foa_rotate_z(72°)
//!     → write back to delay lines (feedback)
//!   wet output mixed with dry B-format → decode to speakers
//!
//! No-ops for <4 channel output (stereo bilateral mode doesn't use this).

use atrium_core::ambisonics::{foa_rotate_z, BFormat};

use crate::pipeline::mix_stage::{MixContext, MixStage};

/// Ring buffer size: 16384 samples ≈ 341 ms at 48 kHz.
const RING_SIZE: usize = 16384;
const RING_MASK: usize = RING_SIZE - 1;

/// Number of delay taps (Rudrich: evenly spaced in 100–300 ms range).
const NUM_TAPS: usize = 4;

/// Delay times in milliseconds (Rudrich Sec. 4.2).
const DELAY_MS: [f32; NUM_TAPS] = [100.0, 167.0, 233.0, 300.0];

/// Feedback attenuation per iteration: 10^(-2.4/20) ≈ 0.759 (Rudrich: "2.4 dB").
const FEEDBACK_GAIN: f32 = 0.759;

/// Z-axis rotation per feedback iteration: 72° = 2π/5 (Rudrich Sec. 4.2).
const ROTATE_ANGLE: f32 = 72.0 * std::f32::consts::PI / 180.0;

/// One-pole filter for tone shaping in feedback loop.
#[derive(Clone, Copy)]
struct OnePole {
    /// Filter coefficient (0..1). Higher = more smoothing.
    coeff: f32,
    state: f32,
}

impl OnePole {
    fn new() -> Self {
        Self {
            coeff: 0.0,
            state: 0.0,
        }
    }

    /// Configure as lowpass: coeff = exp(-2π·fc/sr).
    fn set_lowpass(&mut self, cutoff_hz: f32, sample_rate: f32) {
        self.coeff = (-2.0 * std::f32::consts::PI * cutoff_hz / sample_rate).exp();
    }

    /// Configure as highpass: same one-pole, output = input - lowpass(input).
    fn set_highpass(&mut self, cutoff_hz: f32, sample_rate: f32) {
        self.coeff = (-2.0 * std::f32::consts::PI * cutoff_hz / sample_rate).exp();
    }

    #[inline]
    fn process_lp(&mut self, input: f32) -> f32 {
        self.state = input * (1.0 - self.coeff) + self.state * self.coeff;
        self.state
    }

    #[inline]
    fn process_hp(&mut self, input: f32) -> f32 {
        let lp = self.process_lp(input);
        input - lp
    }

    fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// Multi-Delay Spatial Ambisonics FX (MixStage).
///
/// Operates on B-format in channels 0–3 of the output buffer.
/// For <4 channels, this stage is a no-op.
pub struct AmbiMultiDelayStage {
    /// Per-channel ring buffers [channel][sample]. 4 FOA channels.
    rings: Box<[[f32; RING_SIZE]; 4]>,
    /// Write position in ring buffers.
    write_pos: usize,
    /// Delay lengths in samples (computed from DELAY_MS and sample rate).
    delay_samples: [usize; NUM_TAPS],
    /// Per-channel lowpass filters (4 channels).
    lp: [OnePole; 4],
    /// Per-channel highpass filters (4 channels).
    hp: [OnePole; 4],
    /// Wet/dry mix (0.0 = dry only, 1.0 = full wet).
    wet_gain: f32,
    /// Whether init has been called.
    initialized: bool,
}

impl AmbiMultiDelayStage {
    pub fn new(wet_gain: f32) -> Self {
        Self {
            rings: Box::new([[0.0; RING_SIZE]; 4]),
            write_pos: 0,
            delay_samples: [0; NUM_TAPS],
            lp: [OnePole::new(); 4],
            hp: [OnePole::new(); 4],
            wet_gain,
            initialized: false,
        }
    }
}

impl MixStage for AmbiMultiDelayStage {
    fn init(&mut self, ctx: &MixContext) {
        // Compute delay lengths in samples from ms.
        for (i, &ms) in DELAY_MS.iter().enumerate() {
            self.delay_samples[i] = ((ms / 1000.0) * ctx.sample_rate) as usize;
            // Clamp to ring buffer size.
            if self.delay_samples[i] >= RING_SIZE {
                self.delay_samples[i] = RING_SIZE - 1;
            }
        }

        // Configure tone-shaping filters (Rudrich: LP 6kHz, HP 200Hz).
        for lp in &mut self.lp {
            lp.set_lowpass(6000.0, ctx.sample_rate);
        }
        for hp in &mut self.hp {
            hp.set_highpass(200.0, ctx.sample_rate);
        }

        self.initialized = true;
    }

    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        // No-op for <4 channels (stereo bilateral mode).
        if ctx.channels < 4 || !self.initialized {
            return;
        }

        let num_frames = buffer.len() / ctx.channels;

        for frame in 0..num_frames {
            let base = frame * ctx.channels;

            // Read dry B-format from buffer.
            let dry = BFormat {
                w: buffer[base],
                y: buffer[base + 1],
                z: buffer[base + 2],
                x: buffer[base + 3],
            };

            // Sum wet output from all delay taps.
            let mut wet = BFormat {
                w: 0.0,
                y: 0.0,
                z: 0.0,
                x: 0.0,
            };

            for tap in 0..NUM_TAPS {
                let read_pos = (self.write_pos + RING_SIZE - self.delay_samples[tap]) & RING_MASK;
                let tap_b = BFormat {
                    w: self.rings[0][read_pos],
                    y: self.rings[1][read_pos],
                    z: self.rings[2][read_pos],
                    x: self.rings[3][read_pos],
                };
                wet.w += tap_b.w;
                wet.y += tap_b.y;
                wet.z += tap_b.z;
                wet.x += tap_b.x;
            }

            // Scale wet by 1/NUM_TAPS to normalize tap sum.
            let tap_scale = 1.0 / NUM_TAPS as f32;
            wet.w *= tap_scale;
            wet.y *= tap_scale;
            wet.z *= tap_scale;
            wet.x *= tap_scale;

            // Feedback path: attenuate → LP → HP → rotate → write to ring.
            // Uses last tap's output as feedback source (longest delay, per Rudrich).
            let fb_read =
                (self.write_pos + RING_SIZE - self.delay_samples[NUM_TAPS - 1]) & RING_MASK;
            let mut fb = BFormat {
                w: self.rings[0][fb_read],
                y: self.rings[1][fb_read],
                z: self.rings[2][fb_read],
                x: self.rings[3][fb_read],
            };

            // Feedback attenuation (Rudrich: 2.4 dB per iteration).
            fb.w *= FEEDBACK_GAIN;
            fb.y *= FEEDBACK_GAIN;
            fb.z *= FEEDBACK_GAIN;
            fb.x *= FEEDBACK_GAIN;

            // Tone shaping: LP 6kHz then HP 200Hz per channel.
            fb.w = self.hp[0].process_hp(self.lp[0].process_lp(fb.w));
            fb.y = self.hp[1].process_hp(self.lp[1].process_lp(fb.y));
            fb.z = self.hp[2].process_hp(self.lp[2].process_lp(fb.z));
            fb.x = self.hp[3].process_hp(self.lp[3].process_lp(fb.x));

            // Spatial rotation: 72° Z-axis rotation (Rudrich Sec. 4.2).
            let fb_rotated = foa_rotate_z(&fb, ROTATE_ANGLE);

            // Write to ring: dry input + rotated feedback.
            self.rings[0][self.write_pos] = dry.w + fb_rotated.w;
            self.rings[1][self.write_pos] = dry.y + fb_rotated.y;
            self.rings[2][self.write_pos] = dry.z + fb_rotated.z;
            self.rings[3][self.write_pos] = dry.x + fb_rotated.x;

            self.write_pos = (self.write_pos + 1) & RING_MASK;

            // Mix wet into output buffer.
            buffer[base] = dry.w + wet.w * self.wet_gain;
            buffer[base + 1] = dry.y + wet.y * self.wet_gain;
            buffer[base + 2] = dry.z + wet.z * self.wet_gain;
            buffer[base + 3] = dry.x + wet.x * self.wet_gain;
        }
    }

    fn reset(&mut self) {
        for ring in self.rings.iter_mut() {
            ring.fill(0.0);
        }
        self.write_pos = 0;
        for lp in &mut self.lp {
            lp.reset();
        }
        for hp in &mut self.hp {
            hp.reset();
        }
    }

    fn name(&self) -> &str {
        "ambi_multi_delay"
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use atrium_core::listener::Listener;
    use atrium_core::speaker::SpeakerLayout;
    use atrium_core::types::Vec3;

    fn make_ctx(channels: usize) -> (SpeakerLayout, Listener) {
        let layout = SpeakerLayout::new(&[], None, channels);
        let listener = Listener::new(Vec3::ZERO, 0.0);
        (layout, listener)
    }

    #[test]
    fn silent_input_silent_output() {
        let (layout, listener) = make_ctx(4);
        let mut stage = AmbiMultiDelayStage::new(0.3);
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 4,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
        };
        stage.init(&ctx);

        let mut buffer = vec![0.0f32; 4 * 512];
        stage.process(&mut buffer, &ctx);

        // All zeros in → all zeros out (no self-oscillation).
        let max = buffer.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max < 1e-10,
            "silent input should produce silent output, got {max}"
        );
    }

    #[test]
    fn impulse_produces_delayed_taps() {
        let (layout, listener) = make_ctx(4);
        let mut stage = AmbiMultiDelayStage::new(0.5);
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 4,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
        };
        stage.init(&ctx);

        // Write an impulse in W channel at frame 0.
        let total_frames = 16384;
        let mut buffer = vec![0.0f32; 4 * total_frames];
        buffer[0] = 1.0; // W channel impulse

        stage.process(&mut buffer, &ctx);

        // Check that delayed taps appear around 100ms, 167ms, 233ms, 300ms.
        // At 48kHz: 4800, 8016, 11184, 14400 samples.
        let tap_frames: Vec<usize> = DELAY_MS
            .iter()
            .map(|&ms| ((ms / 1000.0) * 48000.0) as usize)
            .collect();

        for &tap_frame in &tap_frames {
            if tap_frame < total_frames {
                let w_at_tap = buffer[tap_frame * 4].abs();
                assert!(
                    w_at_tap > 1e-4,
                    "W channel should have energy at tap frame {tap_frame}, got {w_at_tap}"
                );
            }
        }
    }

    #[test]
    fn rotation_redistributes_energy() {
        let (layout, listener) = make_ctx(4);
        let mut stage = AmbiMultiDelayStage::new(0.5);
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 4,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
        };
        stage.init(&ctx);

        // Impulse in X channel only (front direction).
        // Process two buffers: first writes input to ring, second reads feedback echoes.
        let frames_per_buf = 16000; // ~333ms, enough for longest delay (300ms)
        let mut buf1 = vec![0.0f32; 4 * frames_per_buf];
        buf1[3] = 1.0; // X channel impulse
        stage.process(&mut buf1, &ctx);

        // Second buffer: feedback from first pass has been rotated by 72°.
        // The longest delay tap (300ms = 14400 samples) fed back rotated content.
        let mut buf2 = vec![0.0f32; 4 * frames_per_buf];
        stage.process(&mut buf2, &ctx);

        // Y channel should now have energy from the rotated feedback echoes.
        let y_energy: f32 = buf2.iter().skip(1).step_by(4).map(|s| s * s).sum();
        assert!(
            y_energy > 1e-8,
            "rotation should redistribute X energy into Y channel, got Y energy {y_energy}"
        );
    }

    #[test]
    fn noop_for_stereo() {
        let (layout, listener) = make_ctx(2);
        let mut stage = AmbiMultiDelayStage::new(0.3);
        let ctx = MixContext {
            listener: &listener,
            layout: &layout,
            sample_rate: 48000.0,
            channels: 2,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
        };
        stage.init(&ctx);

        let mut buffer = vec![0.5f32; 2 * 256];
        let original = buffer.clone();
        stage.process(&mut buffer, &ctx);

        assert_eq!(buffer, original, "stereo buffer should be unchanged");
    }
}
