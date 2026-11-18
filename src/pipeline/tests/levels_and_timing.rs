//! Distance laws, arrival times and total power, checked end to end.

use super::{
    channel_of, default_atmosphere, energy, tone_power_db, ImpulseSource, Rig, ToneSource,
    BLOCK_FRAMES, SAMPLE_RATES,
};
use atrium_core::speaker::{RenderMode, SpeakerLayout};
use atrium_core::types::Vec3;

/// WorldLocked models the source → speaker flight. Stereo pair 1 m either
/// side of the environment centre (static delay compensation = 0 for
/// both). Source on the x axis 10 m beyond the left speaker:
///   left  10 m, outward axis (−1,0,0) points at it → cardioid 1.0
///   right 12 m, outward axis (+1,0,0) points away → cardioid 0.0
///   (source radius 11 m ≥ 2 × ring radius 1 m → full cardioid).
/// Arrival: 10 / 343.42 · fs = 1284.15 / 1397.67 / 2795.35 samples; the
/// linear-interpolated read is first non-zero at floor(delay).
/// Level: area (DC gain) of the left channel = master 0.5 × inverse law
/// (1 m / 10 m) × ISO ground gain(10 m, 0, 0) × air-absorption DC (≈ 1).
#[test]
fn world_locked_arrival_and_level_follow_source_to_speaker_flight() {
    let source_position = Vec3::new(-11.0, 0.0, 0.0);
    let emit_at = 2 * BLOCK_FRAMES; // after the first-buffer gain ramp
    let mut arrivals_seconds = Vec::new();
    for sample_rate in SAMPLE_RATES {
        let mut rig = Rig::free_field(sample_rate);
        rig.layout = SpeakerLayout::stereo(Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
        rig.channels = 2;
        rig.master_gain = 0.5;
        let mut pipeline = rig.build(RenderMode::WorldLocked);
        let mut sources = vec![ImpulseSource::boxed(source_position, emit_at)];

        let delay_samples = 10.0 / rig.environment.atmosphere.speed_of_sound() * sample_rate;
        let total_frames = emit_at + (2.0 * delay_samples) as usize;
        let output = rig.render(&mut pipeline, &mut sources, total_frames);
        let left = channel_of(&output, 2, 0);
        let right = channel_of(&output, 2, 1);

        let first = left
            .iter()
            .position(|sample| *sample != 0.0)
            .expect("left speaker never receives the impulse");
        let arrival = first - emit_at;
        assert!(
            (arrival as f32 - delay_samples.floor()).abs() <= 1.0,
            "{sample_rate} Hz: impulse arrived after {arrival} samples, expected floor({delay_samples:.2})"
        );
        arrivals_seconds.push(arrival as f32 / sample_rate);

        let ground_gain =
            crate::audio::propagation::ground_effect_gain(10.0, 0.0, 0.0, &rig.environment.ground);
        let expected_area = 0.5 * 0.1 * ground_gain as f64;
        let measured_area = left.iter().map(|&sample| sample as f64).sum::<f64>();
        assert!(
            (measured_area / expected_area - 1.0).abs() < 0.01,
            "{sample_rate} Hz: left DC gain {measured_area:.6}, expected {expected_area:.6}"
        );
        assert!(
            right.iter().all(|sample| *sample == 0.0),
            "{sample_rate} Hz: the right microphone faces away (cardioid null) and must stay silent"
        );
    }
    let expected_seconds = 10.0 / default_atmosphere().speed_of_sound();
    for seconds in &arrivals_seconds {
        assert!(
            (seconds - expected_seconds).abs() <= 1.0 / 44_100.0,
            "arrival {seconds:.6} s vs 10 m / c = {expected_seconds:.6} s"
        );
    }
}

/// WorldLocked distance law end to end: speaker distance 10 m → 20 m drops
/// the left channel by 20·log10(2) = 6.02 dB (inverse law, ref 1 m), plus
/// whatever the ISO ground gain changes between the two distances.
#[test]
fn world_locked_doubling_source_to_speaker_distance_drops_6_02_db() {
    let mut rig = Rig::free_field(48_000.0);
    rig.layout = SpeakerLayout::stereo(Vec3::new(-1.0, 0.0, 0.0), Vec3::new(1.0, 0.0, 0.0));
    rig.channels = 2;
    let emit_at = 2 * BLOCK_FRAMES;
    let left_area = |distance: f32| -> f64 {
        let mut pipeline = rig.build(RenderMode::WorldLocked);
        let mut sources = vec![ImpulseSource::boxed(
            Vec3::new(-1.0 - distance, 0.0, 0.0),
            emit_at,
        )];
        let output = rig.render(&mut pipeline, &mut sources, emit_at + 6 * 1400);
        channel_of(&output, 2, 0)
            .iter()
            .map(|&sample| sample as f64)
            .sum()
    };
    let ground = |distance: f32| {
        crate::audio::propagation::ground_effect_gain(distance, 0.0, 0.0, &rig.environment.ground)
    };
    let expected_db = 20.0 * 2.0_f64.log10() + 20.0 * (ground(10.0) / ground(20.0)).log10() as f64;
    let measured_db = 20.0 * (left_area(10.0) / left_area(20.0)).log10();
    assert!(
        (measured_db - expected_db).abs() < 0.05,
        "10 m → 20 m dropped {measured_db:.3} dB, expected {expected_db:.3} dB"
    );
}

/// Direct level end to end: 4 m → 8 m on the same azimuth drops the whole
/// output by 20·log10(2) = 6.02 dB. At 0 m heights the ISO 9613-2 ground
/// bands do not depend on distance (q = 1 for any d > 0), so the ground
/// filter is identical for both. (The HRIR lookup is distance-independent:
/// libmysofa clamps the query radius; checked separately during the audit.)
#[test]
fn doubling_listener_distance_drops_6_02_db_in_listener_centric_modes() {
    let rig = Rig::free_field(48_000.0);
    let near =
        crate::audio::propagation::ground_effect_bands(4.0, 0.0, 0.0, &rig.environment.ground);
    let far =
        crate::audio::propagation::ground_effect_bands(8.0, 0.0, 0.0, &rig.environment.ground);
    assert_eq!(
        near, far,
        "ground bands must not depend on distance at 0 m heights"
    );
    let expected_db = 20.0 * 2.0_f64.log10();
    for mode in [RenderMode::Vbap, RenderMode::Hrtf, RenderMode::Ambisonics] {
        let measured_db = tone_power_db(&rig, mode, Vec3::new(0.0, 4.0, 0.0), 4096)
            - tone_power_db(&rig, mode, Vec3::new(0.0, 8.0, 0.0), 4096);
        assert!(
            (measured_db - expected_db).abs() < 0.05,
            "{mode:?}: 4 m → 8 m dropped {measured_db:.3} dB, expected {expected_db:.3} dB"
        );
    }
}

/// A source circling the listener at 1.5 m (inside the 2 m quad) at 16
/// azimuths: total output power must not depend on direction.
///   VBAP: Σg² = (1/1.5)² for every pair → 0 dB spread.
///   DBAP: inside the reference circle p = 1, b = 1 → Σv² = 1 → 0 dB.
///   Ambisonics: EPAD AllRAD decode is energy-preserving by design.
/// HRTF (head shadow) and WorldLocked (speakers as microphones, not
/// energy-normalised) legitimately vary and are excluded.
#[test]
fn circling_source_keeps_total_power_constant() {
    let rig = Rig::free_field(48_000.0);
    let frames = 4096;
    for (mode, tolerance_db) in [
        (RenderMode::Vbap, 0.2),
        (RenderMode::Dbap, 0.2),
        (RenderMode::Ambisonics, 1.0),
    ] {
        let powers_db: Vec<f64> = (0..16)
            .map(|step| {
                let azimuth = step as f32 * std::f32::consts::TAU / 16.0;
                let position = Vec3::new(1.5 * azimuth.cos(), 1.5 * azimuth.sin(), 0.0);
                let mut pipeline = rig.build(mode);
                let mut sources = vec![ToneSource::boxed(position, 1000.0, usize::MAX)];
                let output = rig.render(&mut pipeline, &mut sources, frames);
                let all_channels: Vec<usize> = (0..rig.channels).collect();
                10.0 * energy(&output, rig.channels, &all_channels, frames / 2).log10()
            })
            .collect();
        let loudest = powers_db.iter().cloned().fold(f64::MIN, f64::max);
        let quietest = powers_db.iter().cloned().fold(f64::MAX, f64::min);
        assert!(
            loudest - quietest < tolerance_db,
            "{mode:?}: power varies {:.3} dB around the circle (limit {tolerance_db} dB): {powers_db:?}",
            loudest - quietest
        );
    }
}
