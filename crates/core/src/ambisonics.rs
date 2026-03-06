//! First-Order Ambisonics (FOA) encoding and decoding.
//!
//! Implements horizontal-only FOA using the SN3D/AmbiX convention with max-rE
//! weighting. Encoding converts a source direction into B-format coefficients
//! (W, Y, X). Decoding converts B-format to speaker gains via a mode-matching
//! pseudo-inverse decoder.
//!
//! Key equations (Arteaga 2025, Zotter & Frank 2019):
//!   Encoding:  W = g/√2,  Y = g·sin(φ)·a₁,  X = g·cos(φ)·a₁
//!   Max-rE:    a₁ = cos(π/4) ≈ 0.7071  (FOA order 1)
//!   Decode:    D = Cᵀ(CCᵀ)⁻¹           (mode-matching pseudo-inverse)
//!   Per-speaker: gᵢ = D[i][0]·W + D[i][1]·Y + D[i][2]·X

use crate::listener::Listener;
use crate::speaker::{ChannelGains, Speaker, MAX_CHANNELS};

/// Max-rE weight for FOA: cos(π / (2·order + 2)) where order = 1.
/// Narrows the panning function for better perceptual localization.
pub const MAX_RE_WEIGHT: f32 = std::f32::consts::FRAC_1_SQRT_2; // cos(45°) = 1/√2

/// SN3D normalization for zeroth-order (W channel): 1/√2.
const W_NORM: f32 = std::f32::consts::FRAC_1_SQRT_2;

/// Horizontal FOA B-format coefficients (ACN ordering, SN3D normalization).
///
/// ACN 0 = W (omnidirectional pressure)
/// ACN 1 = Y (left-right, sin φ)
/// ACN 3 = X (front-back, cos φ)
///
/// Z (ACN 2, up-down) is zero for horizontal-only layouts.
#[derive(Clone, Copy, Debug)]
pub struct BFormat {
    pub w: f32,
    pub y: f32,
    pub x: f32,
}

/// Encode a source at listener-relative azimuth (radians) with gain `g`.
///
/// Azimuth convention: 0 = front (+X), π/2 = left (+Y), π = rear (-X).
/// Max-rE weighting is applied to Y and X channels.
pub fn foa_encode(azimuth: f32, gain: f32) -> BFormat {
    BFormat {
        w: gain * W_NORM,
        y: gain * azimuth.sin() * MAX_RE_WEIGHT,
        x: gain * azimuth.cos() * MAX_RE_WEIGHT,
    }
}

/// Precomputed FOA decoder for a specific speaker layout.
///
/// The decode matrix maps 3 B-format channels (W, Y, X) to N speaker gains.
/// Built once per layout change via mode-matching pseudo-inverse: D = Cᵀ(CCᵀ)⁻¹.
pub struct FoaDecoder {
    /// Nx3 decode matrix. Row i = [d_w, d_y, d_x] for speaker i.
    decode_matrix: Vec<[f32; 3]>,
    /// Output channel index for each speaker.
    channels: Vec<usize>,
}

impl FoaDecoder {
    /// Build a decoder from speaker positions relative to a listener.
    ///
    /// Speaker azimuths are computed in the listener's local frame (same as
    /// the encoder), so both sides of the B-format chain use the same reference.
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

        // Speaker azimuths in listener's local frame (matches VBAP convention)
        let azimuths: Vec<f32> = speakers[..n]
            .iter()
            .map(|s| {
                let dx = s.position.x - listener.position.x;
                let dy = s.position.y - listener.position.y;
                let local_x = dx * cos_y + dy * sin_y; // forward
                let local_y = -dx * sin_y + dy * cos_y; // left
                local_y.atan2(local_x)
            })
            .collect();

        let channels: Vec<usize> = speakers[..n].iter().map(|s| s.channel).collect();

        Self::new(&azimuths, &channels)
    }

    /// Build a decoder from speaker azimuths (radians) and channel indices.
    ///
    /// Azimuths use the same convention as encoding: 0 = front, π/2 = left.
    pub fn new(azimuths: &[f32], channels: &[usize]) -> Self {
        let n = azimuths.len();
        if n == 0 {
            return Self {
                decode_matrix: Vec::new(),
                channels: channels.to_vec(),
            };
        }

        // Build encoding matrix C (3×N):
        //   C[0][i] = 1/√2  (W normalization)
        //   C[1][i] = sin(φᵢ)
        //   C[2][i] = cos(φᵢ)
        let mut c = vec![[0.0_f32; 3]; n]; // transposed: c[i] = column i of C
        for (i, &az) in azimuths.iter().enumerate() {
            c[i][0] = W_NORM;
            c[i][1] = az.sin();
            c[i][2] = az.cos();
        }

        // Compute CCᵀ (3×3)
        let mut cct = [[0.0_f32; 3]; 3];
        for i in 0..3 {
            for j in 0..3 {
                let mut sum = 0.0_f32;
                for ck in c.iter().take(n) {
                    sum += ck[i] * ck[j];
                }
                cct[i][j] = sum;
            }
        }

        // Invert CCᵀ
        let inv = match invert_3x3(&cct) {
            Some(m) => m,
            None => {
                // Singular matrix (collinear speakers) — fall back to equal gains
                let equal = 1.0 / (n as f32).sqrt();
                return Self {
                    decode_matrix: vec![[equal, 0.0, 0.0]; n],
                    channels: channels.to_vec(),
                };
            }
        };

        // D = Cᵀ · (CCᵀ)⁻¹  — each row of D is a speaker's decode weights
        let mut decode_matrix = vec![[0.0_f32; 3]; n];
        for i in 0..n {
            for j in 0..3 {
                let mut sum = 0.0_f32;
                for k in 0..3 {
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
            let g = row[0] * bformat.w + row[1] * bformat.y + row[2] * bformat.x;
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
    use std::f32::consts::PI;

    #[test]
    fn foa_encode_front() {
        let b = foa_encode(0.0, 1.0);
        assert!((b.w - W_NORM).abs() < 1e-6, "W should be 1/√2");
        assert!(b.y.abs() < 1e-6, "Y should be 0 for front source");
        assert!(
            (b.x - MAX_RE_WEIGHT).abs() < 1e-6,
            "X should be max-rE weight"
        );
    }

    #[test]
    fn foa_encode_left() {
        let b = foa_encode(PI / 2.0, 1.0);
        assert!((b.w - W_NORM).abs() < 1e-6);
        assert!(
            (b.y - MAX_RE_WEIGHT).abs() < 1e-6,
            "Y should be max-rE for left"
        );
        assert!(b.x.abs() < 1e-6, "X should be ~0 for left");
    }

    #[test]
    fn foa_encode_rear() {
        let b = foa_encode(PI, 1.0);
        assert!((b.w - W_NORM).abs() < 1e-6);
        assert!(b.y.abs() < 1e-5, "Y should be ~0 for rear");
        assert!(
            (b.x + MAX_RE_WEIGHT).abs() < 1e-6,
            "X should be -max-rE for rear"
        );
    }

    #[test]
    fn foa_encode_zero_gain() {
        let b = foa_encode(1.0, 0.0);
        assert!(b.w.abs() < 1e-10);
        assert!(b.y.abs() < 1e-10);
        assert!(b.x.abs() < 1e-10);
    }

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
        // All rows identical — singular
        let m = [[1.0, 2.0, 3.0], [1.0, 2.0, 3.0], [1.0, 2.0, 3.0]];
        assert!(invert_3x3(&m).is_none());
    }

    #[test]
    fn decoder_stereo_front_equal() {
        // Two speakers at ±30° (typical stereo)
        let left_az = 30.0_f32.to_radians();
        let right_az = -30.0_f32.to_radians();
        let dec = FoaDecoder::new(&[left_az, right_az], &[0, 1]);

        // Source at front (azimuth 0) → equal gains
        let b = foa_encode(0.0, 1.0);
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
        let dec = FoaDecoder::new(&[left_az, right_az], &[0, 1]);

        // Source at left (azimuth 90°) → left speaker louder
        let b = foa_encode(PI / 2.0, 1.0);
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
        // 4 speakers at ±45°, ±135°
        let azimuths = [
            45.0_f32.to_radians(),
            -45.0_f32.to_radians(),
            135.0_f32.to_radians(),
            -135.0_f32.to_radians(),
        ];
        let dec = FoaDecoder::new(&azimuths, &[0, 1, 2, 3]);

        // Source at front: front pair equal, rear pair equal, front > rear
        let b = foa_encode(0.0, 1.0);
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
        // 4 speakers at ±45°, ±135°
        let azimuths = [
            45.0_f32.to_radians(),
            -45.0_f32.to_radians(),
            135.0_f32.to_radians(),
            -135.0_f32.to_radians(),
        ];
        let dec = FoaDecoder::new(&azimuths, &[0, 1, 2, 3]);

        // Check energy at several azimuths
        let mut energies = Vec::new();
        for deg in (0..360).step_by(30) {
            let az = (deg as f32).to_radians();
            let b = foa_encode(az, 1.0);
            let g = dec.decode(&b);
            let energy: f32 = (0..4).map(|ch| g.gains[ch] * g.gains[ch]).sum();
            energies.push(energy);
        }

        let min = energies.iter().cloned().fold(f32::MAX, f32::min);
        let max = energies.iter().cloned().fold(f32::MIN, f32::max);
        let ratio = max / min;

        // Energy should be within 3dB variation for FOA with max-rE
        assert!(
            ratio < 2.0,
            "energy ratio {ratio:.2} (min={min:.3}, max={max:.3}) exceeds 3dB"
        );
    }

    #[test]
    fn decoder_5_1_layout() {
        // 5.1 layout: FL 30°, FR -30°, C 0°, RL 110°, RR -110°
        let azimuths = [
            30.0_f32.to_radians(),
            -30.0_f32.to_radians(),
            0.0_f32.to_radians(),
            110.0_f32.to_radians(),
            -110.0_f32.to_radians(),
        ];
        let dec = FoaDecoder::new(&azimuths, &[0, 1, 2, 4, 5]);

        // Source at front center → center speaker should be significant
        let b = foa_encode(0.0, 1.0);
        let g = dec.decode(&b);
        eprintln!(
            "5.1 front: FL={:.4} FR={:.4} C={:.4} RL={:.4} RR={:.4}",
            g.gains[0], g.gains[1], g.gains[2], g.gains[4], g.gains[5]
        );

        // FL and FR should be symmetric
        assert!(
            (g.gains[0] - g.gains[1]).abs() < 0.01,
            "FL and FR should be equal for front source"
        );
        // Center should get gain for front source
        assert!(g.gains[2] > 0.0, "center should be active for front source");
        // Front speakers should be louder than rear for front source
        assert!(
            g.gains[0] > g.gains[4],
            "FL should be louder than RL for front source"
        );

        // Source at left → FL and RL should be louder than FR and RR
        let b_left = foa_encode(PI / 2.0, 1.0);
        let g_left = dec.decode(&b_left);
        eprintln!(
            "5.1 left:  FL={:.4} FR={:.4} C={:.4} RL={:.4} RR={:.4}",
            g_left.gains[0], g_left.gains[1], g_left.gains[2], g_left.gains[4], g_left.gains[5]
        );
        assert!(
            g_left.gains[0] > g_left.gains[1],
            "FL should be louder than FR for left source"
        );
    }
}
