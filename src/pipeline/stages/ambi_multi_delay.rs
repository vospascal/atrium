//! Ambisonics Feedback Delay Network (FOA).
//!
//! A proper 4-line FDN operating in the First Order Ambisonics domain,
//! inspired by Rudrich et al. 2016 (TMT16, Section 4.2) but corrected
//! to use independent delay lines coupled through a unitary mixing matrix.
//!
//! ## Architecture
//!
//! 4 independent delay lines, each carrying a full B-format signal (W, Y, Z, X).
//! Lines are coupled through a normalized 4×4 Hadamard matrix, which distributes
//! energy between lines without adding or removing it (unitary / energy-preserving).
//!
//! ## Signal flow (per sample)
//!
//! ```text
//! 1. Read delayed output from each of the 4 lines
//! 2. Wet output = normalized sum of line outputs
//! 3. Per-line Jot absorption gain (shorter lines → higher gain)
//! 4. Hadamard mixing across lines (per FOA channel independently)
//! 5. Per-line tone shaping: LP 6 kHz → HP 200 Hz
//! 6. Per-line FOA Z-rotation (72°) for spatial decorrelation
//! 7. Write back: dry input + processed feedback → each line
//! 8. Output: dry + wet × wet_gain
//! ```
//!
//! ## Feedback gain (Jot & Chaigne, AES 1991)
//!
//! Each line's gain is computed from room geometry:
//!
//! 1. **Sabine** (1898): RT60 = 0.161 × V / (S × (1 - reflectivity))
//! 2. **Jot**: g_i = 10^(-3 × d_i / RT60)
//!
//! Because each line is truly independent (its own delay, own gain, own filters),
//! Jot's formula gives exact RT60 control: after RT60 seconds, each line has
//! decayed by exactly 60 dB regardless of its delay length.
//!
//! ## Hadamard mixing matrix
//!
//! H₄ = 0.5 × [[1, 1, 1, 1], [1,-1, 1,-1], [1, 1,-1,-1], [1,-1,-1, 1]]
//!
//! Unitary: H × Hᵀ = I. Maximizes echo density by distributing energy from
//! each line equally into all 4 lines with different sign patterns.
//! Reference: Jot & Chaigne (AES 1991), Rocchesso & Smith (1997).
//!
//! No-ops for <4 channel output (stereo bilateral mode doesn't use this).

use atrium_core::ambisonics::{foa_rotate_z, BFormat};

use crate::pipeline::mix_stage::{MixContext, MixStage};
use crate::pipeline::room_acoustics;

/// Ring buffer size: 16384 samples ≈ 341 ms at 48 kHz.
const RING_SIZE: usize = 16384;
const RING_MASK: usize = RING_SIZE - 1;

/// Number of independent FDN delay lines.
const NUM_LINES: usize = 4;

/// Delay times in milliseconds per line (Rudrich Sec. 4.2).
const DELAY_MS: [f32; NUM_LINES] = [100.0, 167.0, 233.0, 300.0];

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

/// Apply normalized 4×4 Hadamard matrix in-place across delay lines.
///
/// Each FOA channel (W, Y, Z, X) is mixed independently across the 4 lines.
/// H₄ = 0.5 × [[1,1,1,1],[1,-1,1,-1],[1,1,-1,-1],[1,-1,-1,1]]
///
/// Unitary: preserves total energy across lines (H × Hᵀ = I).
/// This is the core of the FDN — it creates echo density by distributing
/// each line's energy into all 4 lines with different sign patterns.
#[inline]
fn hadamard_mix(lines: &mut [BFormat; NUM_LINES]) {
    // Process each FOA component independently across lines.
    macro_rules! mix_component {
        ($field:ident) => {
            let a = lines[0].$field;
            let b = lines[1].$field;
            let c = lines[2].$field;
            let d = lines[3].$field;
            lines[0].$field = 0.5 * (a + b + c + d);
            lines[1].$field = 0.5 * (a - b + c - d);
            lines[2].$field = 0.5 * (a + b - c - d);
            lines[3].$field = 0.5 * (a - b - c + d);
        };
    }
    mix_component!(w);
    mix_component!(y);
    mix_component!(z);
    mix_component!(x);
}

/// Ambisonics Feedback Delay Network (MixStage).
///
/// 4 independent delay lines carrying B-format, coupled through a Hadamard matrix.
/// Operates on channels 0–3 of the output buffer. No-op for <4 channels.
pub struct AmbiMultiDelayStage {
    /// Independent delay line ring buffers: [line][foa_channel][sample].
    rings: Box<[[[f32; RING_SIZE]; 4]; NUM_LINES]>,
    /// Shared write position (all lines advance together).
    write_pos: usize,
    /// Per-line delay lengths in samples.
    delay_samples: [usize; NUM_LINES],
    /// Per-line Jot feedback gains, computed dynamically in init()
    /// from room geometry via Sabine RT60 + Jot absorptive delay gain.
    /// Each line gets its own gain: g_i = 10^(-3 × d_i / RT60).
    feedback_gains: [f32; NUM_LINES],
    /// Per-line, per-channel lowpass filters [line][channel].
    lp: [[OnePole; 4]; NUM_LINES],
    /// Per-line, per-channel highpass filters [line][channel].
    hp: [[OnePole; 4]; NUM_LINES],
    /// Wall reflectivity (0.0–1.0) from room config.
    wall_reflectivity: f32,
    /// Whether init has been called.
    initialized: bool,
}

impl AmbiMultiDelayStage {
    pub fn new(wall_reflectivity: f32) -> Self {
        Self {
            rings: Box::new([[[0.0; RING_SIZE]; 4]; NUM_LINES]),
            write_pos: 0,
            delay_samples: [0; NUM_LINES],
            feedback_gains: [0.0; NUM_LINES],
            lp: [[OnePole::new(); 4]; NUM_LINES],
            hp: [[OnePole::new(); 4]; NUM_LINES],
            wall_reflectivity,
            initialized: false,
        }
    }
}

impl MixStage for AmbiMultiDelayStage {
    fn init(&mut self, ctx: &MixContext) {
        // Compute per-line delay lengths in samples.
        for (i, &ms) in DELAY_MS.iter().enumerate() {
            self.delay_samples[i] = ((ms / 1000.0) * ctx.sample_rate) as usize;
            if self.delay_samples[i] >= RING_SIZE {
                self.delay_samples[i] = RING_SIZE - 1;
            }
        }

        // Configure per-line tone-shaping filters (LP 6kHz, HP 200Hz).
        for line in 0..NUM_LINES {
            for ch in 0..4 {
                self.lp[line][ch].set_lowpass(6000.0, ctx.sample_rate);
                self.hp[line][ch].set_highpass(200.0, ctx.sample_rate);
            }
        }

        // Compute per-line Jot feedback gains from room geometry.
        // Each line gets its own gain based on its delay length.
        // In this FDN topology, each line recirculates independently,
        // so Jot's formula gives exact RT60 control.
        let delay_times_seconds: Vec<f32> = DELAY_MS.iter().map(|&ms| ms / 1000.0).collect();
        let (gains, _rt60) = room_acoustics::compute_feedback_gains(
            ctx.room_min,
            ctx.room_max,
            self.wall_reflectivity,
            &delay_times_seconds,
        );
        for (i, &gain) in gains.iter().enumerate() {
            self.feedback_gains[i] = gain;
        }

        self.initialized = true;
    }

    #[allow(clippy::needless_range_loop)]
    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        // No-op for <4 render channels (stereo bilateral mode).
        if ctx.render_channels < 4 || !self.initialized {
            return;
        }

        let num_frames = buffer.len() / ctx.channels;

        for frame in 0..num_frames {
            let base = frame * ctx.channels;

            // Read dry B-format from main buffer.
            let dry = BFormat {
                w: buffer[base],
                y: buffer[base + 1],
                z: buffer[base + 2],
                x: buffer[base + 3],
            };

            // Read FDN injection from reverb send buffer (distance-weighted per source).
            // Falls back to main buffer if no reverb send is available.
            let inject = match ctx.reverb_input {
                Some(rev) => BFormat {
                    w: rev[base],
                    y: rev[base + 1],
                    z: rev[base + 2],
                    x: rev[base + 3],
                },
                None => dry,
            };

            // 1. Read delayed output from each independent line.
            let mut line_outputs = [BFormat {
                w: 0.0,
                y: 0.0,
                z: 0.0,
                x: 0.0,
            }; NUM_LINES];

            for line in 0..NUM_LINES {
                let read_pos = (self.write_pos + RING_SIZE - self.delay_samples[line]) & RING_MASK;
                line_outputs[line] = BFormat {
                    w: self.rings[line][0][read_pos],
                    y: self.rings[line][1][read_pos],
                    z: self.rings[line][2][read_pos],
                    x: self.rings[line][3][read_pos],
                };
            }

            // 2. Wet output = normalized sum of all line outputs.
            let mut wet = BFormat {
                w: 0.0,
                y: 0.0,
                z: 0.0,
                x: 0.0,
            };
            for line in 0..NUM_LINES {
                wet.w += line_outputs[line].w;
                wet.y += line_outputs[line].y;
                wet.z += line_outputs[line].z;
                wet.x += line_outputs[line].x;
            }
            let output_norm = 1.0 / NUM_LINES as f32;
            wet.w *= output_norm;
            wet.y *= output_norm;
            wet.z *= output_norm;
            wet.x *= output_norm;

            // 3. Apply per-line Jot absorption gain.
            // Each line's gain is exact: g_i = 10^(-3 × d_i / RT60).
            for line in 0..NUM_LINES {
                let gain = self.feedback_gains[line];
                line_outputs[line].w *= gain;
                line_outputs[line].y *= gain;
                line_outputs[line].z *= gain;
                line_outputs[line].x *= gain;
            }

            // 4. Hadamard mixing: distribute energy across lines (per FOA channel).
            hadamard_mix(&mut line_outputs);

            // 5–6. Per-line: tone shaping (LP → HP) then FOA rotation.
            for line in 0..NUM_LINES {
                let b = &mut line_outputs[line];
                b.w = self.hp[line][0].process_hp(self.lp[line][0].process_lp(b.w));
                b.y = self.hp[line][1].process_hp(self.lp[line][1].process_lp(b.y));
                b.z = self.hp[line][2].process_hp(self.lp[line][2].process_lp(b.z));
                b.x = self.hp[line][3].process_hp(self.lp[line][3].process_lp(b.x));
                line_outputs[line] = foa_rotate_z(b, ROTATE_ANGLE);
            }

            // 7. Write back: reverb input (distance-weighted) + feedback → each line.
            for line in 0..NUM_LINES {
                self.rings[line][0][self.write_pos] = inject.w + line_outputs[line].w;
                self.rings[line][1][self.write_pos] = inject.y + line_outputs[line].y;
                self.rings[line][2][self.write_pos] = inject.z + line_outputs[line].z;
                self.rings[line][3][self.write_pos] = inject.x + line_outputs[line].x;
            }

            self.write_pos = (self.write_pos + 1) & RING_MASK;

            // 8. Output: dry (from main buffer) + wet.
            buffer[base] = dry.w + wet.w;
            buffer[base + 1] = dry.y + wet.y;
            buffer[base + 2] = dry.z + wet.z;
            buffer[base + 3] = dry.x + wet.x;
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
                self.hp[line][ch].reset();
            }
        }
    }

    fn name(&self) -> &str {
        "ambi_fdn"
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
        }
    }

    #[test]
    fn silent_input_silent_output() {
        let (layout, listener) = make_ctx(4);
        let mut stage = AmbiMultiDelayStage::new(0.3);
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
        let mut stage = AmbiMultiDelayStage::new(0.5);
        let ctx = test_mix_context(&layout, &listener, 4);
        stage.init(&ctx);

        // Write an impulse in W channel at frame 0.
        let total_frames = 16384;
        let mut buffer = vec![0.0f32; 4 * total_frames];
        buffer[0] = 1.0; // W channel impulse

        stage.process(&mut buffer, &ctx);

        // Each independent line should produce output at its delay time.
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
        let ctx = test_mix_context(&layout, &listener, 4);
        stage.init(&ctx);

        // Impulse in X channel only (front direction).
        let frames_per_buf = 16000;
        let mut buf1 = vec![0.0f32; 4 * frames_per_buf];
        buf1[3] = 1.0; // X channel impulse
        stage.process(&mut buf1, &ctx);

        // Second buffer: feedback from first pass has been rotated by 72°.
        let mut buf2 = vec![0.0f32; 4 * frames_per_buf];
        stage.process(&mut buf2, &ctx);

        // Y channel should now have energy from the rotated feedback.
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
        let ctx = test_mix_context(&layout, &listener, 2);
        stage.init(&ctx);

        let mut buffer = vec![0.5f32; 2 * 256];
        let original = buffer.clone();
        stage.process(&mut buffer, &ctx);

        assert_eq!(buffer, original, "stereo buffer should be unchanged");
    }

    #[test]
    fn hadamard_preserves_energy() {
        // Verify H × Hᵀ = I by checking energy conservation.
        let mut lines = [
            BFormat {
                w: 1.0,
                y: 0.5,
                z: -0.3,
                x: 0.7,
            },
            BFormat {
                w: -0.2,
                y: 0.8,
                z: 0.1,
                x: -0.4,
            },
            BFormat {
                w: 0.6,
                y: -0.1,
                z: 0.9,
                x: 0.3,
            },
            BFormat {
                w: -0.5,
                y: 0.4,
                z: -0.7,
                x: 0.2,
            },
        ];

        // Measure total energy before.
        let energy_before: f32 = lines
            .iter()
            .map(|b| b.w * b.w + b.y * b.y + b.z * b.z + b.x * b.x)
            .sum();

        hadamard_mix(&mut lines);

        // Measure total energy after.
        let energy_after: f32 = lines
            .iter()
            .map(|b| b.w * b.w + b.y * b.y + b.z * b.z + b.x * b.x)
            .sum();

        assert!(
            (energy_before - energy_after).abs() < 1e-6,
            "Hadamard should preserve energy: before={energy_before}, after={energy_after}"
        );
    }

    #[test]
    fn hadamard_is_involution() {
        // H × H = I for the normalized Hadamard: applying it twice returns the original.
        let original = [
            BFormat {
                w: 1.0,
                y: 0.5,
                z: -0.3,
                x: 0.7,
            },
            BFormat {
                w: -0.2,
                y: 0.8,
                z: 0.1,
                x: -0.4,
            },
            BFormat {
                w: 0.6,
                y: -0.1,
                z: 0.9,
                x: 0.3,
            },
            BFormat {
                w: -0.5,
                y: 0.4,
                z: -0.7,
                x: 0.2,
            },
        ];

        let mut lines = original;
        hadamard_mix(&mut lines);
        hadamard_mix(&mut lines);

        for (i, (orig, result)) in original.iter().zip(lines.iter()).enumerate() {
            assert!(
                (orig.w - result.w).abs() < 1e-6
                    && (orig.y - result.y).abs() < 1e-6
                    && (orig.z - result.z).abs() < 1e-6
                    && (orig.x - result.x).abs() < 1e-6,
                "line {i}: H×H should return original, got delta w={} y={} z={} x={}",
                (orig.w - result.w).abs(),
                (orig.y - result.y).abs(),
                (orig.z - result.z).abs(),
                (orig.x - result.x).abs(),
            );
        }
    }

    #[test]
    fn independent_lines_have_distinct_content() {
        // Verify that independent lines accumulate different content over time,
        // confirming the FDN topology (not a shared multi-tap loop).
        let (layout, listener) = make_ctx(4);
        let mut stage = AmbiMultiDelayStage::new(0.5);
        let ctx = test_mix_context(&layout, &listener, 4);
        stage.init(&ctx);

        // Inject impulse and process two full buffers to let feedback circulate.
        let frames = 16384;
        let mut buffer = vec![0.0f32; 4 * frames];
        buffer[0] = 1.0;
        stage.process(&mut buffer, &ctx);

        let mut buffer2 = vec![0.0f32; 4 * frames];
        stage.process(&mut buffer2, &ctx);

        // Compare total energy across a range of ring buffer positions.
        // Each line should have different total energy because of different
        // delay lengths and Jot gains.
        let mut line_energies = [0.0f32; NUM_LINES];
        for line in 0..NUM_LINES {
            for sample in 0..RING_SIZE {
                for ch in 0..4 {
                    line_energies[line] += stage.rings[line][ch][sample].powi(2);
                }
            }
        }

        // All lines should have energy (Hadamard distributes input to all lines).
        for (line, &energy) in line_energies.iter().enumerate() {
            assert!(
                energy > 1e-8,
                "line {line} should have accumulated energy, got {energy:.10}"
            );
        }

        // Lines should have different total energies (different gains).
        // Line 0 (100ms, highest gain) should have more energy than line 3 (300ms, lowest gain).
        assert!(
            line_energies[0] > line_energies[3],
            "line 0 (shorter delay, higher gain) should have more energy ({}) than line 3 ({})",
            line_energies[0],
            line_energies[3]
        );
    }
}
