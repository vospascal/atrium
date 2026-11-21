// Which output channels exist on the rig (ChannelMode) and which rendering
// algorithm drives them (RenderMode).

/// Output channel configuration. Determines which output channels exist on
/// the rig: a masked channel has no speaker (or subwoofer) behind it, so
/// nothing may be sent to it. Works by masking channels of the layout.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ChannelMode {
    /// Stereo: channels 0 (L), 1 (R) only.
    Stereo,
    /// Quad 4.0: channels 0 (FL), 1 (FR), 4 (RL), 5 (RR). Center + LFE masked.
    Quad,
    /// Full 5.1 surround: channels 0 (L), 1 (R), 2 (C), 3 (LFE), 4 (Ls), 5 (Rs).
    /// No renderer pans into the LFE; it carries the bass that bass management
    /// redirects from the mains, which is why it must count as present here.
    Surround51,
}

impl ChannelMode {
    /// Output channel indices that exist in this mode, subwoofer included.
    pub fn active_channels(self) -> &'static [usize] {
        match self {
            ChannelMode::Stereo => &[0, 1],
            ChannelMode::Quad => &[0, 1, 4, 5],
            ChannelMode::Surround51 => &[0, 1, 2, 3, 4, 5],
        }
    }

    /// Valid channel modes for a given render mode.
    pub fn valid_for(mode: RenderMode) -> &'static [ChannelMode] {
        match mode {
            RenderMode::WorldLocked | RenderMode::Dbap | RenderMode::Ambisonics => &[
                ChannelMode::Stereo,
                ChannelMode::Quad,
                ChannelMode::Surround51,
            ],
            RenderMode::Vbap => &[ChannelMode::Quad, ChannelMode::Surround51],
            RenderMode::Hrtf => &[ChannelMode::Stereo],
        }
    }

    /// Wire format name for JSON serialization.
    pub fn as_str(self) -> &'static str {
        match self {
            ChannelMode::Stereo => "stereo",
            ChannelMode::Quad => "quad",
            ChannelMode::Surround51 => "5.1",
        }
    }

    /// Parse from wire format string.
    pub fn parse(s: &str) -> Option<ChannelMode> {
        match s {
            "stereo" => Some(ChannelMode::Stereo),
            "quad" => Some(ChannelMode::Quad),
            "5.1" => Some(ChannelMode::Surround51),
            _ => None,
        }
    }
}

/// Rendering algorithm.
///
/// Each mode is a distinct spatialization strategy. The output channel count
/// is determined by the SpeakerLayout, not the render mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum RenderMode {
    /// Speakers are virtual microphones at fixed world positions.
    /// Propagation is per-speaker (source→speaker path). Listener irrelevant.
    /// Valid layouts: stereo, quad, 5.1.
    WorldLocked,
    /// VBAP: listener-relative triangulated panning to physical speakers.
    /// Requires 3+ speakers for triangulation.
    /// Valid layouts: quad, 5.1.
    Vbap,
    /// HRTF convolution for headphones.
    /// Always outputs stereo (channels 0, 1).
    Hrtf,
    /// DBAP: distance-based amplitude panning. All speakers active, gains
    /// weighted by inverse distance from source. Listener-independent.
    /// Valid layouts: stereo, quad, 5.1.
    Dbap,
    /// FOA: first-order ambisonics. Encodes to B-format, decodes via
    /// mode-matching pseudo-inverse. Listener-relative, all speakers active.
    /// Valid layouts: stereo, quad, 5.1.
    Ambisonics,
}

impl RenderMode {
    /// Index into the pre-allocated pipeline array.
    pub fn index(self) -> usize {
        match self {
            RenderMode::WorldLocked => 0,
            RenderMode::Vbap => 1,
            RenderMode::Hrtf => 2,
            RenderMode::Dbap => 3,
            RenderMode::Ambisonics => 4,
        }
    }

    pub const ALL: [RenderMode; 5] = [
        RenderMode::WorldLocked,
        RenderMode::Vbap,
        RenderMode::Hrtf,
        RenderMode::Dbap,
        RenderMode::Ambisonics,
    ];

    /// Wire format name for JSON serialization.
    pub fn as_str(self) -> &'static str {
        match self {
            RenderMode::WorldLocked => "world_locked",
            RenderMode::Vbap => "vbap",
            RenderMode::Hrtf => "hrtf",
            RenderMode::Dbap => "dbap",
            RenderMode::Ambisonics => "ambisonics",
        }
    }
}
