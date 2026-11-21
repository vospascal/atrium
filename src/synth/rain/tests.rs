use super::*;

#[test]
fn rain_produces_nonzero_output() {
    let mut rain = RainSource::new(Vec3::ZERO, 0.5, 42);
    let sum: f32 = (0..48000).map(|_| rain.next_sample(48000.0).abs()).sum();
    assert!(sum > 0.0, "rain source is silent");
}

#[test]
fn rain_silent_at_zero_intensity() {
    let mut rain = RainSource::new(Vec3::ZERO, 0.0, 42);
    let sum: f32 = (0..48000).map(|_| rain.next_sample(48000.0).abs()).sum();
    assert_eq!(sum, 0.0, "rain should be silent at zero intensity");
}

#[test]
fn rain_louder_at_higher_intensity() {
    let mut light = RainSource::new(Vec3::ZERO, 0.2, 42);
    let mut heavy = RainSource::new(Vec3::ZERO, 0.9, 42);

    let energy_light: f32 = (0..48000).map(|_| light.next_sample(48000.0).powi(2)).sum();
    let energy_heavy: f32 = (0..48000).map(|_| heavy.next_sample(48000.0).powi(2)).sum();

    assert!(
        energy_heavy > energy_light,
        "heavy rain ({energy_heavy}) should be louder than light ({energy_light})"
    );
}

use crate::synth::test_support::{
    averaged_power_spectrum, band_density_db, mean, peak, render, root_mean_square,
};

/// Sample indices at which a drop or drip started. A new event writes
/// burst sample 0 (Gaussian window exp(-2) times filtered noise, never
/// exactly zero) into `ring[ring_idx + 1]`, so that slot changing during a
/// call marks an event. The slot is read only on the following call, so
/// nothing else touches it.
fn event_times(source: &mut RainSource, seconds: f32, sample_rate: f32) -> Vec<usize> {
    let mut times = Vec::new();
    for index in 0..(seconds * sample_rate) as usize {
        let watched = (source.ring_idx + 1) & RING_MASK;
        let before = source.ring[watched];
        source.next_sample(sample_rate);
        if source.ring[watched] != before {
            times.push(index);
        }
    }
    times
}

fn drops_only(intensity: f32, seed: u64) -> RainSource {
    let mut rain = RainSource::new(Vec3::ZERO, intensity, seed);
    rain.drip_rate = 0.0;
    rain
}

/// Poisson arrivals: p = rate / sample_rate per sample gives a mean of
/// `rate` per second. 150 drops/s at full intensity over 10 s is 1500
/// +- 39 (one sigma).
#[test]
fn drop_rate_matches_the_configured_poisson_rate() {
    let count = event_times(&mut drops_only(1.0, 42), 10.0, 48_000.0).len() as f32;
    assert!(
        (count - 1_500.0).abs() < 4.0 * 1_500.0_f32.sqrt(),
        "{count} drops in 10 s"
    );
    // Intensity scales the rate linearly: 0.4 -> 60 per second.
    let count = event_times(&mut drops_only(0.4, 43), 20.0, 48_000.0).len() as f32;
    assert!(
        (count - 1_200.0).abs() < 4.0 * 1_200.0_f32.sqrt(),
        "{count} drops in 20 s"
    );
}

/// The public `drop_rate` knob really sets the rate.
#[test]
fn drop_rate_knob_scales_the_event_rate() {
    let mut sparse = drops_only(1.0, 44);
    sparse.drop_rate = 30.0;
    let count = event_times(&mut sparse, 20.0, 48_000.0).len() as f32;
    assert!(
        (count - 600.0).abs() < 4.0 * 600.0_f32.sqrt(),
        "{count} drops in 20 s"
    );
}

#[test]
fn drip_rate_matches_the_configured_poisson_rate() {
    let mut rain = RainSource::new(Vec3::ZERO, 1.0, 45);
    rain.drop_rate = 0.0;
    let count = event_times(&mut rain, 30.0, 48_000.0).len() as f32;
    assert!(
        (count - 240.0).abs() < 4.0 * 240.0_f32.sqrt(),
        "{count} drips in 30 s"
    );
}

/// Per-sample probability = rate / sample_rate makes the event rate a
/// property of seconds, not of samples.
#[test]
fn drop_rate_is_the_same_at_every_sample_rate() {
    for sample_rate in [44_100.0_f32, 96_000.0] {
        let count = event_times(&mut drops_only(1.0, 46), 8.0, sample_rate).len() as f32;
        assert!(
            (count - 1_200.0).abs() < 4.0 * 1_200.0_f32.sqrt(),
            "{sample_rate} Hz: {count} drops in 8 s"
        );
    }
}

/// Exponential inter-arrival times have mean 1/rate and a coefficient of
/// variation of 1 (a regular pattern would give 0, clumping above 1).
#[test]
fn drop_arrivals_are_exponentially_spaced() {
    let times = event_times(&mut drops_only(1.0, 47), 20.0, 48_000.0);
    let gaps: Vec<f32> = times
        .windows(2)
        .map(|pair| (pair[1] - pair[0]) as f32 / 48_000.0)
        .collect();
    let gap_mean = mean(&gaps) as f32;
    let gap_deviation = crate::synth::test_support::variance(&gaps).sqrt() as f32;
    assert!(
        (gap_mean - 1.0 / 150.0).abs() < 0.0004,
        "mean gap {gap_mean}"
    );
    assert!(
        (gap_deviation / gap_mean - 1.0).abs() < 0.10,
        "CV {}",
        gap_deviation / gap_mean
    );
}

/// Drop radius mixture per intensity class (mm): drizzle 80 % small, heavy
/// rain skewed to large drops.
#[test]
fn drop_radius_mixture_follows_the_intensity_class() {
    let draws = 40_000;
    let share = |intensity: f32, low: f32, high: f32| -> f32 {
        let mut rain = RainSource::new(Vec3::ZERO, intensity, 8);
        (0..draws)
            .filter(|_| (low..high).contains(&rain.pick_drop_radius()))
            .count() as f32
            / draws as f32
    };
    // Drizzle: 80 % in [0.95, 1.2), 20 % in [1.4, 2.0).
    assert!((share(0.1, 0.95, 1.2) - 0.80).abs() < 0.01);
    assert!((share(0.1, 1.4, 2.0) - 0.20).abs() < 0.01);
    // Moderate: 50 % [1.0, 1.4), 35 % [1.6, 2.5), 15 % [2.4, 3.6).
    assert!((share(0.5, 1.0, 1.4) - 0.50).abs() < 0.015);
    assert!((share(0.5, 1.6, 2.4) - 0.35 * 0.8 / 0.9).abs() < 0.015);
    assert!((share(0.5, 2.5, 3.6) - 0.15 * 1.1 / 1.2).abs() < 0.01);
    // Heavy: 30 % [1.2, 1.8), 70 % [2.0, 3.5).
    assert!((share(0.9, 1.2, 1.8) - 0.30).abs() < 0.015);
    assert!((share(0.9, 2.0, 3.5) - 0.70).abs() < 0.015);
    // Never outside the documented 0.95-3.6 mm span.
    for intensity in [0.1, 0.5, 0.9] {
        let mut rain = RainSource::new(Vec3::ZERO, intensity, 9);
        for _ in 0..5_000 {
            let radius = rain.pick_drop_radius();
            assert!((0.95..3.6).contains(&radius), "radius {radius}");
        }
    }
}

/// Mean drop size grows with intensity.
#[test]
fn mean_drop_radius_increases_with_intensity() {
    let mean_radius = |intensity: f32| {
        let mut rain = RainSource::new(Vec3::ZERO, intensity, 10);
        (0..20_000).map(|_| rain.pick_drop_radius()).sum::<f32>() / 20_000.0
    };
    let (drizzle, moderate, heavy) = (mean_radius(0.1), mean_radius(0.5), mean_radius(0.9));
    assert!(
        drizzle < moderate && moderate < heavy,
        "{drizzle} {moderate} {heavy}"
    );
}

/// The bubble is a 1.6 kHz sine with a 3 ms time constant, 15 ms long
/// (the module header still says "~3.3 kHz").
#[test]
fn bubble_is_a_1600_hz_sine_with_a_three_millisecond_decay() {
    let mut rain = RainSource::new(Vec3::ZERO, 0.5, 1);
    let sample_rate = 48_000.0;
    let length = rain.generate_bubble(sample_rate);
    assert_eq!(length, 720);
    let bubble = &rain.burst_scratch[..length];
    let cycles = bubble
        .windows(2)
        .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
        .count();
    assert!((cycles as i32 - 24).abs() <= 1, "{cycles} cycles in 15 ms");
    // Envelope falls by 1/e per time constant (144 samples).
    let envelope_peak = |start: usize| {
        bubble[start..start + 30]
            .iter()
            .fold(0.0_f32, |a, v| a.max(v.abs()))
    };
    let ratio = envelope_peak(144) / envelope_peak(0);
    assert!(
        (ratio - (-1.0_f32).exp()).abs() < 0.07,
        "decay ratio {ratio}"
    );
}

/// Burst length is 25 ms per mm of radius, capped by the scratch buffer.
#[test]
fn burst_length_scales_with_radius_and_is_clamped() {
    let mut rain = RainSource::new(Vec3::ZERO, 0.5, 1);
    assert_eq!(rain.generate_burst(0.025 * 1.0, 600.0, 48_000.0), 1_200);
    assert_eq!(rain.generate_burst(0.025 * 3.0, 346.0, 48_000.0), 3_600);
    assert_eq!(rain.generate_burst(0.0, 600.0, 48_000.0), 8);
    assert_eq!(rain.generate_burst(10.0, 600.0, 48_000.0), MAX_BURST);
}

/// Gaussian window, sigma = len / 4, centred: the burst's mean-square at
/// the centre is (1 / exp(-2))^2 = 55 times that at the edges.
#[test]
fn burst_energy_follows_the_gaussian_window() {
    let mut rain = RainSource::new(Vec3::ZERO, 0.5, 2);
    let length = 1_200;
    let mut centre = 0.0_f64;
    let mut edge = 0.0_f64;
    let bursts = 400;
    for _ in 0..bursts {
        let written = rain.generate_burst(0.025, 600.0, 48_000.0);
        assert_eq!(written, length);
        let middle = length / 2;
        centre += rain.burst_scratch[middle - 20..middle + 20]
            .iter()
            .map(|&v| (v as f64).powi(2))
            .sum::<f64>();
        edge += rain.burst_scratch[10..50]
            .iter()
            .map(|&v| (v as f64).powi(2))
            .sum::<f64>();
    }
    // Window value at i = 10..50 averages ~0.15 of the peak: expect ~44x.
    let ratio = centre / edge;
    assert!(
        (30.0..70.0).contains(&ratio),
        "centre/edge energy {ratio:.1}"
    );
}

/// The ring is read at `ring_idx` and then advanced, so the first burst
/// sample must land one slot ahead (as rain v2 and the river do with
/// `ring_idx + 1`). Landing on the slot just consumed, it would not be
/// heard until the ring wraps, 8192 samples (171 ms) later.
#[test]
fn first_burst_sample_is_not_delayed_by_a_ring_revolution() {
    let mut rain = RainSource::new(Vec3::ZERO, 1.0, 3);
    rain.drip_rate = 0.0;
    rain.drop_rate = 1.0e9; // probability >= 1: the first sample drops
    rain.next_sample(48_000.0);
    let consumed_slot = (rain.ring_idx + RING_MASK) & RING_MASK;
    assert_eq!(
        rain.ring[consumed_slot], 0.0,
        "stray burst sample parked in an already-read slot"
    );
}

/// Full-intensity rain has RMS 0.17 and impulsive bursts with a crest
/// factor near 6, so a 20 s render peaks between 0.83 and 1.04 depending
/// on the seed (seeds 0-39 measured; median 0.92). Unity is not a hard
/// limit for a raw generator (`SynthNode` rescales by the calibrated RMS),
/// so this guards against a level blow-up, not against a rare 1.04.
#[test]
fn output_peaks_near_unity_at_full_intensity() {
    let mut rain = RainSource::new(Vec3::ZERO, 1.0, 5);
    let samples = render(20.0, 48_000.0, |rate| rain.next_sample(rate));
    assert!(samples.iter().all(|value| value.is_finite()));
    assert!(peak(&samples) < 1.2, "peak {}", peak(&samples));
    assert!(mean(&samples).abs() < 0.02, "DC {}", mean(&samples));
}

#[test]
fn rms_is_stable_across_sample_rates() {
    let rms = |sample_rate: f32| {
        let mut rain = RainSource::new(Vec3::ZERO, 0.5, 42);
        let samples = render(8.0, sample_rate, |rate| rain.next_sample(rate));
        root_mean_square(&samples)
    };
    let ratio = rms(96_000.0) / rms(48_000.0);
    assert!((0.85..1.17).contains(&ratio), "RMS ratio {ratio}");
}

#[test]
fn master_gain_scales_output_linearly() {
    let render_with = |master_gain: f32| {
        let mut rain = RainSource::new(Vec3::ZERO, 0.6, 21);
        rain.master_gain = master_gain;
        render(1.0, 48_000.0, |rate| rain.next_sample(rate))
    };
    let unit = render_with(1.0);
    let half = render_with(0.5);
    for (full, halved) in unit.iter().zip(&half) {
        assert!((full * 0.5 - halved).abs() < 1e-6);
    }
}

/// Bubbles add a 1.6 kHz line on top of the bursts; with bubble_gain 0
/// the 1.4-1.8 kHz band is quieter.
#[test]
fn bubble_gain_zero_removes_the_bubble_energy() {
    let band = |bubble_gain: f32| {
        let mut rain = RainSource::new(Vec3::ZERO, 1.0, 12);
        rain.bubble_gain = bubble_gain;
        rain.hiss_gain = 0.0;
        rain.brown_gain = 0.0;
        let samples = render(8.0, 48_000.0, |rate| rain.next_sample(rate));
        band_density_db(
            &averaged_power_spectrum(&samples),
            48_000.0,
            1_400.0,
            1_800.0,
        )
    };
    assert!(band(1.0) > band(0.0) + 0.5);
}

/// Output must be exactly zero (or a normal float) once the rain stops:
/// a subnormal that never decays is expensive on x86 and leaks into the
/// pipeline. `env_state` decays by 0.7 per sample, and 0.7 x 1 ulp rounds
/// back to 1 ulp, so without the flush it sticks at 1.4e-45 forever.
#[test]
fn no_subnormal_output_after_the_rain_stops() {
    let mut rain = RainSource::new(Vec3::ZERO, 0.8, 4);
    for _ in 0..30_000 {
        rain.next_sample(48_000.0);
    }
    rain.intensity = 0.0;
    for _ in 0..40_000 {
        rain.next_sample(48_000.0);
    }
    let tail = rain.next_sample(48_000.0);
    assert!(
        tail == 0.0 || tail.is_normal(),
        "subnormal output {tail:e} (env_state {:e})",
        rain.env_state
    );
}

/// What changes while it plays reaches the sound: heavier rain is
/// louder, no rain is silent, and master gain 0 silences it.
#[test]
fn its_intensity_and_gain_change_while_it_plays() {
    use atrium_core::commands::SynthParam;
    use atrium_core::source::SoundSource;
    let energy = |intensity: f32, gain: f32| {
        let mut rain = RainSource::new(Vec3::ZERO, 0.5, 3);
        rain.set_synth_param(SynthParam::Intensity, intensity);
        rain.set_synth_param(SynthParam::MasterGain, gain);
        rain.set_synth_param(SynthParam::Intensity, f32::NAN);
        (0..96_000)
            .map(|_| rain.next_sample(48_000.0).powi(2))
            .sum::<f32>()
    };
    assert!(energy(1.0, 1.0) > energy(0.1, 1.0) * 2.0);
    assert_eq!(energy(1.0, 0.0), 0.0);
}
