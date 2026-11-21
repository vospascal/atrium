use crate::directivity::DirectivityPattern;
use crate::speaker::{ChannelMode, RenderMode};
use crate::types::Vec3;

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

    /// Switch rendering mode (WorldLocked / Vbap / Hrtf).
    SetRenderMode { mode: RenderMode },

    /// Reposition a speaker by channel index.
    SetSpeakerPosition { channel: u8, position: Vec3 },

    /// Set MDAP spread for a source (0.0 = point, 1.0 = full hemisphere).
    SetSourceSpread { index: u16, spread: f32 },

    /// Set a source's reference SPL (dB at 1 m). The audio thread recomputes the
    /// source's base amplitude in place, so loudness changes live without a
    /// scene rebuild.
    SetSourceSpl { index: u16, spl: f32 },

    /// Set a source's directivity emission pattern.
    SetSourceDirectivity {
        index: u16,
        pattern: DirectivityPattern,
    },

    /// Turn a source to face `yaw`: radians, 0 = facing +X, π/2 = facing +Y,
    /// like the listener's heading. It matters for a source that radiates
    /// more one way than another.
    SetSourceFacing { index: u16, yaw: f32 },

    /// Set how long a source with takes rests between two (s). A source
    /// without takes ignores it.
    SetSourceGap {
        index: u16,
        shortest: f32,
        longest: f32,
    },

    /// Set how many times running a source with takes utters one before it
    /// chooses another, fewest and most. A source without takes ignores it.
    SetSourceBout { index: u16, fewest: u32, most: u32 },

    /// Set how often take `take` of a source is chosen next to its others.
    /// A source without that take ignores it.
    SetTakeChance { index: u16, take: u8, chance: f32 },

    /// Set orbit speed for a source (0 = paused).
    SetSourceOrbitSpeed { index: u16, speed: f32 },

    /// Set orbit radius for a source.
    SetSourceOrbitRadius { index: u16, radius: f32 },

    /// Set orbit angle for a source.
    SetSourceOrbitAngle { index: u16, angle: f32 },

    /// Set channel mode (stereo, quad, 5.1). Applies speaker mask.
    SetChannelMode { mode: ChannelMode },

    /// Set atmospheric conditions for ISO 9613-1 air absorption.
    SetAtmosphere {
        temperature_c: f32,
        humidity_pct: f32,
    },

    /// Set a procedural-synth parameter on a source live (no rebuild). Sources
    /// that don't model the parameter ignore it.
    SetSynthParam {
        index: u16,
        param: SynthParam,
        value: f32,
    },

    /// Reset the scene to its initial state (positions, orbits, gains, etc.).
    ResetScene,
}

/// A live-tunable procedural-synth parameter. A `SoundSource` interprets only
/// the controls it models and ignores the rest.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SynthParam {
    FlowSpeedMin,
    FlowSpeedMax,
    RiverSplashRate,
    MinSpeed,
    MaxSpeed,
    ChangeTimeMin,
    ChangeTimeMax,
    GustDurationMin,
    GustDurationMax,
    TurbulenceTimeMin,
    TurbulenceTimeMax,
    GustStrength,
    RiseBias,
    GustBrightness,
    TurbulenceBrightness,
    LowGain,
    BodyGain,
    MidGain,
    PresenceGain,
    AirGain,
    TurbulenceDepth,
    FoliageDensity,
    LeafDryness,
    BranchLevel,
    DebrisLevel,
    StructureLevel,
    MasterGain,
}
