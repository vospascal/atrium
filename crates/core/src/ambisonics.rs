//! First-Order Ambisonics (FOA) encoding and decoding.
//!
//! Implements full 3D FOA using the SN3D/AmbiX convention with max-rE
//! weighting. Encoding converts a source direction (azimuth + elevation) into
//! B-format coefficients (W, Y, Z, X). Decoding converts B-format to speaker
//! gains via a mode-matching pseudo-inverse decoder.
//!
//! Key equations (Arteaga 2025, Zotter & Frank 2019):
//!   Encoding:  W = g/√2
//!              Y = g·cos(θ)·sin(φ)·a₁
//!              Z = g·sin(θ)·a₁
//!              X = g·cos(θ)·cos(φ)·a₁
//!   Max-rE:    a₁ = cos(π/4) ≈ 0.7071  (FOA order 1)
//!   Decode:    D = Cᵀ(CCᵀ)⁻¹           (mode-matching pseudo-inverse)
//!   Per-speaker: gᵢ = D[i][0]·W + D[i][1]·Y + D[i][2]·Z + D[i][3]·X
//!
//! where φ = azimuth (0 = front, π/2 = left) and θ = elevation (0 = horizon,
//! π/2 = overhead). The decoder adapts to 3-channel (horizontal-only) when all
//! speakers have zero elevation, avoiding the singular 4×4 matrix.

use crate::listener::Listener;
use crate::speaker::{ChannelGains, Speaker, MAX_CHANNELS};

/// Max-rE weight for FOA: cos(π / (2·order + 2)) where order = 1.
/// Narrows the panning function for better perceptual localization.
pub const MAX_RE_WEIGHT: f32 = std::f32::consts::FRAC_1_SQRT_2; // cos(45°) = 1/√2

/// SN3D normalization for zeroth-order (W channel): 1/√2.
const W_NORM: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// FOA B-format coefficients (ACN ordering, SN3D normalization).
///
/// ACN 0 = W (omnidirectional pressure)
/// ACN 1 = Y (left-right, cos θ · sin φ)
/// ACN 2 = Z (up-down, sin θ)
/// ACN 3 = X (front-back, cos θ · cos φ)
#[derive(Clone, Copy, Debug)]
pub struct BFormat {
    pub w: f32,
    pub y: f32,
    pub z: f32,
    pub x: f32,
}

/// Encode a source at listener-relative azimuth and elevation (radians) with gain `g`.
///
/// Azimuth convention: 0 = front (+X), π/2 = left (+Y), π = rear (-X).
/// Elevation convention: 0 = horizon, π/2 = overhead, -π/2 = below.
/// Max-rE weighting is applied to Y, Z, and X channels.
pub fn foa_encode(azimuth: f32, elevation: f32, gain: f32) -> BFormat {
    let cos_el = elevation.cos();
    BFormat {
        w: gain * W_NORM,
        y: gain * cos_el * azimuth.sin() * MAX_RE_WEIGHT,
        z: gain * elevation.sin() * MAX_RE_WEIGHT,
        x: gain * cos_el * azimuth.cos() * MAX_RE_WEIGHT,
    }
}

/// Precomputed FOA decoder for a specific speaker layout.
///
/// The decode matrix maps 4 B-format channels (W, Y, Z, X) to N speaker gains.
/// Built once per layout change via mode-matching pseudo-inverse: D = Cᵀ(CCᵀ)⁻¹.
///
/// Adapts to 3-channel (horizontal-only) decoding when all speakers have zero
/// elevation, storing zeros in the Z column to avoid a singular 4×4 matrix.
pub struct FoaDecoder {
    /// Nx4 decode matrix. Row i = [d_w, d_y, d_z, d_x] for speaker i.
    decode_matrix: Vec<[f32; 4]>,
    /// Output channel index for each speaker.
    channels: Vec<usize>,
}

impl FoaDecoder {
    /// Build a decoder from speaker positions relative to a listener.
    ///
    /// Speaker azimuths and elevations are computed in the listener's local
    /// frame (same as the encoder), so both sides of the B-format chain use
    /// the same reference.
    pub fn from_listener(speakers: &[Speaker], speaker_count: usize, listener: &Listener) -> Self {
        let n = speaker_count;
        if n == 0 {
            return Self {
                decode_matrix: Vec::new(),
                channels: Vec::new(),
            };
        }

        let cos_y = listener.yaw.cos();
        let sin_y = listener.yaw.sin();

        let mut azimuths = Vec::with_capacity(n);
        let mut elevations = Vec::with_capacity(n);

        for s in &speakers[..n] {
            let dx = s.position.x - listener.position.x;
            let dy = s.position.y - listener.position.y;
            let dz = s.position.z - listener.position.z;
            let local_x = dx * cos_y + dy * sin_y; // forward
            let local_y = -dx * sin_y + dy * cos_y; // left
            azimuths.push(local_y.atan2(local_x));
            let horiz = (local_x * local_x + local_y * local_y).sqrt();
            elevations.push(dz.atan2(horiz));
        }

        let channels: Vec<usize> = speakers[..n].iter().map(|s| s.channel).collect();

        Self::new(&azimuths, &elevations, &channels)
    }

    /// Build a decoder from speaker azimuths, elevations (radians), and channel indices.
    ///
    /// Azimuths use the same convention as encoding: 0 = front, π/2 = left.
    /// Elevations: 0 = horizon, π/2 = overhead.
    ///
    /// When all speakers have zero elevation, builds a 3-channel (W, Y, X)
    /// decoder to avoid the singular 4×4 matrix that arises from a flat array.
    pub fn new(azimuths: &[f32], elevations: &[f32], channels: &[usize]) -> Self {
        let n = azimuths.len();
        if n == 0 {
            return Self {
                decode_matrix: Vec::new(),
                channels: channels.to_vec(),
            };
        }

        let has_elevation = elevations.iter().any(|e| e.abs() > 1e-6);

        if has_elevation {
            Self::build_4ch(azimuths, elevations, channels)
        } else {
            Self::build_3ch(azimuths, channels)
        }
    }

    /// 3-channel decoder (horizontal-only): W, Y, X.
    /// Z column is zero in the decode matrix.
    #[allow(clippy::needless_range_loop)]
    fn build_3ch(azimuths: &[f32], channels: &[usize]) -> Self {
        let n = azimuths.len();

        // Build encoding matrix C (3×N): W_NORM, sin(φ), cos(φ)
        let mut c = vec![[0.0_f32; 3]; n];
        for (i, &az) in azimuths.iter().enumerate() {
            c[i][0] = W_NORM;
            c[i][1] = az.sin();
            c[i][2] = az.cos();
        }

        // CCᵀ (3×3)
        let mut cct = [[0.0_f32; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                let mut sum = 0.0_f32;
                for ck in &c {
                    sum += ck[i] * ck[j];
                }
                cct[i][j] = sum;
            }
        }

        let inv = match invert_3x3(&cct) {
            Some(m) => m,
            None => {
                let equal = 1.0 / (n as f32).sqrt();
                return Self {
                    decode_matrix: vec![[equal, 0.0, 0.0, 0.0]; n],
                    channels: channels.to_vec(),
                };
            }
        };

        // D = Cᵀ · (CCᵀ)⁻¹ — store in [d_w, d_y, 0, d_x] layout
        let mut decode_matrix = vec![[0.0_f32; 4]; n];
        for i in 0..n {
            for j in 0..3 {
                let mut sum = 0.0_f32;
                for k in 0..3 {
                    sum += c[i][k] * inv[k][j];
                }
                // Map 3ch indices {0,1,2} → 4ch indices {0,1,3} (skip Z=2)
                let col = if j < 2 { j } else { 3 };
                decode_matrix[i][col] = sum;
            }
        }

        Self {
            decode_matrix,
            channels: channels.to_vec(),
        }
    }

    /// 4-channel decoder (full 3D): W, Y, Z, X.
    fn build_4ch(azimuths: &[f32], elevations: &[f32], channels: &[usize]) -> Self {
        let n = azimuths.len();

        // Build encoding matrix C (4×N): W_NORM, cos(θ)·sin(φ), sin(θ), cos(θ)·cos(φ)
        let mut c = vec![[0.0_f32; 4]; n];
        for i in 0..n {
            let cos_el = elevations[i].cos();
            c[i][0] = W_NORM;
            c[i][1] = cos_el * azimuths[i].sin();
            c[i][2] = elevations[i].sin();
            c[i][3] = cos_el * azimuths[i].cos();
        }

        // CCᵀ (4×4)
        let mut cct = [[0.0_f32; 4]; 4];
        for i in 0..4 {
            for j in 0..4 {
                let mut sum = 0.0_f32;
                for ck in &c {
                    sum += ck[i] * ck[j];
                }
                cct[i][j] = sum;
            }
        }

        let inv = match invert_4x4(&cct) {
            Some(m) => m,
            None => {
                let equal = 1.0 / (n as f32).sqrt();
                return Self {
                    decode_matrix: vec![[equal, 0.0, 0.0, 0.0]; n],
                    channels: channels.to_vec(),
                };
            }
        };

        // D = Cᵀ · (CCᵀ)⁻¹
        let mut decode_matrix = vec![[0.0_f32; 4]; n];
        for i in 0..n {
            for j in 0..4 {
                let mut sum = 0.0_f32;
                for k in 0..4 {
                    sum += c[i][k] * inv[k][j];
                }
                decode_matrix[i][j] = sum;
            }
        }

        Self {
            decode_matrix,
            channels: channels.to_vec(),
        }
    }

    /// Decode B-format coefficients to per-channel speaker gains.
    pub fn decode(&self, bformat: &BFormat) -> ChannelGains {
        let mut gains = ChannelGains::silent(MAX_CHANNELS);
        for (i, row) in self.decode_matrix.iter().enumerate() {
            let g =
                row[0] * bformat.w + row[1] * bformat.y + row[2] * bformat.z + row[3] * bformat.x;
            if i < self.channels.len() {
                gains.gains[self.channels[i]] = g;
            }
        }
        gains
    }

    /// Number of speakers in this decoder.
    pub fn speaker_count(&self) -> usize {
        self.decode_matrix.len()
    }
}

/// 4×4 matrix inverse via cofactor expansion. Returns None if singular.
fn invert_4x4(m: &[[f32; 4]; 4]) -> Option<[[f32; 4]; 4]> {
    // 2×2 sub-determinants from rows 0-1
    let s0 = m[0][0] * m[1][1] - m[0][1] * m[1][0];
    let s1 = m[0][0] * m[1][2] - m[0][2] * m[1][0];
    let s2 = m[0][0] * m[1][3] - m[0][3] * m[1][0];
    let s3 = m[0][1] * m[1][2] - m[0][2] * m[1][1];
    let s4 = m[0][1] * m[1][3] - m[0][3] * m[1][1];
    let s5 = m[0][2] * m[1][3] - m[0][3] * m[1][2];

    // 2×2 sub-determinants from rows 2-3
    let c5 = m[2][2] * m[3][3] - m[2][3] * m[3][2];
    let c4 = m[2][1] * m[3][3] - m[2][3] * m[3][1];
    let c3 = m[2][1] * m[3][2] - m[2][2] * m[3][1];
    let c2 = m[2][0] * m[3][3] - m[2][3] * m[3][0];
    let c1 = m[2][0] * m[3][2] - m[2][2] * m[3][0];
    let c0 = m[2][0] * m[3][1] - m[2][1] * m[3][0];

    let det = s0 * c5 - s1 * c4 + s2 * c3 + s3 * c2 - s4 * c1 + s5 * c0;

    if det.abs() < 1e-10 {
        return None;
    }

    let inv_det = 1.0 / det;

    Some([
        [
            (m[1][1] * c5 - m[1][2] * c4 + m[1][3] * c3) * inv_det,
            (-m[0][1] * c5 + m[0][2] * c4 - m[0][3] * c3) * inv_det,
            (m[3][1] * s5 - m[3][2] * s4 + m[3][3] * s3) * inv_det,
            (-m[2][1] * s5 + m[2][2] * s4 - m[2][3] * s3) * inv_det,
        ],
        [
            (-m[1][0] * c5 + m[1][2] * c2 - m[1][3] * c1) * inv_det,
            (m[0][0] * c5 - m[0][2] * c2 + m[0][3] * c1) * inv_det,
            (-m[3][0] * s5 + m[3][2] * s2 - m[3][3] * s1) * inv_det,
            (m[2][0] * s5 - m[2][2] * s2 + m[2][3] * s1) * inv_det,
        ],
        [
            (m[1][0] * c4 - m[1][1] * c2 + m[1][3] * c0) * inv_det,
            (-m[0][0] * c4 + m[0][1] * c2 - m[0][3] * c0) * inv_det,
            (m[3][0] * s4 - m[3][1] * s2 + m[3][3] * s0) * inv_det,
            (-m[2][0] * s4 + m[2][1] * s2 - m[2][3] * s0) * inv_det,
        ],
        [
            (-m[1][0] * c3 + m[1][1] * c1 - m[1][2] * c0) * inv_det,
            (m[0][0] * c3 - m[0][1] * c1 + m[0][2] * c0) * inv_det,
            (-m[3][0] * s3 + m[3][1] * s1 - m[3][2] * s0) * inv_det,
            (m[2][0] * s3 - m[2][1] * s1 + m[2][2] * s0) * inv_det,
        ],
    ])
}

/// 3×3 matrix inverse via Cramer's rule. Returns None if singular.
fn invert_3x3(m: &[[f32; 3]; 3]) -> Option<[[f32; 3]; 3]> {
    let det = m[0][0] * (m[1][1] * m[2][2] - m[1][2] * m[2][1])
        - m[0][1] * (m[1][0] * m[2][2] - m[1][2] * m[2][0])
        + m[0][2] * (m[1][0] * m[2][1] - m[1][1] * m[2][0]);

    if det.abs() < 1e-10 {
        return None;
    }

    let inv_det = 1.0 / det;
    Some([
        [
            (m[1][1] * m[2][2] - m[1][2] * m[2][1]) * inv_det,
            (m[0][2] * m[2][1] - m[0][1] * m[2][2]) * inv_det,
            (m[0][1] * m[1][2] - m[0][2] * m[1][1]) * inv_det,
        ],
        [
            (m[1][2] * m[2][0] - m[1][0] * m[2][2]) * inv_det,
            (m[0][0] * m[2][2] - m[0][2] * m[2][0]) * inv_det,
            (m[0][2] * m[1][0] - m[0][0] * m[1][2]) * inv_det,
        ],
        [
            (m[1][0] * m[2][1] - m[1][1] * m[2][0]) * inv_det,
            (m[0][1] * m[2][0] - m[0][0] * m[2][1]) * inv_det,
            (m[0][0] * m[1][1] - m[0][1] * m[1][0]) * inv_det,
        ],
    ])
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

    // -- Encoder tests --

    #[test]
    fn foa_encode_front() {
        let b = foa_encode(0.0, 0.0, 1.0);
        assert!((b.w - W_NORM).abs() < 1e-6, "W should be 1/√2");
        assert!(b.y.abs() < 1e-6, "Y should be 0 for front source");
        assert!(b.z.abs() < 1e-6, "Z should be 0 at horizon");
        assert!(
            (b.x - MAX_RE_WEIGHT).abs() < 1e-6,
            "X should be max-rE weight"
        );
    }

    #[test]
    fn foa_encode_left() {
        let b = foa_encode(FRAC_PI_2, 0.0, 1.0);
        assert!((b.w - W_NORM).abs() < 1e-6);
        assert!(
            (b.y - MAX_RE_WEIGHT).abs() < 1e-6,
            "Y should be max-rE for left"
        );
        assert!(b.z.abs() < 1e-6, "Z should be 0 at horizon");
        assert!(b.x.abs() < 1e-6, "X should be ~0 for left");
    }

    #[test]
    fn foa_encode_rear() {
        let b = foa_encode(PI, 0.0, 1.0);
        assert!((b.w - W_NORM).abs() < 1e-6);
        assert!(b.y.abs() < 1e-5, "Y should be ~0 for rear");
        assert!(b.z.abs() < 1e-6, "Z should be 0 at horizon");
        assert!(
            (b.x + MAX_RE_WEIGHT).abs() < 1e-6,
            "X should be -max-rE for rear"
        );
    }

    #[test]
    fn foa_encode_zero_gain() {
        let b = foa_encode(1.0, 0.5, 0.0);
        assert!(b.w.abs() < 1e-10);
        assert!(b.y.abs() < 1e-10);
        assert!(b.z.abs() < 1e-10);
        assert!(b.x.abs() < 1e-10);
    }

    #[test]
    fn foa_encode_overhead() {
        let b = foa_encode(0.0, FRAC_PI_2, 1.0);
        assert!((b.w - W_NORM).abs() < 1e-6, "W unchanged by elevation");
        assert!(b.y.abs() < 1e-6, "Y should be ~0 overhead");
        assert!(
            (b.z - MAX_RE_WEIGHT).abs() < 1e-6,
            "Z should be max-rE overhead"
        );
        assert!(b.x.abs() < 1e-6, "X should be ~0 overhead");
    }

    #[test]
    fn foa_encode_below() {
        let b = foa_encode(0.0, -FRAC_PI_2, 1.0);
        assert!((b.w - W_NORM).abs() < 1e-6);
        assert!(
            (b.z + MAX_RE_WEIGHT).abs() < 1e-6,
            "Z should be -max-rE below"
        );
        assert!(b.x.abs() < 1e-6);
    }

    #[test]
    fn foa_encode_elevated_45() {
        // At 45° elevation, cos(45°) = sin(45°) → X and Z should be equal
        let b = foa_encode(0.0, FRAC_PI_4, 1.0);
        assert!(
            (b.x - b.z).abs() < 1e-6,
            "at 45° elevation, X={} and Z={} should be equal",
            b.x,
            b.z
        );
        assert!(b.y.abs() < 1e-6, "Y should be 0 at azimuth 0");
    }

    // -- Matrix inversion tests --

    #[test]
    fn invert_3x3_identity() {
        let id = [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]];
        let inv = invert_3x3(&id).unwrap();
        for i in 0..3 {
            for j in 0..3 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (inv[i][j] - expected).abs() < 1e-6,
                    "inv[{i}][{j}] = {}, expected {expected}",
                    inv[i][j]
                );
            }
        }
    }

    #[test]
    fn invert_3x3_singular() {
        let m = [[1.0, 2.0, 3.0], [1.0, 2.0, 3.0], [1.0, 2.0, 3.0]];
        assert!(invert_3x3(&m).is_none());
    }

    #[test]
    fn invert_4x4_identity() {
        let id = [
            [1.0, 0.0, 0.0, 0.0],
            [0.0, 1.0, 0.0, 0.0],
            [0.0, 0.0, 1.0, 0.0],
            [0.0, 0.0, 0.0, 1.0],
        ];
        let inv = invert_4x4(&id).unwrap();
        for i in 0..4 {
            for j in 0..4 {
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (inv[i][j] - expected).abs() < 1e-6,
                    "inv[{i}][{j}] = {}, expected {expected}",
                    inv[i][j]
                );
            }
        }
    }

    #[test]
    fn invert_4x4_round_trip() {
        // Non-trivial symmetric positive definite matrix
        let m = [
            [4.0, 1.0, 0.5, 0.2],
            [1.0, 3.0, 0.3, 0.1],
            [0.5, 0.3, 2.0, 0.4],
            [0.2, 0.1, 0.4, 1.5],
        ];
        let inv = invert_4x4(&m).unwrap();
        // M · M⁻¹ should be identity
        for i in 0..4 {
            for j in 0..4 {
                let mut sum = 0.0_f32;
                for k in 0..4 {
                    sum += m[i][k] * inv[k][j];
                }
                let expected = if i == j { 1.0 } else { 0.0 };
                assert!(
                    (sum - expected).abs() < 1e-4,
                    "(M·M⁻¹)[{i}][{j}] = {sum}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn invert_4x4_singular() {
        // All rows identical — singular
        let m = [
            [1.0, 2.0, 3.0, 4.0],
            [1.0, 2.0, 3.0, 4.0],
            [1.0, 2.0, 3.0, 4.0],
            [1.0, 2.0, 3.0, 4.0],
        ];
        assert!(invert_4x4(&m).is_none());
    }

    // -- Decoder tests (flat arrays → 3-channel path) --

    #[test]
    fn decoder_stereo_front_equal() {
        let left_az = 30.0_f32.to_radians();
        let right_az = -30.0_f32.to_radians();
        let dec = FoaDecoder::new(&[left_az, right_az], &[0.0, 0.0], &[0, 1]);

        let b = foa_encode(0.0, 0.0, 1.0);
        let g = dec.decode(&b);
        assert!(
            (g.gains[0] - g.gains[1]).abs() < 0.01,
            "stereo front: L={}, R={} should be equal",
            g.gains[0],
            g.gains[1]
        );
    }

    #[test]
    fn decoder_stereo_left_louder() {
        let left_az = 30.0_f32.to_radians();
        let right_az = -30.0_f32.to_radians();
        let dec = FoaDecoder::new(&[left_az, right_az], &[0.0, 0.0], &[0, 1]);

        let b = foa_encode(FRAC_PI_2, 0.0, 1.0);
        let g = dec.decode(&b);
        assert!(
            g.gains[0] > g.gains[1],
            "left source: L={} should be > R={}",
            g.gains[0],
            g.gains[1]
        );
    }

    #[test]
    fn decoder_quad_symmetric() {
        let azimuths = [
            45.0_f32.to_radians(),
            -45.0_f32.to_radians(),
            135.0_f32.to_radians(),
            -135.0_f32.to_radians(),
        ];
        let dec = FoaDecoder::new(&azimuths, &[0.0; 4], &[0, 1, 2, 3]);

        let b = foa_encode(0.0, 0.0, 1.0);
        let g = dec.decode(&b);
        assert!(
            (g.gains[0] - g.gains[1]).abs() < 0.01,
            "front pair should be equal: FL={}, FR={}",
            g.gains[0],
            g.gains[1]
        );
        assert!(
            (g.gains[2] - g.gains[3]).abs() < 0.01,
            "rear pair should be equal: RL={}, RR={}",
            g.gains[2],
            g.gains[3]
        );
        assert!(
            g.gains[0] > g.gains[2],
            "front should be louder than rear for front source"
        );
    }

    #[test]
    fn decoder_energy_roughly_constant() {
        let azimuths = [
            45.0_f32.to_radians(),
            -45.0_f32.to_radians(),
            135.0_f32.to_radians(),
            -135.0_f32.to_radians(),
        ];
        let dec = FoaDecoder::new(&azimuths, &[0.0; 4], &[0, 1, 2, 3]);

        let mut energies = Vec::new();
        for deg in (0..360).step_by(30) {
            let az = (deg as f32).to_radians();
            let b = foa_encode(az, 0.0, 1.0);
            let g = dec.decode(&b);
            let energy: f32 = (0..4).map(|ch| g.gains[ch] * g.gains[ch]).sum();
            energies.push(energy);
        }

        let min = energies.iter().cloned().fold(f32::MAX, f32::min);
        let max = energies.iter().cloned().fold(f32::MIN, f32::max);
        let ratio = max / min;

        assert!(
            ratio < 2.0,
            "energy ratio {ratio:.2} (min={min:.3}, max={max:.3}) exceeds 3dB"
        );
    }

    #[test]
    fn decoder_5_1_layout() {
        let azimuths = [
            30.0_f32.to_radians(),
            -30.0_f32.to_radians(),
            0.0_f32.to_radians(),
            110.0_f32.to_radians(),
            -110.0_f32.to_radians(),
        ];
        let dec = FoaDecoder::new(&azimuths, &[0.0; 5], &[0, 1, 2, 4, 5]);

        let b = foa_encode(0.0, 0.0, 1.0);
        let g = dec.decode(&b);

        assert!(
            (g.gains[0] - g.gains[1]).abs() < 0.01,
            "FL and FR should be equal for front source"
        );
        assert!(g.gains[2] > 0.0, "center should be active for front source");
        assert!(
            g.gains[0] > g.gains[4],
            "FL should be louder than RL for front source"
        );

        let b_left = foa_encode(FRAC_PI_2, 0.0, 1.0);
        let g_left = dec.decode(&b_left);
        assert!(
            g_left.gains[0] > g_left.gains[1],
            "FL should be louder than FR for left source"
        );
    }

    // -- 3D decoder tests (elevated speakers → 4-channel path) --

    #[test]
    fn decoder_flat_array_ignores_z() {
        // Flat stereo array: Z column should be zero, elevated source should
        // decode identically in the horizontal channels.
        let dec = FoaDecoder::new(
            &[30.0_f32.to_radians(), -30.0_f32.to_radians()],
            &[0.0, 0.0],
            &[0, 1],
        );

        let b_flat = foa_encode(0.0, 0.0, 1.0);
        let b_up = foa_encode(0.0, FRAC_PI_4, 1.0);

        let g_flat = dec.decode(&b_flat);
        let g_up = dec.decode(&b_up);

        // Both should have equal L/R balance (front source)
        assert!(
            (g_flat.gains[0] - g_flat.gains[1]).abs() < 0.01,
            "flat source should have equal L/R"
        );
        assert!(
            (g_up.gains[0] - g_up.gains[1]).abs() < 0.01,
            "elevated source should still have equal L/R on flat array"
        );
    }

    #[test]
    fn decoder_with_elevated_speakers() {
        // 3D layout: 4 floor speakers (quad) + 1 overhead = 5 speakers.
        // Need ≥4 speakers for full-rank 4×4 CCᵀ.
        let azimuths = [
            FRAC_PI_4,
            -FRAC_PI_4,
            3.0 * FRAC_PI_4,
            -3.0 * FRAC_PI_4,
            0.0,
        ];
        let elevations = [0.0, 0.0, 0.0, 0.0, FRAC_PI_2];
        let dec = FoaDecoder::new(&azimuths, &elevations, &[0, 1, 2, 3, 4]);

        // Source overhead → overhead speaker (ch4) should dominate
        let b = foa_encode(0.0, FRAC_PI_2, 1.0);
        let g = dec.decode(&b);
        assert!(
            g.gains[4] > g.gains[0] && g.gains[4] > g.gains[1],
            "overhead speaker (ch4={}) should be louder than floor speakers (ch0={}, ch1={})",
            g.gains[4],
            g.gains[0],
            g.gains[1]
        );

        // Source at floor front → floor speakers should dominate over overhead
        let b_floor = foa_encode(0.0, 0.0, 1.0);
        let g_floor = dec.decode(&b_floor);
        let floor_sum: f32 = (0..4).map(|ch| g_floor.gains[ch]).sum();
        assert!(
            floor_sum > g_floor.gains[4],
            "floor speakers sum ({floor_sum}) should exceed overhead ({}) for floor source",
            g_floor.gains[4]
        );
    }
}
