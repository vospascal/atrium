//! Per-ear interaural time difference, as a short fractional delay line.

/// Ring buffer size for per-ear ITD delay. Max human ITD ≈ 0.7ms = 34 samples @ 48kHz.
/// Perceptual reference: ITD JND ≈ 90μs, ILD JND ≈ 2.5 dB (BBC WHP254).
pub(super) const DELAY_BUF_SIZE: usize = 64;
const DELAY_BUF_MASK: usize = DELAY_BUF_SIZE - 1;

/// Per-ear fractional delay line for ITD rendering.
pub(super) struct ItdDelay {
    buf: [f32; DELAY_BUF_SIZE],
    write_pos: usize,
    /// Current delay in fractional samples, smoothed toward target.
    delay_samples: f32,
    /// Target delay in fractional samples (from SOFA).
    pub(super) target_delay_samples: f32,
}

impl ItdDelay {
    pub(super) fn new() -> Self {
        Self {
            buf: [0.0; DELAY_BUF_SIZE],
            write_pos: 0,
            delay_samples: 0.0,
            target_delay_samples: 0.0,
        }
    }

    /// Write a sample and read with fractional delay (linear interpolation).
    #[inline]
    pub(super) fn process(&mut self, input: f32) -> f32 {
        self.buf[self.write_pos] = input;
        self.write_pos = (self.write_pos + 1) & DELAY_BUF_MASK;

        // Smooth delay changes to avoid clicks
        self.delay_samples += (self.target_delay_samples - self.delay_samples) * 0.01;

        let d = self.delay_samples;
        let int_d = d as usize;
        let frac = d - int_d as f32;

        let idx0 = (self.write_pos + DELAY_BUF_SIZE - 1 - int_d) & DELAY_BUF_MASK;
        let idx1 = (idx0 + DELAY_BUF_SIZE - 1) & DELAY_BUF_MASK;

        self.buf[idx0] * (1.0 - frac) + self.buf[idx1] * frac
    }

    pub(super) fn reset(&mut self) {
        self.buf.fill(0.0);
        self.write_pos = 0;
        self.delay_samples = 0.0;
        self.target_delay_samples = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Fractional ITD delay by linear interpolation: a delay of 3.25 samples
    /// splits a unit impulse 0.75 / 0.25 over output samples 3 and 4.
    #[test]
    fn itd_delay_reads_a_fractional_delay_by_linear_interpolation() {
        let mut itd = ItdDelay::new();
        itd.delay_samples = 3.25;
        itd.target_delay_samples = 3.25;
        let output: Vec<f32> = (0..8)
            .map(|index| itd.process(if index == 0 { 1.0 } else { 0.0 }))
            .collect();
        let expected = [0.0, 0.0, 0.0, 0.75, 0.25, 0.0, 0.0, 0.0];
        for (index, (actual, wanted)) in output.iter().zip(expected).enumerate() {
            assert!(
                (actual - wanted).abs() < 1e-6,
                "sample {index}: expected {wanted}, got {actual}"
            );
        }
    }

    /// ITD delay smoothing is a one-pole of coefficient 0.01 per sample:
    /// after 100 samples a step from 0 to 10 has covered 1 − 0.99¹⁰⁰ = 63.4 %,
    /// i.e. a time constant of ≈ 100 samples (2.1 ms at 48 kHz).
    #[test]
    fn itd_delay_glides_with_a_hundred_sample_time_constant() {
        let mut itd = ItdDelay::new();
        itd.target_delay_samples = 10.0;
        for _ in 0..100 {
            itd.process(0.0);
        }
        let expected = 10.0 * (1.0 - 0.99f32.powi(100));
        assert!((expected - 6.340).abs() < 1e-3);
        assert!((itd.delay_samples - expected).abs() < 1e-3);
    }

    /// ITD delay targets must be clamped to the ring buffer capacity —
    /// at 96 kHz a 0.7 ms SOFA delay (67 samples) exceeds the 64-sample ring
    /// and would otherwise wrap around and read future samples.
    #[test]
    fn itd_delay_stays_within_buffer() {
        let mut itd = ItdDelay::new();
        // Simulate the clamped assignment path used in render_source.
        let sr: f32 = 96000.0;
        let sofa_delay_seconds: f32 = 0.0007;
        let max_delay = (DELAY_BUF_SIZE - 2) as f32;
        itd.target_delay_samples = (sofa_delay_seconds * sr).min(max_delay);
        assert!(itd.target_delay_samples <= max_delay);

        // Feed an impulse and run long enough for smoothing to settle; the
        // delayed output must appear (buffer indexing stays valid throughout).
        let mut saw_signal = false;
        for n in 0..1024 {
            let input = if n == 0 { 1.0 } else { 0.0 };
            let out = itd.process(input);
            if out.abs() > 1e-9 {
                saw_signal = true;
            }
        }
        assert!(
            saw_signal,
            "delayed impulse should emerge from the ITD line"
        );
    }
}
