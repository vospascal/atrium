//! The output choice: which renderer drives which speaker set, and which
//! choices a running scene can actually play.

use atrium_core::speaker::{ChannelMode as EngineChannelMode, RenderMode as EngineRenderMode};

/// How sources are rendered to the speakers.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum RenderMode {
    /// Speakers are fixed microphones in the world; the listener is ignored.
    WorldLocked,
    /// Listener-relative panning between the nearest speakers.
    Vbap,
    /// Binaural rendering for headphones.
    Hrtf,
    /// Distance-based panning; listener-independent.
    Dbap,
    Ambisonics,
}

/// Which speakers are in use.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Enum)]
pub enum ChannelMode {
    Stereo,
    Quad,
    Surround51,
}

/// A renderer and the speaker set it drives.
#[derive(Debug, Clone, Copy, PartialEq, Eq, uniffi::Record)]
pub struct Output {
    pub render_mode: RenderMode,
    pub channel_mode: ChannelMode,
}

impl From<EngineRenderMode> for RenderMode {
    fn from(mode: EngineRenderMode) -> Self {
        match mode {
            EngineRenderMode::WorldLocked => Self::WorldLocked,
            EngineRenderMode::Vbap => Self::Vbap,
            EngineRenderMode::Hrtf => Self::Hrtf,
            EngineRenderMode::Dbap => Self::Dbap,
            EngineRenderMode::Ambisonics => Self::Ambisonics,
        }
    }
}

impl From<RenderMode> for EngineRenderMode {
    fn from(mode: RenderMode) -> Self {
        match mode {
            RenderMode::WorldLocked => Self::WorldLocked,
            RenderMode::Vbap => Self::Vbap,
            RenderMode::Hrtf => Self::Hrtf,
            RenderMode::Dbap => Self::Dbap,
            RenderMode::Ambisonics => Self::Ambisonics,
        }
    }
}

impl From<EngineChannelMode> for ChannelMode {
    fn from(mode: EngineChannelMode) -> Self {
        match mode {
            EngineChannelMode::Stereo => Self::Stereo,
            EngineChannelMode::Quad => Self::Quad,
            EngineChannelMode::Surround51 => Self::Surround51,
        }
    }
}

impl From<ChannelMode> for EngineChannelMode {
    fn from(mode: ChannelMode) -> Self {
        match mode {
            ChannelMode::Stereo => Self::Stereo,
            ChannelMode::Quad => Self::Quad,
            ChannelMode::Surround51 => Self::Surround51,
        }
    }
}

/// Every output a scene can play: each renderer with each speaker set it
/// supports, keeping only speaker sets whose channels exist both in the
/// scene's layout and on the opened device. A speaker set naming a channel
/// either one lacks would render into a speaker nobody hears.
pub(crate) fn playable_outputs(layout_channels: usize, device_channels: usize) -> Vec<Output> {
    let available = layout_channels.min(device_channels);
    EngineRenderMode::ALL
        .iter()
        .flat_map(|render_mode| {
            EngineChannelMode::valid_for(*render_mode)
                .iter()
                .filter(move |channel_mode| {
                    channel_mode
                        .active_channels()
                        .iter()
                        .all(|channel| *channel < available)
                })
                .map(move |channel_mode| Output {
                    render_mode: (*render_mode).into(),
                    channel_mode: (*channel_mode).into(),
                })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn channel_modes_for(outputs: &[Output], render_mode: RenderMode) -> Vec<ChannelMode> {
        outputs
            .iter()
            .filter(|output| output.render_mode == render_mode)
            .map(|output| output.channel_mode)
            .collect()
    }

    #[test]
    fn headphones_on_a_surround_scene_offer_stereo_only() {
        let outputs = playable_outputs(6, 2);
        assert!(outputs
            .iter()
            .all(|output| output.channel_mode == ChannelMode::Stereo));
        assert!(channel_modes_for(&outputs, RenderMode::Vbap).is_empty());
        assert_eq!(
            channel_modes_for(&outputs, RenderMode::Hrtf),
            [ChannelMode::Stereo]
        );
    }

    #[test]
    fn a_stereo_scene_stays_stereo_on_a_surround_device() {
        let outputs = playable_outputs(2, 6);
        assert!(outputs
            .iter()
            .all(|output| output.channel_mode == ChannelMode::Stereo));
    }

    /// Quad uses the rear pair on channels 4 and 5, so four channels are not
    /// enough for it.
    #[test]
    fn quad_needs_the_rear_channels_not_just_four() {
        let outputs = playable_outputs(6, 4);
        assert!(!outputs
            .iter()
            .any(|output| output.channel_mode == ChannelMode::Quad));
    }

    #[test]
    fn a_full_surround_setup_offers_every_supported_pair() {
        let outputs = playable_outputs(6, 6);
        assert_eq!(
            channel_modes_for(&outputs, RenderMode::Vbap),
            [ChannelMode::Quad, ChannelMode::Surround51]
        );
        assert_eq!(
            channel_modes_for(&outputs, RenderMode::Dbap),
            [
                ChannelMode::Stereo,
                ChannelMode::Quad,
                ChannelMode::Surround51
            ]
        );
        assert_eq!(
            channel_modes_for(&outputs, RenderMode::Hrtf),
            [ChannelMode::Stereo]
        );
    }

    #[test]
    fn modes_round_trip_through_the_engine_types() {
        for render_mode in EngineRenderMode::ALL {
            assert_eq!(
                EngineRenderMode::from(RenderMode::from(render_mode)),
                render_mode
            );
        }
        for channel_mode in [
            EngineChannelMode::Stereo,
            EngineChannelMode::Quad,
            EngineChannelMode::Surround51,
        ] {
            assert_eq!(
                EngineChannelMode::from(ChannelMode::from(channel_mode)),
                channel_mode
            );
        }
    }
}
