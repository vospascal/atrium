use std::f64::consts::TAU;

use atrium_core::commands::SynthParam;
use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;
use realfft::RealFftPlanner;

use super::*;
use crate::synth::test_support::{averaged_power_spectrum, render, ANALYSIS_FFT_SIZE};

fn tone(settings: FocusToneSettings, seed: u64) -> FocusToneSource {
    FocusToneSource::new(Vec3::ZERO, settings, seed)
}

fn without_shimmer() -> FocusToneSettings {
    FocusToneSettings {
        shimmer_depth: 0.0,
        ..FocusToneSettings::default()
    }
}

/// Amplitude of the component at `frequency_hz`: a Hann-windowed single-bin
/// DFT, scaled so a sine of amplitude A reads A. Exact when the stretch holds
/// a whole number of cycles of it.
fn amplitude_at(samples: &[f32], sample_rate: f32, frequency_hz: f64) -> f64 {
    let length = samples.len();
    let step = TAU * frequency_hz / sample_rate as f64;
    let (mut real, mut imaginary, mut window_sum) = (0.0, 0.0, 0.0);
    for (index, &sample) in samples.iter().enumerate() {
        let window = 0.5 - 0.5 * (TAU * index as f64 / length as f64).cos();
        let angle = step * index as f64;
        real += window * sample as f64 * angle.cos();
        imaginary -= window * sample as f64 * angle.sin();
        window_sum += window;
    }
    2.0 * (real * real + imaginary * imaginary).sqrt() / window_sum
}

/// Frequency of the loudest component between `low_hz` and `high_hz`: one
/// Hann-windowed FFT of the whole stretch, the peak bin refined by a parabola
/// through the log magnitudes of it and its two neighbours.
fn peak_frequency(samples: &[f32], sample_rate: f32, low_hz: f64, high_hz: f64) -> f64 {
    let length = samples.len();
    let mut planner = RealFftPlanner::<f64>::new();
    let fft = planner.plan_fft_forward(length);
    let mut input: Vec<f64> = samples
        .iter()
        .enumerate()
        .map(|(index, &sample)| {
            sample as f64 * (0.5 - 0.5 * (TAU * index as f64 / length as f64).cos())
        })
        .collect();
    let mut output = fft.make_output_vec();
    fft.process(&mut input, &mut output).unwrap();
    let bin_hz = sample_rate as f64 / length as f64;
    let first = (low_hz / bin_hz) as usize;
    let last = (high_hz / bin_hz) as usize;
    let loudest = (first..=last)
        .max_by(|&a, &b| output[a].norm().total_cmp(&output[b].norm()))
        .unwrap();
    let log_magnitude = |bin: usize| output[bin].norm().ln();
    let (left, centre, right) = (
        log_magnitude(loudest - 1),
        log_magnitude(loudest),
        log_magnitude(loudest + 1),
    );
    let offset = 0.5 * (left - right) / (left - 2.0 * centre + right);
    (loudest as f64 + offset) * bin_hz
}

/// Largest difference between two neighbouring samples.
fn largest_step(samples: &[f32]) -> f32 {
    samples.windows(2).fold(0.0_f32, |largest, pair| {
        largest.max((pair[1] - pair[0]).abs())
    })
}

/// The preset's sound, shimmer and overtones on: the loudest component near
/// the pitch is at 333.0 Hz. 20 s gives 0.05 Hz bins; the shimmer partial
/// sits 0.1 to 0.4 Hz above and does not pull the peak.
#[test]
fn the_fundamental_measures_333_hz_within_a_tenth_of_a_hertz() {
    for sample_rate in [44_100.0_f32, 48_000.0] {
        let mut source = tone(FocusToneSettings::default(), 3);
        let samples = render(21.0, sample_rate, |rate| source.next_sample(rate));
        let measured = peak_frequency(&samples[sample_rate as usize..], sample_rate, 300.0, 370.0);
        assert!(
            (measured - 333.0).abs() < 0.1,
            "{sample_rate}: peak at {measured:.4} Hz"
        );
    }
}

/// The fundamental alone, counted by upward zero crossings over 10 s:
/// 3330 cycles, interpolated between samples. Agrees with 333 Hz to well
/// inside a hundredth of a hertz.
#[test]
fn the_fundamental_alone_crosses_zero_333_times_a_second() {
    let sample_rate = 48_000.0;
    let mut source = tone(
        FocusToneSettings {
            overtone_level: 0.0,
            ..without_shimmer()
        },
        3,
    );
    let samples = render(11.0, sample_rate, |rate| source.next_sample(rate));
    let mut crossings = Vec::new();
    for index in sample_rate as usize..samples.len() - 1 {
        let (before, after) = (samples[index], samples[index + 1]);
        if before < 0.0 && after >= 0.0 {
            let fraction = before as f64 / (before as f64 - after as f64);
            crossings.push((index as f64 + fraction) / sample_rate as f64);
        }
    }
    let cycles = (crossings.len() - 1) as f64;
    let measured = cycles / (crossings.last().unwrap() - crossings[0]);
    assert!((measured - 333.0).abs() < 0.01, "{measured:.5} Hz");
}

/// Harmonic n is at n · 333 Hz with amplitude overtone_level · b^(n − 1),
/// the fundamental at fundamental_level. 10 s of 333 Hz is a whole number of
/// cycles of every harmonic, so the windowed DFT reads each exactly.
#[test]
fn harmonics_sit_at_whole_multiples_and_fall_by_the_brightness_law() {
    let sample_rate = 48_000.0;
    for brightness in [0.35_f32, 0.6] {
        let settings = FocusToneSettings {
            brightness,
            ..without_shimmer()
        };
        let mut source = tone(settings, 5);
        let samples = render(11.0, sample_rate, |rate| source.next_sample(rate));
        let stretch = &samples[sample_rate as usize..];
        let fundamental = amplitude_at(stretch, sample_rate, 333.0);
        let fundamental_db = 20.0 * (fundamental / settings.fundamental_level as f64).log10();
        assert!(
            fundamental_db.abs() < 0.01,
            "fundamental off by {fundamental_db:.3} dB"
        );
        for harmonic in 2..=6 {
            let measured = amplitude_at(stretch, sample_rate, 333.0 * harmonic as f64);
            let expected = settings.overtone_level as f64 * (brightness as f64).powi(harmonic - 1);
            let error_db = 20.0 * (measured / expected).log10();
            assert!(
                error_db.abs() < 0.01,
                "brightness {brightness}, harmonic {harmonic}: {measured:.6} against {expected:.6}"
            );
            // Halfway between two harmonics there is nothing.
            let between = amplitude_at(stretch, sample_rate, 333.0 * (harmonic as f64 - 0.5));
            assert!(between < 1e-5 * expected, "energy at {harmonic} - 1/2");
        }
    }
}

/// Bright and high: 5 kHz at brightness 0.9. At 44.1 kHz the ceiling is
/// 20 kHz, so only 5, 10 and 15 kHz may sound; harmonic 4 lands on the
/// ceiling (weight 0) and harmonic 5 (25 kHz) would fold to 19.1 kHz at
/// 0.66 of full level. Every bin away from the three is more than 80 dB
/// below the total, and at 96 kHz nothing reaches past 20 kHz either.
#[test]
fn nothing_is_made_at_or_above_the_ceiling() {
    let settings = FocusToneSettings {
        pitch_hz: 5_000.0,
        brightness: 0.9,
        overtone_level: 1.0,
        ..without_shimmer()
    };
    for (sample_rate, allowed) in [
        (44_100.0_f32, vec![5_000.0, 10_000.0, 15_000.0]),
        (96_000.0, vec![5_000.0, 10_000.0, 15_000.0]),
    ] {
        let mut source = tone(settings, 9);
        let samples = render(3.0, sample_rate, |rate| source.next_sample(rate));
        let spectrum = averaged_power_spectrum(&samples[sample_rate as usize..]);
        let bin_hz = sample_rate as f64 / ANALYSIS_FFT_SIZE as f64;
        let total: f64 = spectrum.iter().sum();
        let stray: f64 = spectrum
            .iter()
            .enumerate()
            .filter(|(bin, _)| {
                let centre = *bin as f64 * bin_hz;
                allowed
                    .iter()
                    .all(|frequency: &f64| (centre - frequency).abs() > 40.0 * bin_hz)
            })
            .map(|(_, power)| power)
            .sum();
        let stray_db = 10.0 * (stray / total).log10();
        assert!(
            stray_db < -80.0,
            "{sample_rate}: stray power {stray_db:.1} dB"
        );
        for (index, frequency) in allowed.iter().enumerate() {
            // The fundamental at its level 0.2; harmonic n at 0.9^(n - 1).
            let expected = match index {
                0 => 0.2,
                _ => 0.9_f64.powi(index as i32),
            };
            let measured = amplitude_at(&samples[sample_rate as usize..], sample_rate, *frequency);
            // 15 kHz sits below the fade (18 kHz): at full weight.
            assert!(
                (20.0 * (measured / expected).log10()).abs() < 0.05,
                "{sample_rate}: {frequency} Hz at {measured:.4}, expected {expected:.4}"
            );
        }
    }
}

/// The beat rate, measured from the sound: the fundamental layer alone,
/// shifted down by the pitch, is (L / 2i)(1 + d e^(iθ)), so the shimmer's
/// phase θ against the fundamental is the angle of 2i·z/L − 1. Averaged over
/// 0.1 s blocks (the 666 Hz image falls to 0.2 %), unwrapped, and
/// differenced over each second, it gives the beat rate to about 0.003 Hz.
fn measured_beat_rates(
    samples: &[f32],
    first_sample: usize,
    sample_rate: f32,
    settings: FocusToneSettings,
) -> Vec<f64> {
    let block = (0.1 * sample_rate) as usize;
    let step = TAU * settings.pitch_hz as f64 / sample_rate as f64;
    let mut phases: Vec<f64> = Vec::new();
    for (block_index, chunk) in samples.chunks_exact(block).enumerate() {
        let (mut real, mut imaginary) = (0.0, 0.0);
        for (offset, &sample) in chunk.iter().enumerate() {
            let angle = step * (first_sample + block_index * block + offset) as f64;
            real += sample as f64 * angle.cos();
            imaginary -= sample as f64 * angle.sin();
        }
        real /= block as f64;
        imaginary /= block as f64;
        // 2i · z / L − 1, with z = real + i · imaginary.
        let level = settings.fundamental_level as f64;
        let (shimmer_real, shimmer_imaginary) =
            (-2.0 * imaginary / level - 1.0, 2.0 * real / level);
        let angle = shimmer_imaginary.atan2(shimmer_real);
        let unwrapped = match phases.last() {
            Some(&previous) => {
                previous + (angle - previous + std::f64::consts::PI).rem_euclid(TAU)
                    - std::f64::consts::PI
            }
            None => angle,
        };
        phases.push(unwrapped);
    }
    phases
        .windows(11)
        .step_by(10)
        .map(|second| (second[10] - second[0]) / TAU)
        .collect()
}

#[test]
fn the_shimmer_beats_inside_its_range_and_wanders() {
    let sample_rate = 16_000.0;
    let settings = FocusToneSettings {
        overtone_level: 0.0,
        ..FocusToneSettings::default()
    };
    let mut source = tone(settings, 21);
    let mut samples = Vec::new();
    for _ in 0..(120.0 * sample_rate) as usize {
        let rate = source.shimmer_rate_hz();
        assert!(
            (settings.shimmer_rate_min_hz..=settings.shimmer_rate_max_hz).contains(&rate),
            "rate {rate}"
        );
        samples.push(source.next_sample(sample_rate));
    }
    let first_sample = sample_rate as usize;
    let rates = measured_beat_rates(
        &samples[first_sample..],
        first_sample,
        sample_rate,
        settings,
    );
    assert_eq!(rates.len(), 118);
    let slowest = rates.iter().copied().fold(f64::MAX, f64::min);
    let fastest = rates.iter().copied().fold(f64::MIN, f64::max);
    assert!(slowest > 0.1 - 0.01, "slowest beat {slowest:.4} Hz");
    assert!(fastest < 0.4 + 0.01, "fastest beat {fastest:.4} Hz");
    assert!(
        fastest - slowest > 0.1,
        "the beat barely moves: {slowest:.3} to {fastest:.3} Hz"
    );
}

/// A new range applies from the next sample: the glide's two ends are kept
/// inside it, and later rates are drawn from it.
#[test]
fn a_new_shimmer_range_applies_at_once() {
    let sample_rate = 8_000.0;
    let mut source = tone(FocusToneSettings::default(), 4);
    render(3.0, sample_rate, |rate| source.next_sample(rate));
    source.set_synth_param(SynthParam::ShimmerRateMax, 0.6);
    source.set_synth_param(SynthParam::ShimmerRateMin, 0.5);
    for _ in 0..(60.0 * sample_rate) as usize {
        let rate = source.shimmer_rate_hz();
        assert!((0.5..=0.6).contains(&rate), "rate {rate}");
        source.next_sample(sample_rate);
    }
    // A minimum above the maximum is held at the maximum.
    source.set_synth_param(SynthParam::ShimmerRateMin, 2.0);
    assert_eq!(source.target.shimmer_rate_min_hz, 0.6);
}

#[test]
fn the_same_seed_gives_the_same_samples() {
    let render_seed = |seed: u64| {
        let mut source = tone(FocusToneSettings::default(), seed);
        render(5.0, 48_000.0, |rate| source.next_sample(rate))
    };
    let first = render_seed(7);
    assert_eq!(first, render_seed(7));
    assert_ne!(first, render_seed(8));
}

/// It starts from silence and fades in over half a second without a step
/// larger than the steady tone's own.
#[test]
fn it_starts_without_a_click() {
    let sample_rate = 48_000.0;
    let mut source = tone(FocusToneSettings::default(), 2);
    let samples = render(21.0, sample_rate, |rate| source.next_sample(rate));
    assert_eq!(samples[0], 0.0);
    let onset = largest_step(&samples[..(0.5 * sample_rate) as usize]);
    let steady = largest_step(&samples[sample_rate as usize..]);
    assert!(onset <= steady, "onset step {onset}, steady {steady}");
}

/// A change while it plays glides: the largest step across the change is no
/// larger than the larger of the two steady tones' own over the same
/// stretch. The shimmer's drift and phase against the fundamental do not
/// depend on the pitch or levels, so both references beat in step with the
/// changed tone.
#[test]
fn a_change_while_it_plays_steps_no_more_than_the_tone_itself() {
    let sample_rate = 48_000.0;
    let changes = [
        (SynthParam::Pitch, 444.0),
        (SynthParam::Pitch, 222.0),
        (SynthParam::FundamentalLevel, 0.6),
        (SynthParam::OvertoneLevel, 0.8),
        (SynthParam::Brightness, 0.8),
        (SynthParam::ShimmerDepth, 1.0),
    ];
    let change_at = 2 * sample_rate as usize;
    let window = change_at - 4_800..change_at + 24_000;
    for (param, value) in changes {
        let mut changed = tone(FocusToneSettings::default(), 6);
        let mut before = tone(FocusToneSettings::default(), 6);
        let mut after = tone(FocusToneSettings::default(), 6);
        after.set_synth_param(param, value);
        after.current = after.target;
        let mut samples = Vec::new();
        let mut reference_before = Vec::new();
        let mut reference_after = Vec::new();
        for index in 0..change_at + sample_rate as usize {
            if index == change_at {
                changed.set_synth_param(param, value);
            }
            samples.push(changed.next_sample(sample_rate));
            reference_before.push(before.next_sample(sample_rate));
            reference_after.push(after.next_sample(sample_rate));
        }
        let step = largest_step(&samples[window.clone()]);
        let own = largest_step(&reference_before[window.clone()])
            .max(largest_step(&reference_after[window.clone()]));
        assert!(
            step <= own * 1.02,
            "{param:?} to {value}: step {step:.5}, the tone's own {own:.5}"
        );
    }
}

/// The step test above would catch a click: the same level change applied
/// at once, on a sample near the crest, steps far more than the tone does.
#[test]
fn without_the_glide_a_level_change_would_step() {
    let sample_rate = 48_000.0;
    let mut source = tone(
        FocusToneSettings {
            overtone_level: 0.0,
            ..without_shimmer()
        },
        6,
    );
    let samples = render(2.0, sample_rate, |rate| source.next_sample(rate));
    let own = largest_step(&samples[sample_rate as usize..]);
    let mut last = *samples.last().unwrap();
    while source.layers().0 < 0.15 {
        last = source.next_sample(sample_rate);
    }
    source.set_synth_param(SynthParam::FundamentalLevel, 1.0);
    source.current = source.target;
    let jumped = source.next_sample(sample_rate);
    assert!(
        (jumped - last).abs() > 2.0 * own,
        "jump {}, own {own}",
        (jumped - last).abs()
    );
}

/// A pitch held outside its range, a level below zero or a brightness above
/// 0.9 is clamped, not refused; another generator's setting is ignored.
#[test]
fn settings_are_clamped_and_others_ignored() {
    let mut source = tone(FocusToneSettings::default(), 1);
    source.set_synth_param(SynthParam::Pitch, 1.0);
    source.set_synth_param(SynthParam::Brightness, 3.0);
    source.set_synth_param(SynthParam::FundamentalLevel, -1.0);
    source.set_synth_param(SynthParam::ShimmerDepth, 4.0);
    assert_eq!(source.target.pitch_hz, 20.0);
    assert_eq!(source.target.brightness, 0.9);
    assert_eq!(source.target.fundamental_level, 0.0);
    assert_eq!(source.target.shimmer_depth, 1.0);
    let before = source.target;
    source.set_synth_param(SynthParam::GustStrength, 0.5);
    source.set_synth_param(SynthParam::Pitch, f32::NAN);
    assert_eq!(source.target, before);
}
