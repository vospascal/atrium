//! Overlap-add FFT convolver for real-valued audio signals.
//!
//! Designed for short impulse responses (HRIRs, ~128–512 taps) convolved with
//! fixed-size audio blocks. Handles any input length ≤ block_size correctly,
//! and preserves the overlap tail across `set_response()` calls to avoid
//! discontinuities when swapping impulse responses.

use realfft::num_complex::Complex;
use realfft::{ComplexToReal, RealFftPlanner, RealToComplex};
use std::sync::Arc;

/// A single-partition overlap-add FFT convolver for real-valued signals.
///
/// Usage:
/// 1. `init(block_size, impulse_response)` — sets FFT size and pre-computes the IR spectrum.
/// 2. `process(input, output)` — convolves `input` (up to `block_size` samples) and writes to `output`.
/// 3. `set_response(ir)` — swaps the IR without clearing the overlap tail.
/// 4. `reset()` — zeros all internal state (use on full pipeline reset only).
pub struct Convolver {
    block_size: usize,
    fft_size: usize,

    /// Pre-computed impulse response in frequency domain.
    filter_spectrum: Vec<Complex<f32>>,

    /// Overlap tail from previous block (length = filter_len - 1).
    overlap: Vec<f32>,
    filter_len: usize,

    // Working buffers (pre-allocated to avoid per-block allocation).
    time_buf: Vec<f32>,
    freq_buf: Vec<Complex<f32>>,
    ifft_buf: Vec<f32>,

    // FFT plans (shared via Arc, cheap to clone).
    r2c: Arc<dyn RealToComplex<f32>>,
    c2r: Arc<dyn ComplexToReal<f32>>,
}

impl Default for Convolver {
    fn default() -> Self {
        Self::new()
    }
}

impl Convolver {
    pub fn new() -> Self {
        // Dummy plans — will be replaced by init().
        let mut planner = RealFftPlanner::<f32>::new();
        let r2c = planner.plan_fft_forward(2);
        let c2r = planner.plan_fft_inverse(2);
        Self {
            block_size: 0,
            fft_size: 0,
            filter_spectrum: Vec::new(),
            overlap: Vec::new(),
            filter_len: 0,
            time_buf: Vec::new(),
            freq_buf: Vec::new(),
            ifft_buf: Vec::new(),
            r2c,
            c2r,
        }
    }

    /// Initialize the convolver with a block size and impulse response.
    ///
    /// `block_size` is the maximum number of input samples per `process()` call.
    /// `impulse_response` is the time-domain IR (e.g., one ear of an HRIR).
    pub fn init(&mut self, block_size: usize, impulse_response: &[f32]) {
        let filter_len = impulse_response.len().max(1);
        // Linear convolution of N + M - 1 samples; round up to power of 2 for FFT.
        let fft_size = (block_size + filter_len - 1).next_power_of_two();
        let freq_len = fft_size / 2 + 1;

        let mut planner = RealFftPlanner::<f32>::new();
        self.r2c = planner.plan_fft_forward(fft_size);
        self.c2r = planner.plan_fft_inverse(fft_size);

        self.block_size = block_size;
        self.fft_size = fft_size;
        self.filter_len = filter_len;

        // Pre-compute filter spectrum.
        self.time_buf = vec![0.0; fft_size];
        self.time_buf[..filter_len].copy_from_slice(impulse_response);
        self.filter_spectrum = vec![Complex::new(0.0, 0.0); freq_len];
        self.r2c
            .process(&mut self.time_buf, &mut self.filter_spectrum)
            .unwrap();

        // Allocate working buffers.
        self.time_buf = vec![0.0; fft_size];
        self.freq_buf = vec![Complex::new(0.0, 0.0); freq_len];
        self.ifft_buf = vec![0.0; fft_size];

        // Overlap tail: filter_len - 1 samples.
        self.overlap = vec![0.0; filter_len - 1];
    }

    /// Replace the impulse response without clearing the overlap tail.
    ///
    /// This is the key difference from fft-convolver: swapping IRs mid-stream
    /// preserves the convolution tail from the previous block, avoiding clicks.
    pub fn set_response(&mut self, impulse_response: &[f32]) {
        let filter_len = impulse_response.len().max(1);

        // If filter length changed, we need to resize everything.
        if filter_len != self.filter_len {
            let fft_size = (self.block_size + filter_len - 1).next_power_of_two();

            if fft_size != self.fft_size {
                let freq_len = fft_size / 2 + 1;
                let mut planner = RealFftPlanner::<f32>::new();
                self.r2c = planner.plan_fft_forward(fft_size);
                self.c2r = planner.plan_fft_inverse(fft_size);
                self.fft_size = fft_size;
                self.time_buf.resize(fft_size, 0.0);
                self.freq_buf.resize(freq_len, Complex::new(0.0, 0.0));
                self.ifft_buf.resize(fft_size, 0.0);
                self.filter_spectrum
                    .resize(freq_len, Complex::new(0.0, 0.0));
            }

            // Resize overlap — preserve existing samples, zero-pad or truncate.
            let new_overlap_len = filter_len - 1;
            self.overlap.resize(new_overlap_len, 0.0);
            self.filter_len = filter_len;
        }

        // Compute new filter spectrum.
        self.time_buf[..filter_len].copy_from_slice(impulse_response);
        self.time_buf[filter_len..].fill(0.0);
        self.r2c
            .process(&mut self.time_buf, &mut self.filter_spectrum)
            .unwrap();
    }

    /// Adopt another convolver's overlap tail, so this convolver continues
    /// the sound that convolver has already produced.
    ///
    /// Used when handing a stream from one convolver to another: the tail is
    /// the part of earlier input blocks that has not been emitted yet. Without
    /// it the receiving convolver would drop that tail, and would instead emit
    /// whatever stale tail it was left holding the last time it ran.
    pub fn adopt_overlap_from(&mut self, other: &Convolver) {
        let shared = self.overlap.len().min(other.overlap.len());
        self.overlap[..shared].copy_from_slice(&other.overlap[..shared]);
        self.overlap[shared..].fill(0.0);
    }

    /// The FFT size used by this convolver. Convolvers sharing input must have
    /// the same `fft_size` for `process_with_spectrum` to work.
    pub fn fft_size(&self) -> usize {
        self.fft_size
    }

    /// Number of complex frequency bins (fft_size / 2 + 1).
    pub fn freq_len(&self) -> usize {
        self.fft_size / 2 + 1
    }

    /// Compute the forward FFT of `input` into `spectrum`.
    ///
    /// Use this to share one forward FFT across multiple convolvers (e.g., L/R ears).
    /// `spectrum` must have length `freq_len()`. `input` is zero-padded to `fft_size`.
    pub fn forward_fft(&mut self, input: &[f32], spectrum: &mut [Complex<f32>]) {
        let input_len = input.len();
        self.time_buf[..input_len].copy_from_slice(input);
        self.time_buf[input_len..].fill(0.0);
        self.r2c.process(&mut self.time_buf, spectrum).unwrap();
    }

    /// Convolve using a pre-computed input spectrum (from `forward_fft`).
    ///
    /// Saves one forward FFT per call compared to `process()`. The `input_spectrum`
    /// must have been computed with the same `fft_size`.
    pub fn process_with_spectrum(
        &mut self,
        input_spectrum: &[Complex<f32>],
        input_len: usize,
        output: &mut [f32],
    ) {
        debug_assert!(input_len <= self.block_size);
        debug_assert!(output.len() >= input_len);

        if self.fft_size == 0 {
            output[..input_len].fill(0.0);
            return;
        }

        // 1. Complex multiply input spectrum with filter spectrum.
        for (x, (inp, h)) in self
            .freq_buf
            .iter_mut()
            .zip(input_spectrum.iter().zip(self.filter_spectrum.iter()))
        {
            *x = *inp * *h;
        }

        // 2. Inverse FFT.
        self.c2r
            .process(&mut self.freq_buf, &mut self.ifft_buf)
            .unwrap();

        // 3. Scale, overlap-add, save new overlap.
        self.apply_overlap(input_len, output);
    }

    /// Convolve `input` with the stored impulse response, writing to `output`.
    ///
    /// `input.len()` must be ≤ `block_size`. `output` must be at least `input.len()`.
    /// Output is **replaced** (not accumulated).
    pub fn process(&mut self, input: &[f32], output: &mut [f32]) {
        let input_len = input.len();
        debug_assert!(input_len <= self.block_size);
        debug_assert!(output.len() >= input_len);

        if self.fft_size == 0 {
            output[..input_len].fill(0.0);
            return;
        }

        // 1. Zero-pad input to FFT size.
        self.time_buf[..input_len].copy_from_slice(input);
        self.time_buf[input_len..].fill(0.0);

        // 2. Forward FFT.
        self.r2c
            .process(&mut self.time_buf, &mut self.freq_buf)
            .unwrap();

        // 3. Complex multiply with filter spectrum.
        for (x, h) in self.freq_buf.iter_mut().zip(self.filter_spectrum.iter()) {
            *x *= *h;
        }

        // 4. Inverse FFT.
        self.c2r
            .process(&mut self.freq_buf, &mut self.ifft_buf)
            .unwrap();

        // 5. Scale, overlap-add, save new overlap.
        self.apply_overlap(input_len, output);
    }

    /// Common tail: scale IFFT output, add/save overlap.
    fn apply_overlap(&mut self, input_len: usize, output: &mut [f32]) {
        let scale = 1.0 / self.fft_size as f32;

        let overlap_len = self.overlap.len();
        let overlap_use = overlap_len.min(input_len);
        for (out, (ifft, ovlp)) in output[..overlap_use].iter_mut().zip(
            self.ifft_buf[..overlap_use]
                .iter()
                .zip(self.overlap[..overlap_use].iter()),
        ) {
            *out = *ifft * scale + *ovlp;
        }
        for (out, ifft) in output[overlap_use..input_len]
            .iter_mut()
            .zip(self.ifft_buf[overlap_use..input_len].iter())
        {
            *out = *ifft * scale;
        }

        // Save new overlap: samples [input_len .. input_len + overlap_len].
        for i in 0..overlap_len {
            let old_overlap = if i + input_len < overlap_len {
                self.overlap[i + input_len]
            } else {
                0.0
            };
            self.overlap[i] = self.ifft_buf[input_len + i] * scale + old_overlap;
        }
    }

    /// Clear all internal state. Use only on full pipeline reset.
    pub fn reset(&mut self) {
        self.overlap.fill(0.0);
        self.time_buf.fill(0.0);
        self.freq_buf.fill(Complex::new(0.0, 0.0));
        self.ifft_buf.fill(0.0);
    }
}

#[cfg(test)]
mod tests;
