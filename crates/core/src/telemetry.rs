//! Telemetry frame types shared between the audio engine and visualization clients.
//!
//! All types are fixed-size and Copy — no heap allocations, real-time safe.
//! These live in `atrium-core` so the engine (`atrium`) and any
//! visualization can use the same concrete types over rtrb.

use crate::speaker::{ChannelMode, RenderMode};

pub const MAX_SOURCES: usize = 16;
pub const MAX_CHANNELS: usize = 8;

/// Short-time measurement of a source's rendered mono signal, before spatial effects.
#[derive(Clone, Copy, Debug, Default)]
pub struct SourceSignal {
    /// RMS relative to the source's calibrated RMS when the scene was built.
    pub relative_rms: f32,
    /// Share of measured energy below roughly 350 Hz.
    pub low_share: f32,
    /// Share of measured energy above roughly 2.4 kHz. The remainder is midrange.
    pub high_share: f32,
}

/// Per-source telemetry snapshot.
#[derive(Clone, Copy, Debug)]
pub struct SourceTelemetry {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub distance: f32,
    pub gain_dist: f32,
    pub gain_emit: f32,
    pub gain_total: f32,
    pub gain_db: f32,
    pub is_muted: bool,
    /// The signal meter, where the scene source provides one.
    pub signal: Option<SourceSignal>,
    /// Which of its takes the source is uttering, or `None` while it rests
    /// between two. A source that never rests is always on take 0.
    pub sounding_take: Option<u8>,
    /// Perceptual score [0, 1] from masking/salience analysis.
    pub perceptual_score: f32,
    /// Source facing direction (unit vector).
    pub orientation_x: f32,
    pub orientation_y: f32,
    /// Orbit center position.
    pub orbit_center_x: f32,
    pub orbit_center_y: f32,
    /// Orbit radius (meters). 0 = stationary.
    pub orbit_radius: f32,
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
            gain_total: 0.0,
            gain_db: f32::NEG_INFINITY,
            is_muted: false,
            signal: None,
            sounding_take: Some(0),
            perceptual_score: 1.0,
            orientation_x: 1.0,
            orientation_y: 0.0,
            orbit_center_x: 0.0,
            orbit_center_y: 0.0,
            orbit_radius: 0.0,
        }
    }
}

/// Complete telemetry frame: all sources for one update tick.
#[derive(Clone, Copy, Debug)]
pub struct TelemetryFrame {
    pub sources: [SourceTelemetry; MAX_SOURCES],
    pub source_count: u8,
    /// Bit `i` set = slot `i` holds a live source. With the 16-slot pool, slots
    /// can be sparse (a removed source leaves a gap), so consumers must gate on
    /// this mask rather than assuming `0..source_count` are all present.
    pub active_mask: u16,
    /// Current pipeline mode (may change at runtime via SetRenderMode command).
    pub render_mode: RenderMode,
    /// Current speaker configuration.
    pub channel_mode: ChannelMode,
    /// Atmospheric temperature (°C).
    pub temperature_c: f32,
    /// Atmospheric humidity (%).
    pub humidity_pct: f32,
    /// Per-channel peak amplitude (linear) from the most recent render buffer.
    pub channel_peaks: [f32; MAX_CHANNELS],
    /// Number of output channels.
    pub channel_count: u8,
    /// Master output gain in effect (linear).
    pub master_gain: f32,
    /// Listener heading in radians: 0 = facing +X, π/2 = facing +Y. It moves
    /// with the head while a head tracker steers the listener.
    pub listener_yaw: f32,
}

impl Default for TelemetryFrame {
    fn default() -> Self {
        Self {
            sources: [SourceTelemetry::default(); MAX_SOURCES],
            source_count: 0,
            active_mask: 0,
            render_mode: RenderMode::WorldLocked,
            channel_mode: ChannelMode::Surround51,
            temperature_c: 20.0,
            humidity_pct: 50.0,
            channel_peaks: [0.0; MAX_CHANNELS],
            channel_count: 0,
            master_gain: 1.0,
            listener_yaw: 0.0,
        }
    }
}
