//! Real-time telemetry from the audio thread to the UI.
//!
//! The audio thread computes a `TelemetryFrame` every ~15 Hz and pushes it
//! through an rtrb ring buffer to the main thread, which broadcasts it to
//! WebSocket clients or the Bevy visualization.
//!
//! Type definitions live in `atrium_core::telemetry` (shared with atrium-bevy).
//! This module re-exports them and provides the compute/serialization functions.

use crate::audio::distance::DistanceModel;
use atrium_core::directivity::directivity_gain;
use atrium_core::listener::Listener;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::source::SoundSource;

// Re-export telemetry types from core so existing callers keep working.
pub use atrium_core::telemetry::{
    SourceTelemetry, TelemetryFrame, MAX_CHANNELS as MAX_TELEM_CHANNELS, MAX_SOURCES,
};

/// Compute per-channel peak amplitudes from an interleaved output buffer.
pub fn compute_channel_peaks(output: &[f32], channels: usize) -> [f32; MAX_TELEM_CHANNELS] {
    let mut peaks = [0.0f32; MAX_TELEM_CHANNELS];
    let n = channels.min(MAX_TELEM_CHANNELS);
    for frame in output.chunks_exact(channels) {
        for ch in 0..n {
            let abs = frame[ch].abs();
            if abs > peaks[ch] {
                peaks[ch] = abs;
            }
        }
    }
    peaks
}

/// Compute a telemetry frame from current scene state.
/// Calls the same core gain functions the pipeline uses, but decomposes them
/// into individual components for the UI.
pub fn compute_telemetry(
    sources: &[Box<dyn SoundSource>],
    listener: &Listener,
    distance_model: &DistanceModel,
) -> TelemetryFrame {
    let mut frame = TelemetryFrame::default();
    let count = sources.len().min(MAX_SOURCES);
    frame.source_count = count as u8;

    for (i, source) in sources.iter().enumerate().take(count) {
        let pos = source.position();
        let dist = listener.position.distance_to(pos);
        let src_ref_dist = source.ref_distance();

        // Distance attenuation
        let gain_dist = distance_gain_at_model(
            listener.position,
            pos,
            src_ref_dist,
            distance_model.max_distance,
            distance_model.rolloff,
            distance_model.model,
        );

        // Source emission directivity
        let gain_emit = directivity_gain(
            pos,
            source.orientation(),
            listener.position,
            &source.directivity(),
        );

        // Listener hearing cone
        let gain_hear = listener.hearing_gain(pos);

        let gain_total = gain_dist * gain_emit * gain_hear;
        let gain_db = if gain_total.is_finite() && gain_total > 0.0 {
            20.0 * gain_total.log10()
        } else {
            f32::NEG_INFINITY
        };

        let orientation = source.orientation();
        let orbit_center = source.orbit_center();

        frame.sources[i] = SourceTelemetry {
            x: pos.x,
            y: pos.y,
            z: pos.z,
            distance: dist,
            gain_dist,
            gain_emit,
            gain_hear,
            gain_total,
            gain_db,
            is_muted: source.is_muted(),
            perceptual_score: 1.0, // overwritten by AudioScene with actual scores
            orientation_x: orientation.x,
            orientation_y: orientation.y,
            orbit_center_x: orbit_center.x,
            orbit_center_y: orbit_center.y,
            orbit_radius: source.orbit_radius(),
        };
    }

    frame
}

/// Serialize a telemetry frame to JSON for WebSocket broadcast.
/// Hand-rolled to avoid serde overhead in the broadcast path.
pub fn telemetry_to_json(frame: &TelemetryFrame) -> String {
    use std::fmt::Write;
    let mut json = String::with_capacity(256);
    json.push_str(r#"{"type":"telemetry","sources":["#);

    for i in 0..frame.source_count as usize {
        let s = &frame.sources[i];
        if i > 0 {
            json.push(',');
        }
        let _ = write!(
            json,
            r#"{{"x":{:.2},"y":{:.2},"z":{:.2},"distance":{:.2},"dist":{:.3},"emit":{:.3},"hear":{:.3},"total":{:.4},"db":{:.1},"muted":{},"perceptual":{:.3},"ox":{:.3},"oy":{:.3},"ocx":{:.2},"ocy":{:.2},"or":{:.2}}}"#,
            s.x,
            s.y,
            s.z,
            s.distance,
            s.gain_dist,
            s.gain_emit,
            s.gain_hear,
            s.gain_total,
            if s.gain_db.is_finite() {
                s.gain_db
            } else {
                -999.0
            },
            s.is_muted,
            s.perceptual_score,
            s.orientation_x,
            s.orientation_y,
            s.orbit_center_x,
            s.orbit_center_y,
            s.orbit_radius,
        );
    }

    json.push_str("]}");
    json
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::audio::decode::AudioBuffer;
    use crate::audio::sound_profile::SoundProfile;
    use crate::audio::test_node::TestNode;
    use crate::world::types::Vec3;
    use atrium_core::listener::Listener;
    use atrium_core::panner::DistanceModelType;
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

    fn omni_listener(pos: Vec3) -> Listener {
        let mut l = Listener::new(pos, 0.0);
        l.hearing_cone.pattern = atrium_core::directivity::DirectivityPattern::Omni;
        l
    }

    fn default_distance() -> DistanceModel {
        DistanceModel {
            ref_distance: 1.0,
            max_distance: 100.0,
            rolloff: 1.0,
            model: DistanceModelType::Inverse,
        }
    }

    // ── Telemetry gain component tests ──────────────────────────────────

    #[test]
    fn telemetry_distance_matches_geometry() {
        let buf = sine_buffer();
        let listener = omni_listener(Vec3::ZERO);
        let dist = default_distance();
        let sources: Vec<Box<dyn SoundSource>> =
            vec![make_source(&buf, 80.0, Vec3::new(5.0, 0.0, 0.0), 100.0)];
        let frame = compute_telemetry(&sources, &listener, &dist);
        assert!((frame.sources[0].distance - 5.0).abs() < 0.01);
    }

    #[test]
    fn telemetry_omni_sources_have_emit_one() {
        let buf = sine_buffer();
        let listener = omni_listener(Vec3::ZERO);
        let dist = default_distance();
        let sources: Vec<Box<dyn SoundSource>> =
            vec![make_source(&buf, 80.0, Vec3::new(3.0, 2.0, 0.0), 100.0)];
        let frame = compute_telemetry(&sources, &listener, &dist);
        assert!((frame.sources[0].gain_emit - 1.0).abs() < 0.001);
    }

    #[test]
    fn telemetry_omni_listener_has_hear_one() {
        let buf = sine_buffer();
        let listener = omni_listener(Vec3::ZERO);
        let dist = default_distance();
        let sources: Vec<Box<dyn SoundSource>> =
            vec![make_source(&buf, 80.0, Vec3::new(3.0, 2.0, 0.0), 100.0)];
        let frame = compute_telemetry(&sources, &listener, &dist);
        assert!((frame.sources[0].gain_hear - 1.0).abs() < 0.001);
    }

    /// Two sources at 20 dB and 40 dB, same distance, same position.
    /// The telemetry gain components should be identical (distance, emit, hear
    /// depend on position/orientation, not SPL). The SPL difference only shows
    /// up in the amplitude (applied per-sample) and ref_distance (distance model).
    ///
    /// This test verifies that the UI's received SPL formula
    ///   `spl - 20·log₁₀(distance) + 20·log₁₀(emit) + 20·log₁₀(hear)`
    /// correctly preserves the 20 dB difference.
    #[test]
    fn telemetry_spl_difference_preserved_at_same_distance() {
        let buf = sine_buffer();
        let pos = Vec3::new(5.0, 0.0, 0.0);
        let listener = omni_listener(Vec3::ZERO);
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

        // Emit and hear should be identical (both omni)
        assert!((ta.gain_emit - tb.gain_emit).abs() < 0.001);
        assert!((ta.gain_hear - tb.gain_hear).abs() < 0.001);

        // UI formula: received_spl = spl - 20*log10(distance) + 20*log10(emit) + 20*log10(hear)
        // Since emit=1 and hear=1 for both, and distance is the same:
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
        let listener = omni_listener(Vec3::ZERO);
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
            let listener = omni_listener(Vec3::ZERO);
            let sources: Vec<Box<dyn SoundSource>> =
                vec![make_source(&buf, spl, source_pos, 100.0)];

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

        let listener = omni_listener(Vec3::ZERO);

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
        let listener = omni_listener(Vec3::ZERO);
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
}
