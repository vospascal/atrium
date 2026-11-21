use std::f32::consts::TAU;

use super::driver::RandomRamp;
use super::*;

#[test]
fn river_is_a_located_object_and_produces_audio() {
    let mut river = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 42);
    assert_eq!(river.emitter_kind(), EmitterKind::Object);
    let energy: f32 = (0..48_000)
        .map(|_| river.next_sample(48_000.0).powi(2))
        .sum();
    assert!(energy > 0.0);
}

#[test]
fn zero_flow_is_silent() {
    let mut river = RiverSource::new(Vec3::ZERO, 0.0, 0.0, 42);
    let energy: f32 = (0..48_000)
        .map(|_| river.next_sample(48_000.0).powi(2))
        .sum();
    assert_eq!(energy, 0.0);
}

#[test]
fn graphics_ticks_do_not_advance_or_change_the_river() {
    let mut untouched = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 99);
    let mut ticked = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 99);
    for sample in 0..96_000 {
        if sample % 800 == 0 {
            ticked.tick(1.0 / 60.0);
        }
        assert_eq!(
            untouched.next_sample(48_000.0).to_bits(),
            ticked.next_sample(48_000.0).to_bits()
        );
    }
}

#[test]
fn level_is_stable_across_sample_rates() {
    let rms = |sample_rate: f32| {
        let mut river = RiverSource::new(Vec3::ZERO, 0.75, 0.75, 73);
        let samples = (sample_rate * 3.0) as usize;
        let energy: f64 = (0..samples)
            .map(|_| river.next_sample(sample_rate) as f64)
            .map(|sample| sample * sample)
            .sum();
        (energy / samples as f64).sqrt() as f32
    };
    let ratio = rms(96_000.0) / rms(48_000.0).max(1e-12);
    assert!(
        (0.65..1.45).contains(&ratio),
        "river level changed too much with sample rate: {ratio}"
    );
}

#[test]
fn defaults_define_a_multi_scale_flow() {
    let river = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 42);
    assert_eq!(river.change_time_range(), (15.0, 110.0));
    assert_eq!(river.eddy_time_range(), (0.35, 1.25));
    assert_eq!(river.eddy_depth, 0.65);
    assert!(river.body_gain > 0.0);
    assert!(river.current_gain > 0.0);
    assert!(river.bubble_activity > 0.0);
    assert!(river.splash_rate > 0.0);
}

use crate::synth::test_support::{
    averaged_power_spectrum, band_density_db, band_power, mean, peak, render, root_mean_square,
};

const SAMPLE_RATES: [f32; 3] = [44_100.0, 48_000.0, 96_000.0];

// ── RandomRamp ───────────────────────────────────────────────────────

#[test]
fn smootherstep_has_zero_slope_at_both_ends_and_is_monotonic() {
    assert_eq!(RandomRamp::smootherstep(0.0), 0.0);
    assert_eq!(RandomRamp::smootherstep(1.0), 1.0);
    assert!((RandomRamp::smootherstep(0.5) - 0.5).abs() < 1e-6);
    // Out-of-range input is clamped.
    assert_eq!(RandomRamp::smootherstep(-3.0), 0.0);
    assert_eq!(RandomRamp::smootherstep(9.0), 1.0);
    // 6t^5 - 15t^4 + 10t^3: first derivative 30 t^2 (1 - t)^2 vanishes at 0 and 1.
    let slope_at =
        |t: f32| (RandomRamp::smootherstep(t + 1e-3) - RandomRamp::smootherstep(t)) / 1e-3;
    assert!(slope_at(0.0) < 1e-4);
    assert!(slope_at(0.999) < 1e-3);
    let mut previous = 0.0;
    for step in 1..=100 {
        let value = RandomRamp::smootherstep(step as f32 / 100.0);
        assert!(value >= previous);
        previous = value;
    }
}

/// The ramp never leaves its target range, never jumps, and each segment
/// lasts between `time_min` and `time_max`.
#[test]
fn random_ramp_is_bounded_continuous_and_honours_segment_durations() {
    let mut rng = Rng::new(5);
    let mut ramp = RandomRamp::new(0.0);
    let sample_rate = 1_000.0;
    let dt = 1.0 / sample_rate;
    let mut previous = 0.0_f32;
    let mut segment_starts = vec![0usize];
    let mut last_duration = ramp.duration;
    for index in 0..60_000 {
        let value = ramp.next(&mut rng, dt, -1.0, 1.0, 0.5, 2.0);
        assert!((-1.0..=1.0).contains(&value));
        // Largest slope of a smootherstep is 1.875 x (span / duration).
        assert!(
            (value - previous).abs() < 2.0 * 1.875 / (0.5 * sample_rate) + 1e-4,
            "jump at {index}"
        );
        previous = value;
        if ramp.duration != last_duration {
            segment_starts.push(index);
            last_duration = ramp.duration;
        }
    }
    assert!(segment_starts.len() > 20);
    for pair in segment_starts.windows(2).skip(1) {
        let seconds = (pair[1] - pair[0]) as f32 * dt;
        assert!(
            (0.49..=2.01).contains(&seconds),
            "segment lasted {seconds} s"
        );
    }
}

#[test]
fn random_ramp_with_equal_targets_is_constant() {
    let mut rng = Rng::new(6);
    let mut ramp = RandomRamp::new(0.75);
    for _ in 0..5_000 {
        assert_eq!(ramp.next(&mut rng, 0.001, 0.75, 0.75, 1.0, 5.0), 0.75);
    }
}

// ── Driver ───────────────────────────────────────────────────────────

#[test]
fn driver_time_ranges_are_ordered_and_floored() {
    let mut driver = RiverDriver::new(1, 0.5);
    assert_eq!(driver.change_time_range(), (15.0, 110.0));
    assert_eq!(driver.eddy_time_range(), (0.35, 1.25));
    driver.set_change_time_range(40.0, 20.0);
    assert_eq!(driver.change_time_range(), (20.0, 40.0));
    driver.set_change_time_range(0.0, 0.0);
    assert_eq!(driver.change_time_range(), (0.1, 0.1));
    driver.set_eddy_time_range(2.0, 0.5);
    assert_eq!(driver.eddy_time_range(), (0.5, 2.0));
    driver.set_eddy_time_range(0.0, 0.0);
    assert_eq!(driver.eddy_time_range(), (0.05, 0.05));
}

#[test]
fn flow_state_stays_inside_its_bounds() {
    let mut driver = RiverDriver::new(9, 1.0);
    driver.set_change_time_range(0.2, 0.5);
    driver.set_eddy_time_range(0.05, 0.2);
    let (mut lowest, mut highest) = (f32::MAX, f32::MIN);
    for _ in 0..200_000 {
        let state = driver.next(2_000.0, 0.4, 1.6);
        assert!((0.4..=1.6).contains(&state.speed_m_s));
        assert!((-1.0..=1.0).contains(&state.slow_eddy));
        assert!((-1.0..=1.0).contains(&state.fast_eddy));
        lowest = lowest.min(state.speed_m_s);
        highest = highest.max(state.speed_m_s);
    }
    // Over 100 s of 0.2-0.5 s segments the speed explores most of the range.
    assert!(
        lowest < 0.6 && highest > 1.4,
        "explored {lowest}..{highest}"
    );
}

// ── Event processes ──────────────────────────────────────────────────

/// Bubbles spawned over `seconds`: `bubble_threshold` is redrawn from an
/// Exp(1) variate on every spawn.
fn count_bubbles(river: &mut RiverSource, seconds: f32, sample_rate: f32) -> usize {
    let mut count = 0;
    for _ in 0..(seconds * sample_rate) as usize {
        let before = river.bubble_threshold;
        river.next_sample(sample_rate);
        if river.bubble_threshold != before {
            count += 1;
        }
    }
    count
}

fn steady_river(speed: f32, seed: u64) -> RiverSource {
    let mut river = RiverSource::new(Vec3::ZERO, speed, speed, seed);
    river.eddy_depth = 0.0;
    river
}

/// Bubble rate = activity * (85 + 155 flow_norm) * min(speed / 0.15, 1) per
/// second with flow_norm = sqrt(speed / 1.2): a hazard integral against
/// Exp(1) thresholds, i.e. a Poisson process.
#[test]
fn bubble_rate_follows_the_flow_speed_law() {
    for speed in [0.075_f32, 0.3, 1.2, 3.0] {
        let flow_norm = (speed / 1.2).sqrt().clamp(0.0, 2.2);
        let flow_activity = (speed / 0.15).clamp(0.0, 1.0);
        let rate = 0.75 * (85.0 + 155.0 * flow_norm) * flow_activity;
        let seconds = 20.0;
        let expected = rate * seconds;
        let count = count_bubbles(&mut steady_river(speed, 3), seconds, 16_000.0) as f32;
        assert!(
            (count - expected).abs() < 4.0 * expected.sqrt() + 2.0,
            "speed {speed}: {count} bubbles in {seconds} s, expected {expected:.0}"
        );
    }
}

#[test]
fn bubble_rate_is_the_same_at_every_sample_rate() {
    let expected = 0.75 * 240.0 * 8.0; // speed 1.2: flow_norm 1, activity 1
    for sample_rate in [16_000.0_f32, 44_100.0, 96_000.0] {
        let seconds = if sample_rate > 40_000.0 { 4.0 } else { 8.0 };
        let count = count_bubbles(&mut steady_river(1.2, 4), seconds, sample_rate) as f32;
        let scaled = count * 8.0 / seconds;
        assert!(
            (scaled - expected).abs() < 5.0 * expected.sqrt(),
            "{sample_rate} Hz: {scaled:.0} per 8 s, expected {expected:.0}"
        );
    }
}

/// Splash starts: with bubbles and pops off, the only ring writes are
/// splashes, which begin at `ring_idx + 1` with a 1.2 ms attack ramp.
fn count_splashes(river: &mut RiverSource, seconds: f32, sample_rate: f32) -> usize {
    let mut count = 0;
    for _ in 0..(seconds * sample_rate) as usize {
        let watched = (river.ring_idx + 2) & RING_MASK;
        let before = river.ring[watched];
        river.next_sample(sample_rate);
        if river.ring[watched] != before {
            count += 1;
        }
    }
    count
}

/// splash probability = 1 - exp(-rate / sample_rate), rate =
/// splash_rate (0.3 + 0.7 flow_norm) activity (1 + 0.45 fast_eddy), so
/// at 1.2 m/s the mean rate is `splash_rate`.
#[test]
fn splash_rate_matches_the_configured_rate() {
    let mut river = steady_river(1.2, 5);
    river.bubble_activity = 0.0;
    river.splash_rate = 10.0;
    let count = count_splashes(&mut river, 30.0, 16_000.0) as f32;
    assert!(
        (count - 300.0).abs() < 4.0 * 300.0_f32.sqrt() + 15.0,
        "{count} splashes in 30 s"
    );
}

// ── Event shapes ─────────────────────────────────────────────────────

/// A bubble is a chirped decaying sine: cycles = f T + chirp T / 2 with
/// chirp / f / (1/T) between 3 % and 12 %, so 1.015 to 1.06 f T cycles,
/// over a duration 5.5 / decay clamped to 6-50 ms.
#[test]
fn written_bubble_is_a_slightly_rising_decaying_sine() {
    let sample_rate = 48_000.0;
    for (frequency, decay) in [(450.0_f32, 200.0_f32), (1_000.0, 400.0), (2_500.0, 800.0)] {
        let mut river = RiverSource::new(Vec3::ZERO, 0.5, 0.5, 1);
        river.write_bubble(frequency, decay, 1.0, sample_rate);
        let duration = (5.5 / decay).clamp(0.006, 0.050);
        let length = (duration * sample_rate) as usize;
        let bubble: Vec<f32> = (0..length + 4)
            .map(|offset| river.ring[1 + offset])
            .collect();
        assert!(bubble[length..].iter().all(|&value| value == 0.0));
        let cycles = bubble[..length]
            .windows(2)
            .filter(|pair| pair[0] <= 0.0 && pair[1] > 0.0)
            .count() as f32;
        let base = frequency * duration;
        assert!(
            cycles >= base * 1.01 - 1.0 && cycles <= base * 1.07 + 1.0,
            "{frequency} Hz: {cycles} cycles, base {base}"
        );
        // Amplitude at the end of the burst is e^-5.5 of the peak (0.4 %),
        // unless the 50 ms ceiling cuts a slow decay short.
        let early = bubble[..bubble.len() / 4]
            .iter()
            .fold(0.0_f32, |a, v| a.max(v.abs()));
        let late = bubble[length * 9 / 10..length]
            .iter()
            .fold(0.0_f32, |a, v| a.max(v.abs()));
        assert!(
            late < 0.05 * early,
            "{frequency} Hz: tail {late} vs peak {early}"
        );
    }
}

/// The comment on `write_bubble` says 1.2-7 mm bubbles map to 450-2700 Hz
/// airborne components through Minnaert's f = 3.26 / r (kHz, mm).
#[test]
fn spawned_bubble_frequencies_cover_the_documented_minnaert_span() {
    let minnaert = |radius_mm: f32| 3_260.0 / radius_mm;
    assert!((minnaert(7.0) - 466.0).abs() < 2.0 && (minnaert(1.2) - 2_717.0).abs() < 2.0);
    // spawn_bubble draws 450-900 (45 %), 900-1700 (40 %), 1700-2700 (15 %),
    // all scaled by 0.9 + 0.16 flow_norm: at flow_norm = 0 the span is
    // 405-2430 Hz, at flow_norm = 1.5 it is 1.14x higher: 513-3078 Hz.
    let low = 450.0_f32 * 0.90;
    let high = 2_700.0_f32 * (0.90 + 0.16 * 1.5);
    assert!(low > 380.0 && high < 3_200.0);
}

#[test]
fn splash_and_pop_last_as_long_as_requested() {
    let sample_rate = 48_000.0;
    let mut river = RiverSource::new(Vec3::ZERO, 0.5, 0.5, 2);
    river.write_splash(0.040, 2_500.0, 1.0, sample_rate);
    let last = (1..RING_SIZE / 2)
        .rev()
        .find(|&index| river.ring[index] != 0.0)
        .unwrap();
    assert!((last as i32 - 1_920).abs() <= 3, "splash ends at {last}");
    river.ring.fill(0.0);
    river.write_pop(1.0, sample_rate);
    let last = (1..RING_SIZE / 2)
        .rev()
        .find(|&index| river.ring[index] != 0.0)
        .unwrap();
    // 1.2-4.0 ms: 57-192 samples.
    assert!((55..=195).contains(&last), "pop ends at {last}");
}

// ── Output filters ───────────────────────────────────────────────────

fn sine_gain_db(sample_rate: f32, frequency_hz: f32, mut process: impl FnMut(f32) -> f32) -> f32 {
    let total = (sample_rate * 0.5) as usize;
    let (mut input_energy, mut output_energy) = (0.0_f64, 0.0_f64);
    for index in 0..total {
        let input = (TAU * frequency_hz * index as f32 / sample_rate).sin();
        let output = process(input);
        if index >= total / 2 {
            input_energy += (input as f64).powi(2);
            output_energy += (output as f64).powi(2);
        }
    }
    10.0 * ((output_energy / input_energy).log10() as f32)
}

/// `set_peak(frequency, GAIN_DB, Q, rate)`: the call sites pass
/// (650, 1.0, 0.75) and (1450, 4.0, 0.75), i.e. +1 dB and +4 dB, both
/// Q = 0.75 and therefore broad and gentle, despite the comment calling
/// them "resonant ridges". If the author meant (frequency, Q, gain) the
/// arguments are swapped; the measured gains below are what ships today.
#[test]
fn output_ridges_and_shelf_have_the_gains_the_call_sites_pass() {
    for sample_rate in SAMPLE_RATES {
        let mut river = RiverSource::new(Vec3::ZERO, 0.5, 0.5, 1);
        river.retune(sample_rate);
        let body = sine_gain_db(sample_rate, 650.0, |x| river.body_ridge.process(x));
        let babble = sine_gain_db(sample_rate, 1_450.0, |x| river.babble_ridge.process(x));
        let shelf = sine_gain_db(sample_rate, 12_000.0, |x| river.high_shelf.process(x));
        let well_below = sine_gain_db(sample_rate, 60.0, |x| river.high_shelf.process(x));
        assert!(
            (body - 1.0).abs() < 0.15,
            "{sample_rate}: body ridge {body:.2} dB"
        );
        assert!(
            (babble - 4.0).abs() < 0.15,
            "{sample_rate}: babble ridge {babble:.2} dB"
        );
        assert!(
            (shelf + 5.5).abs() < 0.5,
            "{sample_rate}: shelf {shelf:.2} dB at 12 kHz"
        );
        assert!(
            well_below.abs() < 0.2,
            "shelf touches 60 Hz: {well_below:.2} dB"
        );
    }
}

/// `retune` guards the 6.5 kHz and 12 kHz lowpasses at 0.45 Nyquist.
#[test]
fn retune_keeps_lowpass_corners_below_nyquist_at_low_sample_rates() {
    let mut river = RiverSource::new(Vec3::ZERO, 0.5, 0.5, 1);
    river.next_sample(8_000.0);
    let samples = render(2.0, 8_000.0, |rate| river.next_sample(rate));
    assert!(samples.iter().all(|value| value.is_finite()));
    assert!(peak(&samples) < 1.5);
}

// ── Output statistics ────────────────────────────────────────────────

/// The reference creek concentrates at 500-1600 Hz and falls steeply
/// above 4 kHz (comment in `retune`).
#[test]
fn spectrum_is_concentrated_in_the_creek_band() {
    let mut river = RiverSource::new(Vec3::ZERO, 0.75, 0.75, 42);
    let samples = render(12.0, 48_000.0, |rate| river.next_sample(rate));
    let spectrum = averaged_power_spectrum(&samples);
    let total = band_power(&spectrum, 48_000.0, 90.0, 24_000.0);
    let core = band_power(&spectrum, 48_000.0, 500.0, 1_600.0) / total;
    let brilliance = band_power(&spectrum, 48_000.0, 4_000.0, 24_000.0) / total;
    assert!(core > 0.5, "500-1600 Hz holds only {core:.2} of the power");
    assert!(brilliance < 0.07, "above 4 kHz holds {brilliance:.3}");
}

#[test]
fn output_is_finite_zero_mean_and_inside_unity_at_every_speed() {
    for speed in [0.1_f32, 1.2, 5.0] {
        let mut river = RiverSource::new(Vec3::ZERO, speed, speed, 8);
        let samples = render(10.0, 48_000.0, |rate| river.next_sample(rate));
        assert!(samples.iter().all(|value| value.is_finite()));
        assert!(peak(&samples) < 1.0, "{speed} m/s: peak {}", peak(&samples));
        assert!(mean(&samples).abs() < 0.01, "DC {}", mean(&samples));
    }
}

/// Faster water is louder and brighter.
#[test]
fn level_rises_with_flow_speed() {
    let level = |speed: f32| {
        let mut river = RiverSource::new(Vec3::ZERO, speed, speed, 11);
        let samples = render(6.0, 24_000.0, |rate| river.next_sample(rate));
        root_mean_square(&samples)
    };
    let (trickle, stream, torrent) = (level(0.05), level(0.8), level(4.0));
    assert!(
        trickle < stream && stream < torrent,
        "{trickle} {stream} {torrent}"
    );
}

/// The pink-driven body of the river holds its density at a fixed
/// frequency when the host rate changes. (Above ~4 kHz the white-noise
/// spray layer loses ~2 dB at 96 kHz; see the cross-synth test.)
#[test]
fn density_at_one_kilohertz_is_sample_rate_independent() {
    let density = |sample_rate: f32| {
        let mut river = RiverSource::new(Vec3::ZERO, 0.75, 0.75, 73);
        let samples = render(8.0, sample_rate, |rate| river.next_sample(rate));
        band_density_db(
            &averaged_power_spectrum(&samples),
            sample_rate,
            900.0,
            1_100.0,
        )
    };
    let reference = density(48_000.0);
    for sample_rate in [44_100.0, 96_000.0] {
        let difference = (density(sample_rate) - reference).abs();
        assert!(
            difference < 1.0,
            "{sample_rate} Hz: {difference:.2} dB from 48 kHz"
        );
    }
}

// ── Parameters ───────────────────────────────────────────────────────

#[test]
fn constructor_and_setters_clamp_flow_speed() {
    let river = RiverSource::new(Vec3::ZERO, -2.0, 9.0, 1);
    assert_eq!((river.min_flow_speed, river.max_flow_speed), (0.0, 5.0));
    let river = RiverSource::new(Vec3::ZERO, 4.0, 1.0, 1);
    assert_eq!((river.min_flow_speed, river.max_flow_speed), (4.0, 4.0));
    let mut river = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 1);
    river.set_flow_speed_range(3.0, 0.5);
    assert_eq!((river.min_flow_speed, river.max_flow_speed), (0.5, 3.0));
    river.set_flow_speed_range(-1.0, 10.0);
    assert_eq!((river.min_flow_speed, river.max_flow_speed), (0.0, 5.0));
}

#[test]
fn synth_params_are_clamped_and_routed() {
    let mut river = RiverSource::new(Vec3::ZERO, 0.3, 1.2, 1);
    river.set_synth_param(SynthParam::FlowSpeedMin, 9.0);
    assert_eq!(river.min_flow_speed, 1.2);
    river.set_synth_param(SynthParam::FlowSpeedMax, 99.0);
    assert_eq!(river.max_flow_speed, 5.0);
    river.set_synth_param(SynthParam::TurbulenceDepth, 3.0);
    assert_eq!(river.eddy_depth, 1.0);
    river.set_synth_param(SynthParam::MasterGain, 7.0);
    assert_eq!(river.master_gain, 2.0);
    river.set_synth_param(SynthParam::RiverSplashRate, -1.0);
    assert_eq!(river.splash_rate, 0.0);
    river.set_synth_param(SynthParam::ChangeTimeMin, 500.0);
    assert_eq!(river.change_time_range(), (110.0, 110.0));
    river.set_synth_param(SynthParam::ChangeTimeMax, 5.0);
    assert_eq!(river.change_time_range(), (110.0, 110.0));
    river.set_synth_param(SynthParam::LowGain, 0.9);
    river.set_synth_param(SynthParam::BodyGain, 0.8);
    river.set_synth_param(SynthParam::MidGain, 0.7);
    river.set_synth_param(SynthParam::PresenceGain, 0.6);
    river.set_synth_param(SynthParam::AirGain, 0.5);
    assert_eq!(
        (
            river.body_gain,
            river.current_gain,
            river.bubble_activity,
            river.splash_gain,
            river.spray_gain
        ),
        (0.9, 0.8, 0.7, 0.6, 0.5)
    );
}

#[test]
fn different_seeds_give_different_rivers() {
    let render_seed = |seed: u64| {
        let mut river = RiverSource::new(Vec3::ZERO, 0.8, 0.8, seed);
        render(0.5, 48_000.0, |rate| river.next_sample(rate))
    };
    assert_eq!(render_seed(1), render_seed(1));
    assert_ne!(render_seed(1), render_seed(2));
}
