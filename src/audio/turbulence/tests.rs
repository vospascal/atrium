//! The published formulas, checked against values worked out by hand.

use super::*;

const SPEED_OF_SOUND: f32 = 343.0;
/// 20 °C.
const TEMPERATURE: f32 = 293.15;
const EAR_HEIGHT: f32 = 1.5;

fn breeze() -> AirMotion {
    AirMotion {
        wind_speed: 5.0,
        wind_toward: [1.0, 0.0],
        heat_flux: 0.0,
    }
}

fn sunny_and_calm() -> AirMotion {
    AirMotion {
        wind_speed: 0.0,
        wind_toward: [1.0, 0.0],
        heat_flux: CLEAR_NOON_HEAT_FLUX,
    }
}

fn close(value: f32, expected: f32, tolerance: f32) -> bool {
    (value - expected).abs() <= tolerance * expected.abs()
}

/// A 5 m/s breeze at night over grass, 1.5 m up:
///   u* = 0.4 × 5 / ln(10 / 0.03) = 2 / 5.8091 = 0.34428 m/s
///   σ_v² = 3 u*² = 0.35560 m²/s²,  L_v = 1.8 × 1.5 = 2.7 m
///   U(1.5) = u* / 0.4 × ln(1.5 / 0.03) = 0.86071 × 3.91202 = 3.3671 m/s
/// No sun, so no temperature eddies and no thermals.
#[test]
fn a_breeze_at_night_is_wind_shear_alone() {
    let layer = surface_layer(&breeze(), EAR_HEIGHT, TEMPERATURE);
    assert!(close(layer.velocity_variance, 0.35560, 1e-4));
    assert!(close(layer.velocity_length, 2.7, 1e-6));
    assert!(close(layer.mean_wind, 3.3671, 1e-4));
    assert_eq!(layer.temperature_variance, 0.0);
    assert!(close(layer.eddy_speed, 0.35560f32.sqrt(), 1e-4));
}

/// A still, clear noon (200 W/m²), 1.5 m up. With no wind the friction
/// velocity is taken as 0.05 m/s and the heat does the stirring:
///   T* = 200 / (1206 × 0.05) = 3.3167 K
///   −z/L_o = 1.5 × 9.81 × 0.4 × 200 / (0.05³ × 293.15 × 1206) = 26.638
///   σ_T² = 4 × 3.3167² / (1 + 266.38)^(2/3) = 44.003 / 41.50 = 1.0602 K²
///   L_T = 2 × 1.5 × (1 + 186.47) / (1 + 266.38) = 2.1034 m
///   w* = (1000 × 9.81 × 200 / (1206 × 293.15))^(1/3) = 1.7705 m/s
///   eddy speed = √(0.35) × 1.7705 = 1.0474 m/s
/// A temperature wavering of about 1 K is what is measured near the ground
/// on a sunny day.
#[test]
fn a_still_sunny_noon_is_thermals_alone() {
    let layer = surface_layer(&sunny_and_calm(), EAR_HEIGHT, TEMPERATURE);
    assert_eq!(layer.velocity_variance, 0.0);
    assert_eq!(layer.mean_wind, 0.0);
    assert!(close(layer.temperature_variance, 1.0602, 1e-3));
    assert!(close(layer.temperature_length, 2.1034, 1e-3));
    assert!(close(layer.eddy_speed, 1.0474, 1e-3));
}

/// The level of a 4 kHz tone 40 m away in the breeze:
///   k = 2π × 4000 / 343 = 73.274 rad/m
///   φ_v² = 2 k² L σ_v² ℒ_v / c² = 2 × 214 766 × 0.35560 × 0.74683 × 2.7 / 117 649 = 2.6178
///   D = 40 / (73.274 × 2.7²) = 0.07488,  G_v = 0.06428 (between two table rows)
///   weak ⟨χ²⟩ = 2.6178 / 2 × 0.06428 = 0.08413
///   eased: 0.08413 / (1 + √(0.08413 × 6)) = 0.04919,  σ = 8.686 × √0.04919 = 1.93 dB
/// and at 1 kHz it is 0.94 dB: the treble wavers twice as much.
#[test]
fn a_far_tone_wavers_by_about_two_decibels_in_a_breeze() {
    let layer = surface_layer(&breeze(), EAR_HEIGHT, TEMPERATURE);
    let weak = weak_variance(&layer, 4000.0, 40.0, SPEED_OF_SOUND, TEMPERATURE);
    assert!(close(weak, 0.08413, 2e-3), "{weak}");
    let treble = log_amplitude_variance(&layer, 4000.0, 40.0, SPEED_OF_SOUND, TEMPERATURE);
    assert!(close(treble, 0.04919, 2e-3), "{treble}");
    assert!(close(DECIBELS_PER_NEPER * treble.sqrt(), 1.926, 2e-3));
    let middle = log_amplitude_variance(&layer, 1000.0, 40.0, SPEED_OF_SOUND, TEMPERATURE);
    assert!(close(DECIBELS_PER_NEPER * middle.sqrt(), 0.939, 2e-3));
}

/// The same tone on the still sunny noon, from temperature eddies:
///   φ_T² = k² L σ_T² ℒ_T / (2 T²) = 214 766 × 1.0602 × 0.74683 × 2.1034 / (2 × 293.15²) = 2.0810
///   D = 40 / (73.274 × 2.1034²) = 0.12339,  G_T = 0.05306
///   weak ⟨χ²⟩ = 0.05521, eased 0.03504,  σ = 1.63 dB
/// Heat alone makes the air shimmer about as much as a breeze does.
#[test]
fn a_far_tone_wavers_on_a_still_sunny_day_too() {
    let layer = surface_layer(&sunny_and_calm(), EAR_HEIGHT, TEMPERATURE);
    let weak = weak_variance(&layer, 4000.0, 40.0, SPEED_OF_SOUND, TEMPERATURE);
    assert!(close(weak, 0.05521, 3e-3), "{weak}");
    let variance = log_amplitude_variance(&layer, 4000.0, 40.0, SPEED_OF_SOUND, TEMPERATURE);
    assert!(close(DECIBELS_PER_NEPER * variance.sqrt(), 1.626, 3e-3));
}

/// Close by there is nothing to hear: 0.3 dB at 1 kHz from 10 m in the
/// breeze, and nothing at all in still air at night, at any distance.
#[test]
fn a_near_sound_and_still_air_do_not_waver() {
    let layer = surface_layer(&breeze(), EAR_HEIGHT, TEMPERATURE);
    let near = log_amplitude_variance(&layer, 1000.0, 10.0, SPEED_OF_SOUND, TEMPERATURE);
    assert!(close(DECIBELS_PER_NEPER * near.sqrt(), 0.302, 3e-3));

    assert!(AirMotion::STILL.is_still());
    let still = surface_layer(&AirMotion::STILL, EAR_HEIGHT, TEMPERATURE);
    for (frequency, distance) in [(500.0, 5.0), (4000.0, 40.0), (16000.0, 500.0)] {
        assert_eq!(
            log_amplitude_variance(&still, frequency, distance, SPEED_OF_SOUND, TEMPERATURE),
            0.0
        );
    }
}

/// Where the eddies are much larger than the Fresnel zone the weak variance
/// follows the classic law ⟨χ²⟩ ∝ k^(7/6) L^(11/6): doubling the frequency
/// multiplies it by 2^(7/6) = 2.245, doubling the distance by 2^(11/6) = 3.564.
#[test]
fn the_weak_variance_follows_the_seven_sixths_law() {
    // 300 m up the eddies are hundreds of metres across: D stays tiny.
    let layer = surface_layer(&breeze(), 300.0, TEMPERATURE);
    let at = |frequency, distance| {
        weak_variance(&layer, frequency, distance, SPEED_OF_SOUND, TEMPERATURE)
    };
    let base = at(2000.0, 50.0);
    assert!(close(at(4000.0, 50.0) / base, 2f32.powf(7.0 / 6.0), 5e-3));
    assert!(close(at(2000.0, 100.0) / base, 2f32.powf(11.0 / 6.0), 5e-3));
}

/// More wind, more distance and more treble each make it waver more, and it
/// never passes saturation: 5.6 dB.
#[test]
fn it_grows_with_wind_distance_and_frequency_up_to_saturation() {
    let variance = |wind_speed: f32, frequency: f32, distance: f32| {
        let air = AirMotion {
            wind_speed,
            ..breeze()
        };
        let layer = surface_layer(&air, EAR_HEIGHT, TEMPERATURE);
        log_amplitude_variance(&layer, frequency, distance, SPEED_OF_SOUND, TEMPERATURE)
    };
    assert!(variance(8.0, 2000.0, 30.0) > variance(4.0, 2000.0, 30.0));
    assert!(variance(4.0, 2000.0, 60.0) > variance(4.0, 2000.0, 30.0));
    assert!(variance(4.0, 8000.0, 30.0) > variance(4.0, 2000.0, 30.0));
    let storm_far = variance(13.0, 16000.0, 5000.0);
    assert_eq!(storm_far, SATURATED_VARIANCE);
    assert!(close(DECIBELS_PER_NEPER * storm_far.sqrt(), 5.57, 5e-3));
}

/// The easing is the paper's equation 42: at the boundary of weak
/// scattering, b² = 1/6, it halves the variance; far below it changes
/// little; and it never decreases.
#[test]
fn saturation_eases_in_at_the_weak_scattering_limit() {
    assert_eq!(saturating(0.0), 0.0);
    assert!(close(saturating(1.0 / 6.0), 1.0 / 12.0, 1e-5));
    assert!(close(saturating(1e-6), 1e-6, 3e-3));
    let mut previous = 0.0;
    for step in 1..200 {
        let eased = saturating(step as f32 * 0.02);
        assert!(eased >= previous);
        previous = eased;
    }
    assert_eq!(previous, SATURATED_VARIANCE);
}

/// The tables are the paper's integrals. Integrated again here, coarsely,
/// they agree within 1 %: G(D) = ∫₀¹ [1 − ∫₀^∞ w(x) cos(η(1−η) x D) dx] dη
/// with w = α(α−1) x (1+x)^(−α−1) for wind and (α−1)(1+x)^(−α) for
/// temperature, α = 11/6.
#[test]
fn the_tables_are_the_integrals_they_claim_to_be() {
    let alpha = 11.0_f64 / 6.0;
    let integrated = |wave_parameter: f64, wind: bool| {
        let path_steps = 200;
        let scale_steps = 40_000;
        let (lowest, highest) = (-14.0_f64, 14.0_f64);
        let step = (highest - lowest) / scale_steps as f64;
        let mut share = 0.0;
        for path_step in 0..path_steps {
            let along = (path_step as f64 + 0.5) / path_steps as f64;
            let spread = along * (1.0 - along) * wave_parameter;
            let mut kept = 0.0;
            for scale_step in 0..scale_steps {
                // x = e^u, so dx = x du; the cosine is averaged over a cell
                // where it turns faster than the grid.
                let x = (lowest + (scale_step as f64 + 0.5) * step).exp();
                let weight = if wind {
                    alpha * (alpha - 1.0) * x / (1.0 + x).powf(alpha + 1.0)
                } else {
                    (alpha - 1.0) / (1.0 + x).powf(alpha)
                };
                let half_turn = spread * x * step / 2.0;
                let averaged = if half_turn.abs() < 1e-9 {
                    1.0
                } else {
                    half_turn.sin() / half_turn
                };
                kept += weight * x * (spread * x).cos() * averaged * step;
            }
            share += (1.0 - kept) / path_steps as f64;
        }
        share as f32
    };
    for wave_parameter in [0.01_f32, 0.1, 1.0, 10.0] {
        let wind = diffraction_share(&VELOCITY_SHARE, wave_parameter);
        let temperature = diffraction_share(&TEMPERATURE_SHARE, wave_parameter);
        assert!(
            close(wind, integrated(wave_parameter as f64, true), 0.01),
            "wind at D = {wave_parameter}: {wind}"
        );
        assert!(
            close(temperature, integrated(wave_parameter as f64, false), 0.01),
            "temperature at D = {wave_parameter}: {temperature}"
        );
    }
    // Below the table it continues as D^(5/6); above, it is all diffraction.
    assert!(close(
        diffraction_share(&VELOCITY_SHARE, 1e-5),
        VELOCITY_SHARE[0] * 0.01f32.powf(5.0 / 6.0),
        1e-4
    ));
    assert_eq!(diffraction_share(&VELOCITY_SHARE, 1e4), 1.0);
    assert_eq!(diffraction_share(&VELOCITY_SHARE, 0.0), 0.0);
}

/// The wind condition is metres per second and a compass bearing it comes
/// from: a west wind (270°) blows toward the east (+x), a north wind (0°)
/// toward the south (−y). Daylight is the heat of the sun.
#[test]
fn conditions_become_wind_and_sun() {
    let west = AirMotion::from_weather(0.5, 1.0, 270.0);
    assert!(close(west.wind_speed, 6.5, 1e-6));
    assert!((west.wind_toward[0] - 1.0).abs() < 1e-6 && west.wind_toward[1].abs() < 1e-6);
    assert_eq!(west.heat_flux, CLEAR_NOON_HEAT_FLUX);
    let north = AirMotion::from_weather(2.0, -1.0, 0.0);
    assert_eq!(north.wind_speed, FULL_WIND_METRES_PER_SECOND);
    assert!(north.wind_toward[0].abs() < 1e-6 && (north.wind_toward[1] + 1.0).abs() < 1e-6);
    assert_eq!(north.heat_flux, 0.0);
    assert!(AirMotion::from_weather(0.0, 0.0, 123.0).is_still());
}

/// How long the level holds. At 4 kHz over 40 m the Fresnel zone is
/// √(0.08575 × 40) = 1.852 m. A 5 m/s breeze across the path carries it past
/// in 1.852 / √(3.3671² + 0.3556) = 0.542 s; blowing along the path only the
/// eddies' own churning moves it, 1.852 / 0.5963 = 3.106 s.
#[test]
fn a_crosswind_makes_it_waver_faster_than_a_wind_along_the_path() {
    let layer = surface_layer(&breeze(), EAR_HEIGHT, TEMPERATURE);
    let across = wavering_seconds(&layer, 40.0, SPEED_OF_SOUND, 1.0);
    let along = wavering_seconds(&layer, 40.0, SPEED_OF_SOUND, 0.0);
    assert!(close(across, 0.5416, 2e-3), "{across}");
    assert!(close(along, 3.1058, 2e-3), "{along}");
    // Still air has a finite answer too, which nothing uses.
    let still = surface_layer(&AirMotion::STILL, EAR_HEIGHT, TEMPERATURE);
    assert!(wavering_seconds(&still, 40.0, SPEED_OF_SOUND, 1.0).is_finite());
}

// ── The wavering filter ─────────────────────────────────────────────────

const SAMPLE_RATE: f32 = 48_000.0;

fn path<'a>(air: &'a AirMotion, from: Vec3, to: Vec3) -> AirPath<'a> {
    AirPath {
        air,
        open_air: true,
        from,
        to,
        ground: 0.0,
        distance: from.distance_to(to),
        temperature_c: 20.0,
        speed_of_sound: 343.42,
    }
}

use atrium_core::types::Vec3;

/// A blackbird 40 m north, 1.8 m up, heard at 1.2 m in a 5 m/s west wind:
/// the path is 1.5 m above the ground on average and the wind blows straight
/// across it. At 20 °C (c = 343.42 m/s) the theory gives σ = 0.0717 Np
/// (0.62 dB) at 500 Hz and 0.2675 Np (2.32 dB) at 6 kHz, holding for
/// √(343.42 / 4000 × 40) / √(3.3671² + 0.3556) = 0.542 s.
#[test]
fn a_far_bird_in_a_crosswind_wavers_as_the_theory_says() {
    let air = AirMotion::from_weather(5.0 / FULL_WIND_METRES_PER_SECOND, 0.0, 270.0);
    let wavering = Wavering::along(&path(
        &air,
        Vec3::new(0.0, 40.0, 1.8),
        Vec3::new(0.0, 0.0, 1.2),
    ));
    assert!(close(wavering.low, 0.0717, 0.01), "{}", wavering.low);
    assert!(close(wavering.high, 0.2675, 0.01), "{}", wavering.high);
    assert!(close(wavering.seconds, 0.542, 0.01), "{}", wavering.seconds);
}

/// The same bird with the wind blowing along the path wavers as deeply but
/// more slowly, and one straight overhead has the whole wind across it.
#[test]
fn the_wind_direction_sets_how_fast_it_wavers() {
    let from_west = AirMotion::from_weather(0.4, 0.0, 270.0);
    let from_north = AirMotion::from_weather(0.4, 0.0, 0.0);
    let (bird, ears) = (Vec3::new(0.0, 40.0, 1.8), Vec3::new(0.0, 0.0, 1.2));
    let across = Wavering::along(&path(&from_west, bird, ears));
    let along = Wavering::along(&path(&from_north, bird, ears));
    assert_eq!(across.high, along.high);
    assert!(along.seconds > 3.0 * across.seconds);
    let overhead = Wavering::along(&path(&from_north, Vec3::new(0.0, 0.0, 41.2), ears));
    let level = Wavering::along(&path(
        &from_west,
        Vec3::new(0.0, 40.0, 41.2),
        Vec3::new(0.0, 0.0, 1.2),
    ));
    assert!(
        overhead.seconds < along.seconds,
        "a path straight up is all crosswind"
    );
    assert!(level.seconds.is_finite());
}

/// Indoors, in still air, and at no distance nothing wavers.
#[test]
fn indoors_and_in_still_air_nothing_wavers() {
    let air = AirMotion::from_weather(0.5, 1.0, 270.0);
    let (bird, ears) = (Vec3::new(0.0, 40.0, 1.8), Vec3::new(0.0, 0.0, 1.2));
    let indoors = AirPath {
        open_air: false,
        ..path(&air, bird, ears)
    };
    assert_eq!(Wavering::along(&indoors), Wavering::NONE);
    assert_eq!(
        Wavering::along(&path(&AirMotion::STILL, bird, ears)),
        Wavering::NONE
    );
    assert_eq!(Wavering::along(&path(&air, ears, ears)), Wavering::NONE);
}

/// With nothing to waver the filter is not in the signal at all: every
/// sample comes out bit for bit as it went in.
#[test]
fn still_air_passes_the_sound_untouched() {
    let mut waver = AirWaver::new(1);
    waver.set(Wavering::NONE, SAMPLE_RATE);
    for index in 0..10_000 {
        let sample = (index as f32 * 0.37).sin() * 0.8;
        assert_eq!(waver.process(sample).to_bits(), sample.to_bits());
    }
}

/// The level wanders as a log-normal with the deviation asked for, and its
/// mean square stays 1: the air moves the level about and takes no energy.
/// Ten minutes of a steady signal with σ = 0.2 Np (1.74 dB), holding 50 ms.
#[test]
fn the_level_wanders_by_the_deviation_asked_and_keeps_its_energy() {
    let mut waver = AirWaver::new(7);
    waver.set(
        Wavering {
            low: 0.2,
            high: 0.2,
            seconds: 0.05,
        },
        SAMPLE_RATE,
    );
    let samples = 600 * SAMPLE_RATE as usize;
    let (mut log_sum, mut log_square_sum, mut square_sum) = (0.0_f64, 0.0_f64, 0.0_f64);
    for _ in 0..samples {
        let gain = waver.process(1.0) as f64;
        log_sum += gain.ln();
        log_square_sum += gain.ln() * gain.ln();
        square_sum += gain * gain;
    }
    let count = samples as f64;
    let mean = log_sum / count;
    let deviation = (log_square_sum / count - mean * mean).sqrt();
    assert!((deviation - 0.2).abs() < 0.015, "deviation {deviation}");
    assert!(
        (mean + 0.04).abs() < 0.01,
        "the mean log-amplitude is −σ²: {mean}"
    );
    assert!(
        (square_sum / count - 1.0).abs() < 0.03,
        "mean square {}",
        square_sum / count
    );
}

/// The level of a tone through a waver, as the deviation of its
/// log-amplitude over ten minutes of 10 ms stretches (nepers).
fn tone_deviation(wavering: Wavering, frequency_hz: f32) -> f32 {
    let mut waver = AirWaver::new(11);
    waver.set(wavering, SAMPLE_RATE);
    let stretch = 480;
    let mut levels = Vec::new();
    for block in 0..60_000 {
        let mut energy = 0.0_f64;
        for index in 0..stretch {
            let time = (block * stretch + index) as f32 / SAMPLE_RATE;
            let tone = (std::f32::consts::TAU * frequency_hz * time).sin();
            energy += (waver.process(tone) as f64).powi(2);
        }
        levels.push(0.5 * (energy / stretch as f64 * 2.0).ln());
    }
    let mean = levels.iter().sum::<f64>() / levels.len() as f64;
    (levels
        .iter()
        .map(|level| (level - mean).powi(2))
        .sum::<f64>()
        / levels.len() as f64)
        .sqrt() as f32
}

/// Measured on the sound itself: tones through the filter waver by what the
/// theory says for their frequency. For the bird 40 m off in a breeze every
/// tone from 500 Hz to 6 kHz is within 15 %, and the treble wavers three
/// times as much as the bass. In a gale at 60 m, where the level swings by
/// 4 dB and more, two parts fall up to a fifth short.
#[test]
fn tones_waver_by_what_the_theory_says_for_their_frequency() {
    for (distance, wind_speed, tolerance) in [(40.0, 5.0, 0.15), (60.0, 9.0, 0.22)] {
        let air = AirMotion {
            wind_speed,
            ..breeze()
        };
        let layer = surface_layer(&air, 1.5, TEMPERATURE);
        let theory = |frequency_hz: f32| {
            log_amplitude_variance(&layer, frequency_hz, distance, 343.42, TEMPERATURE).sqrt()
        };
        let wavering = Wavering {
            low: theory(500.0),
            high: theory(6000.0),
            seconds: 0.2,
        };
        let measured: Vec<f32> = [500.0, 1000.0, 2000.0, 4000.0, 6000.0]
            .into_iter()
            .map(|frequency_hz| {
                let measured = tone_deviation(wavering, frequency_hz);
                assert!(
                    close(measured, theory(frequency_hz), tolerance),
                    "{distance} m, {wind_speed} m/s, {frequency_hz} Hz: {measured} for {}",
                    theory(frequency_hz)
                );
                measured
            })
            .collect();
        assert!(measured[4] > 2.5 * measured[0], "the treble wavers most");
    }
}

/// Two parts stand in for a deviation that really grows steadily with
/// frequency. A tone gets the parts' deviations mixed by how much of it lies
/// in each, and that mix stays within an eighth of the theory from 500 Hz to
/// 6 kHz: near and far, in light air and in a gale.
#[test]
fn two_parts_follow_the_theory_within_an_eighth() {
    use super::waver::{low_share, HIGH_HZ, LOW_HZ};
    for (distance, wind_speed) in [(40.0, 5.0), (10.0, 3.0), (60.0, 9.0), (20.0, 5.0)] {
        let air = AirMotion {
            wind_speed,
            ..breeze()
        };
        let layer = surface_layer(&air, 1.5, TEMPERATURE);
        let theory = |frequency_hz: f32| {
            log_amplitude_variance(&layer, frequency_hz, distance, 343.42, TEMPERATURE).sqrt()
        };
        // The parts, solved as the filter solves them.
        let (at_low, at_high) = (
            low_share(LOW_HZ, SAMPLE_RATE),
            low_share(HIGH_HZ, SAMPLE_RATE),
        );
        let high_part = (theory(HIGH_HZ) * at_low - theory(LOW_HZ) * at_high) / (at_low - at_high);
        let low_part = (theory(LOW_HZ) - (1.0 - at_low) * high_part) / at_low;
        assert!(low_part > 0.0 && high_part > low_part);
        for frequency_hz in [500.0, 700.0, 1000.0, 1400.0, 2000.0, 2800.0, 4000.0, 6000.0] {
            let share = low_share(frequency_hz, SAMPLE_RATE);
            let mixed = share * low_part + (1.0 - share) * high_part;
            assert!(
                close(mixed, theory(frequency_hz), 0.125),
                "{distance} m, {wind_speed} m/s, {frequency_hz} Hz: {mixed} for {}",
                theory(frequency_hz)
            );
        }
    }
}

/// Two paths never waver together, and the same path always wavers the
/// same way: each has a wander of its own, from its seed.
#[test]
fn each_path_has_its_own_wander() {
    let run = |seed: u64| {
        let mut waver = AirWaver::new(seed);
        waver.set(
            Wavering {
                low: 0.2,
                high: 0.2,
                seconds: 0.05,
            },
            SAMPLE_RATE,
        );
        (0..480_000)
            .map(|_| waver.process(1.0).ln())
            .collect::<Vec<f32>>()
    };
    let (first, again, other) = (run(3), run(3), run(4));
    assert_eq!(first, again);
    let mean = |values: &[f32]| values.iter().sum::<f32>() / values.len() as f32;
    let (first_mean, other_mean) = (mean(&first), mean(&other));
    let covariance = first
        .iter()
        .zip(&other)
        .map(|(a, b)| (a - first_mean) * (b - other_mean))
        .sum::<f32>()
        / first.len() as f32;
    assert!(
        covariance.abs() < 0.2 * 0.2 * 0.25,
        "correlated: {covariance}"
    );
}

/// The gains glide, never jump: between two samples of a steady signal the
/// output moves by less than a thousandth, even in saturated turbulence.
#[test]
fn the_gains_never_jump() {
    let mut waver = AirWaver::new(5);
    waver.set(
        Wavering {
            low: 0.3,
            high: SATURATED_VARIANCE.sqrt(),
            seconds: 0.2,
        },
        SAMPLE_RATE,
    );
    // The signal's own start is a step, which the split passes as one; the
    // gains are judged once it has settled on the steady signal.
    for _ in 0..1000 {
        waver.process(1.0);
    }
    let mut previous = waver.process(1.0);
    for _ in 0..(20.0 * SAMPLE_RATE) as usize {
        let now = waver.process(1.0);
        assert!((now - previous).abs() < 1e-3, "{previous} → {now}");
        previous = now;
    }
}

/// When the air stills the gains glide home and the filter steps out of the
/// signal again, without a jump.
#[test]
fn it_settles_back_to_untouched_when_the_air_stills() {
    let mut waver = AirWaver::new(9);
    waver.set(
        Wavering {
            low: 0.3,
            high: 0.5,
            seconds: 0.05,
        },
        SAMPLE_RATE,
    );
    let mut previous = 1.0;
    for _ in 0..48_000 {
        previous = waver.process(1.0);
    }
    waver.set(Wavering::NONE, SAMPLE_RATE);
    for _ in 0..48_000 {
        let now = waver.process(1.0);
        assert!((now - previous).abs() < 0.05, "a jump: {previous} → {now}");
        previous = now;
    }
    for index in 0..1000 {
        let sample = (index as f32 * 0.11).sin();
        assert_eq!(waver.process(sample).to_bits(), sample.to_bits());
    }
}
