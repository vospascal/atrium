//! Signals whose measures are known by hand, for the tests.

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

pub(crate) const RATE: u32 = 48_000;

/// `seconds` of a sine at `frequency` Hz with peak `amplitude`.
pub(crate) fn sine(frequency: f64, amplitude: f64, seconds: f64) -> Vec<f32> {
    (0..(seconds * RATE as f64).round() as usize)
        .map(|index| {
            (amplitude * (std::f64::consts::TAU * frequency * index as f64 / RATE as f64).sin())
                as f32
        })
        .collect()
}

/// A deterministic source of Gaussian noise (xorshift64*, Box–Muller), so a
/// test reads the same numbers on every run.
pub(crate) struct Noise {
    state: u64,
}

impl Noise {
    pub(crate) fn new(seed: u64) -> Self {
        Self {
            state: seed.wrapping_mul(0x9E37_79B9_7F4A_7C15) | 1,
        }
    }

    fn uniform(&mut self) -> f64 {
        self.state ^= self.state >> 12;
        self.state ^= self.state << 25;
        self.state ^= self.state >> 27;
        let value = self.state.wrapping_mul(0x2545_F491_4F6C_DD1D);
        ((value >> 11) as f64 + 0.5) / (1u64 << 53) as f64
    }

    pub(crate) fn gaussian(&mut self) -> f64 {
        let radius = (-2.0 * self.uniform().ln()).sqrt();
        radius * (std::f64::consts::TAU * self.uniform()).cos()
    }

    /// `seconds` of white Gaussian noise of RMS `rms`.
    pub(crate) fn white(&mut self, rms: f64, seconds: f64) -> Vec<f32> {
        (0..(seconds * RATE as f64).round() as usize)
            .map(|_| (rms * self.gaussian()) as f32)
            .collect()
    }

    /// `seconds` of pink noise (power ∝ 1/f) of RMS `rms`, shaped in the
    /// frequency domain so every bin has exactly the 1/f mean power.
    pub(crate) fn pink(&mut self, rms: f64, seconds: f64) -> Vec<f32> {
        let length = (seconds * RATE as f64).round() as usize;
        let mut planner = RealFftPlanner::<f64>::new();
        let inverse = planner.plan_fft_inverse(length);
        let mut bins = inverse.make_input_vec();
        for (index, bin) in bins.iter_mut().enumerate().skip(1) {
            let amplitude = 1.0 / (index as f64).sqrt();
            *bin = Complex::new(self.gaussian(), self.gaussian()) * amplitude;
        }
        let last = bins.len() - 1;
        bins[last].im = 0.0;
        let mut samples = inverse.make_output_vec();
        inverse.process(&mut bins, &mut samples).expect("real ends");
        let power = samples.iter().map(|sample| sample * sample).sum::<f64>() / length as f64;
        let scale = rms / power.sqrt();
        samples
            .into_iter()
            .map(|sample| (sample * scale) as f32)
            .collect()
    }
}

/// Interleave equally long channels.
pub(crate) fn interleave(channels: &[&[f32]]) -> Vec<f32> {
    let frames = channels[0].len();
    (0..frames)
        .flat_map(|frame| channels.iter().map(move |channel| channel[frame]))
        .collect()
}
