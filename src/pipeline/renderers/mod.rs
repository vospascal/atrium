//! Renderer implementations.
//!
//! - `multichannel`: gain ramp × sample per channel (VBAP)
//! - `world_locked`: per-speaker PathStages + gain ramp
//! - `hrtf`: HRTF FFT convolution to stereo headphones
//! - `dbap`: gain ramp × sample per channel (DBAP)
//! - `ambisonics`: FOA B-format decode + gain ramp (Ambisonics)

pub mod ambisonics;
pub mod binaural;
pub mod dbap;
pub mod multichannel;
pub mod world_locked;
