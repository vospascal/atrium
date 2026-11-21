use super::*;

#[test]
fn soft_wind_is_a_field_and_produces_audio() {
    let mut wind = SoftWindSource::new(Vec3::ZERO, 1.0, 5.0, 42);
    assert_eq!(wind.emitter_kind(), EmitterKind::Field);
    let energy: f32 = (0..48_000)
        .map(|_| wind.next_sample(48_000.0).powi(2))
        .sum();
    assert!(energy > 0.0);
}

#[test]
fn tuned_defaults_preserve_the_reference_derived_driver_layers() {
    let wind = SoftWindSource::new(Vec3::ZERO, 1.0, 5.0, 42);
    assert_eq!(wind.change_time_range(), (10.0, 24.0));
    assert_eq!(wind.gust_duration_range(), (0.8, 4.0));
    assert_eq!(wind.turbulence_time_range(), (0.05, 0.40));
    assert_eq!(wind.gust_strength, 0.25);
    assert_eq!(wind.turbulence_depth, 0.35);
    assert_eq!(wind.gust_brightness, 0.18);
    assert_eq!(wind.turbulence_brightness, 0.10);
}

#[test]
fn soft_wind_is_substantially_brighter_than_field_wind() {
    fn high_frequency_fraction(source: &mut dyn SoundSource) -> f32 {
        let sample_rate = 48_000.0;
        let mut highpass = OnePoleHP::new(2_000.0, sample_rate);
        let (mut total, mut high) = (0.0_f32, 0.0_f32);
        for _ in 0..192_000 {
            let sample = source.next_sample(sample_rate);
            total += sample * sample;
            let bright = highpass.process(sample);
            high += bright * bright;
        }
        high / total.max(f32::EPSILON)
    }

    let mut soft = SoftWindSource::new(Vec3::ZERO, 1.0, 5.0, 46);
    let mut field = super::super::field_wind::FieldWindSource::new(Vec3::ZERO, 1.0, 5.0, 46);
    let soft_bright = high_frequency_fraction(&mut soft);
    let field_bright = high_frequency_fraction(&mut field);
    assert!(
        soft_bright > field_bright * 4.0,
        "soft reference response should be much brighter: {soft_bright} vs {field_bright}"
    );
}

#[test]
fn zero_speed_range_is_true_calm() {
    let mut wind = SoftWindSource::new(Vec3::ZERO, 0.0, 0.0, 43);
    for _ in 0..48_000 {
        assert_eq!(wind.next_sample(48_000.0), 0.0);
    }
}

#[test]
fn graphics_tick_and_callback_chunking_do_not_change_audio() {
    fn render(chunk_size: usize) -> Vec<f32> {
        let sample_rate = 48_000.0;
        let mut wind = SoftWindSource::new(Vec3::ZERO, 1.0, 5.0, 99);
        let mut output = Vec::with_capacity(96_000);
        while output.len() < 96_000 {
            let frames = chunk_size.min(96_000 - output.len());
            wind.tick(frames as f32 / sample_rate);
            output.extend((0..frames).map(|_| wind.next_sample(sample_rate)));
        }
        output
    }

    assert_eq!(render(64), render(1024));
}

use crate::synth::test_support::{
    averaged_power_spectrum, band_density_db, band_power, mean, peak, render, root_mean_square,
};

/// "Relatively modest level movement". The explicit level term is the sum
/// of weather (+-0.9 dB), gust (0 to +3.4 dB) and eddy (+-0.58 dB at
/// default depth), but gusts also open the presence and air layers
/// (brightness scales them 5x), so half-second RMS windows swing about
/// 7.4 dB with the weather and gusts sped up 20x to fit a short test.
#[test]
fn level_movement_stays_within_a_few_decibels() {
    let mut wind = SoftWindSource::new(Vec3::ZERO, 2.0, 5.0, 12);
    wind.set_change_time_range(0.5, 1.0);
    wind.set_gust_duration_range(0.2, 0.6);
    let sample_rate = 16_000.0;
    let samples = render(60.0, sample_rate, |rate| wind.next_sample(rate));
    let window = sample_rate as usize / 2;
    let levels: Vec<f64> = samples
        .chunks_exact(window)
        .map(|chunk| 20.0 * root_mean_square(chunk).log10())
        .collect();
    let highest = levels.iter().cloned().fold(f64::MIN, f64::max);
    let lowest = levels.iter().cloned().fold(f64::MAX, f64::min);
    assert!(
        highest - lowest < 9.0,
        "level swing {:.1} dB",
        highest - lowest
    );
    assert!(
        highest - lowest > 1.0,
        "no movement at all: {:.1} dB",
        highest - lowest
    );
}

/// Most of the energy is above 2 kHz (reference: substantial 2-16 kHz
/// energy with a high, stable noise floor).
#[test]
fn energy_sits_in_the_upper_spectrum() {
    let mut wind = SoftWindSource::new(Vec3::ZERO, 3.0, 3.0, 5);
    let samples = render(12.0, 48_000.0, |rate| wind.next_sample(rate));
    let spectrum = averaged_power_spectrum(&samples);
    let above = band_power(&spectrum, 48_000.0, 2_000.0, 24_000.0);
    let total = band_power(&spectrum, 48_000.0, 20.0, 24_000.0);
    assert!(
        above / total > 0.5,
        "above 2 kHz holds only {:.2}",
        above / total
    );
    // The bright body is louder per Hz than the deliberately thin low
    // layer (low_gain 0.02).
    assert!(
        band_density_db(&spectrum, 48_000.0, 3_000.0, 6_000.0)
            > band_density_db(&spectrum, 48_000.0, 80.0, 160.0) + 1.0
    );
}

/// Faster airflow opens the high bands: brightness = 0.52 + 0.18
/// speed_norm + ..., so the HF-to-mid balance rises with speed.
#[test]
fn faster_wind_is_brighter() {
    let balance = |speed: f32| {
        let mut wind = SoftWindSource::new(Vec3::ZERO, speed, speed, 6);
        let samples = render(12.0, 48_000.0, |rate| wind.next_sample(rate));
        let spectrum = averaged_power_spectrum(&samples);
        band_density_db(&spectrum, 48_000.0, 5_000.0, 9_000.0)
            - band_density_db(&spectrum, 48_000.0, 500.0, 900.0)
    };
    assert!(balance(8.0) > balance(1.0) + 0.5);
}

#[test]
fn output_is_finite_zero_mean_and_inside_unity() {
    for (minimum, maximum) in [(1.0_f32, 5.0_f32), (5.0, 25.0)] {
        let mut wind = SoftWindSource::new(Vec3::ZERO, minimum, maximum, 7);
        let samples = render(20.0, 48_000.0, |rate| wind.next_sample(rate));
        assert!(samples.iter().all(|value| value.is_finite()));
        assert!(peak(&samples) < 1.0, "peak {}", peak(&samples));
        assert!(mean(&samples).abs() < 0.002, "DC {}", mean(&samples));
    }
}

/// Unlike field wind there is no speed-squared pressure law: the level is
/// flat-ish in airflow speed (a high, stable floor).
#[test]
fn level_is_nearly_independent_of_airflow_speed() {
    let level = |speed: f32| {
        let mut wind = SoftWindSource::new(Vec3::ZERO, speed, speed, 8);
        root_mean_square(&render(10.0, 24_000.0, |rate| wind.next_sample(rate)))
    };
    let ratio = level(10.0) / level(1.0);
    assert!(
        (0.8..2.0).contains(&ratio),
        "RMS ratio 10 vs 1 m/s is {ratio}"
    );
}

#[test]
fn mid_band_density_is_sample_rate_independent() {
    let density = |sample_rate: f32| {
        let mut wind = SoftWindSource::new(Vec3::ZERO, 3.0, 3.0, 73);
        let samples = render(10.0, sample_rate, |rate| wind.next_sample(rate));
        let spectrum = averaged_power_spectrum(&samples);
        (
            band_density_db(&spectrum, sample_rate, 300.0, 500.0),
            band_density_db(&spectrum, sample_rate, 700.0, 1_000.0),
        )
    };
    let reference = density(48_000.0);
    for sample_rate in [44_100.0, 96_000.0] {
        let (low, mid) = density(sample_rate);
        assert!(
            (low - reference.0).abs() < 1.0,
            "{sample_rate}: {low} vs {}",
            reference.0
        );
        assert!(
            (mid - reference.1).abs() < 1.0,
            "{sample_rate}: {mid} vs {}",
            reference.1
        );
    }
}

#[test]
fn same_seed_is_bit_identical_and_other_seeds_differ() {
    let run = |seed: u64| {
        let mut wind = SoftWindSource::new(Vec3::ZERO, 1.0, 6.0, seed);
        render(0.5, 48_000.0, |rate| wind.next_sample(rate))
    };
    assert_eq!(run(5), run(5));
    assert_ne!(run(5), run(6));
}

#[test]
fn parameters_are_clamped_and_routed() {
    let mut wind = SoftWindSource::new(Vec3::ZERO, -1.0, 80.0, 1);
    assert_eq!((wind.min_speed, wind.max_speed), (0.0, 25.0));
    wind.set_speed_range(9.0, 3.0);
    assert_eq!((wind.min_speed, wind.max_speed), (3.0, 9.0));
    wind.set_synth_param(SynthParam::GustBrightness, 3.0);
    assert_eq!(wind.gust_brightness, 1.0);
    wind.set_synth_param(SynthParam::TurbulenceDepth, -3.0);
    assert_eq!(wind.turbulence_depth, 0.0);
    wind.set_synth_param(SynthParam::PresenceGain, 0.5);
    assert_eq!(wind.presence_gain, 0.5);
    wind.set_synth_param(SynthParam::MasterGain, 5.0);
    assert_eq!(wind.master_gain, 2.0);
}
