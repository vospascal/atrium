use crate::world::types::Vec3;

/// Commands sent from the control thread to the audio thread via rtrb ring buffer.
/// All variants must be small and Copy — no heap allocations.
#[derive(Clone, Copy, Debug)]
pub enum Command {
    /// Update the listener's position and orientation.
    SetListenerPose { position: Vec3, yaw: f32 },

    /// Set master output gain (0.0 to 1.0).
    SetMasterGain { gain: f32 },

    /// Mute or unmute a source by index.
    SetSourceMuted { index: u8, muted: bool },

    /// Reposition a source by index.
    SetSourcePosition { index: u8, position: Vec3 },
    // Future:
    // AddSource { id: u32, source_type: SourceType, position: Vec3 },
    // RemoveSource { id: u32 },
    // SetRoomGeometry { ... },
}
