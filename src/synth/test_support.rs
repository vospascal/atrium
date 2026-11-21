//! Measurement helpers shared by the synth tests: Welch spectra, octave-band
//! power density, spectral slope and basic sample statistics. Test-only.

use realfft::RealFftPlanner;

/// FFT length used by the spectral measurements. At 48 kHz the bin spacing is
/// 11.7 Hz, fine enough for the lowest band any test inspects (100 Hz).
pub const ANALYSIS_FFT_SIZE: usize = 4096;

/// Welch-averaged power spectrum: Hann window, 50 % overlap. Returns
/// `ANALYSIS_FFT_SIZE / 2 + 1` power values, one per bin.
pub fn averaged_power_spectrum(samples: &[f32]) -> Vec<f64> {
    let fft_size = ANALYSIS_FFT_SIZE;
    assert!(samples.len() >= fft_size, "need at least one FFT frame");
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(fft_size);
    let window: Vec<f32> = (0..fft_size)
        .map(|index| 0.5 - 0.5 * (std::f32::consts::TAU * index as f32 / fft_size as f32).cos())
        .collect();
    let mut input = vec![0.0_f32; fft_size];
    let mut output = fft.make_output_vec();
    let mut accumulated = vec![0.0_f64; fft_size / 2 + 1];
    let hop = fft_size / 2;
    let mut frame_count = 0usize;
    let mut start = 0usize;
    while start + fft_size <= samples.len() {
        for index in 0..fft_size {
            input[index] = samples[start + index] * window[index];
        }
        fft.process(&mut input, &mut output).unwrap();
        for (bin, value) in output.iter().enumerate() {
            accumulated[bin] += (value.re * value.re + value.im * value.im) as f64;
        }
        frame_count += 1;
        start += hop;
    }
    for value in &mut accumulated {
        *value /= frame_count as f64;
    }
    accumulated
}

/// Total power in the bins whose centre lies in `[low_hz, high_hz)`.
pub fn band_power(spectrum: &[f64], sample_rate: f32, low_hz: f32, high_hz: f32) -> f64 {
    let bin_hz = sample_rate as f64 / ANALYSIS_FFT_SIZE as f64;
    spectrum
        .iter()
        .enumerate()
        .filter(|(bin, _)| {
            let centre = *bin as f64 * bin_hz;
            centre >= low_hz as f64 && centre < high_hz as f64
        })
        .map(|(_, power)| *power)
        .sum()
}

/// Power spectral density (power per Hz, in dB, arbitrary reference) of the
/// band `[low_hz, high_hz)`. Dividing by bandwidth makes bands of different
/// width comparable and keeps the value independent of the sample rate.
pub fn band_density_db(spectrum: &[f64], sample_rate: f32, low_hz: f32, high_hz: f32) -> f64 {
    let bin_hz = sample_rate as f64 / ANALYSIS_FFT_SIZE as f64;
    // Bin power of a Hann-windowed frame is proportional to the per-sample
    // variance, while the continuous-time PSD is that variance spread over
    // sample_rate / 2 Hz. Dividing by the sample rate therefore makes the same
    // physical noise read the same density at 44.1, 48 and 96 kHz.
    let bins = spectrum
        .iter()
        .enumerate()
        .filter(|(bin, _)| {
            let centre = *bin as f64 * bin_hz;
            centre >= low_hz as f64 && centre < high_hz as f64
        })
        .count()
        .max(1) as f64;
    let power = band_power(spectrum, sample_rate, low_hz, high_hz) / bins;
    10.0 * (power / sample_rate as f64).max(1e-30).log10()
}

/// Least-squares slope in dB per octave of the power density across third-octave
/// bands between `low_hz` and `high_hz`.
pub fn spectral_slope_db_per_octave(
    spectrum: &[f64],
    sample_rate: f32,
    low_hz: f32,
    high_hz: f32,
) -> f64 {
    let third_octave = 2.0_f32.powf(1.0 / 3.0);
    let mut points: Vec<(f64, f64)> = Vec::new();
    let mut band_low = low_hz;
    while band_low * third_octave <= high_hz * 1.0001 {
        let band_high = band_low * third_octave;
        let centre = (band_low * band_high).sqrt();
        points.push((
            (centre as f64).log2(),
            band_density_db(spectrum, sample_rate, band_low, band_high),
        ));
        band_low = band_high;
    }
    assert!(points.len() >= 3, "frequency span too narrow for a slope");
    let count = points.len() as f64;
    let mean_x = points.iter().map(|(x, _)| x).sum::<f64>() / count;
    let mean_y = points.iter().map(|(_, y)| y).sum::<f64>() / count;
    let covariance: f64 = points
        .iter()
        .map(|(x, y)| (x - mean_x) * (y - mean_y))
        .sum();
    let variance: f64 = points
        .iter()
        .map(|(x, _)| (x - mean_x) * (x - mean_x))
        .sum();
    covariance / variance
}

pub fn mean(samples: &[f32]) -> f64 {
    samples.iter().map(|&value| value as f64).sum::<f64>() / samples.len() as f64
}

pub fn variance(samples: &[f32]) -> f64 {
    let average = mean(samples);
    samples
        .iter()
        .map(|&value| (value as f64 - average).powi(2))
        .sum::<f64>()
        / samples.len() as f64
}

pub fn root_mean_square(samples: &[f32]) -> f64 {
    (samples
        .iter()
        .map(|&value| (value as f64).powi(2))
        .sum::<f64>()
        / samples.len() as f64)
        .sqrt()
}

pub fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0.0_f32, |highest, value| highest.max(value.abs()))
}

/// Render `seconds` of audio from `next_sample`.
pub fn render(seconds: f32, sample_rate: f32, mut next_sample: impl FnMut(f32) -> f32) -> Vec<f32> {
    (0..(seconds * sample_rate) as usize)
        .map(|_| next_sample(sample_rate))
        .collect()
}

/// Cross-synth checks that need every generator side by side.
#[cfg(test)]
mod conformance {
    use super::*;
    use crate::synth::canopy_wind::CanopyWindSource;
    use crate::synth::field_wind::FieldWindSource;
    use crate::synth::rain::RainSource;
    use crate::synth::rain_v2::RainSourceV2;
    use crate::synth::river::RiverSource;
    use crate::synth::soft_wind::SoftWindSource;
    use crate::synth::storm_wind::StormWindSource;
    use crate::synth::thunder::{ThunderSource, KIND_CLAP};
    use crate::synth::wave::WaveSource;
    use crate::synth::wisp::WispSource;
    use atrium_core::source::{SoundSource, Utterance};
    use atrium_core::types::Vec3;

    type Maker = Box<dyn Fn() -> Box<dyn SoundSource>>;

    fn generators() -> Vec<(&'static str, Maker)> {
        vec![
            (
                "rain",
                Box::new(|| Box::new(RainSource::new(Vec3::ZERO, 0.5, 42))),
            ),
            (
                "rain_v2",
                Box::new(|| Box::new(RainSourceV2::new(Vec3::ZERO, 0.5, 42))),
            ),
            (
                "wave",
                Box::new(|| Box::new(WaveSource::new(Vec3::ZERO, 6.0, 1.0, 42))),
            ),
            (
                "river",
                Box::new(|| Box::new(RiverSource::new(Vec3::ZERO, 0.75, 0.75, 42))),
            ),
            (
                "wisp",
                Box::new(|| Box::new(WispSource::new(Vec3::ZERO, 42))),
            ),
            (
                "field_wind",
                Box::new(|| Box::new(FieldWindSource::new(Vec3::ZERO, 8.0, 8.0, 42))),
            ),
            (
                "soft_wind",
                Box::new(|| Box::new(SoftWindSource::new(Vec3::ZERO, 3.0, 3.0, 42))),
            ),
            (
                "storm_wind",
                Box::new(|| Box::new(StormWindSource::new(Vec3::ZERO, 15.0, 15.0, 42))),
            ),
            (
                "canopy_wind",
                Box::new(|| Box::new(CanopyWindSource::new(Vec3::ZERO, 8.0, 8.0, 42))),
            ),
            (
                // Silent until told to strike: one mid clap from the start.
                "thunder",
                Box::new(|| {
                    let mut thunder = ThunderSource::new(Vec3::ZERO, 42);
                    thunder.schedule(Utterance {
                        take: KIND_CLAP,
                        at_sample: 0,
                        gain: 1.0,
                        rate: 1.0,
                        skip_seconds: 0.0,
                    });
                    Box::new(thunder)
                }),
            ),
        ]
    }

    /// 20 s per render: the event-driven generators (rain v2: ~5 large drips
    /// in 5 s) spread their 6-10 kHz density by about +-0.4 dB between seeds
    /// over 5 s, and the 48 and 96 kHz renders draw different event streams,
    /// so a 5 s comparison carried up to ~1.4 dB of realization noise.
    fn density_6_to_10_khz(make: &Maker, sample_rate: f32) -> f64 {
        let mut source = make();
        let samples = render(20.0, sample_rate, |rate| source.next_sample(rate));
        band_density_db(
            &averaged_power_spectrum(&samples),
            sample_rate,
            6_000.0,
            10_000.0,
        )
    }

    /// Every live parameter of every generator that takes them ignores NaN
    /// and infinities: the previous value is kept and the output stays finite.
    #[test]
    fn non_finite_parameters_are_rejected_by_every_generator() {
        use atrium_core::commands::SynthParam;
        let parameters = [
            SynthParam::FlowSpeedMin,
            SynthParam::FlowSpeedMax,
            SynthParam::RiverSplashRate,
            SynthParam::MinSpeed,
            SynthParam::MaxSpeed,
            SynthParam::ChangeTimeMin,
            SynthParam::ChangeTimeMax,
            SynthParam::GustDurationMin,
            SynthParam::GustDurationMax,
            SynthParam::TurbulenceTimeMin,
            SynthParam::TurbulenceTimeMax,
            SynthParam::GustStrength,
            SynthParam::RiseBias,
            SynthParam::GustBrightness,
            SynthParam::TurbulenceBrightness,
            SynthParam::LowGain,
            SynthParam::BodyGain,
            SynthParam::MidGain,
            SynthParam::PresenceGain,
            SynthParam::AirGain,
            SynthParam::TurbulenceDepth,
            SynthParam::FoliageDensity,
            SynthParam::LeafDryness,
            SynthParam::BranchLevel,
            SynthParam::DebrisLevel,
            SynthParam::StructureLevel,
            SynthParam::MasterGain,
        ];
        for (name, make) in generators() {
            let mut reference = make();
            let mut poisoned = make();
            for parameter in parameters {
                for value in [f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
                    poisoned.set_synth_param(parameter, value);
                }
            }
            for _ in 0..4_800 {
                let expected = reference.next_sample(48_000.0);
                let measured = poisoned.next_sample(48_000.0);
                assert!(measured.is_finite(), "{name} produced {measured}");
                assert_eq!(
                    measured, expected,
                    "{name}: a non-finite value changed state"
                );
            }
        }
    }

    /// Every generator's output is a finite, bounded stream at the three
    /// host rates the engine can be configured for.
    #[test]
    fn every_generator_is_finite_and_bounded_at_every_sample_rate() {
        for (name, make) in generators() {
            for sample_rate in [44_100.0_f32, 48_000.0, 96_000.0] {
                let mut source = make();
                let samples = render(2.0, sample_rate, |rate| source.next_sample(rate));
                assert!(
                    samples.iter().all(|value| value.is_finite()),
                    "{name} @ {sample_rate}"
                );
                assert!(
                    peak(&samples) < 1.0,
                    "{name} @ {sample_rate}: peak {}",
                    peak(&samples)
                );
                assert!(
                    root_mean_square(&samples) > 1e-4,
                    "{name} @ {sample_rate} is silent"
                );
            }
        }
    }

    /// The same scene rendered at 48 kHz and 96 kHz should have the same
    /// power spectral density at a fixed frequency. Pink-driven layers do
    /// (their 1/f law is self-similar). A layer fed by raw `next_bipolar`
    /// white noise would put a fixed per-sample variance into twice the
    /// bandwidth and fall 3 dB per doubling of the rate, so every such layer
    /// is scaled by `noise::white_noise_density_scale`; filters and smoothing
    /// constants are specified in Hz, not per sample. Before that the 6-10 kHz
    /// density moved by -1.9 to -3.9 dB (river, wisp and the winds) and +1.6
    /// to +2.8 dB (rain, rain v2, wave) between 48 and 96 kHz.
    #[test]
    fn high_band_density_is_sample_rate_independent_for_every_generator() {
        let mut offenders = Vec::new();
        for (name, make) in generators() {
            let delta = density_6_to_10_khz(&make, 96_000.0) - density_6_to_10_khz(&make, 48_000.0);
            if delta.abs() > 1.5 {
                offenders.push(format!("{name}: {delta:+.1} dB"));
            }
        }
        assert!(
            offenders.is_empty(),
            "6-10 kHz density moved: {offenders:?}"
        );
    }
}
