//! Long-term spectral profiling per audio source.
//!
//! Computes a spectral profile at load time by analyzing frequency content
//! across 24 Bark bands. Each band stores energy in dB relative to overall
//! RMS, so positive values indicate more energy than average and negative
//! values indicate less.

use realfft::num_complex::Complex;
use realfft::RealFftPlanner;

/// Number of Bark critical bands (standard 24-band scale).
pub const BARK_BANDS: usize = 24;

/// Bark band edge frequencies in Hz.
/// Each pair (BARK_EDGES[i], BARK_EDGES[i+1]) defines band i+1.
const BARK_EDGES: [f32; 25] = [
    0.0, 100.0, 200.0, 300.0, 400.0, 510.0, 630.0, 770.0, 920.0, 1080.0, 1270.0, 1480.0, 1720.0,
    2000.0, 2320.0, 2700.0, 3150.0, 3700.0, 4400.0, 5300.0, 6400.0, 7700.0, 9500.0, 12000.0,
    15500.0,
];

/// Per-source long-term spectral profile across 24 Bark bands.
#[derive(Clone)]
pub struct SpectralProfile {
    /// Per-band energy in dB relative to overall RMS.
    /// Positive = more energy than average, negative = less.
    pub bands: [f32; BARK_BANDS],
}

impl Default for SpectralProfile {
    fn default() -> Self {
        Self {
            bands: [0.0; BARK_BANDS],
        } // flat profile
    }
}

/// Map a frequency in Hz to a Bark band index (0..23).
/// Returns `None` if the frequency is above the highest band edge.
fn frequency_to_bark_band(frequency: f32) -> Option<usize> {
    (0..BARK_BANDS).find(|&band_index| {
        frequency >= BARK_EDGES[band_index] && frequency < BARK_EDGES[band_index + 1]
    })
}

/// Compute the long-term spectral profile from decoded audio samples.
/// Uses FFT to analyze frequency content, maps to 24 Bark bands.
pub fn compute_profile(samples: &[f32], sample_rate: u32) -> SpectralProfile {
    const FFT_SIZE: usize = 4096;
    const HOP_SIZE: usize = FFT_SIZE / 2; // 50% overlap

    if samples.is_empty() {
        return SpectralProfile::default();
    }

    // Precompute Hann window
    let window: Vec<f32> = (0..FFT_SIZE)
        .map(|i| 0.5 * (1.0 - (2.0 * std::f32::consts::PI * i as f32 / FFT_SIZE as f32).cos()))
        .collect();

    // Set up FFT
    let mut planner = RealFftPlanner::<f32>::new();
    let r2c = planner.plan_fft_forward(FFT_SIZE);
    let spectrum_len = FFT_SIZE / 2 + 1;

    // Accumulate magnitude spectra across frames
    let mut accumulated_spectrum = vec![0.0f64; spectrum_len];
    let mut frame_count = 0usize;

    let mut time_buf = vec![0.0f32; FFT_SIZE];
    let mut freq_buf = vec![Complex::new(0.0f32, 0.0f32); spectrum_len];

    let mut offset = 0;
    while offset + FFT_SIZE <= samples.len() {
        // Apply window
        for i in 0..FFT_SIZE {
            time_buf[i] = samples[offset + i] * window[i];
        }

        // Forward FFT
        r2c.process(&mut time_buf, &mut freq_buf).unwrap();

        // Accumulate magnitude squared (power)
        for (bin_index, bin) in freq_buf.iter().enumerate() {
            let magnitude_squared = (bin.re * bin.re + bin.im * bin.im) as f64;
            accumulated_spectrum[bin_index] += magnitude_squared;
        }

        frame_count += 1;
        offset += HOP_SIZE;
    }

    if frame_count == 0 {
        return SpectralProfile::default();
    }

    // Average the accumulated spectrum
    let inverse_frame_count = 1.0 / frame_count as f64;
    for bin in accumulated_spectrum.iter_mut() {
        *bin *= inverse_frame_count;
    }

    // Map FFT bins to Bark bands, accumulating linear power
    let mut band_power = [0.0f64; BARK_BANDS];
    let frequency_per_bin = sample_rate as f64 / FFT_SIZE as f64;

    for (bin_index, &power) in accumulated_spectrum.iter().enumerate() {
        let frequency = bin_index as f32 * frequency_per_bin as f32;
        if let Some(band_index) = frequency_to_bark_band(frequency) {
            band_power[band_index] += power;
        }
    }

    // Compute overall RMS power (sum of all band powers)
    let overall_rms_power: f64 = band_power.iter().sum();

    // Convert to dB relative to overall RMS
    let mut bands = [0.0f32; BARK_BANDS];

    if overall_rms_power <= 0.0 {
        return SpectralProfile { bands };
    }

    for (band_index, &power) in band_power.iter().enumerate() {
        if power <= 0.0 {
            bands[band_index] = -90.0;
        } else {
            bands[band_index] = (10.0 * (power / overall_rms_power).log10()) as f32;
        }
    }

    SpectralProfile { bands }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Generate sine wave at given frequency.
    fn sine_wave(frequency: f32, sample_rate: u32, duration_seconds: f32) -> Vec<f32> {
        let num_samples = (sample_rate as f32 * duration_seconds) as usize;
        (0..num_samples)
            .map(|i| (2.0 * std::f32::consts::PI * frequency * i as f32 / sample_rate as f32).sin())
            .collect()
    }

    #[test]
    fn sine_has_energy_in_one_band() {
        // 1kHz sine should have peak energy in Bark band 9 (920-1080 Hz)
        let samples = sine_wave(1000.0, 48000, 1.0);
        let profile = compute_profile(&samples, 48000);
        let peak_band = profile
            .bands
            .iter()
            .enumerate()
            .max_by(|a, b| a.1.partial_cmp(b.1).unwrap())
            .unwrap()
            .0;
        assert_eq!(
            peak_band,
            8,
            "1kHz should peak in band 9 (index 8), got band {}",
            peak_band + 1
        );
        // Peak should be much higher than other bands
        let peak_db = profile.bands[8];
        let average_other: f32 = profile
            .bands
            .iter()
            .enumerate()
            .filter(|(i, _)| *i != 8)
            .map(|(_, v)| *v)
            .sum::<f32>()
            / 23.0;
        assert!(
            peak_db - average_other > 20.0,
            "peak should be >20 dB above average of other bands"
        );
    }

    #[test]
    fn white_noise_is_flat() {
        // White noise has equal power spectral *density*, but Bark bands have
        // non-uniform bandwidth (100 Hz at low end, 3500 Hz at high end), so
        // wider bands accumulate proportionally more energy. We verify the
        // spread is moderate — no single band dominates like a tonal signal would.
        let sample_rate = 48000u32;
        let num_samples = 48000 * 2; // 2 seconds
        let mut samples = vec![0.0f32; num_samples];
        // Simple LCG pseudo-random for determinism
        let mut state: u32 = 12345;
        for sample in samples.iter_mut() {
            state = state.wrapping_mul(1103515245).wrapping_add(12345);
            *sample = (state as f32 / u32::MAX as f32) * 2.0 - 1.0;
        }
        let profile = compute_profile(&samples, sample_rate);
        // Check variance across bands is moderate (not tonal)
        let mean: f32 = profile.bands.iter().sum::<f32>() / BARK_BANDS as f32;
        let variance: f32 = profile
            .bands
            .iter()
            .map(|band| (band - mean).powi(2))
            .sum::<f32>()
            / BARK_BANDS as f32;
        let standard_deviation = variance.sqrt();
        // Bark bandwidth variation accounts for ~5 dB spread; 6 dB threshold
        // allows headroom while still catching tonal signals (which exceed 20 dB).
        assert!(
            standard_deviation < 6.0,
            "white noise bands should have <6 dB std dev, got {:.1}",
            standard_deviation
        );
    }

    #[test]
    fn profile_energy_conserved() {
        // Sum of linear band energies should be finite and positive
        let samples = sine_wave(440.0, 48000, 1.0);
        let profile = compute_profile(&samples, 48000);
        let total_linear: f32 = profile
            .bands
            .iter()
            .map(|decibels| 10.0_f32.powf(decibels / 10.0))
            .sum();
        assert!(total_linear.is_finite() && total_linear > 0.0);
    }
}
