//! What the Ambisonics mix bus carries: the one decision every consumer asks.

use atrium_core::speaker::{Speaker, SpeakerLayout, MAX_CHANNELS};
use atrium_core::types::Vec3;

/// What the Ambisonics mix bus carries between the renderer and the post-mix
/// stages. Produced only by [`ambisonic_bus`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum AmbisonicBus {
    /// Channels 0–3 carry first-order B-format (W, Y, Z, X), decoded by the
    /// decode stage with this decoder.
    BFormat(BusDecoder),
    /// Fewer than four bus channels: the renderer decodes inline with the
    /// bilateral cardioid pair to these channels.
    InlineBilateral(EarChannels),
}

/// The decoder the decode stage applies to a B-format bus.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum BusDecoder {
    /// EPAD-corrected AllRAD designed over exactly these speakers.
    AllRad(DecodeSpeakers),
    /// Bilateral cardioid pair to these channels; every other channel is
    /// silenced.
    Bilateral(EarChannels),
}

/// Where a bilateral decode puts each ear signal. Both fields name the same
/// channel when only one speaker is reachable — the two ears then sum to the
/// omni signal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct EarChannels {
    pub left: usize,
    pub right: usize,
}

/// The speakers an AllRAD decoder is designed over: active (unmasked),
/// non-LFE speakers on a channel the bus can reach.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct DecodeSpeakers {
    speakers: [Speaker; MAX_CHANNELS],
    count: usize,
}

impl DecodeSpeakers {
    pub fn as_slice(&self) -> &[Speaker] {
        &self.speakers[..self.count]
    }
}

/// Decide what the Ambisonics mix bus carries and how it is decoded.
///
/// - `render_channels`: channels the pipeline lets the renderer write
///   (`MixContext::render_channels`).
/// - `device_channels`: interleaved channels in the output buffer.
///
/// A speaker is *reachable* when its channel is below both counts, is active
/// in the layout's mask, and is not the LFE channel.
///
/// | bus channels | reachable speakers | result                                   |
/// |--------------|--------------------|------------------------------------------|
/// | < 4          | any                | `InlineBilateral` (renderer decodes)     |
/// | ≥ 4          | ≥ 3                | `BFormat(AllRad)` over the reachable set |
/// | ≥ 4          | ≤ 2                | `BFormat(Bilateral)` (decode stage)      |
///
/// Three is the fewest speakers an AllRAD ring can surround the listener
/// with. A bilateral decode goes to the two lowest reachable channels —
/// lowest is left, the order of `SpeakerLayout::stereo` and ITU 5.1 — or to
/// channels 0 and 1 when nothing is reachable (a positionless headphone host).
///
/// Designing the decoder over reachable speakers only is the point: a decoder
/// designed over all five 5.1 speakers and masked afterwards puts a rear
/// source almost entirely into the silenced surrounds (−21 dB, antiphase).
pub fn ambisonic_bus(
    layout: &SpeakerLayout,
    render_channels: usize,
    device_channels: usize,
) -> AmbisonicBus {
    let bus_channels = render_channels.min(device_channels);
    let mut reachable = DecodeSpeakers {
        speakers: [Speaker {
            position: Vec3::ZERO,
            channel: 0,
        }; MAX_CHANNELS],
        count: 0,
    };
    for speaker in layout.speakers() {
        if speaker.channel < bus_channels
            && layout.is_channel_active(speaker.channel)
            && Some(speaker.channel) != layout.lfe_channel()
            && reachable.count < MAX_CHANNELS
        {
            reachable.speakers[reachable.count] = *speaker;
            reachable.count += 1;
        }
    }

    if bus_channels < 4 {
        AmbisonicBus::InlineBilateral(ear_channels(&reachable, bus_channels))
    } else if reachable.count >= 3 {
        AmbisonicBus::BFormat(BusDecoder::AllRad(reachable))
    } else {
        AmbisonicBus::BFormat(BusDecoder::Bilateral(ear_channels(
            &reachable,
            bus_channels,
        )))
    }
}

/// The two lowest reachable channels as left and right ear.
fn ear_channels(reachable: &DecodeSpeakers, bus_channels: usize) -> EarChannels {
    let mut lowest = usize::MAX;
    let mut second_lowest = usize::MAX;
    for speaker in reachable.as_slice() {
        if speaker.channel < lowest {
            second_lowest = lowest;
            lowest = speaker.channel;
        } else if speaker.channel < second_lowest {
            second_lowest = speaker.channel;
        }
    }
    match (lowest, second_lowest) {
        (usize::MAX, _) => EarChannels {
            left: 0,
            right: bus_channels.saturating_sub(1).min(1),
        },
        (only, usize::MAX) => EarChannels {
            left: only,
            right: only,
        },
        (left, right) => EarChannels { left, right },
    }
}
