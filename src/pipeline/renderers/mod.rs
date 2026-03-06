//! Renderer implementations.
//!
//! - `multichannel`: gain ramp × sample per channel (VBAP, Stereo)
//! - `world_locked`: per-speaker PathStages + gain ramp
//! - `binaural`: HRTF FFT convolution to stereo

pub mod binaural;
pub mod multichannel;
pub mod world_locked;
