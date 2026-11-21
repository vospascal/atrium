use super::*;
use crate::audio::near_field::reference::distance_variation_db;
use crate::audio::near_field::EarFilter;

fn listener() -> Listener {
    // Facing +x: forward is +x, left is +y, up is +z.
    Listener::new(Vec3::ZERO, 0.0)
}

fn corrections(position: Vec3) -> Option<EarCorrections> {
    near_field_corrections(position, &listener(), shared_table())
}

/// A source in the median plane, straight ahead, overhead, behind or
/// anywhere between, is the same distance from both ears and at the same
/// angle to each: both get the same correction, at every distance and
/// however the head is turned. Facing along an axis it is exactly the same;
/// turned, the rotation leaves the source a few nanometres off the plane in
/// f32, and the two agree to a millionth.
#[test]
fn the_median_plane_gives_both_ears_the_same() {
    let close = |a: EarCorrection, b: EarCorrection| {
        (a.low_gain / b.low_gain - 1.0).abs() < 1e-5
            && (a.high_gain / b.high_gain - 1.0).abs() < 1e-5
            && (a.corner_hz / b.corner_hz - 1.0).abs() < 1e-5
    };
    for yaw in [0.0f32, 0.7, 2.0, -2.9] {
        let listener = Listener::new(Vec3::new(1.0, -2.0, 1.2), yaw);
        let forward = Vec3::new(yaw.cos(), yaw.sin(), 0.0);
        let up = Vec3::new(0.0, 0.0, 1.0);
        for degrees in [0.0f32, 30.0, 90.0, 135.0, 180.0, 250.0] {
            let direction = forward * degrees.to_radians().cos() + up * degrees.to_radians().sin();
            for distance in [0.05f32, 0.12, 0.2, 0.5, 1.0, 1.19] {
                let position = listener.position + direction * distance;
                let ears = near_field_corrections(position, &listener, shared_table())
                    .expect("inside the switch-out distance");
                if yaw == 0.0 {
                    assert_eq!(ears.left, ears.right, "{degrees}° at {distance} m");
                }
                assert!(
                    close(ears.left, ears.right),
                    "{degrees}° in the median plane at {distance} m, yaw {yaw}: {:?} and {:?}",
                    ears.left,
                    ears.right
                );
            }
        }
    }
}

/// From the switch-out distance on the correction is not in the signal at
/// all; just inside it is, and small.
#[test]
fn from_the_switch_out_distance_there_is_none() {
    for direction in [
        Vec3::new(1.0, 0.0, 0.0),
        Vec3::new(0.0, -1.0, 0.0),
        Vec3::new(-0.6, 0.8, 0.0),
    ] {
        for distance in [
            SWITCH_OUT_DISTANCE_METRES,
            1.3,
            MEASURED_DISTANCE_METRES,
            5.0,
            300.0,
        ] {
            assert_eq!(corrections(direction * distance), None, "at {distance} m");
        }
        let inside =
            corrections(direction * (SWITCH_OUT_DISTANCE_METRES - 0.01)).expect("just inside");
        for ear in [inside.left, inside.right] {
            for frequency in [100.0, 1000.0, 8000.0] {
                let decibels = 20.0 * ear.magnitude_at(frequency, 48_000.0).log10();
                assert!(decibels.abs() < 0.3, "{frequency} Hz: {decibels} dB");
            }
        }
    }
}

/// A source on the line through the right ear (100° to the right of
/// straight ahead) is at 0° to that ear and 160° to the left one; the near
/// ear's bass comes up and the far ear's goes down.
#[test]
fn each_ear_sees_the_source_at_its_own_angle() {
    let table = shared_table();
    let azimuth = 100f32.to_radians();
    // Right is −y when facing +x.
    let direction = Vec3::new(azimuth.cos(), -azimuth.sin(), 0.0);
    for distance in [0.15f32, 0.3, 0.8] {
        let ears = corrections(direction * distance).expect("near");
        let (near, far) = (
            table.correction(0.0, distance),
            table.correction(160f32.to_radians(), distance),
        );
        assert!((ears.right.low_gain / near.low_gain - 1.0).abs() < 1e-4);
        assert!((ears.right.high_gain / near.high_gain - 1.0).abs() < 1e-4);
        assert!((ears.left.low_gain / far.low_gain - 1.0).abs() < 1e-4);
        assert!((ears.left.high_gain / far.high_gain - 1.0).abs() < 1e-4);
        assert!(ears.right.low_gain > 1.0 && ears.left.low_gain < 1.0);
    }
    // Inside the head counts as at the nearest distance, in its direction.
    let nearest = crate::audio::near_field::NEAREST_NORMALISED_DISTANCE
        * crate::audio::near_field::HEAD_RADIUS_METRES;
    assert_eq!(
        corrections(direction * 0.04),
        corrections(direction * nearest)
    );
}

/// Why the correction leaves the signal at 1.2 m: there, relative to the
/// 1.4 m the HRIRs were measured at, the sphere changes no ear by more than
/// a quarter of a decibel at any frequency from 20 Hz to 20 kHz and any
/// angle, and the ears' level difference by no more than 0.35 dB, both
/// under the half decibel that is about the smallest interaural level
/// change anyone hears. At 1 m it is still over half a decibel, which is
/// why it is not switched out sooner. The numbers are the module's.
#[test]
fn the_switch_out_distance_is_where_the_sphere_falls_silent() {
    let largest = |distance: f64| {
        let (mut ear, mut between) = (0.0f64, 0.0f64);
        for step in 0..=30 {
            let frequency = 20.0 * 1000f64.powf(step as f64 / 30.0);
            for azimuth in (0..=180).step_by(5) {
                let azimuth = azimuth as f64;
                // Ears at ±100°, the source in the horizontal plane.
                let right = (azimuth - 100.0).abs();
                let left = (azimuth + 100.0).min(360.0 - azimuth - 100.0);
                let right = distance_variation_db(distance, 1.4, right, frequency);
                let left = distance_variation_db(distance, 1.4, left, frequency);
                ear = ear.max(right.abs()).max(left.abs());
                between = between.max((right - left).abs());
            }
        }
        (ear, between)
    };
    let (ear, between) = largest(SWITCH_OUT_DISTANCE_METRES as f64);
    assert!(
        ear < 0.25 && between < 0.35,
        "at the switch-out: {ear:.3} dB in one ear, {between:.3} dB between them"
    );
    let (ear, between) = largest(1.0);
    assert!(
        ear > 0.5 && between > 0.7,
        "at 1 m: {ear:.3} dB, {between:.3} dB"
    );
}

/// The table in the root README: for a source straight to the side (90°,
/// so 10° from the near ear and 170° from the far one), the sphere's level
/// difference between the ears, and what the correction adds to the 1.4 m
/// measurements, at 200 Hz and 4 kHz.
#[test]
fn the_readme_numbers_are_the_spheres() {
    use crate::audio::near_field::reference::{normalised_frequency, sphere, HEAD_RADIUS};
    let between = |distance: f64, frequency: f64| {
        let mu = normalised_frequency(frequency);
        let rho = distance / HEAD_RADIUS;
        20.0 * (sphere(rho, mu, 10f64.to_radians()) / sphere(rho, mu, 170f64.to_radians())).log10()
    };
    // (distance, 200 Hz, 4 kHz) for the sphere, then for the correction.
    let rows = [
        (0.15, 16.3, 23.1, 14.5, 13.6),
        (0.25, 9.3, 16.5, 7.6, 7.0),
        (0.5, 4.6, 12.1, 2.9, 2.6),
        (1.0, 2.4, 10.1, 0.6, 0.6),
    ];
    for (distance, low, high, added_low, added_high) in rows {
        let measured = [between(1.4, 200.0), between(1.4, 4000.0)];
        let sphere = [between(distance, 200.0), between(distance, 4000.0)];
        let added = [sphere[0] - measured[0], sphere[1] - measured[1]];
        for (worked, written) in sphere
            .iter()
            .chain(&added)
            .zip([low, high, added_low, added_high])
        {
            assert!(
                (worked - written).abs() <= 0.06,
                "{distance} m: worked {worked:.2}, written {written}"
            );
        }
    }
}

/// A source flying past at 3 m/s, from 2 m in front, 12 cm past the right
/// ear and 2 m on behind, never clicks. Its corrections are aimed at once
/// a buffer, as the renderer does, and the steepest step between samples
/// of each ear's output is held to that of the same tone through that ear's
/// loudest correction held still the whole way: a click would be a step the
/// tone alone does not have. The correction comes into the signal at 1.2 m
/// and leaves it again, so both are crossed.
#[test]
fn a_source_flying_past_never_clicks() {
    let sample_rate = 48_000.0f32;
    let frames = 512;
    let speed = 3.0;
    let start_x = (2.0f32 * 2.0 - 0.12 * 0.12).sqrt();
    let buffers = (2.0 * start_x / speed * sample_rate / frames as f32) as usize;
    let path: Vec<Option<EarCorrections>> = (0..buffers)
        .map(|buffer| {
            let seconds = buffer as f32 * frames as f32 / sample_rate;
            corrections(Vec3::new(start_x - speed * seconds, -0.12, 0.0))
        })
        .collect();
    assert!(path.first().unwrap().is_none() && path.last().unwrap().is_none());
    assert!(path.iter().filter(|ears| ears.is_some()).count() > 20);

    for frequency in [200.0f32, 1000.0] {
        let tone = |index: usize| {
            (std::f32::consts::TAU * frequency * index as f32 / sample_rate).sin() * 0.5
        };
        for ear in [
            |ears: &EarCorrections| ears.left,
            |ears: &EarCorrections| ears.right,
        ] {
            let steepest = |targets: &mut dyn FnMut(usize) -> Option<EarCorrection>| {
                let mut filter = EarFilter::new();
                let (mut previous, mut steepest) = (0.0f32, 0.0f32);
                for buffer in 0..buffers {
                    filter.glide_to(targets(buffer), frames, sample_rate);
                    for index in 0..frames {
                        let output = filter.process(tone(buffer * frames + index));
                        if buffer > 0 {
                            steepest = steepest.max((output - previous).abs());
                        }
                        previous = output;
                    }
                }
                steepest
            };
            let loudest = path
                .iter()
                .flatten()
                .map(ear)
                .max_by(|a, b| {
                    a.magnitude_at(frequency as f64, sample_rate as f64)
                        .total_cmp(&b.magnitude_at(frequency as f64, sample_rate as f64))
                })
                .unwrap();
            let moving = steepest(&mut |buffer| path[buffer].as_ref().map(ear));
            // Held at the loudest, or, where every correction on the way
            // is a cut, the tone with none at all, which the far stretches
            // of the path are.
            let held = steepest(&mut |_| Some(loudest)).max(steepest(&mut |_| None));
            eprintln!(
                "near field flying past, {frequency} Hz: steepest step {moving:.5}, held {held:.5}"
            );
            assert!(
                moving <= held * 1.02,
                "{frequency} Hz: steepest step {moving:.5} flying past, {held:.5} held still"
            );
        }
    }
}
