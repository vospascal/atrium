// Multichannel speaker layout and rendering.
//
// Five render modes (RenderMode):
//   1. WorldLocked — each speaker is a virtual microphone at a fixed position in the room.
//      Gain = distance_attenuation(source → speaker) × source_directivity(source → speaker).
//      The listener's position does NOT affect speaker gains (spatial image comes from
//      physical speaker placement). Valid layouts: stereo, quad, 5.1.
//   2. Vbap — 2D Vector Base Amplitude Panning (Pulkki 1997). Speakers form a ring around
//      the listener. Source direction from listener determines which speaker pair activates.
//      Requires 3+ speakers. Valid layouts: quad, 5.1.
//   3. Hrtf — HRTF convolution for headphone rendering. Always stereo output.
//   4. Dbap — Distance-Based Amplitude Panning (Lossius 2009, improved 2021). All speakers
//      active, gains weighted by inverse distance. Listener-independent. Valid layouts:
//      stereo, quad, 5.1.
//   5. Ambisonics — First-Order Ambisonics (FOA). Encodes sources into B-format (W, Y, Z, X),
//      then decodes to any speaker layout via mode-matching pseudo-inverse decoder.
//      Listener-relative. All speakers active. Valid layouts: stereo, quad, 5.1.
//
// Speaker positions are configurable per room. Layouts: stereo, quad 4.0, surround 5.1.
mod channel_gains;
mod layout;
mod mdap;
mod modes;
mod source;
mod vbap;
mod vbap_extended;
mod vbap_lookup;

#[cfg(test)]
mod test_support;

pub use channel_gains::{ChannelGains, MAX_CHANNELS};
pub use layout::{channel_name, Speaker, SpeakerLayout};
pub use modes::{ChannelMode, RenderMode};
pub use source::{DistanceParams, SourceSpatial};
pub use vbap_lookup::VbapLookup;
