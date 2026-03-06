use crate::world::types::Vec3;
use atrium_core::speaker::RenderMode;

/// Commands sent from the control thread to the audio thread via rtrb ring buffer.
/// All variants must be small and Copy — no heap allocations.
///
/// Source indices are `u16` to accommodate both real sources and virtual sources
/// (e.g. reflection images from ray-traced audio), which can multiply quickly.
#[derive(Clone, Copy, Debug)]
pub enum Command {
    /// Update the listener's position and orientation.
    SetListenerPose { position: Vec3, yaw: f32 },

    /// Set master output gain (0.0 to 1.0).
    SetMasterGain { gain: f32 },

    /// Mute or unmute a source by index.
    SetSourceMuted { index: u16, muted: bool },

    /// Reposition a source by index.
    SetSourcePosition { index: u16, position: Vec3 },

    /// Switch rendering mode (SpeakerAsMic or Vbap).
    SetRenderMode { mode: RenderMode },

    /// Reposition a speaker by channel index.
    SetSpeakerPosition { channel: u8, position: Vec3 },

    /// Set MDAP spread for a source (0.0 = point, 1.0 = full hemisphere).
    SetSourceSpread { index: u16, spread: f32 },

    /// Set orbit speed for a source (0 = paused).
    SetSourceOrbitSpeed { index: u16, speed: f32 },

    /// Set orbit radius for a source.
    SetSourceOrbitRadius { index: u16, radius: f32 },

    /// Set orbit angle for a source.
    SetSourceOrbitAngle { index: u16, angle: f32 },

    /// Set atmospheric conditions for ISO 9613-1 air absorption.
    SetAtmosphere { temperature_c: f32, humidity_pct: f32 },

    /// Reset the scene to its initial state (positions, orbits, gains, etc.).
    ResetScene,
    // Future:
    // AddSource { id: u32, source_type: SourceType, position: Vec3 },
    // RemoveSource { id: u32 },
    // SetRoomGeometry { ... },
}
