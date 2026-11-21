//! Tests of telemetry: the frame, channel peaks and the JSON.

use super::*;
use crate::audio::decode::AudioBuffer;
use crate::audio::sound_profile::SoundProfile;
use crate::audio::test_node::TestNode;
use atrium_core::listener::Listener;
use atrium_core::panner::DistanceModelType;
use atrium_core::types::Vec3;
use std::sync::Arc;

/// 1 kHz sine tone, 1 second.
fn sine_buffer() -> Arc<AudioBuffer> {
    let sr = 48000.0_f32;
    let n = sr as usize;
    let samples: Vec<f32> = (0..n)
        .map(|i| (2.0 * std::f32::consts::PI * 1000.0 * i as f32 / sr).sin())
        .collect();
    let rms = (samples.iter().map(|s| s * s).sum::<f32>() / n as f32).sqrt();
    let spectral_profile = crate::audio::spectral_profile::compute_profile(&samples, sr as u32);
    Arc::new(AudioBuffer {
        samples,
        sample_rate: sr as u32,
        rms,
        spectral_profile,
    })
}

fn make_source(
    buf: &Arc<AudioBuffer>,
    spl: f32,
    pos: Vec3,
    max_source_spl: f32,
) -> Box<dyn SoundSource> {
    let profile = SoundProfile { reference_spl: spl };
    let amplitude = profile.amplitude(buf.rms, 0.1, max_source_spl);
    let ref_dist = profile.ref_distance(1.0);
    let mut node = TestNode::new(Arc::clone(buf), pos, 0.0, 0.0);
    node.amplitude = amplitude;
    node.ref_dist = ref_dist;
    Box::new(node)
}

fn listener_at(pos: Vec3) -> Listener {
    Listener::new(pos, 0.0)
}

fn default_distance() -> DistanceModel {
    DistanceModel {
        ref_distance: 1.0,
        max_distance: 100.0,
        rolloff: 1.0,
        model: DistanceModelType::Inverse,
    }
}

struct FieldAnchor(Vec3);

impl SoundSource for FieldAnchor {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        0.0
    }

    fn position(&self) -> Vec3 {
        self.0
    }

    fn emitter_kind(&self) -> EmitterKind {
        EmitterKind::Field
    }

    fn tick(&mut self, _dt: f32) {}
}

// ── Telemetry gain component tests ──────────────────────────────────

#[test]
fn telemetry_distance_matches_geometry() {
    let buf = sine_buffer();
    let listener = listener_at(Vec3::ZERO);
    let dist = default_distance();
    let sources: Vec<Box<dyn SoundSource>> =
        vec![make_source(&buf, 80.0, Vec3::new(5.0, 0.0, 0.0), 100.0)];
    let frame = compute_telemetry(&sources, &listener, &dist);
    assert!((frame.sources[0].distance - 5.0).abs() < 0.01);
}

/// A map that draws which way the listener faces reads it from here, so
/// it shows the heading the audio thread renders with.
#[test]
fn frame_reports_the_listener_heading() {
    let heading = 2.5;
    let listener = Listener::new(Vec3::new(3.0, 4.0, 1.2), heading);
    let frame = compute_telemetry(&[], &listener, &default_distance());
    assert_eq!(frame.listener_yaw, heading);
}

#[test]
fn field_telemetry_uses_anchor_position_without_point_attenuation() {
    let listener = Listener::new(Vec3::ZERO, 0.0);
    let sources: Vec<Box<dyn SoundSource>> =
        vec![Box::new(FieldAnchor(Vec3::new(90.0, -40.0, 12.0)))];
    let frame = compute_telemetry(&sources, &listener, &default_distance());
    let source = &frame.sources[0];
    assert!(source.distance > 90.0, "anchor distance remains observable");
    assert_eq!(source.gain_dist, 1.0);
    assert_eq!(source.gain_emit, 1.0);
    assert_eq!(source.gain_total, 1.0);
}

#[test]
fn telemetry_omni_sources_have_emit_one() {
    let buf = sine_buffer();
    let listener = listener_at(Vec3::ZERO);
    let dist = default_distance();
    let sources: Vec<Box<dyn SoundSource>> =
        vec![make_source(&buf, 80.0, Vec3::new(3.0, 2.0, 0.0), 100.0)];
    let frame = compute_telemetry(&sources, &listener, &dist);
    assert!((frame.sources[0].gain_emit - 1.0).abs() < 0.001);
}

/// Two sources at 20 dB and 40 dB, same distance, same position.
/// The telemetry gain components should be identical (distance, emit
/// depend on position/orientation, not SPL). The SPL difference only shows
/// up in the amplitude (applied per-sample) and ref_distance (distance model).
///
/// This test verifies that the UI's received SPL formula
///   `spl - 20·log₁₀(distance) + 20·log₁₀(emit)`
/// correctly preserves the 20 dB difference.
#[test]
fn telemetry_spl_difference_preserved_at_same_distance() {
    let buf = sine_buffer();
    let pos = Vec3::new(5.0, 0.0, 0.0);
    let listener = listener_at(Vec3::ZERO);
    let dist = default_distance();

    // Source A: 20 dB SPL
    let source_a = make_source(&buf, 20.0, pos, 100.0);
    // Source B: 40 dB SPL
    let source_b = make_source(&buf, 40.0, pos, 100.0);

    let sources_a: Vec<Box<dyn SoundSource>> = vec![source_a];
    let sources_b: Vec<Box<dyn SoundSource>> = vec![source_b];

    let frame_a = compute_telemetry(&sources_a, &listener, &dist);
    let frame_b = compute_telemetry(&sources_b, &listener, &dist);

    let ta = &frame_a.sources[0];
    let tb = &frame_b.sources[0];

    // Both at same distance
    assert!((ta.distance - tb.distance).abs() < 0.01);

    // Emit should be identical (both omni)
    assert!((ta.gain_emit - tb.gain_emit).abs() < 0.001);

    // UI formula: received_spl = spl - 20*log10(distance) + 20*log10(emit)
    // Since emit=1 for both, and distance is the same:
    //   received_a = 20 - 20*log10(5) ≈ 20 - 14.0 = 6.0
    //   received_b = 40 - 20*log10(5) ≈ 40 - 14.0 = 26.0
    //   difference = 20 dB ✓
    let d = ta.distance.max(1.0);
    let received_a = 20.0 - 20.0 * d.log10();
    let received_b = 40.0 - 20.0 * d.log10();
    let spl_diff = received_b - received_a;
    assert!(
        (spl_diff - 20.0).abs() < 0.1,
        "SPL difference should be 20 dB, got {spl_diff:.1}"
    );
}

/// The UI display formula should give physically correct inverse square law:
/// doubling the distance → -6 dB.
#[test]
fn telemetry_inverse_square_law_6db_per_doubling() {
    let buf = sine_buffer();
    let listener = listener_at(Vec3::ZERO);
    let dist = default_distance();
    let spl = 80.0;

    let sources_near: Vec<Box<dyn SoundSource>> =
        vec![make_source(&buf, spl, Vec3::new(5.0, 0.0, 0.0), 100.0)];
    let sources_far: Vec<Box<dyn SoundSource>> =
        vec![make_source(&buf, spl, Vec3::new(10.0, 0.0, 0.0), 100.0)];

    let frame_near = compute_telemetry(&sources_near, &listener, &dist);
    let frame_far = compute_telemetry(&sources_far, &listener, &dist);

    let d_near = frame_near.sources[0].distance.max(1.0);
    let d_far = frame_far.sources[0].distance.max(1.0);

    // UI formula
    let received_near = spl - 20.0 * d_near.log10();
    let received_far = spl - 20.0 * d_far.log10();
    let drop = received_near - received_far;

    assert!(
        (drop - 6.0).abs() < 0.1,
        "Doubling distance should drop 6 dB, got {drop:.1}"
    );
}

/// Find the distance at which campfire (55 dB) and purring (30 dB) each
/// produce exactly 45 dB SPL received, using the real engine telemetry path.
///
/// Inverse distance model (ref_dist=1, rolloff=1, omni source, omni listener):
///   received = reference_spl + gain_db
///   gain_db  = 20·log₁₀(1/d) = -20·log₁₀(d)   (for d ≥ 1)
///
/// Solving: d = 10^((reference_spl - target) / 20)
///   Campfire: 10^((55 - 45)/20) = 10^0.5  ≈ 3.162 m  (move away)
///   Purring:  10^((30 - 45)/20) = 10^-0.75 ≈ 0.178 m  (move closer, near-field boost)
#[test]
fn distance_for_45db_received_campfire_and_purring() {
    let buf = sine_buffer();
    let dist_model = default_distance();
    let target_received = 45.0_f32;

    for (name, spl, expected_distance) in [
        ("campfire", 55.0_f32, 10.0_f32.powf(0.5)),  // ≈ 3.162 m
        ("purring", 30.0_f32, 10.0_f32.powf(-0.75)), // ≈ 0.178 m
    ] {
        // Place source at the analytically derived distance along +X
        let source_pos = Vec3::new(expected_distance, 0.0, 0.0);
        let listener = listener_at(Vec3::ZERO);
        let sources: Vec<Box<dyn SoundSource>> = vec![make_source(&buf, spl, source_pos, 100.0)];

        let frame = compute_telemetry(&sources, &listener, &dist_model);
        let telemetry = &frame.sources[0];

        // Verify distance
        assert!(
            (telemetry.distance - expected_distance).abs() < 0.01,
            "{name}: distance should be {expected_distance:.3}, got {:.3}",
            telemetry.distance,
        );

        // Verify received SPL = reference_spl + gain_db ≈ 45 dB
        let received = spl + telemetry.gain_db;
        assert!(
            (received - target_received).abs() < 0.5,
            "{name}: received should be ~{target_received} dB SPL, got {received:.1} \
                 (spl={spl}, gain_db={:.1}, dist={:.3}, gain_dist={:.4})",
            telemetry.gain_db,
            telemetry.distance,
            telemetry.gain_dist,
        );

        println!(
            "  {name}: {spl:.0} dB SPL @ {:.3}m → received {received:.1} dB SPL \
                 (gain_db={:.1}, dist_gain={:.4})",
            telemetry.distance, telemetry.gain_db, telemetry.gain_dist,
        );
    }
}

/// Verify that at their respective 45 dB distances, campfire and purring
/// produce the SAME actual digital audio amplitude.
///
/// The audio thread multiplies: output = sample × source_amplitude × gain_total
/// If the received SPL is the same, the digital amplitude must be the same.
#[test]
fn actual_audio_amplitude_matches_at_equal_received_spl() {
    let buf = sine_buffer();
    let dist_model = default_distance();
    let spl_reference = 94.0_f32; // IEC 61672
    let target_rms = 0.1;

    // Distances that give exactly 45 dB received (from the test above)
    let campfire_distance = 10.0_f32.powf(0.5); // 3.162 m
    let purring_distance = 10.0_f32.powf(-0.75); // 0.178 m

    let campfire_profile = SoundProfile {
        reference_spl: 55.0,
    };
    let purring_profile = SoundProfile {
        reference_spl: 30.0,
    };

    let campfire_amp = campfire_profile.amplitude(buf.rms, target_rms, spl_reference);
    let purring_amp = purring_profile.amplitude(buf.rms, target_rms, spl_reference);

    let listener = listener_at(Vec3::ZERO);

    let campfire_sources: Vec<Box<dyn SoundSource>> = vec![make_source(
        &buf,
        55.0,
        Vec3::new(campfire_distance, 0.0, 0.0),
        spl_reference,
    )];
    let purring_sources: Vec<Box<dyn SoundSource>> = vec![make_source(
        &buf,
        30.0,
        Vec3::new(purring_distance, 0.0, 0.0),
        spl_reference,
    )];

    let campfire_frame = compute_telemetry(&campfire_sources, &listener, &dist_model);
    let purring_frame = compute_telemetry(&purring_sources, &listener, &dist_model);

    let campfire_output = campfire_amp * campfire_frame.sources[0].gain_total;
    let purring_output = purring_amp * purring_frame.sources[0].gain_total;

    println!(
        "  campfire: amp={campfire_amp:.6} × gain={:.4} = output {campfire_output:.8}",
        campfire_frame.sources[0].gain_total
    );
    println!(
        "  purring:  amp={purring_amp:.6} × gain={:.4} = output {purring_output:.8}",
        purring_frame.sources[0].gain_total
    );
    println!(
        "  ratio: {:.4} (should be 1.0)",
        campfire_output / purring_output
    );

    // If both receive 45 dB SPL, their digital output must be equal
    assert!(
        (campfire_output - purring_output).abs() / campfire_output.max(1e-10) < 0.01,
        "At equal received SPL, digital amplitudes should match: \
             campfire={campfire_output:.8}, purring={purring_output:.8}"
    );
}

#[test]
fn telemetry_json_roundtrips() {
    let buf = sine_buffer();
    let listener = listener_at(Vec3::ZERO);
    let dist = default_distance();
    let sources: Vec<Box<dyn SoundSource>> =
        vec![make_source(&buf, 80.0, Vec3::new(3.0, 0.0, 0.0), 100.0)];
    let frame = compute_telemetry(&sources, &listener, &dist);
    let json = telemetry_to_json(&frame);

    // Should be valid JSON with expected structure
    let parsed: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(parsed["type"], "telemetry");
    assert_eq!(parsed["sources"].as_array().unwrap().len(), 1);
    let src = &parsed["sources"][0];
    assert!((src["x"].as_f64().unwrap() - 3.0).abs() < 0.1);
    assert!((src["distance"].as_f64().unwrap() - 3.0).abs() < 0.1);
    assert!(src["dist"].as_f64().unwrap() > 0.0);
    assert!((src["emit"].as_f64().unwrap() - 1.0).abs() < 0.01); // omni
}

/// A control surface reads the engine's real state from the stream: the
/// active render mode and channel mode by their wire names, the master
/// gain, and one peak
/// per output channel in dBFS. Peak 0.5 → 20·log₁₀ 0.5 = −6.0 dB; a
/// silent channel reads the meter floor, −120 dB.
#[test]
fn telemetry_json_reports_modes_and_channel_peaks() {
    use atrium_core::speaker::{ChannelMode, RenderMode};
    let mut frame = TelemetryFrame {
        render_mode: RenderMode::Dbap,
        channel_mode: ChannelMode::Quad,
        channel_count: 3,
        master_gain: 0.25,
        ..TelemetryFrame::default()
    };
    frame.channel_peaks[0] = 1.0;
    frame.channel_peaks[1] = 0.5;
    frame.channel_peaks[2] = 0.0;

    let parsed: serde_json::Value = serde_json::from_str(&telemetry_to_json(&frame)).unwrap();
    assert_eq!(parsed["render_mode"], "dbap");
    assert_eq!(parsed["channel_mode"], "quad");
    assert_eq!(parsed["master_gain"], 0.25);
    let peaks: Vec<f64> = parsed["peaks_db"]
        .as_array()
        .unwrap()
        .iter()
        .map(|peak| peak.as_f64().unwrap())
        .collect();
    assert_eq!(peaks, vec![0.0, -6.0, -120.0]);
}
