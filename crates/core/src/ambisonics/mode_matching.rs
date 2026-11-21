//! Mode-matching FOA decoder: D = Cᵀ(CCᵀ)⁻¹ from the real speaker positions.

use super::encode::{BFormat, W_NORM};
use super::matrix::{decode_rows, invert_3x3, invert_4x4};
use crate::listener::Listener;
use crate::speaker::{ChannelGains, Speaker};

/// Largest speaker ring an [`FoaDecoder`] can hold, and the largest virtual
/// ring [`AllRadDecoder::from_listener_with_count`](super::AllRadDecoder::from_listener_with_count) accepts.
///
/// Decoders keep their matrices in fixed-size stack arrays so that building and
/// using one never touches the heap — the decoder is rebuilt from the listener
/// pose on the audio thread. 32 virtual speakers is a 11.25° spacing, far past
/// the point where first-order material gains anything (FOA carries four
/// channels of spatial information). Speakers beyond this bound are ignored.
pub(super) const MAX_VIRTUAL_SPEAKERS: usize = 32;

/// Precomputed FOA decoder for a specific speaker layout.
///
/// The decode matrix maps 4 B-format channels (W, Y, Z, X) to N speaker gains.
/// Built once per layout change via mode-matching pseudo-inverse: D = Cᵀ(CCᵀ)⁻¹.
///
/// Adapts to 3-channel (horizontal-only) decoding when all speakers have zero
/// elevation, storing zeros in the Z column to avoid a singular 4×4 matrix.
///
/// Holds at most [`MAX_VIRTUAL_SPEAKERS`] speakers; construction and decoding
/// perform no heap allocation.
pub struct FoaDecoder {
    /// Nx4 decode matrix. Row i = [d_w, d_y, d_z, d_x] for speaker i; only the
    /// first `speaker_count` rows are meaningful.
    pub(super) decode_matrix: [[f32; 4]; MAX_VIRTUAL_SPEAKERS],
    /// Number of valid rows in `decode_matrix`.
    speaker_count: usize,
    /// Output channel index for each speaker; only the first `channel_count`
    /// entries are meaningful.
    channels: [usize; MAX_VIRTUAL_SPEAKERS],
    /// Number of valid entries in `channels`.
    channel_count: usize,
}

impl FoaDecoder {
    /// Build a decoder from speaker positions relative to a listener.
    ///
    /// Speaker azimuths and elevations are computed in the listener's local
    /// frame (same as the encoder), so both sides of the B-format chain use
    /// the same reference.
    ///
    /// At most [`MAX_VIRTUAL_SPEAKERS`] speakers are used.
    pub fn from_listener(speakers: &[Speaker], speaker_count: usize, listener: &Listener) -> Self {
        let n = speaker_count.min(MAX_VIRTUAL_SPEAKERS);
        if n == 0 {
            return Self::without_speakers(&[]);
        }

        let cos_y = listener.yaw.cos();
        let sin_y = listener.yaw.sin();

        let mut azimuths = [0.0_f32; MAX_VIRTUAL_SPEAKERS];
        let mut elevations = [0.0_f32; MAX_VIRTUAL_SPEAKERS];
        let mut channels = [0_usize; MAX_VIRTUAL_SPEAKERS];

        for (i, s) in speakers[..n].iter().enumerate() {
            let dx = s.position.x - listener.position.x;
            let dy = s.position.y - listener.position.y;
            let dz = s.position.z - listener.position.z;
            let local_x = dx * cos_y + dy * sin_y; // forward
            let local_y = -dx * sin_y + dy * cos_y; // left
            azimuths[i] = local_y.atan2(local_x);
            let horiz = (local_x * local_x + local_y * local_y).sqrt();
            elevations[i] = dz.atan2(horiz);
            channels[i] = s.channel;
        }

        Self::new(&azimuths[..n], &elevations[..n], &channels[..n])
    }

    /// A decoder with no speakers whose channel table is `channels` (truncated
    /// to [`MAX_VIRTUAL_SPEAKERS`]).
    fn without_speakers(channels: &[usize]) -> Self {
        let channel_count = channels.len().min(MAX_VIRTUAL_SPEAKERS);
        let mut channel_table = [0_usize; MAX_VIRTUAL_SPEAKERS];
        channel_table[..channel_count].copy_from_slice(&channels[..channel_count]);
        Self {
            decode_matrix: [[0.0; 4]; MAX_VIRTUAL_SPEAKERS],
            speaker_count: 0,
            channels: channel_table,
            channel_count,
        }
    }

    /// A decoder whose first `n` rows all carry `[equal, 0, 0, 0]` (omni fallback
    /// for a singular encoding matrix).
    fn omni_fallback(n: usize, channels: &[usize]) -> Self {
        let equal = 1.0 / (n as f32).sqrt();
        let mut decoder = Self::without_speakers(channels);
        decoder.speaker_count = n;
        for row in &mut decoder.decode_matrix[..n] {
            *row = [equal, 0.0, 0.0, 0.0];
        }
        decoder
    }

    /// Build a decoder from speaker azimuths, elevations (radians), and channel indices.
    ///
    /// Azimuths use the same convention as encoding: 0 = front, π/2 = left.
    /// Elevations: 0 = horizon, π/2 = overhead.
    ///
    /// When all speakers have zero elevation, builds a 3-channel (W, Y, X)
    /// decoder to avoid the singular 4×4 matrix that arises from a flat array.
    ///
    /// At most [`MAX_VIRTUAL_SPEAKERS`] speakers are used; any beyond that are ignored.
    pub fn new(azimuths: &[f32], elevations: &[f32], channels: &[usize]) -> Self {
        let n = azimuths.len().min(MAX_VIRTUAL_SPEAKERS);
        if n == 0 {
            return Self::without_speakers(channels);
        }
        let azimuths = &azimuths[..n];

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
        let mut c = [[0.0_f32; 3]; MAX_VIRTUAL_SPEAKERS];
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
                for ck in &c[..n] {
                    sum += ck[i] * ck[j];
                }
                cct[i][j] = sum;
            }
        }

        let inv = match invert_3x3(&cct) {
            Some(m) => m,
            None => return Self::omni_fallback(n, channels),
        };

        // D = Cᵀ · (CCᵀ)⁻¹ — store in [d_w, d_y, 0, d_x] layout
        let mut decoder = Self::without_speakers(channels);
        decoder.speaker_count = n;
        let decode_matrix = &mut decoder.decode_matrix;
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

        decoder
    }

    /// 4-channel decoder (full 3D): W, Y, Z, X.
    fn build_4ch(azimuths: &[f32], elevations: &[f32], channels: &[usize]) -> Self {
        let n = azimuths.len();

        // Build encoding matrix C (4×N): W_NORM, cos(θ)·sin(φ), sin(θ), cos(θ)·cos(φ)
        let mut c = [[0.0_f32; 4]; MAX_VIRTUAL_SPEAKERS];
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
                for ck in &c[..n] {
                    sum += ck[i] * ck[j];
                }
                cct[i][j] = sum;
            }
        }

        let inv = match invert_4x4(&cct) {
            Some(m) => m,
            None => return Self::omni_fallback(n, channels),
        };

        // D = Cᵀ · (CCᵀ)⁻¹
        let mut decoder = Self::without_speakers(channels);
        decoder.speaker_count = n;
        let decode_matrix = &mut decoder.decode_matrix;
        for i in 0..n {
            for j in 0..4 {
                let mut sum = 0.0_f32;
                for k in 0..4 {
                    sum += c[i][k] * inv[k][j];
                }
                decode_matrix[i][j] = sum;
            }
        }

        decoder
    }

    /// Decode B-format coefficients to per-channel speaker gains.
    pub fn decode(&self, bformat: &BFormat) -> ChannelGains {
        decode_rows(
            &self.decode_matrix[..self.speaker_count],
            &self.channels[..self.channel_count],
            bformat,
        )
    }

    /// Number of speakers in this decoder.
    pub fn speaker_count(&self) -> usize {
        self.speaker_count
    }
}

#[cfg(test)]
mod tests;
