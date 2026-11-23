//! White noise shaped to a power gain given in hertz, made in blocks in the
//! frequency domain: what grey and green are made with.
//!
//! [`BLOCK`] samples of white noise, transformed, each bin multiplied by the
//! shape's amplitude at its frequency, transformed back, and each block
//! cross-faded into the next with a sine window over half a block
//! (sin² + cos² = 1, so the power holds through the fade). The shape is
//! evaluated in hertz for the rate, so the spectrum is the same at every
//! rate, with none of the warping a digital filter made by the bilinear
//! transform has near the rate's limit (a second-order low-pass at 1 kHz so
//! made is 1.3 dB steeper at 8 kHz at 48 kHz than at 96 kHz). Every buffer is
//! made at construction: nothing is allocated while it plays. A block of
//! 32 768 samples is computed every 16 384 (0.34 s at 48 kHz); its bins are
//! 1.5 Hz apart at 48 kHz, the window's main lobe four of them wide.

use std::sync::Arc;

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};

use crate::synth::noise::{white_noise_density_scale, Rng};

/// Samples per block.
const BLOCK: usize = 32_768;
/// Samples between blocks: each is heard over two hops.
const HOP: usize = BLOCK / 2;

pub(super) struct Shaped {
    /// Power gain at a frequency (Hz).
    shape: fn(f64) -> f64,
    /// Amplitude gain on the white noise at 48 kHz.
    level: f64,
    rng: Rng,
    forward: Arc<dyn RealToComplex<f32>>,
    inverse: Arc<dyn ComplexToReal<f32>>,
    /// Each bin's amplitude gain at the cached rate, with the transforms'
    /// 1/[`BLOCK`], the level and the white density scale in it.
    gains: Vec<f32>,
    /// The sine cross-fade, over a whole block.
    window: Vec<f32>,
    block: Vec<f32>,
    spectrum: Vec<Complex<f32>>,
    scratch: Vec<Complex<f32>>,
    /// The hop being played.
    ready: Vec<f32>,
    /// The second half of the last block, faded out over the next hop.
    tail: Vec<f32>,
    /// Next sample of `ready` to play.
    position: usize,
    sample_rate_cached: f32,
}

impl Shaped {
    /// White noise from `seed`, at `level`, shaped by the power gain
    /// `shape` gives at each frequency.
    pub(super) fn new(shape: fn(f64) -> f64, level: f64, seed: u64) -> Self {
        let mut planner = RealFftPlanner::<f32>::new();
        let forward = planner.plan_fft_forward(BLOCK);
        let inverse = planner.plan_fft_inverse(BLOCK);
        let scratch_length = forward
            .get_scratch_len()
            .max(inverse.get_scratch_len())
            .max(1);
        let window = (0..BLOCK)
            .map(|index| (std::f64::consts::PI * (index as f64 + 0.5) / BLOCK as f64).sin() as f32)
            .collect();
        Self {
            shape,
            level,
            rng: Rng::new(seed),
            spectrum: forward.make_output_vec(),
            forward,
            inverse,
            gains: vec![0.0; BLOCK / 2 + 1],
            window,
            block: vec![0.0; BLOCK],
            scratch: vec![Complex::default(); scratch_length],
            ready: vec![0.0; HOP],
            tail: vec![0.0; HOP],
            position: HOP,
            sample_rate_cached: 0.0,
        }
    }

    /// The shape at every bin for `sample_rate`. On the first call it also
    /// makes a block whose second half is the tail, so the noise is at its
    /// level from the first sample.
    fn retune(&mut self, sample_rate: f32) {
        let first = self.sample_rate_cached == 0.0;
        self.sample_rate_cached = sample_rate;
        let bin_hz = sample_rate as f64 / BLOCK as f64;
        let scale = self.level * white_noise_density_scale(sample_rate) as f64 / BLOCK as f64;
        let last = self.gains.len() - 1;
        for (bin, gain) in self.gains.iter_mut().enumerate() {
            // Nothing at 0 Hz or at the rate's limit.
            *gain = if bin == 0 || bin == last {
                0.0
            } else {
                (scale * (self.shape)(bin as f64 * bin_hz).sqrt()) as f32
            };
        }
        if first {
            self.make_block();
            self.tail.copy_from_slice(&self.block[HOP..]);
        }
    }

    /// The next block, shaped and windowed, into `block`.
    fn make_block(&mut self) {
        for sample in self.block.iter_mut() {
            *sample = self.rng.next_bipolar();
        }
        // The lengths are fixed at construction, so neither transform can
        // refuse them; the inverse's only other check is that the 0 Hz and
        // last bins are real, and their gains are 0.
        let _ = self.forward.process_with_scratch(
            &mut self.block,
            &mut self.spectrum,
            &mut self.scratch,
        );
        for (value, &gain) in self.spectrum.iter_mut().zip(&self.gains) {
            *value *= gain;
        }
        let _ = self.inverse.process_with_scratch(
            &mut self.spectrum,
            &mut self.block,
            &mut self.scratch,
        );
        for (sample, &weight) in self.block.iter_mut().zip(&self.window) {
            *sample *= weight;
        }
    }

    #[inline]
    pub(super) fn next_sample(&mut self, sample_rate: f32) -> f32 {
        if self.sample_rate_cached != sample_rate {
            self.retune(sample_rate);
        }
        if self.position == HOP {
            self.make_block();
            for ((ready, tail), &new) in self
                .ready
                .iter_mut()
                .zip(self.tail.iter_mut())
                .zip(&self.block[..HOP])
            {
                *ready = *tail + new;
            }
            self.tail.copy_from_slice(&self.block[HOP..]);
            self.position = 0;
        }
        let sample = self.ready[self.position];
        self.position += 1;
        sample
    }
}
