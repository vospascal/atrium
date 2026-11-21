use super::*;

fn mean_square(source: &mut CanopyWindSource, seconds: f32) -> f32 {
    let sample_rate = 48_000.0;
    let count = (seconds * sample_rate) as usize;
    (0..count)
        .map(|_| source.next_sample(sample_rate).powi(2))
        .sum::<f32>()
        / count as f32
}

#[test]
fn canopy_wind_is_a_field_and_produces_audio() {
    let mut wind = CanopyWindSource::new(Vec3::ZERO, 2.0, 8.0, 42);
    assert_eq!(wind.emitter_kind(), EmitterKind::Field);
    assert!(mean_square(&mut wind, 1.0) > 0.0);
}

#[test]
fn zero_speed_range_is_true_calm() {
    let mut wind = CanopyWindSource::new(Vec3::ZERO, 0.0, 0.0, 43);
    for _ in 0..48_000 {
        assert_eq!(wind.next_sample(48_000.0), 0.0);
    }
}

#[test]
fn graphics_tick_and_callback_chunking_do_not_change_audio() {
    fn render(chunk_size: usize) -> Vec<f32> {
        let sample_rate = 48_000.0;
        let mut wind = CanopyWindSource::new(Vec3::ZERO, 2.0, 8.0, 99);
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
fn stronger_wind_has_more_acoustic_energy() {
    let mut light = CanopyWindSource::new(Vec3::ZERO, 2.0, 2.0, 71);
    let mut brisk = CanopyWindSource::new(Vec3::ZERO, 8.0, 8.0, 71);
    light.branch_level = 0.0;
    brisk.branch_level = 0.0;
    let light_energy = mean_square(&mut light, 2.0);
    let brisk_energy = mean_square(&mut brisk, 2.0);
    assert!(
        brisk_energy > light_energy * 4.0,
        "8 m/s should be clearly stronger: {brisk_energy} vs {light_energy}"
    );
}

#[test]
fn dry_leaves_are_spectrally_brighter() {
    fn high_frequency_fraction(dryness: f32) -> f32 {
        let sample_rate = 48_000.0;
        let mut wind = CanopyWindSource::new(Vec3::ZERO, 8.0, 8.0, 83);
        wind.leaf_dryness = dryness;
        wind.branch_level = 0.0;
        let mut highpass = OnePoleHP::new(2_500.0, sample_rate);
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

    let green = high_frequency_fraction(0.0);
    let dry = high_frequency_fraction(1.0);
    assert!(
        dry > green * 1.10,
        "dry foliage should add high-frequency energy: {dry} vs {green}"
    );
}

use crate::synth::test_support::{
    averaged_power_spectrum, band_density_db, mean, peak, render, root_mean_square,
};

/// Leaf contacts started over `seconds` (a contact begins at
/// `contact_ring_index + 1`; slot `+ 2` changing during a call marks one).
fn count_contacts(wind: &mut CanopyWindSource, seconds: f32, sample_rate: f32) -> usize {
    let mut count = 0;
    for _ in 0..(seconds * sample_rate) as usize {
        let watched = (wind.contact_ring_index + 2) & CONTACT_RING_MASK;
        let before = wind.contact_ring[watched];
        wind.next_sample(sample_rate);
        if wind.contact_ring[watched] != before {
            count += 1;
        }
    }
    count
}

/// contact rate = foliage x min(2 + 180 s^1.7, 900) per second with s =
/// speed / 8 clamped to 3.125: 14.3/s at 2 m/s, 136.5/s at 8 m/s, and the
/// 900/s ceiling (675/s at foliage 0.75) from about 17 m/s.
#[test]
fn leaf_contact_rate_follows_the_speed_law() {
    for (speed, expected_rate) in [(2.0_f32, 14.3), (8.0, 136.5), (25.0, 675.0)] {
        let mut wind = CanopyWindSource::new(Vec3::ZERO, speed, speed, 4);
        wind.branch_level = 0.0;
        let seconds = 20.0;
        let expected = expected_rate * seconds;
        let count = count_contacts(&mut wind, seconds, 16_000.0) as f32;
        assert!(
            (count - expected).abs() < 4.0 * expected.sqrt() + 5.0,
            "{speed} m/s: {count} contacts in {seconds} s, expected {expected:.0}"
        );
    }
}

#[test]
fn leaf_contact_rate_is_independent_of_sample_rate_and_scales_with_foliage() {
    let expected = 136.5 * 4.0;
    for sample_rate in [16_000.0_f32, 44_100.0, 96_000.0] {
        let mut wind = CanopyWindSource::new(Vec3::ZERO, 8.0, 8.0, 5);
        wind.branch_level = 0.0;
        let count = count_contacts(&mut wind, 4.0, sample_rate) as f32;
        assert!(
            (count - expected).abs() < 4.0 * expected.sqrt() + 5.0,
            "{sample_rate} Hz: {count} contacts, expected {expected}"
        );
    }
    let mut bare = CanopyWindSource::new(Vec3::ZERO, 8.0, 8.0, 5);
    bare.foliage_density = 0.0;
    bare.branch_level = 0.0;
    assert_eq!(count_contacts(&mut bare, 4.0, 16_000.0), 0);
}

/// Run `next_branch` alone at 1 kHz: count events and their lengths.
fn run_branch(
    wind: &mut CanopyWindSource,
    speed: f32,
    seconds: f32,
    sample_rate: f32,
) -> (usize, Vec<usize>, Vec<f32>) {
    let (mut events, mut lengths, mut decays) = (0, Vec::new(), Vec::new());
    let mut running = 0usize;
    for _ in 0..(seconds * sample_rate) as usize {
        let was_active = wind.branch_envelope > 0.0;
        wind.next_branch(speed, sample_rate);
        let is_active = wind.branch_envelope > 0.0;
        if !was_active && is_active {
            events += 1;
            running = 0;
            decays.push(wind.branch_decay);
            assert!((75.0..=230.0).contains(&wind.branch_frequency));
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

/// Branch creaks fall 60 dB over their 0.25-1.15 s duration, like the
/// storm's strain events.
#[test]
fn branch_creaks_decay_by_60_db_over_their_duration() {
    let sample_rate = 1_000.0;
    let mut wind = CanopyWindSource::new(Vec3::ZERO, 12.0, 12.0, 9);
    wind.branch_level = 1.0;
    let (events, lengths, decays) = run_branch(&mut wind, 12.0, 400.0, sample_rate);
    assert!(events > 100 && lengths.len() >= events - 1);
    for (length, decay) in lengths.iter().zip(&decays) {
        let duration = -6.9 / (decay.ln() * sample_rate);
        assert!((0.25..=1.15).contains(&duration), "duration {duration}");
        let expected = duration * sample_rate * (6.9078 / 6.9);
        assert!(
            (*length as f32 - expected).abs() <= 2.0,
            "{length} vs {expected}"
        );
    }
}

/// Above 3 m/s only; rate = level (0.02 + 1.2 bend^2) with bend =
/// (speed - 3) / 9, then the renewal correction for the 0.7 s mean duration.
#[test]
fn branch_creak_rate_follows_the_renewal_law_and_speed_gate() {
    let sample_rate = 1_000.0;
    let seconds = 1_500.0;
    let mut wind = CanopyWindSource::new(Vec3::ZERO, 12.0, 12.0, 10);
    wind.branch_level = 1.0;
    let (events, _, _) = run_branch(&mut wind, 12.0, seconds, sample_rate);
    let expected = seconds / (1.0 / 1.22 + 0.70 * 1.0011);
    assert!(
        (events as f32 - expected).abs() < 5.0 * expected.sqrt(),
        "{events} creaks in {seconds} s, expected {expected:.0}"
    );
    let mut calm = CanopyWindSource::new(Vec3::ZERO, 3.0, 3.0, 10);
    calm.branch_level = 1.0;
    assert_eq!(run_branch(&mut calm, 3.0, 500.0, sample_rate).0, 0);
    let mut off = CanopyWindSource::new(Vec3::ZERO, 12.0, 12.0, 10);
    off.branch_level = 0.0;
    assert_eq!(run_branch(&mut off, 12.0, 500.0, sample_rate).0, 0);
}

/// Gain law (speed / 8)^1.25: 6 m/s against 3 m/s is a gain ratio of
/// 2^1.25 = 2.4, plus the speed-dependent contact density.
#[test]
fn level_follows_the_speed_law() {
    let level = |speed: f32| {
        let mut wind = CanopyWindSource::new(Vec3::ZERO, speed, speed, 71);
        wind.branch_level = 0.0;
        root_mean_square(&render(12.0, 16_000.0, |rate| wind.next_sample(rate)))
    };
    let ratio = level(6.0) / level(3.0);
    assert!((2.3..9.0).contains(&ratio), "RMS ratio {ratio}");
    assert!(level(1.0) < level(3.0) && level(3.0) < level(12.0));
}

/// Dry leaves: contact filters open up (HP 550 -> 1600 Hz, LP 2.8 -> 8.6
/// kHz) and the dry layer is added.
#[test]
fn leaf_dryness_brightens_the_contact_grains() {
    let brightness = |dryness: f32| {
        let mut wind = CanopyWindSource::new(Vec3::ZERO, 12.0, 12.0, 6);
        wind.leaf_dryness = dryness;
        wind.branch_level = 0.0;
        let samples = render(10.0, 48_000.0, |rate| wind.next_sample(rate));
        let spectrum = averaged_power_spectrum(&samples);
        band_density_db(&spectrum, 48_000.0, 5_000.0, 9_000.0)
            - band_density_db(&spectrum, 48_000.0, 300.0, 700.0)
    };
    assert!(brightness(1.0) > brightness(0.0) + 3.0);
}

#[test]
fn output_is_finite_zero_mean_and_inside_unity_at_default_speeds() {
    let mut wind = CanopyWindSource::new(Vec3::ZERO, 2.0, 8.0, 7);
    let samples = render(20.0, 48_000.0, |rate| wind.next_sample(rate));
    assert!(samples.iter().all(|value| value.is_finite()));
    assert!(peak(&samples) < 1.0, "peak {}", peak(&samples));
    assert!(mean(&samples).abs() < 0.002, "DC {}", mean(&samples));
}

/// At the 25 m/s extreme the clamps (speed_at_canopy 3.125, gain cap 4.2)
/// keep the raw output finite and under 1.5.
#[test]
fn storm_speed_output_stays_bounded() {
    let mut wind = CanopyWindSource::new(Vec3::ZERO, 25.0, 25.0, 7);
    let samples = render(10.0, 48_000.0, |rate| wind.next_sample(rate));
    assert!(samples.iter().all(|value| value.is_finite()));
    assert!(peak(&samples) < 1.5, "peak {}", peak(&samples));
    assert!(3.125_f32.powf(1.25) < 4.2);
}

#[test]
fn body_density_is_sample_rate_independent() {
    let density = |sample_rate: f32| {
        let mut wind = CanopyWindSource::new(Vec3::ZERO, 8.0, 8.0, 73);
        wind.branch_level = 0.0;
        let samples = render(10.0, sample_rate, |rate| wind.next_sample(rate));
        band_density_db(
            &averaged_power_spectrum(&samples),
            sample_rate,
            200.0,
            300.0,
        )
    };
    let reference = density(48_000.0);
    for sample_rate in [44_100.0, 96_000.0] {
        assert!(
            (density(sample_rate) - reference).abs() < 1.0,
            "{sample_rate} Hz"
        );
    }
}

#[test]
fn parameters_are_clamped_and_routed() {
    let mut wind = CanopyWindSource::new(Vec3::ZERO, 30.0, 40.0, 1);
    assert_eq!((wind.min_speed, wind.max_speed), (25.0, 25.0));
    wind.set_speed_range(10.0, 2.0);
    assert_eq!((wind.min_speed, wind.max_speed), (2.0, 10.0));
    wind.set_synth_param(SynthParam::FoliageDensity, 3.0);
    assert_eq!(wind.foliage_density, 1.0);
    wind.set_synth_param(SynthParam::LeafDryness, -3.0);
    assert_eq!(wind.leaf_dryness, 0.0);
    wind.set_synth_param(SynthParam::BranchLevel, 0.5);
    assert_eq!(wind.branch_level, 0.5);
    wind.set_synth_param(SynthParam::LowGain, 0.1);
    assert_eq!(wind.body_gain, 0.1);
    wind.set_synth_param(SynthParam::MidGain, 0.2);
    assert_eq!(wind.rustle_gain, 0.2);
    wind.set_synth_param(SynthParam::AirGain, 0.3);
    assert_eq!(wind.contact_gain, 0.3);
}
