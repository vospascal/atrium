//! Which channels sound: the channel mode the listener chose, cut down to
//! the channels the output device has.

use atrium_core::speaker::{ChannelMode, MAX_CHANNELS};

use super::AudioScene;

impl AudioScene {
    /// Record how many channels the output device opened with and switch off
    /// every speaker beyond them. The renderers still compute gains for all
    /// speakers; a masked speaker's share goes to the ones that remain.
    pub fn fit_to_output_device(&mut self, device_channels: usize) {
        self.output_channels = Some(device_channels);
        self.mask_channels_the_device_lacks();
    }

    /// Switch to `mode`, on the channels the output device has.
    pub(super) fn set_channel_mode(&mut self, mode: ChannelMode) {
        // Remember what the scene started with, the first time it
        // changes: that mask includes the fit to the output device.
        self.startup_channels
            .get_or_insert((self.active_channel_mode, self.speaker_layout.active));
        self.active_channel_mode = mode;
        self.speaker_layout
            .set_active_channels(mode.active_channels());
        self.mask_channels_the_device_lacks();
    }

    /// Undo channel-mode changes. The startup mask is restored as it was, not
    /// rebuilt from the layout: rebuilding it re-enabled channels a smaller
    /// output device does not have.
    pub(super) fn restore_startup_channels(&mut self) {
        if let Some((channel_mode, active_channels)) = self.startup_channels.take() {
            self.active_channel_mode = channel_mode;
            self.speaker_layout.active = active_channels;
        }
    }

    /// Switch off active channels the output device does not have.
    fn mask_channels_the_device_lacks(&mut self) {
        let Some(device_channels) = self.output_channels else {
            return;
        };
        for channel in device_channels..MAX_CHANNELS {
            self.speaker_layout.active[channel] = false;
        }
    }
}
