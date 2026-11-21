//! AmbisonicsRenderer — per-path full 3D FOA encode.
//!
//! Used by Ambisonics mode. Iterates over propagation paths from the PathResolver.
//! For the **direct path**, computes full FOA encoding (azimuth + elevation +
//! distance attenuation + directivity). For **reflection paths**, computes
//! angular-only FOA encoding (direction from path.direction), with the
//! reflection's energy carried by path.gain.
//!
//! What the mix bus carries is decided once, by [`ambisonic_bus`], and every
//! consumer (this renderer, the field renderer, `AmbiMultiDelayStage`,
//! `AmbisonicsDecodeStage`) asks that function instead of applying its own
//! channel-count rule:
//! - [`AmbisonicBus::BFormat`]: writes B-format (W,Y,Z,X) to channels 0-3.
//!   `AmbiMultiDelayStage` decorrelates it and `AmbisonicsDecodeStage`
//!   decodes it with the [`BusDecoder`] the decision names.
//! - [`AmbisonicBus::InlineBilateral`]: the bus is too narrow for four
//!   components, so the renderer decodes inline — B-format rotated ±90° per
//!   ear, then forward cardioid weights to a left and a right channel. The
//!   multi-delay and decode stages no-op.

mod bus;
mod renderer;

pub use bus::{ambisonic_bus, AmbisonicBus, BusDecoder, DecodeSpeakers, EarChannels};
pub use renderer::AmbisonicsRenderer;
