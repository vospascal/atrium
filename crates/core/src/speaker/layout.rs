// Speaker positions, channel assignment and the active-channel mask.

use super::channel_gains::{ChannelGains, MAX_CHANNELS};
use crate::types::Vec3;

/// The name of output `channel` in the 5.1 order every layout here uses
/// (L, R, C, LFE, Ls, Rs): stereo is its first two channels, quad its first
/// two and last two. Two channels on headphones are the left and right ear.
pub fn channel_name(channel: usize) -> &'static str {
    match channel {
        0 => "L",
        1 => "R",
        2 => "C",
        3 => "LFE",
        4 => "Ls",
        5 => "Rs",
        _ => "?",
    }
}

/// A physical speaker with a position in the virtual atrium.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Speaker {
    /// World position in the atrium (meters).
    pub position: Vec3,
    /// Output channel index in the interleaved buffer.
    pub channel: usize,
}

/// Multichannel speaker configuration.
#[derive(Clone, Debug, PartialEq)]
pub struct SpeakerLayout {
    /// Stereo speakers (excludes LFE).
    pub(super) speakers: [Speaker; MAX_CHANNELS],
    /// Number of spatial speakers.
    pub(super) count: usize,
    /// LFE channel index, if present. LFE has no spatial position.
    pub(super) lfe_channel: Option<usize>,
    /// Total output channels (spatial speakers + LFE if present).
    pub(super) total_channels: usize,
    /// Speaker mask: which channels are active. Inactive channels produce silence.
    /// Used to run e.g. quad on 5.1 hardware by masking center+LFE.
    /// Default: all channels active.
    pub active: [bool; MAX_CHANNELS],
}

impl SpeakerLayout {
    /// Create a custom speaker layout.
    ///
    /// `speakers` defines the spatial speakers (position + channel index).
    /// `lfe_channel` optionally designates a channel for low-frequency effects.
    /// `total_channels` is the total output channel count (must be ≥ max channel index + 1).
    pub fn new(speakers: &[Speaker], lfe_channel: Option<usize>, total_channels: usize) -> Self {
        let mut arr = [Speaker {
            position: Vec3::ZERO,
            channel: 0,
        }; MAX_CHANNELS];
        let count = speakers.len().min(MAX_CHANNELS);
        arr[..count].copy_from_slice(&speakers[..count]);
        // Validate channel indices are within bounds
        for (i, speaker) in arr.iter().enumerate().take(count) {
            assert!(
                speaker.channel < MAX_CHANNELS,
                "speaker {} channel {} exceeds MAX_CHANNELS ({})",
                i,
                speaker.channel,
                MAX_CHANNELS,
            );
        }
        if let Some(lfe) = lfe_channel {
            assert!(
                lfe < MAX_CHANNELS,
                "LFE channel {} exceeds MAX_CHANNELS ({})",
                lfe,
                MAX_CHANNELS,
            );
        }
        Self {
            speakers: arr,
            count,
            lfe_channel,
            total_channels,
            active: [true; MAX_CHANNELS],
        }
    }

    /// Standard stereo layout.
    pub fn stereo(left_pos: Vec3, right_pos: Vec3) -> Self {
        Self::new(
            &[
                Speaker {
                    position: left_pos,
                    channel: 0,
                },
                Speaker {
                    position: right_pos,
                    channel: 1,
                },
            ],
            None,
            2,
        )
    }

    /// Quad 4.0 layout: front-left, front-right, rear-left, rear-right.
    ///
    /// Implemented as masked 5.1 (6 channels) with center and LFE silent.
    /// This guarantees channel semantics match ITU 5.1 on the device —
    /// channels 0/1/4/5 are FL/FR/Ls/Rs, channels 2/3 are zeroed.
    pub fn quad(fl: Vec3, fr: Vec3, rl: Vec3, rr: Vec3) -> Self {
        let mut layout = Self::new(
            &[
                Speaker {
                    position: fl,
                    channel: 0,
                },
                Speaker {
                    position: fr,
                    channel: 1,
                },
                Speaker {
                    position: rl,
                    channel: 4,
                },
                Speaker {
                    position: rr,
                    channel: 5,
                },
            ],
            Some(3),
            6,
        );
        // Mask center (2) and LFE (3) — quad only uses FL/FR/RL/RR.
        layout.set_active_channels(&[0, 1, 4, 5]);
        layout
    }

    /// ITU 5.1 surround: L(0), R(1), C(2), LFE(3), Ls(4), Rs(5).
    pub fn surround_5_1(l: Vec3, r: Vec3, c: Vec3, ls: Vec3, rs: Vec3) -> Self {
        Self::new(
            &[
                Speaker {
                    position: l,
                    channel: 0,
                },
                Speaker {
                    position: r,
                    channel: 1,
                },
                Speaker {
                    position: c,
                    channel: 2,
                },
                Speaker {
                    position: ls,
                    channel: 4,
                },
                Speaker {
                    position: rs,
                    channel: 5,
                },
            ],
            Some(3),
            6,
        )
    }

    /// Total output channels (including LFE).
    pub fn total_channels(&self) -> usize {
        self.total_channels
    }

    /// Number of spatial speakers (excludes LFE).
    pub fn speaker_count(&self) -> usize {
        self.count
    }

    /// Spatial speakers as a slice (excludes LFE).
    pub fn speakers(&self) -> &[Speaker] {
        &self.speakers[..self.count]
    }

    /// Middle of the spatial speakers' bounding box; the origin for a layout
    /// without speakers.
    ///
    /// This is the middle of the area the speakers enclose, not their mean: a
    /// 5.1 layout has three speakers in front and two behind, and the mean
    /// would sit forward of the room's middle.
    pub fn bounding_box_centre(&self) -> Vec3 {
        let speakers = self.speakers();
        let Some(first) = speakers.first() else {
            return Vec3::ZERO;
        };
        let mut min = first.position;
        let mut max = first.position;
        for speaker in speakers {
            min = Vec3::new(
                min.x.min(speaker.position.x),
                min.y.min(speaker.position.y),
                min.z.min(speaker.position.z),
            );
            max = Vec3::new(
                max.x.max(speaker.position.x),
                max.y.max(speaker.position.y),
                max.z.max(speaker.position.z),
            );
        }
        (min + max) * 0.5
    }

    /// LFE channel index, if this layout has one.
    pub fn lfe_channel(&self) -> Option<usize> {
        self.lfe_channel
    }

    /// Get a reference to a speaker by its array index (0..speaker_count).
    pub fn speaker_by_index(&self, index: usize) -> Option<&Speaker> {
        if index < self.count {
            Some(&self.speakers[index])
        } else {
            None
        }
    }

    /// Get a mutable reference to a speaker by channel index.
    pub fn speaker_by_channel_mut(&mut self, channel: usize) -> Option<&mut Speaker> {
        self.speakers[..self.count]
            .iter_mut()
            .find(|s| s.channel == channel)
    }

    /// Check if a channel is active (not masked).
    pub fn is_channel_active(&self, channel: usize) -> bool {
        channel < MAX_CHANNELS && self.active[channel]
    }

    /// Set which channels are active. Channels not in the list are masked.
    pub fn set_active_channels(&mut self, channels: &[usize]) {
        self.active = [false; MAX_CHANNELS];
        for &ch in channels {
            if ch < MAX_CHANNELS {
                self.active[ch] = true;
            }
        }
    }

    /// Apply the speaker mask to a gain array: zero out inactive channels.
    pub fn apply_mask(&self, gains: &mut ChannelGains) {
        for ch in 0..gains.count {
            if !self.active[ch] {
                gains.gains[ch] = 0.0;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::super::modes::{ChannelMode, RenderMode};
    use super::super::test_support::{equidistant_quad, room_layout};
    use super::*;

    #[test]
    fn mic_5_1_has_six_channels() {
        let layout = SpeakerLayout::surround_5_1(
            Vec3::new(1.0, 3.0, 0.0),
            Vec3::new(5.0, 3.0, 0.0),
            Vec3::new(3.0, 4.0, 0.0),
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(6.0, 0.0, 0.0),
        );
        assert_eq!(layout.total_channels(), 6);
        assert_eq!(layout.speaker_count(), 5);
        assert_eq!(layout.lfe_channel(), Some(3));
    }

    // ── Hand-derived VBAP / layout values ─────────────────────────────

    #[test]
    fn itu_5_1_and_quad_channel_conventions() {
        // ITU/SMPTE 5.1 order: L 0, R 1, C 2, LFE 3, Ls 4, Rs 5.
        let surround = room_layout();
        let channels: Vec<usize> = surround.speakers().iter().map(|s| s.channel).collect();
        assert_eq!(channels, vec![0, 1, 2, 4, 5]);
        assert_eq!(surround.lfe_channel(), Some(3));
        assert_eq!(surround.total_channels(), 6);
        // Quad is masked 5.1: FL/FR/RL/RR on 0/1/4/5, C and LFE silent.
        let quad = equidistant_quad();
        let quad_channels: Vec<usize> = quad.speakers().iter().map(|s| s.channel).collect();
        assert_eq!(quad_channels, vec![0, 1, 4, 5]);
        let mut gains = ChannelGains::silent(6);
        gains.gains[..6].copy_from_slice(&[1.0; 6]);
        quad.apply_mask(&mut gains);
        assert_eq!(&gains.gains[..6], &[1.0, 1.0, 0.0, 0.0, 1.0, 1.0]);
        assert_eq!(ChannelMode::Quad.active_channels(), &[0, 1, 4, 5]);
        for mode in [
            ChannelMode::Stereo,
            ChannelMode::Quad,
            ChannelMode::Surround51,
        ] {
            assert_eq!(ChannelMode::parse(mode.as_str()), Some(mode));
        }
        for (index, mode) in RenderMode::ALL.iter().enumerate() {
            assert_eq!(mode.index(), index);
        }
    }
}
