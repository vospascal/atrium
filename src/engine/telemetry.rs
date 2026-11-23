//! Real-time telemetry from the audio thread to the UI.
//!
//! The audio thread computes a `TelemetryFrame` every ~15 Hz and pushes it
//! through an rtrb ring buffer to the main thread, which broadcasts it to
//! WebSocket clients.
//!
//! Type definitions live in `atrium_core::telemetry` (shared with the engine).
//! This module re-exports them and provides the compute/serialization functions.

use crate::audio::distance::DistanceModel;
use atrium_core::directivity::directivity_gain;
use atrium_core::listener::Listener;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::source::{EmitterKind, SoundSource};

use atrium_core::telemetry::{SourceTelemetry, TelemetryFrame, MAX_CHANNELS, MAX_SOURCES};

/// Compute per-channel peak amplitudes from an interleaved output buffer.
pub fn compute_channel_peaks(output: &[f32], channels: usize) -> [f32; MAX_CHANNELS] {
    let mut peaks = [0.0f32; MAX_CHANNELS];
    let n = channels.min(MAX_CHANNELS);
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
    frame.listener_yaw = listener.yaw;

    for (i, source) in sources.iter().enumerate().take(count) {
        // Mark this slot present in the active mask. Empty pool slots hold a
        // `SilenceNode` (`is_active() == false`) and stay clear.
        if source.is_active() {
            frame.active_mask |= 1 << i;
        }
        // Where it is heard from: a tone held beside the head is drawn there.
        let pos = atrium_core::source::sounding_position(source.as_ref(), listener);
        let dist = listener.position.distance_to(pos);
        let src_ref_dist = source.ref_distance();

        let (gain_dist, gain_emit) = if source.emitter_kind() == EmitterKind::Field {
            // Position remains useful as an editor anchor, but it has no
            // acoustic attenuation or directivity semantics for a field.
            (1.0, 1.0)
        } else {
            let gain_dist = distance_gain_at_model(
                listener.position,
                pos,
                src_ref_dist,
                distance_model.max_distance,
                distance_model.rolloff,
                distance_model.model,
            );
            let gain_emit = directivity_gain(
                pos,
                source.orientation(),
                listener.position,
                &source.directivity(),
            );
            (gain_dist, gain_emit)
        };

        let gain_total = gain_dist * gain_emit;
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
            gain_total,
            gain_db,
            is_muted: source.is_muted(),
            signal: source.signal(),
            sounding_take: source.sounding_take(),
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

/// Level reported for a silent channel, in dBFS.
const SILENT_PEAK_DB: f32 = -120.0;

/// A channel's linear peak in dBFS. Silence is reported as the meter floor
/// rather than −∞, so every consumer sees the same finite number.
pub fn peak_db(peak: f32) -> f32 {
    (20.0 * peak.max(1e-6).log10()).max(SILENT_PEAK_DB)
}

/// Serialize a telemetry frame to JSON for WebSocket broadcast.
/// Hand-rolled to avoid serde overhead in the broadcast path.
pub fn telemetry_to_json(frame: &TelemetryFrame) -> String {
    use std::fmt::Write;
    let mut json = String::with_capacity(256);
    json.push_str(r#"{"type":"telemetry","sources":["#);

    let mut written = 0;
    for i in 0..frame.source_count as usize {
        // Skip empty pool slots (sparse after live remove).
        if frame.active_mask & (1 << i) == 0 {
            continue;
        }
        let s = &frame.sources[i];
        if written > 0 {
            json.push(',');
        }
        written += 1;
        let _ = write!(
            json,
            r#"{{"x":{:.2},"y":{:.2},"z":{:.2},"distance":{:.2},"dist":{:.3},"emit":{:.3},"total":{:.4},"db":{:.1},"muted":{},"perceptual":{:.3},"ox":{:.3},"oy":{:.3},"ocx":{:.2},"ocy":{:.2},"or":{:.2}}}"#,
            s.x,
            s.y,
            s.z,
            s.distance,
            s.gain_dist,
            s.gain_emit,
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

    json.push(']');

    // What the engine is doing right now, so a control surface shows the
    // real state and not what it last asked for.
    let _ = write!(
        json,
        r#","render_mode":"{}","channel_mode":"{}","master_gain":{:.4},"peaks_db":["#,
        frame.render_mode.as_str(),
        frame.channel_mode.as_str(),
        frame.master_gain,
    );
    let channel_count = (frame.channel_count as usize).min(frame.channel_peaks.len());
    for (channel, peak) in frame.channel_peaks[..channel_count].iter().enumerate() {
        if channel > 0 {
            json.push(',');
        }
        let _ = write!(json, "{:.1}", peak_db(*peak));
    }
    json.push_str("]}");
    json
}

#[cfg(test)]
mod tests;
