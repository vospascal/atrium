//! The near field held to the rigid sphere, and the sphere held to the paper.

use super::reference::{distance_variation_db, normalised_frequency, sphere};
use super::sphere::{response_at_zero_frequency, response_magnitude};
use super::*;

/// The HRIRs' measurement distance the shipped renderer uses (m).
const MEASURED: f64 = 1.4;

fn decibels(ratio: f64) -> f64 {
    20.0 * ratio.log10()
}

/// The reference sphere gives the values Duda and Martens print and plot.
///
/// Read off their figures to about half a decibel: figure 5, the interaural
/// level difference at μ = 0.1 for a source at azimuth 100° (ears at ±100°,
/// so the near ear at 0° incidence and the far one at 160°); figure 3, the
/// level at μ = 0.1 at 0° and 150° incidence. From the text: "approximately
/// 3 dB above the free-field value when the normalized frequency is unity"
/// at normal incidence far off, the doubling (6 dB) of equation 12 at high
/// frequency, and at ρ = 1.25 "the extra high-frequency rise at the front
/// of the sphere, instead of being 6 dB, is only about 2 dB".
#[test]
fn the_reference_sphere_gives_duda_and_martens_values() {
    let level = |rho: f64, mu: f64, degrees: f64| decibels(sphere(rho, mu, degrees.to_radians()));
    let figure_5 = [
        (1.25, 26.4),
        (1.5, 19.9),
        (2.0, 13.8),
        (3.0, 8.8),
        (5.0, 5.1),
        (10.0, 2.6),
    ];
    for (rho, plotted) in figure_5 {
        let difference = level(rho, 0.1, 0.0) - level(rho, 0.1, 160.0);
        assert!(
            (difference - plotted).abs() < 0.5,
            "figure 5, ρ = {rho}: {difference:.2} dB, plotted {plotted}"
        );
    }
    let figure_3 = [
        (1.25, 18.0, -8.0),
        (1.5, 13.0, -6.8),
        (2.0, 8.5, -5.2),
        (4.0, 3.6, -2.7),
        (8.0, 1.7, -1.4),
    ];
    for (rho, front, back) in figure_3 {
        let (at_front, at_back) = (level(rho, 0.1, 0.0), level(rho, 0.1, 150.0));
        assert!(
            (at_front - front).abs() < 0.5 && (at_back - back).abs() < 0.5,
            "figure 3, ρ = {rho}: {at_front:.2} and {at_back:.2} dB, plotted {front} and {back}"
        );
    }
    let far = 1e4;
    let at_unity = level(far, 1.0, 0.0);
    assert!((at_unity - 3.0).abs() < 0.2, "μ = 1, far: {at_unity:.2} dB");
    let doubled = level(far, 60.0, 0.0);
    assert!((doubled - 6.02).abs() < 0.2, "μ = 60, far: {doubled:.2} dB");
    let rise = level(1.25, 60.0, 0.0) - level(1.25, 0.05, 0.0);
    assert!((rise - 2.0).abs() < 0.6, "ρ = 1.25: rise {rise:.2} dB");
}

/// The closed form at zero frequency is the series' limit, and gives the
/// hand-worked value: at ρ = 1.25 straight at the source, x = 0.8,
/// 2/(1 − x) + ln(1 − x)/x = 10 − 2.01180 = 7.98820, or 18.049 dB.
#[test]
fn the_zero_frequency_form_is_the_series_limit() {
    let hand = 2.0 / 0.2 + (0.2f64).ln() / 0.8;
    assert!((hand - 7.98820).abs() < 1e-5);
    assert!((response_at_zero_frequency(1.25, 0.0) - hand).abs() < 1e-9);
    for rho in [1.15, 1.25, 2.0, 5.0, 16.0] {
        for degrees in [0.0f64, 10.0, 45.0, 90.0, 100.0, 135.0, 170.0, 180.0] {
            let incidence = degrees.to_radians();
            let closed = response_at_zero_frequency(rho, incidence);
            let series = sphere(rho, 1e-4, incidence);
            assert!(
                (decibels(closed) - decibels(series)).abs() < 1e-3,
                "ρ = {rho}, {degrees}°: closed {closed}, series {series}"
            );
        }
    }
}

/// Duda and Martens' Q-polynomial recursion, as the table uses it, and the
/// Hankel functions directly agree to a millionth of a decibel.
#[test]
fn two_independent_series_agree() {
    let mut worst = 0.0f64;
    for rho in [1.15, 1.37, 2.0, 5.7, 16.0] {
        for frequency in [20.0, 100.0, 1000.0, 4000.0, 8000.0, 16_000.0] {
            for degrees in (0..=180).step_by(15) {
                let mu = normalised_frequency(frequency);
                let incidence = (degrees as f64).to_radians();
                let difference = decibels(response_magnitude(rho, mu, incidence))
                    - decibels(sphere(rho, mu, incidence));
                worst = worst.max(difference.abs());
            }
        }
    }
    assert!(worst < 1e-6, "the two series differ by up to {worst} dB");
}

fn table() -> NearFieldTable {
    NearFieldTable::new(HEAD_RADIUS_METRES, MEASURED as f32)
}

/// The largest difference (dB) between the filter's response for the
/// table's correction and the sphere's distance variation function, over
/// 100 Hz to 8 kHz in twelfth octaves at 48 kHz.
fn worst_against_sphere(table: &NearFieldTable, distance: f64, degrees: f64) -> (f64, f64) {
    let correction = table.correction((degrees as f32).to_radians(), distance as f32);
    let mut worst = (0.0f64, 0.0f64);
    for step in 0..=76 {
        let frequency = 100.0 * 2f64.powf(step as f64 / 12.0);
        let filter = decibels(correction.magnitude_at(frequency, 48_000.0));
        let sphere = distance_variation_db(distance, MEASURED, degrees, frequency);
        if (filter - sphere).abs() > worst.0 {
            worst = ((filter - sphere).abs(), frequency);
        }
    }
    worst
}

/// Each ear's correction, as the filter at 48 kHz renders it, is within
/// 1.5 dB of the sphere's distance variation function from 100 Hz to 8 kHz,
/// at the distances and angles of the plan and at points between the
/// table's. Spagnol et al. report up to about 1.4 dB of spectral distortion
/// (an RMS over 100 Hz–15 kHz) for their fit at the nearest points near 90°;
/// this is a maximum, and it is under half of that bound.
#[test]
fn the_correction_follows_the_sphere() {
    let table = table();
    let mut worst = (0.0f64, 0.0, 0.0, 0.0);
    let planned = [0.15, 0.25, 0.5, 1.0];
    let between = [0.12, 0.18, 0.33, 0.71];
    for distance in planned.into_iter().chain(between) {
        for degrees in [
            0.0, 45.0, 90.0, 135.0, 180.0, 12.5, 62.0, 98.0, 151.0, 177.0,
        ] {
            let (error, frequency) = worst_against_sphere(&table, distance, degrees);
            if error > worst.0 {
                worst = (error, distance, degrees, frequency);
            }
        }
    }
    eprintln!(
        "near field: worst {:.2} dB from the sphere, at {} m, {}°, {:.0} Hz",
        worst.0, worst.1, worst.2, worst.3
    );
    assert!(
        worst.0 <= 1.5,
        "{:.2} dB from the sphere at {} m, {}°, {:.0} Hz",
        worst.0,
        worst.1,
        worst.2,
        worst.3
    );
}

/// At the distance the HRIRs were measured at, and beyond, there is nothing
/// to correct: the correction is unity, exactly.
#[test]
fn at_the_measured_distance_the_correction_is_unity() {
    let table = table();
    for degrees in [0.0f32, 37.0, 90.0, 145.0, 180.0] {
        for distance in [MEASURED as f32, 1.5, 3.0, 50.0] {
            assert_eq!(
                table.correction(degrees.to_radians(), distance),
                EarCorrection::UNITY,
                "{degrees}° at {distance} m"
            );
        }
        let just_inside = table.correction(degrees.to_radians(), MEASURED as f32 - 1e-3);
        for frequency in [100.0, 1000.0, 8000.0] {
            let decibels = decibels(just_inside.magnitude_at(frequency, 48_000.0));
            assert!(
                decibels.abs() < 0.01,
                "{degrees}° at 1.399 m: {decibels} dB"
            );
        }
    }
}

/// Going away, the correction returns to unity without overshoot wherever
/// the sphere's does, at every angle and frequency.
///
/// The sphere's does not everywhere. Between about 20° and 90° from the
/// source its correction first grows and then shrinks again as the source
/// comes in, and at 40°–85° it changes sign: where the near-field response
/// crosses 0 dB moves towards the source as it nears (Spagnol et al.'s
/// "low-frequency parallax"). The latest such turn is at 65° and 4 kHz,
/// 1.19 m out, by hundredths of a decibel. There the correction follows the
/// sphere (the test above), not a rule the sphere breaks.
#[test]
fn the_correction_fades_with_distance_as_the_spheres_does() {
    let table = table();
    let distances: Vec<f32> = (0..=40)
        .map(|step| 0.1 * (14.0f32).powf(step as f32 / 40.0))
        .collect();
    let (mut held, mut skipped) = (0, 0);
    for degrees in (0..=72).map(|step| step as f32 * 2.5) {
        for frequency in [50.0, 200.0, 1000.0, 4000.0, 8000.0] {
            let sphere: Vec<f64> = distances
                .iter()
                .map(|&distance| {
                    distance_variation_db(
                        (distance as f64).max(0.1007),
                        MEASURED,
                        degrees as f64,
                        frequency,
                    )
                    .abs()
                })
                .collect();
            if sphere.windows(2).any(|pair| pair[1] > pair[0] + 1e-3) {
                skipped += 1;
                continue;
            }
            held += 1;
            let mut previous = f64::INFINITY;
            for &distance in &distances {
                let correction = table.correction(degrees.to_radians(), distance);
                let size = decibels(correction.magnitude_at(frequency, 48_000.0)).abs();
                assert!(
                    size <= previous + 0.02,
                    "{degrees}°, {frequency} Hz: {size:.4} dB at {distance:.3} m after {previous:.4}"
                );
                previous = size;
            }
        }
    }
    eprintln!("near field: monotonic as the sphere at {held} angle-frequency pairs, {skipped} where the sphere is not");
    assert!(
        held > 200 && skipped < 160,
        "{held} held, {skipped} skipped"
    );
}

/// A source inside the head, or touching it, is taken at the nearest
/// distance the sphere is taken at.
#[test]
fn nearer_than_the_nearest_is_the_nearest() {
    let table = table();
    let nearest = NEAREST_NORMALISED_DISTANCE * HEAD_RADIUS_METRES;
    for degrees in [0.0f32, 90.0, 180.0] {
        let at_nearest = table.correction(degrees.to_radians(), nearest);
        for distance in [0.0, 0.03, 0.0875, 0.1] {
            assert_eq!(table.correction(degrees.to_radians(), distance), at_nearest);
        }
    }
}

/// Out of the signal the filter passes samples bit for bit, before it is
/// ever used and after it has glided home.
#[test]
fn a_filter_out_of_the_signal_passes_samples_bit_for_bit() {
    let samples: Vec<f32> = (0..2048)
        .map(|index| ((index as f32 * 0.37).sin() * 0.8) - 0.0)
        .chain([-0.0, 0.0, f32::MIN_POSITIVE, 1e-40, -1.0, 1.0])
        .collect();
    let mut filter = EarFilter::new();
    filter.glide_to(None, 512, 48_000.0);
    for sample in &samples {
        assert_eq!(filter.process(*sample).to_bits(), sample.to_bits());
    }
    let near = EarCorrection {
        low_gain: 2.5,
        high_gain: 0.6,
        corner_hz: 900.0,
    };
    filter.glide_to(Some(near), 512, 48_000.0);
    let changed = samples[..512]
        .iter()
        .filter(|sample| filter.process(**sample).to_bits() != sample.to_bits())
        .count();
    assert!(changed > 400, "the correction did nothing");
    // Home over one buffer, then out.
    filter.glide_to(None, 512, 48_000.0);
    for sample in &samples[..512] {
        filter.process(*sample);
    }
    filter.glide_to(None, 512, 48_000.0);
    for sample in &samples {
        assert_eq!(filter.process(*sample).to_bits(), sample.to_bits());
    }
}

/// The filter's gain on a held tone is what its correction describes.
#[test]
fn the_filter_has_the_response_its_correction_describes() {
    let sample_rate = 48_000.0;
    for correction in [
        EarCorrection {
            low_gain: 2.8,
            high_gain: 1.9,
            corner_hz: 600.0,
        },
        EarCorrection {
            low_gain: 0.55,
            high_gain: 0.35,
            corner_hz: 6500.0,
        },
    ] {
        for frequency in [100.0f64, 1000.0, 4000.0, 8000.0] {
            let mut filter = EarFilter::new();
            filter.glide_to(Some(correction), 1, sample_rate as f32);
            let (mut input_energy, mut output_energy) = (0.0f64, 0.0f64);
            for index in 0..96_000 {
                let sample = (std::f64::consts::TAU * frequency * index as f64 / sample_rate).sin();
                let output = filter.process(sample as f32) as f64;
                if index >= 48_000 {
                    input_energy += sample * sample;
                    output_energy += output * output;
                }
            }
            let measured = 10.0 * (output_energy / input_energy).log10();
            let described = decibels(correction.magnitude_at(frequency, sample_rate));
            assert!(
                (measured - described).abs() < 0.02,
                "{frequency} Hz: measured {measured:.3} dB, described {described:.3} dB"
            );
        }
    }
}
