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

    /// Pause or resume the whole scene. Paused, the session's clock stops,
    /// so every sound, the plan and the weather hold where they are; the
    /// sound fades out and back in so nothing clicks.
    SetPaused { paused: bool },

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
    /// How hard it rains, 0 to 1.
    Intensity,
    /// Drops per second at full intensity.
    DropRate,
    /// Large drips per second at full intensity.
    DripRate,
    /// Seconds from one wave to the next.
    WavePeriod,
    /// How likely a wave breaks with a crash, 0 to 1.
    CrashChance,
    /// Shortest and longest a flying source hovers before it moves on (s).
    HoverTimeMin,
    HoverTimeMax,
    /// How long a wind stays steady between two moves of its repertoire, as
    /// a share of its preset's rest: 1 as written, 0.2 five times as often.
    MoveRest,
    /// How strong a wind's moves are, as a share of its preset's: 1 as
    /// written, 2 twice as strong.
    MoveStrength,
}

impl SynthParam {
    /// Every setting, in the order they are declared.
    pub const ALL: [SynthParam; 36] = [
        SynthParam::FlowSpeedMin,
        SynthParam::FlowSpeedMax,
        SynthParam::RiverSplashRate,
        SynthParam::MinSpeed,
        SynthParam::MaxSpeed,
        SynthParam::ChangeTimeMin,
        SynthParam::ChangeTimeMax,
        SynthParam::GustDurationMin,
        SynthParam::GustDurationMax,
        SynthParam::TurbulenceTimeMin,
        SynthParam::TurbulenceTimeMax,
        SynthParam::GustStrength,
        SynthParam::RiseBias,
        SynthParam::GustBrightness,
        SynthParam::TurbulenceBrightness,
        SynthParam::LowGain,
        SynthParam::BodyGain,
        SynthParam::MidGain,
        SynthParam::PresenceGain,
        SynthParam::AirGain,
        SynthParam::TurbulenceDepth,
        SynthParam::FoliageDensity,
        SynthParam::LeafDryness,
        SynthParam::BranchLevel,
        SynthParam::DebrisLevel,
        SynthParam::StructureLevel,
        SynthParam::MasterGain,
        SynthParam::Intensity,
        SynthParam::DropRate,
        SynthParam::DripRate,
        SynthParam::WavePeriod,
        SynthParam::CrashChance,
        SynthParam::HoverTimeMin,
        SynthParam::HoverTimeMax,
        SynthParam::MoveRest,
        SynthParam::MoveStrength,
    ];

    /// The name a file uses for this setting, in snake case.
    pub fn name(self) -> &'static str {
        match self {
            SynthParam::FlowSpeedMin => "flow_speed_min",
            SynthParam::FlowSpeedMax => "flow_speed_max",
            SynthParam::RiverSplashRate => "river_splash_rate",
            SynthParam::MinSpeed => "min_speed",
            SynthParam::MaxSpeed => "max_speed",
            SynthParam::ChangeTimeMin => "change_time_min",
            SynthParam::ChangeTimeMax => "change_time_max",
            SynthParam::GustDurationMin => "gust_duration_min",
            SynthParam::GustDurationMax => "gust_duration_max",
            SynthParam::TurbulenceTimeMin => "turbulence_time_min",
            SynthParam::TurbulenceTimeMax => "turbulence_time_max",
            SynthParam::GustStrength => "gust_strength",
            SynthParam::RiseBias => "rise_bias",
            SynthParam::GustBrightness => "gust_brightness",
            SynthParam::TurbulenceBrightness => "turbulence_brightness",
            SynthParam::LowGain => "low_gain",
            SynthParam::BodyGain => "body_gain",
            SynthParam::MidGain => "mid_gain",
            SynthParam::PresenceGain => "presence_gain",
            SynthParam::AirGain => "air_gain",
            SynthParam::TurbulenceDepth => "turbulence_depth",
            SynthParam::FoliageDensity => "foliage_density",
            SynthParam::LeafDryness => "leaf_dryness",
            SynthParam::BranchLevel => "branch_level",
            SynthParam::DebrisLevel => "debris_level",
            SynthParam::StructureLevel => "structure_level",
            SynthParam::MasterGain => "master_gain",
            SynthParam::Intensity => "intensity",
            SynthParam::DropRate => "drop_rate",
            SynthParam::DripRate => "drip_rate",
            SynthParam::WavePeriod => "wave_period",
            SynthParam::CrashChance => "crash_chance",
            SynthParam::HoverTimeMin => "hover_time_min",
            SynthParam::HoverTimeMax => "hover_time_max",
            SynthParam::MoveRest => "move_rest",
            SynthParam::MoveStrength => "move_strength",
        }
    }

    /// The setting a file names, if there is one.
    pub fn named(name: &str) -> Option<Self> {
        Self::ALL.into_iter().find(|param| param.name() == name)
    }
}
