//! Ambisonics B-format decorrelation stage.
//!
//! Adds spatial width to the B-format signal by mixing in short-delayed,
//! Z-rotated copies. Operates before the AmbisonicsDecodeStage (in B-format
//! domain, channels 0–3). No feedback — pure delay taps only.
//!
//! Late reverb is handled separately by FdnReverbStage (added after decode,
//! same architecture as VBAP/HRTF/DBAP).
//!
//! ## Signal flow (per sample)
//!
//! ```text
//! 1. Read dry B-format from main buffer
//! 2. Read 4 delayed taps from independent ring buffers
//! 3. Per-tap: lowpass → Z-rotation (72° × line index)
//! 4. Write dry input to each ring buffer (no feedback)
//! 5. Output: dry + depth/√N × Σ(rotated taps)
//! ```
//!
//! ## Why short delays?
//!
//! Rudrich (2016) used 100–300ms delays for decorrelating an already-diffuse
//! reverb tail. When applied to the direct B-format signal (as we do), those
//! long delays create audible discrete echoes. Delays of 7–23ms stay below
//! the echo threshold (~50ms) while providing effective spatial decorrelation.
//!
//! No-ops for <4 channel output (stereo bilateral mode doesn't use this).

use atrium_core::ambisonics::{foa_rotate_z, BFormat};

use crate::pipeline::mix_stage::{MixContext, MixStage};

/// Ring buffer size: 2048 samples ≈ 42.7 ms at 48 kHz (fits all taps).
const RING_SIZE: usize = 2048;
const RING_MASK: usize = RING_SIZE - 1;

/// Number of decorrelation delay lines.
const NUM_LINES: usize = 4;

/// Delay times in milliseconds. Mutually coprime to avoid modal patterns.
/// All below the 50ms echo threshold for imperceptible spatial widening.
const DELAY_MS: [f32; NUM_LINES] = [7.0, 11.0, 17.0, 23.0];

/// Per-line Z-rotation angles. Each line gets a different rotation to
/// distribute the delayed signal across different spatial directions.
/// Multiples of 72° (2π/5) — coprime with the 4 lines for maximal spread.
const ROTATE_ANGLES: [f32; NUM_LINES] = [
    72.0 * std::f32::consts::PI / 180.0,
    144.0 * std::f32::consts::PI / 180.0,
    216.0 * std::f32::consts::PI / 180.0,
    288.0 * std::f32::consts::PI / 180.0,
];

/// Decorrelation depth: controls the wet level of the delayed taps.
/// Each tap gets gain = DEPTH / √NUM_LINES. Total wet energy = DEPTH².
/// At 0.3: wet energy is 9% of dry (~-10.5 dB) — subtle spatial widening.
const DEPTH: f32 = 0.3;

/// One-pole lowpass filter for anti-aliasing the delayed taps.
#[derive(Clone, Copy)]
struct OnePole {
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

    fn set_lowpass(&mut self, cutoff_hz: f32, sample_rate: f32) {
        self.coeff = (-2.0 * std::f32::consts::PI * cutoff_hz / sample_rate).exp();
    }

    #[inline]
    fn process(&mut self, input: f32) -> f32 {
        self.state = input * (1.0 - self.coeff) + self.state * self.coeff;
        self.state
    }

    fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// B-format decorrelation stage (MixStage).
///
/// 4 independent delay lines with Z-rotation, no feedback.
/// Operates on channels 0–3 of the output buffer. No-op for <4 channels.
pub struct AmbiMultiDelayStage {
    /// Ring buffers: [line][foa_channel][sample].
    rings: Box<[[[f32; RING_SIZE]; 4]; NUM_LINES]>,
    /// Shared write position.
    write_pos: usize,
    /// Per-line delay lengths in samples.
    delay_samples: [usize; NUM_LINES],
    /// Per-line, per-channel lowpass filters.
    lp: [[OnePole; 4]; NUM_LINES],
    /// Per-tap gain: DEPTH / √NUM_LINES.
    tap_gain: f32,
    /// Whether init has been called.
    initialized: bool,
}

impl AmbiMultiDelayStage {
    pub fn new() -> Self {
        Self {
            rings: Box::new([[[0.0; RING_SIZE]; 4]; NUM_LINES]),
            write_pos: 0,
            delay_samples: [0; NUM_LINES],
            lp: [[OnePole::new(); 4]; NUM_LINES],
            tap_gain: DEPTH / (NUM_LINES as f32).sqrt(),
            initialized: false,
        }
    }
}

impl Default for AmbiMultiDelayStage {
    fn default() -> Self {
        Self::new()
    }
}

impl MixStage for AmbiMultiDelayStage {
    fn init(&mut self, ctx: &MixContext) {
        for (i, &ms) in DELAY_MS.iter().enumerate() {
            self.delay_samples[i] = ((ms / 1000.0) * ctx.sample_rate) as usize;
            if self.delay_samples[i] >= RING_SIZE {
                self.delay_samples[i] = RING_SIZE - 1;
            }
        }

        for line in 0..NUM_LINES {
            for ch in 0..4 {
                self.lp[line][ch].set_lowpass(6000.0, ctx.sample_rate);
            }
        }

        self.initialized = true;
    }

    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        if ctx.render_channels < 4 || !self.initialized {
            return;
        }

        let num_frames = buffer.len() / ctx.channels;
        let gain = self.tap_gain;

        for frame in 0..num_frames {
            let base = frame * ctx.channels;

            // Read dry B-format from main buffer.
            let dry = BFormat {
                w: buffer[base],
                y: buffer[base + 1],
                z: buffer[base + 2],
                x: buffer[base + 3],
            };

            // Sum delayed, filtered, rotated taps.
            let mut wet = BFormat {
                w: 0.0,
                y: 0.0,
                z: 0.0,
                x: 0.0,
            };

            for line in 0..NUM_LINES {
                let read_pos = (self.write_pos + RING_SIZE - self.delay_samples[line]) & RING_MASK;
                let mut tap = BFormat {
                    w: self.rings[line][0][read_pos],
                    y: self.rings[line][1][read_pos],
                    z: self.rings[line][2][read_pos],
                    x: self.rings[line][3][read_pos],
                };

                // Lowpass for anti-aliasing.
                tap.w = self.lp[line][0].process(tap.w);
                tap.y = self.lp[line][1].process(tap.y);
                tap.z = self.lp[line][2].process(tap.z);
                tap.x = self.lp[line][3].process(tap.x);

                // Z-rotation for spatial decorrelation.
                let rotated = foa_rotate_z(&tap, ROTATE_ANGLES[line]);

                wet.w += rotated.w;
                wet.y += rotated.y;
                wet.z += rotated.z;
                wet.x += rotated.x;
            }

            // Write dry input to ring buffers (no feedback).
            for line in 0..NUM_LINES {
                self.rings[line][0][self.write_pos] = dry.w;
                self.rings[line][1][self.write_pos] = dry.y;
                self.rings[line][2][self.write_pos] = dry.z;
                self.rings[line][3][self.write_pos] = dry.x;
            }

            self.write_pos = (self.write_pos + 1) & RING_MASK;

            // Output: dry + scaled wet.
            buffer[base] = dry.w + wet.w * gain;
            buffer[base + 1] = dry.y + wet.y * gain;
            buffer[base + 2] = dry.z + wet.z * gain;
            buffer[base + 3] = dry.x + wet.x * gain;
        }
    }

    fn reset(&mut self) {
        for line in self.rings.iter_mut() {
            for channel in line.iter_mut() {
                channel.fill(0.0);
            }
        }
        self.write_pos = 0;
        for line in 0..NUM_LINES {
            for ch in 0..4 {
                self.lp[line][ch].reset();
            }
        }
    }

    fn name(&self) -> &str {
        "ambi_decorrelation"
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

    fn test_mix_context<'a>(
        layout: &'a SpeakerLayout,
        listener: &'a Listener,
        channels: usize,
    ) -> MixContext<'a> {
        MixContext {
            listener,
            layout,
            sample_rate: 48000.0,
            channels,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            master_gain: 1.0,
            render_channels: channels,
            reverb_input: None,
            wall_reflectivity: 0.9,
        }
    }

    #[test]
    fn silent_input_silent_output() {
        let (layout, listener) = make_ctx(4);
        let mut stage = AmbiMultiDelayStage::new();
        let ctx = test_mix_context(&layout, &listener, 4);
        stage.init(&ctx);

        let mut buffer = vec![0.0f32; 4 * 512];
        stage.process(&mut buffer, &ctx);

        let max = buffer.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max < 1e-10,
            "silent input should produce silent output, got {max}"
        );
    }

    #[test]
    fn impulse_produces_delayed_taps() {
        let (layout, listener) = make_ctx(4);
        let mut stage = AmbiMultiDelayStage::new();
        let ctx = test_mix_context(&layout, &listener, 4);
        stage.init(&ctx);

        // Write an impulse in W channel at frame 0.
        let total_frames = 2048;
        let mut buffer = vec![0.0f32; 4 * total_frames];
        buffer[0] = 1.0; // W channel impulse

        stage.process(&mut buffer, &ctx);

        // Each line should produce output at its delay time.
        // At 48kHz: 336, 528, 816, 1104 samples.
        let tap_frames: Vec<usize> = DELAY_MS
            .iter()
            .map(|&ms| ((ms / 1000.0) * 48000.0) as usize)
            .collect();

        for &tap_frame in &tap_frames {
            if tap_frame < total_frames {
                // Check any FOA channel — rotation distributes energy across W/Y/X.
                let energy_at_tap: f32 = (0..4).map(|ch| buffer[tap_frame * 4 + ch].powi(2)).sum();
                assert!(
                    energy_at_tap > 1e-6,
                    "should have energy at tap frame {tap_frame}, got {energy_at_tap}"
                );
            }
        }
    }

    #[test]
    fn rotation_redistributes_energy() {
        let (layout, listener) = make_ctx(4);
        let mut stage = AmbiMultiDelayStage::new();
        let ctx = test_mix_context(&layout, &listener, 4);
        stage.init(&ctx);

        // Impulse in X channel only (front direction).
        let total_frames = 2048;
        let mut buffer = vec![0.0f32; 4 * total_frames];
        buffer[3] = 1.0; // X channel impulse

        stage.process(&mut buffer, &ctx);

        // After the first tap (7ms = 336 samples), Y channel should have
        // energy from the rotated X input.
        let y_energy: f32 = buffer[336 * 4..]
            .iter()
            .skip(1)
            .step_by(4)
            .map(|s| s * s)
            .sum();
        assert!(
            y_energy > 1e-8,
            "rotation should redistribute X energy into Y channel, got Y energy {y_energy}"
        );
    }

    #[test]
    fn noop_for_stereo() {
        let (layout, listener) = make_ctx(2);
        let mut stage = AmbiMultiDelayStage::new();
        let ctx = test_mix_context(&layout, &listener, 2);
        stage.init(&ctx);

        let mut buffer = vec![0.5f32; 2 * 256];
        let original = buffer.clone();
        stage.process(&mut buffer, &ctx);

        assert_eq!(buffer, original, "stereo buffer should be unchanged");
    }

    /// No feedback means no lingering tail: energy dies after the last tap.
    #[test]
    fn no_feedback_tail() {
        let (layout, listener) = make_ctx(4);
        let mut stage = AmbiMultiDelayStage::new();
        let ctx = test_mix_context(&layout, &listener, 4);
        stage.init(&ctx);

        // Impulse in first buffer.
        let frames = 2048;
        let mut buf1 = vec![0.0f32; 4 * frames];
        buf1[0] = 1.0;
        stage.process(&mut buf1, &ctx);

        // Second buffer (all zeros input): should have no energy because
        // without feedback, taps only reproduce what was written to the ring.
        // The impulse from buf1 was at frame 0, and all taps are < 23ms = 1104 samples.
        // By the start of buf2, all taps have already been read.
        let mut buf2 = vec![0.0f32; 4 * frames];
        stage.process(&mut buf2, &ctx);

        let max = buf2.iter().map(|s| s.abs()).fold(0.0f32, f32::max);
        assert!(
            max < 1e-6,
            "no feedback should mean no lingering tail, got max {max}"
        );
    }

    /// Decorrelation adds energy (wet > dry) but within the expected depth.
    #[test]
    fn decorrelation_adds_bounded_energy() {
        let (layout, listener) = make_ctx(4);
        let mut stage = AmbiMultiDelayStage::new();
        let ctx = test_mix_context(&layout, &listener, 4);
        stage.init(&ctx);

        // Constant signal in W channel for 2048 frames.
        let frames = 2048;
        let mut buffer = vec![0.0f32; 4 * frames];
        for frame in 0..frames {
            buffer[frame * 4] = 0.5; // W
        }
        let dry_energy: f32 = buffer.iter().map(|s| s * s).sum();

        stage.process(&mut buffer, &ctx);

        let wet_energy: f32 = buffer.iter().map(|s| s * s).sum();

        // Wet energy should exceed dry (decorrelation adds the taps).
        assert!(
            wet_energy > dry_energy,
            "decorrelation should add energy: wet={wet_energy}, dry={dry_energy}"
        );
        // With a W-only constant signal, Z-rotation preserves W, so all 4 taps
        // add coherently: output W = dry + 4 × tap_gain × dry. Worst case ratio
        // is (1 + 4 × 0.15)² / 1 ≈ 2.56. Allow up to 3× for safety.
        assert!(
            wet_energy < dry_energy * 3.0,
            "decorrelation energy should be bounded: wet={wet_energy}, dry={dry_energy}"
        );
    }
}
