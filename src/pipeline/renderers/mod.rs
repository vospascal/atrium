//! Renderer implementations.
//!
//! - `multichannel`: gain ramp × sample per channel (VBAP)
//! - `world_locked`: per-speaker PathStages + gain ramp
//! - `hrtf`: HRTF FFT convolution to stereo headphones

pub mod binaural;
pub mod multichannel;
pub mod world_locked;
