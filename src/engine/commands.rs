use crate::world::types::Vec3;

/// Commands sent from the control thread to the audio thread via rtrb ring buffer.
/// All variants must be small and Copy — no heap allocations.
#[derive(Clone, Copy, Debug)]
pub enum Command {
    /// Update the listener's position and orientation.
    SetListenerPose { position: Vec3, yaw: f32 },

    /// Set master output gain (0.0 to 1.0).
    SetMasterGain { gain: f32 },
    // Future:
    // AddSource { id: u32, source_type: SourceType, position: Vec3 },
    // RemoveSource { id: u32 },
    // SetSourcePosition { id: u32, position: Vec3 },
    // SetRoomGeometry { ... },
}
