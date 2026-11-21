use super::*;

#[test]
fn v2_produces_nonzero_output() {
    let mut rain = RainSourceV2::new(Vec3::ZERO, 0.5, 42);
    let sum: f32 = (0..48000).map(|_| rain.next_sample(48000.0).abs()).sum();
    assert!(sum > 0.0, "rain v2 is silent");
}

#[test]
fn v2_silent_at_zero_intensity() {
    let mut rain = RainSourceV2::new(Vec3::ZERO, 0.0, 42);
    let sum: f32 = (0..48000).map(|_| rain.next_sample(48000.0).abs()).sum();
    assert_eq!(sum, 0.0, "rain v2 should be silent at zero intensity");
}

#[test]
fn v2_louder_at_higher_intensity() {
    let mut light = RainSourceV2::new(Vec3::ZERO, 0.2, 42);
    let mut heavy = RainSourceV2::new(Vec3::ZERO, 0.9, 42);

    let energy_light: f32 = (0..48000).map(|_| light.next_sample(48000.0).powi(2)).sum();
    let energy_heavy: f32 = (0..48000).map(|_| heavy.next_sample(48000.0).powi(2)).sum();

    assert!(
        energy_heavy > energy_light,
        "heavy rain ({energy_heavy}) should be louder than light ({energy_light})"
    );
}

#[test]
fn v2_bed_is_quieter_than_drops() {
    // Continuous layers alone should be significantly quieter than full mix.
    // Drops are silenced via impact_gain/bubble_gain so the event stream
    // (and with it every rng draw) is the same in both renders.
    let mut bed_only = RainSourceV2::new(Vec3::ZERO, 0.5, 42);
    bed_only.impact_gain = 0.0;
    bed_only.bubble_gain = 0.0;
    bed_only.texture_gain = 0.0;

    let mut full = RainSourceV2::new(Vec3::ZERO, 0.5, 42);

    let energy_bed: f32 = (0..48000)
        .map(|_| bed_only.next_sample(48000.0).powi(2))
        .sum();
    let energy_full: f32 = (0..48000).map(|_| full.next_sample(48000.0).powi(2)).sum();

    assert!(
        energy_full > energy_bed * 1.2,
        "drops should add significant energy over bed alone (bed={energy_bed}, full={energy_full})"
    );
}

use crate::synth::test_support::{
    averaged_power_spectrum, band_power, mean, peak, render, root_mean_square,
};

/// Sample indices at which a drop or drip started. Every event writes its
/// impact starting at `ring[ring_idx + 1]` with a 0.8 ms attack ramp, so
/// burst sample 1 (slot `ring_idx + 2`) is non-zero; that slot changing
/// during a call marks an event. It is first read two calls later.
fn event_count(source: &mut RainSourceV2, seconds: f32, sample_rate: f32) -> usize {
    let mut count = 0;
    for _ in 0..(seconds * sample_rate) as usize {
        let watched = (source.ring_idx + 2) & RING_MASK;
        let before = source.ring[watched];
        source.next_sample(sample_rate);
        if source.ring[watched] != before {
            count += 1;
        }
    }
    count
}

fn drops_only(intensity: f32, seed: u64) -> RainSourceV2 {
    let mut rain = RainSourceV2::new(Vec3::ZERO, intensity, seed);
    rain.drip_rate = 0.0;
    rain
}

/// Drop-rate curve: 15/s at intensity 0.2, 50/s at 0.5, 75/s at 0.9 and
/// above, linear in between and from zero. Counts are Poisson, so allow
/// four standard deviations.
#[test]
fn drop_rate_follows_the_piecewise_intensity_curve() {
    for (intensity, expected_rate) in [
        (0.1, 7.5),
        (0.2, 15.0),
        (0.35, 32.5),
        (0.5, 50.0),
        (0.7, 62.5),
        (0.9, 75.0),
        (1.0, 75.0),
    ] {
        let seconds = 40.0;
        let expected = expected_rate * seconds;
        let count = event_count(&mut drops_only(intensity, 31), seconds, 16_000.0) as f32;
        assert!(
            (count - expected).abs() < 4.0 * expected.sqrt(),
            "intensity {intensity}: {count} drops in {seconds} s, expected {expected}"
        );
    }
}

/// The default `drop_rate` (75/s) times the intensity fraction is the
/// tuned curve exactly: 15/s at 0.2, 50/s at 0.5, 75/s from 0.9 up.
#[test]
fn default_drop_rate_reproduces_the_tuned_curve() {
    let default_rate = RainSourceV2::new(Vec3::ZERO, 0.5, 1).drop_rate;
    for (intensity, expected_rate) in [
        (0.0, 0.0),
        (0.1, 7.5),
        (0.2, 15.0),
        (0.35, 32.5),
        (0.5, 50.0),
        (0.7, 62.5),
        (0.9, 75.0),
        (1.0, 75.0),
    ] {
        let rate = default_rate * drop_rate_fraction(intensity);
        assert!(
            (rate - expected_rate).abs() < 1e-4,
            "intensity {intensity}: {rate} drops/s, expected {expected_rate}"
        );
    }
}

#[test]
fn drop_rate_is_the_same_at_every_sample_rate() {
    for sample_rate in [16_000.0_f32, 44_100.0, 96_000.0] {
        let seconds = if sample_rate > 40_000.0 { 8.0 } else { 20.0 };
        let expected = 50.0 * seconds;
        let count = event_count(&mut drops_only(0.5, 32), seconds, sample_rate) as f32;
        assert!(
            (count - expected).abs() < 4.0 * expected.sqrt(),
            "{sample_rate} Hz: {count} drops in {seconds} s, expected {expected}"
        );
    }
}

/// Drips are an independent Poisson stream at `drip_rate * drop_rate / 75`.
#[test]
fn drip_rate_adds_its_own_poisson_stream() {
    let seconds = 30.0;
    let mut dripping = RainSourceV2::new(Vec3::ZERO, 0.9, 33);
    dripping.drip_rate = 40.0;
    let total = event_count(&mut dripping, seconds, 16_000.0) as f32;
    let expected = (75.0 + 40.0) * seconds;
    assert!(
        (total - expected).abs() < 4.0 * expected.sqrt(),
        "{total} events in {seconds} s, expected {expected}"
    );
}

/// `drop_rate` is the rate at full intensity and must reach the DSP: it
/// used to be ignored in favour of a fixed curve, so `drop_rate` = 10 and
/// 600 produced identical event streams (793 events in 10 s either way).
#[test]
fn drop_rate_knob_changes_the_event_rate() {
    let mut sparse = drops_only(0.9, 34);
    sparse.drop_rate = 10.0;
    let mut dense = drops_only(0.9, 34);
    dense.drop_rate = 600.0;
    let sparse_count = event_count(&mut sparse, 10.0, 16_000.0);
    let dense_count = event_count(&mut dense, 10.0, 16_000.0);
    assert!(
        dense_count > sparse_count * 5,
        "{sparse_count} vs {dense_count}"
    );
}

/// Per size class: bubble probability and frequency / decay ranges.
/// Small (<1.2 mm) 0.5 p, medium 1.0 p, large 1.2 p with p = 0.12 / 0.35 /
/// 0.20 for drizzle / moderate / heavy.
#[test]
fn bubble_parameters_follow_the_size_class_tables() {
    let draws = 30_000;
    for (intensity, probability) in [(0.2, 0.12), (0.5, 0.35), (0.9, 0.20)] {
        for (radius, scale, freq_range, decay_range) in [
            (1.0_f32, 0.5, (500.0, 2_500.0), (600.0, 1_000.0)),
            (1.6, 1.0, (400.0, 2_000.0), (450.0, 750.0)),
            (3.0, 1.2, (250.0, 1_200.0), (300.0, 500.0)),
        ] {
            let mut rain = RainSourceV2::new(Vec3::ZERO, intensity, 77);
            let mut present = 0;
            for _ in 0..draws {
                if let Some((frequency, decay)) = rain.bubble_params(radius) {
                    present += 1;
                    assert!(
                        (freq_range.0..=freq_range.1).contains(&frequency),
                        "radius {radius}: {frequency} Hz outside {freq_range:?}"
                    );
                    assert!(
                        (decay_range.0..decay_range.1).contains(&decay),
                        "radius {radius}: decay {decay} outside {decay_range:?}"
                    );
                }
            }
            let measured = present as f32 / draws as f32;
            let expected = probability * scale;
            assert!(
                (measured - expected).abs() < 0.012,
                "intensity {intensity}, radius {radius}: bubble share {measured}, expected {expected}"
            );
        }
    }
}

/// Bigger drops ring lower: the mean bubble frequency falls with radius
/// (the physical Minnaert direction, f ~ 1/radius), though the values sit
/// well below Minnaert's 3.26/r kHz*mm (see module header: shifted down
/// for warmth).
#[test]
fn bubble_frequency_falls_as_drops_get_bigger() {
    let mean_frequency = |radius: f32| {
        let mut rain = RainSourceV2::new(Vec3::ZERO, 0.5, 5);
        let (mut sum, mut count) = (0.0, 0);
        for _ in 0..20_000 {
            if let Some((frequency, _)) = rain.bubble_params(radius) {
                sum += frequency;
                count += 1;
            }
        }
        sum / count as f32
    };
    let (small, medium, large) = (
        mean_frequency(1.0),
        mean_frequency(1.6),
        mean_frequency(3.0),
    );
    assert!(small > medium && medium > large, "{small} {medium} {large}");
}

/// Reference: Minnaert resonance f = (1 / 2 pi r) sqrt(3 gamma P0 / rho)
/// = 3.26 kHz*mm / r for an air bubble in water. The river's comment that
/// 1.2-7 mm bubbles give 450-2700 Hz follows from it.
#[test]
fn minnaert_constant_is_3_26_hz_metres() {
    let gamma = 1.4_f64;
    let pressure = 101_325.0_f64;
    let density = 998.0_f64;
    let constant = (3.0 * gamma * pressure / density).sqrt() / std::f64::consts::TAU;
    assert!((constant - 3.26).abs() < 0.03, "{constant}");
    let frequency = |radius_mm: f64| constant / (radius_mm * 1e-3);
    assert!((frequency(1.2) - 2_717.0).abs() < 30.0);
    assert!((frequency(7.0) - 466.0).abs() < 6.0);
}

/// A written bubble is a decaying sine: its cycle count gives the
/// frequency, its envelope falls by 1/e per 1/decay seconds, it lasts
/// four time constants, and its gain is scaled by 1 - f / 20 kHz.
#[test]
fn written_bubble_has_the_requested_frequency_decay_and_duration() {
    let sample_rate = 48_000.0;
    let (frequency, decay) = (1_000.0_f32, 500.0_f32);
    let mut rain = RainSourceV2::new(Vec3::ZERO, 0.5, 1);
    rain.write_bubble(frequency, decay, 1.0, sample_rate);
    let start = 1;
    let length = (4.0 / decay * sample_rate) as usize; // 384 samples
    let bubble: Vec<f32> = (0..length + 8)
        .map(|offset| rain.ring[start + offset])
        .collect();
    assert!(
        bubble[length..].iter().all(|&value| value == 0.0),
        "bubble longer than 4/decay"
    );
    let cycles = bubble[..length]
        .windows(2)
        .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
        .count();
    let expected_cycles = frequency * 4.0 / decay; // 8
    assert!(
        (cycles as f32 - expected_cycles).abs() <= 1.0,
        "{cycles} cycles"
    );
    // Envelope: peak within one cycle (48 samples) at t0 vs at t = 1/decay.
    let window_peak = |from: usize| {
        bubble[from..from + 48]
            .iter()
            .fold(0.0_f32, |a, v| a.max(v.abs()))
    };
    let time_constant = (sample_rate / decay) as usize; // 96 samples
    let ratio = window_peak(time_constant) / window_peak(0);
    assert!(
        (ratio - (-1.0_f32).exp()).abs() < 0.12,
        "envelope ratio {ratio}"
    );
    // Gain scaling: at most 1 - 1000/20000 = 0.95 of the unit gain; the
    // random start phase and the 0.3 ms attack ramp take some of it off.
    let largest = peak(&bubble);
    assert!((0.5..=0.951).contains(&largest), "peak {largest}");
}

/// Impact bursts last 3 ms + 1.7 ms per mm of radius (radius capped at
/// 3 mm) and decay with tau = 0.4 of that duration.
#[test]
fn impact_duration_grows_with_drop_radius() {
    let sample_rate = 48_000.0;
    for (radius, expected_length) in [(1.0_f32, 225usize), (2.0, 306), (3.0, 388), (3.5, 388)] {
        let mut rain = RainSourceV2::new(Vec3::ZERO, 0.5, 2);
        rain.write_impact(radius, 1.0, sample_rate);
        let last = (1..rain.ring.len() / 2)
            .rev()
            .find(|&index| rain.ring[index] != 0.0)
            .unwrap();
        assert!(
            (last as i32 - expected_length as i32).abs() <= 4,
            "radius {radius}: last non-zero slot {last}, expected {expected_length}"
        );
    }
}

/// "light rain is brighter, heavy rain darker": the impact lowpass moves
/// from ~4.7 kHz at intensity 0.2 to ~1.7 kHz at 0.9. The share of power
/// above 1.5 kHz must therefore fall as intensity rises.
#[test]
fn impact_spectrum_darkens_as_intensity_rises() {
    let sample_rate = 48_000.0;
    let bright_share = |intensity: f32| -> f64 {
        let mut rain = RainSourceV2::new(Vec3::ZERO, intensity, 3);
        let mut stream = Vec::new();
        while stream.len() < 16_384 {
            rain.ring.fill(0.0);
            rain.write_impact(1.5, 1.0, sample_rate);
            stream.extend((1..400).map(|index| rain.ring[index]));
        }
        let spectrum = averaged_power_spectrum(&stream);
        band_power(&spectrum, sample_rate, 1_500.0, 12_000.0)
            / band_power(&spectrum, sample_rate, 150.0, 12_000.0)
    };
    let light = bright_share(0.2);
    let moderate = bright_share(0.5);
    let heavy = bright_share(0.9);
    assert!(
        light > moderate && moderate > heavy,
        "{light:.3} {moderate:.3} {heavy:.3}"
    );
}

#[test]
fn output_is_finite_zero_mean_and_inside_unity() {
    for intensity in [0.2, 0.5, 1.0] {
        let mut rain = RainSourceV2::new(Vec3::ZERO, intensity, 5);
        let samples = render(15.0, 48_000.0, |rate| rain.next_sample(rate));
        assert!(samples.iter().all(|value| value.is_finite()));
        assert!(
            peak(&samples) < 1.0,
            "intensity {intensity}: peak {}",
            peak(&samples)
        );
        assert!(mean(&samples).abs() < 0.01, "DC {}", mean(&samples));
    }
}

#[test]
fn rms_is_roughly_stable_across_sample_rates() {
    let rms = |sample_rate: f32| {
        let mut rain = RainSourceV2::new(Vec3::ZERO, 0.5, 42);
        let samples = render(8.0, sample_rate, |rate| rain.next_sample(rate));
        root_mean_square(&samples)
    };
    // Measured 1.07 (+0.6 dB, it read 0.84 before the white-noise bursts
    // kept their density per Hz); the residual comes from the
    // backward-Euler one-pole lowpasses, whose corner rises with the rate.
    let ratio = rms(96_000.0) / rms(48_000.0);
    assert!((0.70..1.30).contains(&ratio), "RMS ratio {ratio}");
}

/// With only the continuous layers left, a blank intensity still leaves
/// exact silence, and raising it raises RMS monotonically at 0.2/0.5/0.9.
#[test]
fn level_rises_with_intensity_up_to_the_rate_plateau() {
    let level = |intensity: f32| {
        let mut rain = RainSourceV2::new(Vec3::ZERO, intensity, 6);
        let samples = render(6.0, 16_000.0, |rate| rain.next_sample(rate));
        root_mean_square(&samples)
    };
    assert!(level(0.2) < level(0.5));
    assert!(level(0.5) > 0.0 && level(0.0) == 0.0);
}

/// What changes while it plays reaches the sound: heavier rain is
/// louder, and master gain 0 silences it.
#[test]
fn its_intensity_and_gain_change_while_it_plays() {
    use atrium_core::commands::SynthParam;
    use atrium_core::source::SoundSource;
    let energy = |intensity: f32, gain: f32| {
        let mut rain = RainSourceV2::new(Vec3::ZERO, 0.5, 3);
        rain.set_synth_param(SynthParam::Intensity, intensity);
        rain.set_synth_param(SynthParam::MasterGain, gain);
        (0..96_000)
            .map(|_| rain.next_sample(48_000.0).powi(2))
            .sum::<f32>()
    };
    assert!(energy(1.0, 1.0) > energy(0.1, 1.0) * 2.0);
    assert_eq!(energy(1.0, 0.0), 0.0);
}
