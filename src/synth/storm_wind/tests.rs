use super::*;

fn mean_square(source: &mut StormWindSource, seconds: f32) -> f32 {
    let sample_rate = 48_000.0;
    let count = (seconds * sample_rate) as usize;
    (0..count)
        .map(|_| source.next_sample(sample_rate).powi(2))
        .sum::<f32>()
        / count as f32
}

#[test]
fn storm_wind_is_a_field_and_produces_audio() {
    let mut wind = StormWindSource::new(Vec3::ZERO, 8.0, 25.0, 42);
    assert_eq!(wind.emitter_kind(), EmitterKind::Field);
    assert!(mean_square(&mut wind, 1.0) > 0.0);
}

#[test]
fn tuned_defaults_remain_stable() {
    let wind = StormWindSource::new(Vec3::ZERO, 8.0, 18.0, 42);
    assert_eq!((wind.min_speed, wind.max_speed), (8.0, 18.0));
    assert_eq!(wind.change_time_range(), (12.0, 40.0));
    assert_eq!(wind.gust_duration_range(), (1.5, 14.0));
    assert_eq!(wind.turbulence_time_range(), (0.12, 0.80));
    assert_eq!(wind.gust_strength, 0.55);
    assert_eq!(wind.rise_bias, 0.45);
    assert_eq!(wind.turbulence_depth, 0.65);
    assert_eq!(wind.gust_brightness, 0.0);
    assert_eq!(wind.turbulence_brightness, 0.0);
    assert_eq!(wind.debris_level, 0.06);
    assert_eq!(wind.structure_level, 0.05);
    assert_eq!(wind.pressure_gain, 1.03);
    assert_eq!(wind.roar_gain, 1.50);
    assert_eq!(wind.shear_gain, 0.70);
    assert_eq!(wind.tear_gain, 0.35);
    assert_eq!(wind.master_gain, 0.85);
}

#[test]
fn zero_speed_range_is_true_calm() {
    let mut wind = StormWindSource::new(Vec3::ZERO, 0.0, 0.0, 43);
    for _ in 0..48_000 {
        assert_eq!(wind.next_sample(48_000.0), 0.0);
    }
}

#[test]
fn graphics_tick_and_callback_chunking_do_not_change_audio() {
    fn render(chunk_size: usize) -> Vec<f32> {
        let sample_rate = 48_000.0;
        let mut wind = StormWindSource::new(Vec3::ZERO, 8.0, 25.0, 99);
        let mut output = Vec::with_capacity(48_000);
        while output.len() < 48_000 {
            let frames = chunk_size.min(48_000 - output.len());
            wind.tick(frames as f32 / sample_rate);
            output.extend((0..frames).map(|_| wind.next_sample(sample_rate)));
        }
        output
    }

    assert_eq!(render(64), render(1024));
}

#[test]
fn storm_force_wind_has_much_more_energy_than_strong_breeze() {
    let mut strong = StormWindSource::new(Vec3::ZERO, 10.0, 10.0, 71);
    let mut storm = StormWindSource::new(Vec3::ZERO, 25.0, 25.0, 71);
    strong.debris_level = 0.0;
    strong.structure_level = 0.0;
    storm.debris_level = 0.0;
    storm.structure_level = 0.0;
    let strong_energy = mean_square(&mut strong, 2.0);
    let storm_energy = mean_square(&mut storm, 2.0);
    assert!(
        storm_energy > strong_energy * 10.0,
        "25 m/s should be much stronger: {storm_energy} vs {strong_energy}"
    );
}

#[test]
fn stronger_wind_adds_turbulent_high_frequency_tearing() {
    fn high_frequency_fraction(speed: f32) -> f32 {
        let sample_rate = 48_000.0;
        let mut wind = StormWindSource::new(Vec3::ZERO, speed, speed, 83);
        wind.debris_level = 0.0;
        wind.structure_level = 0.0;
        let mut highpass = OnePoleHP::new(1_800.0, sample_rate);
        let mut total = 0.0;
        let mut high = 0.0;
        for _ in 0..96_000 {
            let sample = wind.next_sample(sample_rate);
            total += sample * sample;
            let bright = highpass.process(sample);
            high += bright * bright;
        }
        high / total.max(f32::EPSILON)
    }

    let strong = high_frequency_fraction(10.0);
    let storm = high_frequency_fraction(25.0);
    assert!(
        storm > strong * 1.5,
        "storm force should tear brighter: {storm} vs {strong}"
    );
}

use crate::synth::test_support::{
    averaged_power_spectrum, band_density_db, mean, peak, render, root_mean_square,
};

// ── Sparse event layers ──────────────────────────────────────────────

/// Debris events started over `seconds`: each begins at
/// `debris_ring_index + 1` with an attack ramp, so slot `+ 2` changing
/// during a call marks one.
fn count_debris(wind: &mut StormWindSource, seconds: f32, sample_rate: f32) -> usize {
    let mut count = 0;
    for _ in 0..(seconds * sample_rate) as usize {
        let watched = (wind.debris_ring_index + 2) & DEBRIS_RING_MASK;
        let before = wind.debris_ring[watched];
        wind.next_sample(sample_rate);
        if wind.debris_ring[watched] != before {
            count += 1;
        }
    }
    count
}

/// rate = debris_level (0.03 + 2 storm_norm^2) per second above 10 m/s,
/// zero at or below it; storm_norm = (speed - 8) / 17.
#[test]
fn debris_rate_follows_the_storm_law_and_is_gated_by_speed() {
    let mut gale = StormWindSource::new(Vec3::ZERO, 25.0, 25.0, 3);
    gale.debris_level = 1.0;
    gale.structure_level = 0.0;
    let seconds = 100.0;
    let expected = 2.03 * seconds;
    let count = count_debris(&mut gale, seconds, 8_000.0) as f32;
    assert!(
        (count - expected).abs() < 4.0 * expected.sqrt(),
        "{count} debris events in {seconds} s, expected {expected}"
    );

    let mut strong = StormWindSource::new(Vec3::ZERO, 15.0, 15.0, 3);
    strong.debris_level = 1.0;
    strong.structure_level = 0.0;
    // storm_norm = 7 / 17: rate = 0.03 + 2 (0.412)^2 = 0.37 per second.
    let expected = (0.03 + 2.0 * (7.0_f32 / 17.0).powi(2)) * seconds;
    let count = count_debris(&mut strong, seconds, 8_000.0) as f32;
    assert!(
        (count - expected).abs() < 4.0 * expected.sqrt(),
        "{count} debris events at 15 m/s, expected {expected}"
    );

    let mut breeze = StormWindSource::new(Vec3::ZERO, 9.0, 9.0, 3);
    breeze.debris_level = 1.0;
    breeze.structure_level = 0.0;
    assert_eq!(count_debris(&mut breeze, 60.0, 8_000.0), 0);
}

/// Drive `next_structure` alone at a low rate so thousands of seconds of
/// events are cheap. Returns (events, lengths in samples, decays).
fn run_structure(
    wind: &mut StormWindSource,
    speed: f32,
    storm_norm: f32,
    seconds: f32,
    sample_rate: f32,
) -> (usize, Vec<usize>, Vec<f32>) {
    let (mut events, mut lengths, mut decays) = (0, Vec::new(), Vec::new());
    let mut running = 0usize;
    for _ in 0..(seconds * sample_rate) as usize {
        let was_active = wind.structure_envelope > 0.0;
        wind.next_structure(speed, storm_norm, sample_rate);
        let is_active = wind.structure_envelope > 0.0;
        if !was_active && is_active {
            events += 1;
            running = 0;
            decays.push(wind.structure_decay);
            assert!((35.0..=135.0).contains(&wind.structure_frequency));
        }
        if is_active || was_active {
            running += 1;
        }
        if was_active && !is_active {
            lengths.push(running);
        }
    }
    (events, lengths, decays)
}

/// The strain event's envelope is exp(-6.9 n / (duration x rate)): it
/// reaches -60 dB (the 0.001 cut-off) after exactly `duration` seconds.
#[test]
fn structure_events_decay_by_60_db_over_their_duration() {
    let sample_rate = 1_000.0;
    let mut wind = StormWindSource::new(Vec3::ZERO, 25.0, 25.0, 9);
    wind.structure_level = 1.0;
    let (events, lengths, decays) = run_structure(&mut wind, 25.0, 1.0, 400.0, sample_rate);
    assert!(events > 50 && lengths.len() >= events - 1);
    for (length, decay) in lengths.iter().zip(&decays) {
        let duration = -6.9 / (decay.ln() * sample_rate);
        assert!((0.40..=2.0).contains(&duration), "duration {duration}");
        // -6.9 nepers is 0.001; the loop stops once it falls below, so the
        // length is duration x rate to within the rounding of one sample.
        let expected = duration * sample_rate * (6.9078 / 6.9);
        assert!(
            (*length as f32 - expected).abs() <= 2.0,
            "{length} vs {expected}"
        );
    }
}

/// Events start only above 13 m/s; their rate is level (0.01 + 0.6
/// storm_norm^2) and a new one cannot start until the last has died, so
/// the observed renewal rate is 1 / (1 / rate + mean duration 1.2 s).
#[test]
fn structure_event_rate_follows_the_renewal_law_and_speed_gate() {
    let sample_rate = 1_000.0;
    let mut wind = StormWindSource::new(Vec3::ZERO, 25.0, 25.0, 10);
    wind.structure_level = 1.0;
    let seconds = 2_000.0;
    let (events, _, _) = run_structure(&mut wind, 25.0, 1.0, seconds, sample_rate);
    let rate = 0.01 + 0.6;
    let expected = seconds / (1.0 / rate + 1.2 * 1.0011);
    assert!(
        (events as f32 - expected).abs() < 5.0 * expected.sqrt(),
        "{events} strain events in {seconds} s, expected {expected:.0}"
    );
    let mut calm = StormWindSource::new(Vec3::ZERO, 12.9, 12.9, 10);
    calm.structure_level = 1.0;
    assert_eq!(run_structure(&mut calm, 12.9, 0.3, 500.0, sample_rate).0, 0);
    let mut off = StormWindSource::new(Vec3::ZERO, 25.0, 25.0, 10);
    off.structure_level = 0.0;
    assert_eq!(run_structure(&mut off, 25.0, 1.0, 500.0, sample_rate).0, 0);
}

// ── Rendered audio ───────────────────────────────────────────────────

/// Pressure law: (speed / 12)^1.8 capped at 3.8 (never reached below 25 m/s).
#[test]
fn level_follows_the_power_law_in_speed() {
    let level = |speed: f32| {
        let mut wind = StormWindSource::new(Vec3::ZERO, speed, speed, 71);
        wind.debris_level = 0.0;
        wind.structure_level = 0.0;
        root_mean_square(&render(15.0, 48_000.0, |rate| wind.next_sample(rate)))
    };
    let ratio = level(20.0) / level(10.0);
    // Gain ratio 2^1.8 = 3.5, times a stronger shear/tear mix as
    // storm_norm rises from 0.12 to 0.71: texture RMS grows about 2.1x,
    // so the ratio is about 7.5 (measured 7.4-7.7 over five seeds).
    // Rendered at the 48 kHz reference: at 16 kHz the tear band is cut
    // off at the 8 kHz Nyquist limit, and this read 12.4 there before
    // the white tear layer kept its density per Hz (the unscaled noise put
    // the whole 48 kHz variance below 8 kHz, +4.8 dB of tear).
    assert!(
        (6.0..10.0).contains(&ratio),
        "RMS ratio 20 vs 10 m/s is {ratio}"
    );
    assert!((25.0_f32 / 12.0).powf(1.8) < 3.8);
}

#[test]
fn output_is_finite_zero_mean_and_inside_unity_for_a_strong_breeze() {
    let mut wind = StormWindSource::new(Vec3::ZERO, 8.0, 18.0, 7);
    let samples = render(20.0, 48_000.0, |rate| wind.next_sample(rate));
    assert!(samples.iter().all(|value| value.is_finite()));
    assert!(peak(&samples) < 1.0, "peak {}", peak(&samples));
    assert!(mean(&samples).abs() < 0.01, "DC {}", mean(&samples));
}

/// The pressure layer (18 Hz highpass, two 180 Hz lowpass poles) is the
/// rumble: removing it drops the 30-120 Hz density by several dB.
#[test]
fn pressure_layer_supplies_the_low_frequency_rumble() {
    let rumble = |pressure_gain: f32| {
        let mut wind = StormWindSource::new(Vec3::ZERO, 18.0, 18.0, 5);
        wind.debris_level = 0.0;
        wind.structure_level = 0.0;
        wind.pressure_gain = pressure_gain;
        let samples = render(12.0, 48_000.0, |rate| wind.next_sample(rate));
        band_density_db(&averaged_power_spectrum(&samples), 48_000.0, 30.0, 120.0)
    };
    assert!(rumble(1.03) > rumble(0.0) + 3.0);
}

#[test]
fn low_and_mid_band_density_is_sample_rate_independent() {
    let density = |sample_rate: f32| {
        let mut wind = StormWindSource::new(Vec3::ZERO, 14.0, 14.0, 73);
        wind.debris_level = 0.0;
        wind.structure_level = 0.0;
        let samples = render(10.0, sample_rate, |rate| wind.next_sample(rate));
        let spectrum = averaged_power_spectrum(&samples);
        (
            band_density_db(&spectrum, sample_rate, 100.0, 200.0),
            band_density_db(&spectrum, sample_rate, 700.0, 1_000.0),
        )
    };
    let reference = density(48_000.0);
    for sample_rate in [44_100.0, 96_000.0] {
        let (low, mid) = density(sample_rate);
        assert!(
            (low - reference.0).abs() < 1.0,
            "{sample_rate}: low {low} vs {}",
            reference.0
        );
        assert!(
            (mid - reference.1).abs() < 1.0,
            "{sample_rate}: mid {mid} vs {}",
            reference.1
        );
    }
}

#[test]
fn same_seed_is_bit_identical_and_other_seeds_differ() {
    let run = |seed: u64| {
        let mut wind = StormWindSource::new(Vec3::ZERO, 12.0, 25.0, seed);
        render(0.5, 48_000.0, |rate| wind.next_sample(rate))
    };
    assert_eq!(run(5), run(5));
    assert_ne!(run(5), run(6));
}

// ── Parameters ───────────────────────────────────────────────────────

#[test]
fn speed_range_and_levels_are_clamped_and_routed() {
    let mut wind = StormWindSource::new(Vec3::ZERO, -3.0, 99.0, 1);
    assert_eq!((wind.min_speed, wind.max_speed), (0.0, 25.0));
    wind.set_speed_range(20.0, 6.0);
    assert_eq!((wind.min_speed, wind.max_speed), (6.0, 20.0));
    wind.set_synth_param(SynthParam::DebrisLevel, 4.0);
    assert_eq!(wind.debris_level, 1.0);
    wind.set_synth_param(SynthParam::StructureLevel, -4.0);
    assert_eq!(wind.structure_level, 0.0);
    wind.set_synth_param(SynthParam::LowGain, 0.1);
    wind.set_synth_param(SynthParam::BodyGain, 0.2);
    wind.set_synth_param(SynthParam::MidGain, 0.3);
    wind.set_synth_param(SynthParam::AirGain, 0.4);
    assert_eq!(
        (
            wind.pressure_gain,
            wind.roar_gain,
            wind.shear_gain,
            wind.tear_gain
        ),
        (0.1, 0.2, 0.3, 0.4)
    );
    wind.set_synth_param(SynthParam::MasterGain, 3.0);
    assert_eq!(wind.master_gain, 2.0);
}
