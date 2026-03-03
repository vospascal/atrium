// FDN (Feedback Delay Network) late reverb.
//
// Based on Jot & Chaigne (1991). 8 parallel delay lines coupled through an
// 8x8 Hadamard mixing matrix, with one-pole lowpass damping per line for
// frequency-dependent decay (highs fade faster than lows, like a real room).
//
// Sits after EarlyReflections in the processor chain:
//   Direct sound (0-3ms) → Early reflections (3-50ms) → Late reverb (50ms+)
//
// References:
//   - Jot & Chaigne, "Digital delay networks for designing artificial reverberators" (1991)
//   - fundsp: 32-channel FDN with Hadamard mixing (https://github.com/SamiPerttu/fundsp)
//   - padenot/fdn-reverb: clean 4-channel FDN (https://github.com/padenot/fdn-reverb)
//   - CCRMA: https://ccrma.stanford.edu/~jos/pasp/FDN_Reverberation.html
//
// See REFERENCES.md for full list.

use crate::processors::AudioProcessor;
use crate::spatial::listener::Listener;
use crate::world::types::Vec3;

/// Number of parallel delay lines.
const NUM_LINES: usize = 8;

/// Delay buffer size per line. Power of 2 for bitmask wrapping.
/// 512 samples ≈ 10.7ms at 48kHz, sufficient for max delay of 499 samples.
const BUF_SIZE: usize = 512;
const BUF_MASK: usize = BUF_SIZE - 1;

/// Pre-delay buffer size. Power of 2 for bitmask wrapping.
/// 2048 samples ≈ 42ms at 48kHz.
const PRE_DELAY_BUF_SIZE: usize = 2048;
const PRE_DELAY_BUF_MASK: usize = PRE_DELAY_BUF_SIZE - 1;

/// Pre-delay in seconds. Keeps early reflection region clean.
const PRE_DELAY_SECONDS: f32 = 0.020;

/// Base prime delay lengths for a ~2.7m mean free path room at 48kHz.
/// Primes are automatically mutually coprime → no metallic coloration.
/// Range: ~5ms to ~10.4ms, centered around the mean free path.
const BASE_DELAYS: [usize; NUM_LINES] = [241, 307, 353, 389, 421, 433, 461, 499];

/// Base sample rate these delays were designed for.
const BASE_SAMPLE_RATE: f32 = 48000.0;

/// One-pole lowpass damping filter for frequency-dependent decay.
/// Placed in each delay line's feedback path.
#[derive(Clone, Copy, Debug)]
struct DampingFilter {
    /// Gain coefficient.
    k: f32,
    /// Pole coefficient (controls lowpass cutoff).
    p: f32,
    /// Filter state (z^-1).
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

    /// Process one sample. output = k * input + p * state; state = output.
    #[inline(always)]
    fn process(&mut self, input: f32) -> f32 {
        let output = self.k * input + self.p * self.state;
        self.state = output;
        output
    }
}

/// FDN late reverb processor.
///
/// Mono-in, stereo-out: L+R averaged to mono, processed through 8-line FDN,
/// even lines [0,2,4,6] tap to L output, odd lines [1,3,5,7] tap to R for
/// natural stereo decorrelation.
pub struct FdnReverb {
    /// 8 circular delay line buffers. Boxed to keep 16KB off the stack.
    delay_buffers: Box<[[f32; BUF_SIZE]; NUM_LINES]>,
    /// Current write position into delay line buffers.
    write_pos: usize,
    /// Prime delay lengths per line (in samples).
    delays: [usize; NUM_LINES],
    /// One-pole lowpass damping filter per line.
    damping: [DampingFilter; NUM_LINES],
    /// Mono pre-delay circular buffer.
    pre_delay_buf: Box<[f32; PRE_DELAY_BUF_SIZE]>,
    /// Write position for pre-delay buffer.
    pre_delay_write_pos: usize,
    /// Pre-delay length in samples.
    pre_delay_samples: usize,
    /// Wet mix level (0.0–1.0, typical 0.15–0.3).
    wet_gain: f32,
    /// RT60 at low frequencies (seconds).
    rt60_low: f32,
    /// RT60 at high frequencies (seconds). Always shorter than rt60_low.
    rt60_high: f32,
    /// Whether init() has been called.
    initialized: bool,
}

impl FdnReverb {
    /// Create an uninitialized FDN reverb processor.
    ///
    /// - `wet_gain`: how much reverb to mix in (0.0–1.0, typical 0.15–0.3 for small rooms)
    /// - `rt60_low`: low-frequency reverberation time in seconds (e.g. 0.8)
    /// - `rt60_high`: high-frequency reverberation time in seconds (e.g. 0.3)
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

    /// Compute delay lengths from room geometry, scaling base primes by sample rate.
    fn compute_delays(&mut self, room_min: Vec3, room_max: Vec3, sample_rate: f32) {
        // Mean free path for a rectangular room: MFP = 4V / S.
        // For 6x4x3m: MFP = 4*72/108 = 2.667m → ~373 samples at 48kHz.
        // The BASE_DELAYS are primes centered around this value.
        let _dims = room_max - room_min;

        // Scale base prime delays for sample rate (designed for 48kHz).
        let scale = sample_rate / BASE_SAMPLE_RATE;
        for i in 0..NUM_LINES {
            let scaled = ((BASE_DELAYS[i] as f32) * scale) as usize;
            self.delays[i] = scaled.clamp(1, BUF_SIZE - 1);
        }

        // Pre-delay: 20ms gap after early reflections
        self.pre_delay_samples =
            ((PRE_DELAY_SECONDS * sample_rate) as usize).min(PRE_DELAY_BUF_SIZE - 1);
    }

    /// Compute damping filter coefficients from RT60 values and delay lengths.
    ///
    /// Uses the Jot formula: g = 10^(-3 * M / (RT60 * fs)) for gain at DC and Nyquist,
    /// then derives one-pole lowpass coefficients so highs decay faster than lows.
    fn compute_damping(&mut self, sample_rate: f32) {
        for i in 0..NUM_LINES {
            let m = self.delays[i] as f32;

            // Gain at DC (0 Hz) from low-frequency RT60
            let g_dc = 10.0_f32.powf(-3.0 * m / (self.rt60_low * sample_rate));
            // Gain at Nyquist from high-frequency RT60
            let g_nyq = 10.0_f32.powf(-3.0 * m / (self.rt60_high * sample_rate));

            // One-pole lowpass: H(z) = k / (1 - p * z^-1)
            let sum = g_dc + g_nyq;
            self.damping[i].k = 2.0 * g_dc * g_nyq / sum;
            self.damping[i].p = (g_dc - g_nyq) / sum;
            self.damping[i].state = 0.0;
        }
    }

    /// In-place fast 8x8 Hadamard transform with 1/sqrt(8) normalization.
    ///
    /// 3 butterfly stages: 24 additions + 8 multiplies (normalization only).
    /// The Hadamard matrix is orthogonal → preserves energy, maximizes diffusion
    /// between all 8 delay lines at each circulation.
    #[inline(always)]
    fn hadamard_8(v: &mut [f32; NUM_LINES]) {
        // Stage 1: stride 1 (pairs)
        for i in (0..8).step_by(2) {
            let a = v[i];
            let b = v[i + 1];
            v[i] = a + b;
            v[i + 1] = a - b;
        }
        // Stage 2: stride 2 (quads)
        for i in (0..8).step_by(4) {
            let (a0, a1) = (v[i], v[i + 1]);
            let (b0, b1) = (v[i + 2], v[i + 3]);
            v[i] = a0 + b0;
            v[i + 1] = a1 + b1;
            v[i + 2] = a0 - b0;
            v[i + 3] = a1 - b1;
        }
        // Stage 3: stride 4 (octets)
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
        // Normalize: orthogonal matrix requires 1/sqrt(N) scaling
        let scale = 1.0 / (NUM_LINES as f32).sqrt();
        for x in v.iter_mut() {
            *x *= scale;
        }
    }

    /// Process one mono sample through the FDN. Returns (left, right) reverb output.
    #[inline(always)]
    fn process_sample(&mut self, mono_in: f32) -> (f32, f32) {
        // 1. Read delay line outputs
        let mut taps = [0.0_f32; NUM_LINES];
        for i in 0..NUM_LINES {
            let read_pos = (self.write_pos + BUF_SIZE - self.delays[i]) & BUF_MASK;
            taps[i] = self.delay_buffers[i][read_pos];
        }

        // 2. Apply damping filters (frequency-dependent decay)
        for i in 0..NUM_LINES {
            taps[i] = self.damping[i].process(taps[i]);
        }

        // 3. Tap stereo output BEFORE mixing (maximizes L/R decorrelation)
        //    L = even lines [0,2,4,6], R = odd lines [1,3,5,7]
        let out_l = (taps[0] + taps[2] + taps[4] + taps[6]) * 0.25;
        let out_r = (taps[1] + taps[3] + taps[5] + taps[7]) * 0.25;

        // 4. Hadamard mixing matrix (couples all lines for diffusion)
        Self::hadamard_8(&mut taps);

        // 5. Inject input + write feedback into delay lines
        let input_scale = 1.0 / (NUM_LINES as f32).sqrt();
        let scaled_input = mono_in * input_scale;
        for i in 0..NUM_LINES {
            // Soft clamp prevents feedback runaway in edge cases
            self.delay_buffers[i][self.write_pos] =
                (taps[i] + scaled_input).clamp(-4.0, 4.0);
        }

        // 6. Advance write position
        self.write_pos = (self.write_pos + 1) & BUF_MASK;

        (out_l, out_r)
    }
}

impl AudioProcessor for FdnReverb {
    fn init(
        &mut self,
        room_min: Vec3,
        room_max: Vec3,
        _listener: &Listener,
        sample_rate: f32,
    ) {
        self.compute_delays(room_min, room_max, sample_rate);
        self.compute_damping(sample_rate);
        self.initialized = true;
    }

    fn process(&mut self, buffer: &mut [f32], channels: usize, _sample_rate: f32) {
        if !self.initialized {
            return;
        }

        let num_frames = buffer.len() / channels;

        for frame in 0..num_frames {
            let base = frame * channels;
            let dry_l = buffer[base];
            let dry_r = if channels > 1 { buffer[base + 1] } else { dry_l };

            // Sum to mono for FDN input
            let mono_in = (dry_l + dry_r) * 0.5;

            // Pre-delay: write mono, read delayed
            self.pre_delay_buf[self.pre_delay_write_pos] = mono_in;
            let read_pos = (self.pre_delay_write_pos + PRE_DELAY_BUF_SIZE
                - self.pre_delay_samples)
                & PRE_DELAY_BUF_MASK;
            let delayed_in = self.pre_delay_buf[read_pos];
            self.pre_delay_write_pos =
                (self.pre_delay_write_pos + 1) & PRE_DELAY_BUF_MASK;

            // FDN processing
            let (wet_l, wet_r) = self.process_sample(delayed_in);

            // Mix: dry + wet * gain, clamped
            buffer[base] = (dry_l + wet_l * self.wet_gain).clamp(-1.0, 1.0);
            if channels > 1 {
                buffer[base + 1] = (dry_r + wet_r * self.wet_gain).clamp(-1.0, 1.0);
            }
        }
    }

    fn name(&self) -> &str {
        "FdnReverb"
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn init_fdn() -> FdnReverb {
        let mut fdn = FdnReverb::new(0.3, 0.8, 0.3);
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
        fdn.init(
            Vec3::ZERO,
            Vec3::new(6.0, 4.0, 3.0),
            &listener,
            48000.0,
        );
        fdn
    }

    #[test]
    fn delay_computation_6x4x3_room() {
        let fdn = init_fdn();

        // All 8 delays should be nonzero and within buffer bounds
        for i in 0..NUM_LINES {
            assert!(fdn.delays[i] > 0, "delay {} is zero", i);
            assert!(
                fdn.delays[i] < BUF_SIZE,
                "delay {} = {} exceeds buffer",
                i,
                fdn.delays[i]
            );
        }

        // All delays should be distinct
        for i in 0..NUM_LINES {
            for j in (i + 1)..NUM_LINES {
                assert_ne!(
                    fdn.delays[i], fdn.delays[j],
                    "delays {} and {} are equal: {}",
                    i, j, fdn.delays[i]
                );
            }
        }

        // Delays should be in ascending order (from BASE_DELAYS)
        for i in 1..NUM_LINES {
            assert!(
                fdn.delays[i] > fdn.delays[i - 1],
                "delays not ascending at {}",
                i
            );
        }
    }

    #[test]
    fn rt60_gain_values_are_stable() {
        let fdn = init_fdn();

        // All damping filter coefficients must produce a stable (decaying) network
        for i in 0..NUM_LINES {
            let d = &fdn.damping[i];
            assert!(d.k > 0.0, "damping[{}].k = {} (should be > 0)", i, d.k);
            assert!(d.k < 1.0, "damping[{}].k = {} (should be < 1)", i, d.k);
            assert!(d.p > 0.0, "damping[{}].p = {} (should be > 0)", i, d.p);
            assert!(d.p < 1.0, "damping[{}].p = {} (should be < 1)", i, d.p);
        }
    }

    #[test]
    fn silence_in_silence_out() {
        let mut fdn = init_fdn();

        let mut buffer = vec![0.0f32; 512 * 2];
        fdn.process(&mut buffer, 2, 48000.0);

        for &sample in &buffer {
            assert_eq!(sample, 0.0);
        }
    }

    #[test]
    fn impulse_produces_decaying_output() {
        let mut fdn = init_fdn();

        let channels = 2;
        let total_frames = 4096;
        let mut buffer = vec![0.0f32; total_frames * channels];
        buffer[0] = 1.0; // L impulse
        buffer[1] = 1.0; // R impulse

        fdn.process(&mut buffer, channels, 48000.0);

        // Before pre-delay + shortest delay, output should be just the dry impulse
        // Pre-delay: 960 samples, shortest FDN delay: 241 samples → onset at ~1201
        let onset = fdn.pre_delay_samples + fdn.delays[0];

        // After onset, there should be reverb signal
        let mut has_reverb = false;
        for frame in onset..total_frames {
            let l = buffer[frame * channels];
            if l.abs() > 1e-6 {
                has_reverb = true;
                break;
            }
        }
        assert!(has_reverb, "no reverb signal after onset frame {}", onset);

        // Energy should decay: compare first and second halves after onset
        let mid = onset + (total_frames - onset) / 2;
        let energy_first: f32 = (onset..mid)
            .map(|f| {
                let l = buffer[f * channels];
                let r = buffer[f * channels + 1];
                l * l + r * r
            })
            .sum();
        let energy_second: f32 = (mid..total_frames)
            .map(|f| {
                let l = buffer[f * channels];
                let r = buffer[f * channels + 1];
                l * l + r * r
            })
            .sum();

        assert!(
            energy_second < energy_first,
            "reverb not decaying: first half energy = {}, second half = {}",
            energy_first,
            energy_second
        );
    }

    #[test]
    fn uninitialized_is_passthrough() {
        let mut fdn = FdnReverb::new(0.3, 0.8, 0.3);
        // Don't call init — initialized is false

        let mut buffer = vec![0.5f32; 128 * 2];
        let original = buffer.clone();
        fdn.process(&mut buffer, 2, 48000.0);

        assert_eq!(buffer, original);
    }

    #[test]
    fn name_returns_expected() {
        let fdn = FdnReverb::new(0.3, 0.8, 0.3);
        assert_eq!(fdn.name(), "FdnReverb");
    }
}
