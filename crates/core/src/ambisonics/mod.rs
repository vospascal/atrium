//! First-Order Ambisonics (FOA) encoding, decoding, and bilateral binaural.
//!
//! Implements full 3D FOA using the SN3D/AmbiX convention with max-rE
//! weighting. Encoding converts a source direction (azimuth + elevation) into
//! B-format coefficients (W, Y, Z, X). Decoding converts B-format to speaker
//! gains via either:
//!   - **Mode-matching** pseudo-inverse (`FoaDecoder`)
//!   - **AllRAD** (All-Round Ambisonic Decoding, `AllRadDecoder`): decode to
//!     virtual speakers, then VBAP re-pan to real speakers (Zotter & Frank 2012)
//!
//! Additionally, `BilateralDecoder` provides ambisonics-to-binaural (stereo
//! headphone) rendering by rotating B-format per ear and applying binaural
//! decode weights. This produces ILD (inter-aural level difference) from
//! rotation-based level differences. Note: actual ITD (inter-aural time delay)
//! is not implemented — that would require per-sample delay lines or HRTF convolution.
//!
//! Key equations (Arteaga 2025, Zotter & Frank 2019):
//!   Encoding:  W = g/√2
//!              Y = g·cos(θ)·sin(φ)·a₁
//!              Z = g·sin(θ)·a₁
//!              X = g·cos(θ)·cos(φ)·a₁
//!   Max-rE:    a₁ = cos(π/4) ≈ 0.7071  (FOA order 1)
//!   Decode:    D = Cᵀ(CCᵀ)⁻¹           (mode-matching pseudo-inverse)
//!   AllRAD:    D_real = V_repan × D_virtual  (combined matrix)
//!   Per-speaker: gᵢ = D[i][0]·W + D[i][1]·Y + D[i][2]·Z + D[i][3]·X
//!
//! where φ = azimuth (0 = front, π/2 = left) and θ = elevation (0 = horizon,
//! π/2 = overhead). The decoder adapts to 3-channel (horizontal-only) when all
//! speakers have zero elevation, avoiding the singular 4×4 matrix.
mod allrad;
mod bilateral;
mod encode;
mod epad;
mod matrix;
mod mode_matching;
mod rotate;

#[cfg(test)]
mod test_support;

pub use allrad::{AllRadDecoder, DEFAULT_ALLRAD_VIRTUAL_COUNT};
pub use bilateral::BilateralDecoder;
pub use encode::{foa_encode, BFormat, MAX_RE_WEIGHT};
pub use mode_matching::FoaDecoder;
pub use rotate::foa_rotate_z;
