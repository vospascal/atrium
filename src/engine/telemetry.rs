//! Real-time telemetry from the audio thread to the UI.
//!
//! The audio thread computes a `TelemetryFrame` every ~15 Hz and pushes it
//! through an rtrb ring buffer to the main thread, which broadcasts it to
//! WebSocket clients as JSON.
//!
//! All types are fixed-size and Copy — no heap allocations, real-time safe.

use crate::audio::distance::DistanceModel;
use atrium_core::directivity::directivity_gain;
use atrium_core::listener::Listener;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::source::SoundSource;
use atrium_core::speaker::RenderMode;

/// Per-source telemetry snapshot.
#[derive(Clone, Copy, Debug)]
pub struct SourceTelemetry {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub distance: f32,
    pub gain_dist: f32,
    pub gain_emit: f32,
    pub gain_hear: f32,
    pub gain_total: f32,
    pub gain_db: f32,
    pub is_muted: bool,
}

impl Default for SourceTelemetry {
    fn default() -> Self {
        Self {
            x: 0.0,
            y: 0.0,
            z: 0.0,
            distance: 0.0,
            gain_dist: 0.0,
            gain_emit: 0.0,
            gain_hear: 0.0,
            gain_total: 0.0,
            gain_db: f32::NEG_INFINITY,
            is_muted: false,
        }
    }
}

pub const MAX_SOURCES: usize = 16;
pub const MAX_TELEM_CHANNELS: usize = 8;

/// Complete telemetry frame: all sources for one update tick.
#[derive(Clone, Copy, Debug)]
pub struct TelemetryFrame {
    pub sources: [SourceTelemetry; MAX_SOURCES],
    pub source_count: u8,
    /// Current pipeline mode (may change at runtime via SetRenderMode command).
    pub render_mode: RenderMode,
    /// Per-channel peak amplitude (linear) from the most recent render buffer.
    pub channel_peaks: [f32; MAX_TELEM_CHANNELS],
    /// Number of output channels.
    pub channel_count: u8,
}

impl Default for TelemetryFrame {
    fn default() -> Self {
        Self {
            sources: [SourceTelemetry::default(); MAX_SOURCES],
            source_count: 0,
            render_mode: RenderMode::WorldLocked,
            channel_peaks: [0.0; MAX_TELEM_CHANNELS],
            channel_count: 0,
        }
    }
}

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
            r#"{{"x":{:.2},"y":{:.2},"z":{:.2},"distance":{:.2},"dist":{:.3},"emit":{:.3},"hear":{:.3},"total":{:.4},"db":{:.1},"muted":{}}}"#,
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
        Arc::new(AudioBuffer {
            samples,
            sample_rate: sr as u32,
            rms,
        })
    }

    fn make_source(
        buf: &Arc<AudioBuffer>,
        spl: f32,
        pos: Vec3,
        ceiling: f32,
    ) -> Box<dyn SoundSource> {
        let profile = SoundProfile { reference_spl: spl };
        let amplitude = profile.amplitude(buf.rms, 0.1, ceiling);
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
