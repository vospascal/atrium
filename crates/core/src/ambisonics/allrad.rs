//! AllRAD decoder: FOA to a regular virtual ring, then VBAP re-panning onto the
//! real speakers, folded into one decode matrix.

use super::encode::BFormat;
use super::epad::apply_epad;
use super::matrix::decode_rows;
use super::mode_matching::{FoaDecoder, MAX_VIRTUAL_SPEAKERS};
use crate::listener::Listener;
use crate::speaker::{ChannelGains, Speaker, MAX_CHANNELS};

/// Default number of virtual speakers for AllRAD horizontal ring.
/// 12 = 30° spacing. FOA (order 1) only needs ≥4 mathematically (t-design order 2),
/// but 12 gives smoother VBAP interpolation between virtual→real re-panning.
/// Going higher (24, 50) has diminishing returns since FOA only carries 4 channels
/// of spatial information. 12 is the standard choice in literature (Zotter & Frank).
///
/// This is just the default — `from_listener_with_count()` accepts any value in
/// `4..=MAX_VIRTUAL_SPEAKERS`.
pub const DEFAULT_ALLRAD_VIRTUAL_COUNT: usize = 12;

/// AllRAD decoder: decode FOA to virtual speakers, VBAP re-pan to real speakers.
///
/// **Why AllRAD over mode-matching?** Mode-matching (D = Cᵀ(CCᵀ)⁻¹) computes the
/// decode matrix directly from real speaker positions. For irregular layouts, this
/// can produce negative gains or uneven energy. AllRAD decodes to a perfectly
/// regular virtual ring first, then uses VBAP (always well-behaved, never negative)
/// to re-pan each virtual speaker to the real layout. This smooths out irregularities.
///
/// **Why precompute?** Both operations (FOA decode and VBAP re-pan) are linear, so
/// D_combined = V_repan × D_virtual. The result is an N_real × 4 matrix — same shape
/// as mode-matching. Runtime `decode()` is identical cost: one 4-element dot product
/// per real speaker. The virtual speakers only exist at build time.
///
/// The virtual speaker count is configurable via `from_listener_with_count()`.
/// Any value ≥4 works; 12 is the default. Higher values give marginally smoother
/// re-panning at no runtime cost (only build time).
///
/// Holds at most `MAX_CHANNELS` real speakers; construction and decoding perform
/// no heap allocation.
pub struct AllRadDecoder {
    /// N_real × 4 combined decode matrix; only the first `speaker_count` rows
    /// are meaningful.
    decode_matrix: [[f32; 4]; MAX_CHANNELS],
    /// Output channel index for each real speaker (first `speaker_count` entries).
    channels: [usize; MAX_CHANNELS],
    /// Number of real speakers.
    speaker_count: usize,
}

impl AllRadDecoder {
    /// Build an AllRAD decoder with the default 12 virtual speakers.
    pub fn from_listener(speakers: &[Speaker], speaker_count: usize, listener: &Listener) -> Self {
        Self::from_listener_with_count(
            speakers,
            speaker_count,
            listener,
            DEFAULT_ALLRAD_VIRTUAL_COUNT,
        )
    }

    /// Build an AllRAD decoder with a specific number of virtual speakers.
    ///
    /// `virtual_count` must be ≥4 (minimum for FOA t-design) and is clamped to
    /// `4..=`[`MAX_VIRTUAL_SPEAKERS`] — the virtual ring lives in stack storage so
    /// building never allocates. Typical values: 12 (default, 30° spacing),
    /// 16 (22.5°), 24 (15°). Higher values give marginally smoother re-panning
    /// but only affect build time, not runtime.
    ///
    /// At most `MAX_CHANNELS` real speakers are used (the VBAP matrix is indexed
    /// by output channel).
    ///
    /// Steps:
    /// 1. Places `virtual_count` virtual speakers at equal intervals in listener-local frame.
    /// 2. Builds a standard FOA mode-matching decoder for the virtual ring.
    /// 3. For each virtual speaker, computes VBAP panning gains to real speakers.
    /// 4. Multiplies: combined[real_i][k] = Σⱼ vbap[j→real_i] × decode[j][k].
    pub fn from_listener_with_count(
        speakers: &[Speaker],
        speaker_count: usize,
        listener: &Listener,
        virtual_count: usize,
    ) -> Self {
        let virtual_count = virtual_count.clamp(4, MAX_VIRTUAL_SPEAKERS);
        let n_real = speaker_count.min(MAX_CHANNELS);
        if n_real == 0 {
            return Self {
                decode_matrix: [[0.0; 4]; MAX_CHANNELS],
                channels: [0; MAX_CHANNELS],
                speaker_count: 0,
            };
        }

        let mut channels = [0_usize; MAX_CHANNELS];
        for (channel, speaker) in channels.iter_mut().zip(&speakers[..n_real]) {
            *channel = speaker.channel;
        }

        // Step 1: Virtual speakers at equal angular intervals (listener-local frame)
        let mut virt_azimuths = [0.0f32; MAX_VIRTUAL_SPEAKERS];
        for (i, az) in virt_azimuths[..virtual_count].iter_mut().enumerate() {
            *az = (i as f32) * std::f32::consts::TAU / virtual_count as f32;
        }
        let virt_elevations = [0.0f32; MAX_VIRTUAL_SPEAKERS];
        let mut virt_channels = [0_usize; MAX_VIRTUAL_SPEAKERS];
        for (i, channel) in virt_channels[..virtual_count].iter_mut().enumerate() {
            *channel = i;
        }

        // Step 2: FOA mode-matching decoder for the virtual ring
        let virt_decoder = FoaDecoder::new(
            &virt_azimuths[..virtual_count],
            &virt_elevations[..virtual_count],
            &virt_channels[..virtual_count],
        );

        // Step 3: VBAP re-panning matrix — for each virtual speaker, compute
        // gains to real speakers. Virtual speakers are in local frame, so we
        // need real speaker angles in local frame too.
        let cos_y = listener.yaw.cos();
        let sin_y = listener.yaw.sin();

        // Compute real speaker angles in listener-local frame + sort by angle
        let mut real_angles = [(0.0_f32, 0_usize); MAX_CHANNELS];
        for (i, s) in speakers[..n_real].iter().enumerate() {
            let dx = s.position.x - listener.position.x;
            let dy = s.position.y - listener.position.y;
            let local_x = dx * cos_y + dy * sin_y;
            let local_y = -dx * sin_y + dy * cos_y;
            real_angles[i] = (local_y.atan2(local_x), i);
        }
        let real_angles = &mut real_angles[..n_real];
        // Stable insertion sort by angle: `slice::sort_by` may allocate a merge
        // buffer, and n_real ≤ MAX_CHANNELS makes insertion sort the right size.
        for unsorted in 1..real_angles.len() {
            let mut position = unsorted;
            while position > 0
                && real_angles[position - 1]
                    .0
                    .partial_cmp(&real_angles[position].0)
                    .unwrap()
                    == std::cmp::Ordering::Greater
            {
                real_angles.swap(position - 1, position);
                position -= 1;
            }
        }

        // For each virtual speaker, find its VBAP gains to real speakers
        let mut vbap_matrix = [[0.0f32; MAX_CHANNELS]; MAX_VIRTUAL_SPEAKERS];
        let vbap_matrix = &mut vbap_matrix[..virtual_count];
        for (v, vbap_row) in vbap_matrix.iter_mut().enumerate() {
            let vaz = virt_azimuths[v];
            let vx = vaz.cos();
            let vy = vaz.sin();

            let mut best_a = 0usize;
            let mut best_b = 0usize;
            let mut best_ga = 0.0f32;
            let mut best_gb = 0.0f32;
            let mut found = false;

            for pair_idx in 0..n_real {
                let idx_a = pair_idx;
                let idx_b = (pair_idx + 1) % n_real;
                let (angle_a, _si_a) = real_angles[idx_a];
                let (angle_b, _si_b) = real_angles[idx_b];

                let (ax, ay) = (angle_a.cos(), angle_a.sin());
                let (bx, by) = (angle_b.cos(), angle_b.sin());
                let det = ax * by - bx * ay;
                if det.abs() < 1e-8 {
                    continue;
                }
                let inv_det = 1.0 / det;
                let ga = (by * vx - bx * vy) * inv_det;
                let gb = (-ay * vx + ax * vy) * inv_det;

                if ga >= -1e-6 && gb >= -1e-6 {
                    found = true;
                    best_a = real_angles[idx_a].1;
                    best_b = real_angles[idx_b].1;
                    best_ga = ga.max(0.0);
                    best_gb = gb.max(0.0);
                    break;
                }
            }

            if !found {
                // Fallback: nearest real speaker
                let mut min_diff = f32::MAX;
                for &(angle, idx) in real_angles.iter() {
                    let diff = angle_diff(vaz, angle).abs();
                    if diff < min_diff {
                        min_diff = diff;
                        best_a = idx;
                        best_ga = 1.0;
                        best_gb = 0.0;
                    }
                }
                best_b = best_a;
            }

            // Constant-power normalization: ga² + gb² = 1.
            // This preserves perceived loudness regardless of how the virtual
            // speaker falls between real speakers. Without normalization, a virtual
            // speaker exactly between two real speakers would be ~3dB quieter.
            let norm = (best_ga * best_ga + best_gb * best_gb).sqrt();
            if norm > 1e-8 {
                best_ga /= norm;
                best_gb /= norm;
            }

            vbap_row[speakers[best_a].channel] = best_ga;
            if best_b != best_a {
                vbap_row[speakers[best_b].channel] = best_gb;
            }
        }

        // Step 4: Combine into a single N_real × 4 matrix.
        // combined[real_i][k] = Σⱼ vbap[j→real_i] × virt_decode[j][k]
        // This folds the two-stage decode into one matrix multiply at runtime.
        let mut decode_matrix = [[0.0f32; 4]; MAX_CHANNELS];
        for (i, speaker) in speakers[..n_real].iter().enumerate() {
            let ch = speaker.channel;
            for (k, dm) in decode_matrix[i].iter_mut().enumerate() {
                let mut sum = 0.0f32;
                for (v, vbap_row) in vbap_matrix.iter().enumerate() {
                    sum += vbap_row[ch] * virt_decoder.decode_matrix[v][k];
                }
                *dm = sum;
            }
        }

        Self {
            decode_matrix,
            channels,
            speaker_count: n_real,
        }
    }

    /// Decode B-format to per-channel speaker gains (same interface as FoaDecoder).
    pub fn decode(&self, bformat: &BFormat) -> ChannelGains {
        decode_rows(
            &self.decode_matrix[..self.speaker_count],
            &self.channels[..self.speaker_count],
            bformat,
        )
    }

    /// Build an AllRAD decoder with EPAD (Energy-Preserving AllRAD) correction.
    ///
    /// EPAD equalizes the singular values of the combined decode matrix via SVD,
    /// so that energy is uniform across all source directions. Without EPAD, our
    /// 5.1 layout shows ~3 dB energy variation; with EPAD this drops to ~0 dB.
    ///
    /// Reference: Zotter & Frank 2019, "Ambisonics" §4.9.4.
    /// Algorithm: D_EPAD = U × (σ_mean × I) × Vᵀ, where USVᵀ = SVD(D_AllRAD).
    pub fn from_listener_epad(
        speakers: &[Speaker],
        speaker_count: usize,
        listener: &Listener,
    ) -> Self {
        let mut decoder = Self::from_listener(speakers, speaker_count, listener);
        apply_epad(&mut decoder.decode_matrix[..decoder.speaker_count]);
        decoder
    }

    /// Number of real speakers in this decoder.
    pub fn speaker_count(&self) -> usize {
        self.speaker_count
    }
}

/// Angle difference wrapped to [-π, π].
pub(super) fn angle_diff(a: f32, b: f32) -> f32 {
    let mut d = a - b;
    while d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    while d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    d
}

#[cfg(test)]
mod tests {
    use super::super::encode::foa_encode;
    use super::super::test_support::{
        assert_close, origin_listener, speaker_at, test_5_1_speakers, test_listener,
    };
    use super::*;
    use std::f32::consts::FRAC_PI_2;

    #[test]
    fn allrad_front_source_balanced_lr() {
        let speakers = test_5_1_speakers();
        let listener = test_listener();
        let dec = AllRadDecoder::from_listener(&speakers, 6, &listener);

        let b = foa_encode(0.0, 0.0, 1.0); // front
        let g = dec.decode(&b);

        // FL (ch0) and FR (ch1) should be roughly equal for a centered front source
        assert!(
            (g.gains[0] - g.gains[1]).abs() < 0.15,
            "AllRAD front: FL={:.3} and FR={:.3} should be roughly equal",
            g.gains[0],
            g.gains[1]
        );
    }

    #[test]
    fn allrad_left_source_more_fl() {
        let speakers = test_5_1_speakers();
        let listener = test_listener();
        let dec = AllRadDecoder::from_listener(&speakers, 6, &listener);

        let b = foa_encode(FRAC_PI_2, 0.0, 1.0); // left
        let g = dec.decode(&b);

        let left_sum = g.gains[0] + g.gains[4]; // FL + RL
        let right_sum = g.gains[1] + g.gains[5]; // FR + RR
        assert!(
            left_sum > right_sum,
            "AllRAD left source: left channels ({:.3}) should exceed right ({:.3})",
            left_sum,
            right_sum
        );
    }

    #[test]
    fn allrad_energy_roughly_constant() {
        let speakers = test_5_1_speakers();
        let listener = test_listener();
        let dec = AllRadDecoder::from_listener(&speakers, 6, &listener);

        let mut energies = Vec::new();
        for deg in (0..360).step_by(30) {
            let az = (deg as f32).to_radians();
            let b = foa_encode(az, 0.0, 1.0);
            let g = dec.decode(&b);
            let energy: f32 = (0..6).map(|ch| g.gains[ch] * g.gains[ch]).sum();
            energies.push(energy);
        }

        let min = energies.iter().cloned().fold(f32::MAX, f32::min);
        let max = energies.iter().cloned().fold(f32::MIN, f32::max);
        let ratio = max / min.max(1e-10);
        assert!(
            ratio < 4.0,
            "AllRAD energy ratio {ratio:.2} (min={min:.4}, max={max:.4}) exceeds 6dB"
        );
    }

    #[test]
    fn allrad_vs_mode_matching_different_output() {
        // AllRAD and mode-matching should produce meaningfully different gains
        // (AllRAD smooths via virtual speakers).
        let speakers = test_5_1_speakers();
        let listener = test_listener();

        let allrad = AllRadDecoder::from_listener(&speakers, 6, &listener);
        let mode_match = FoaDecoder::from_listener(&speakers, 6, &listener);

        let b = foa_encode(0.7, 0.0, 1.0); // off-axis
        let g_allrad = allrad.decode(&b);
        let g_mm = mode_match.decode(&b);

        let diff: f32 = (0..6)
            .map(|ch| (g_allrad.gains[ch] - g_mm.gains[ch]).abs())
            .sum();
        // They should be different (AllRAD applies VBAP re-panning)
        assert!(
            diff > 0.001,
            "AllRAD and mode-matching should differ: diff={diff:.6}"
        );
    }

    #[test]
    fn allrad_virtual_counts_12_16_24_all_valid() {
        let speakers = test_5_1_speakers();
        let listener = test_listener();

        for &count in &[12, 16, 24] {
            let dec = AllRadDecoder::from_listener_with_count(&speakers, 6, &listener, count);

            // Front source should produce nonzero, balanced FL/FR
            let b_front = foa_encode(0.0, 0.0, 1.0);
            let g = dec.decode(&b_front);
            let energy: f32 = g.gains[..6].iter().map(|x| x * x).sum();
            assert!(
                energy > 0.01,
                "virtual_count={count}: front source energy too low: {energy:.4}"
            );
            assert!(
                (g.gains[0] - g.gains[1]).abs() < 0.15,
                "virtual_count={count}: FL={:.3} and FR={:.3} should be roughly equal",
                g.gains[0],
                g.gains[1]
            );

            // Left source should favor FL over FR
            let b_left = foa_encode(FRAC_PI_2, 0.0, 1.0);
            let g_left = dec.decode(&b_left);
            assert!(
                g_left.gains[0] > g_left.gains[1],
                "virtual_count={count}: left source FL={:.3} should exceed FR={:.3}",
                g_left.gains[0],
                g_left.gains[1]
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // Audit 2026-11: hand-derived values and invariants
    // ─────────────────────────────────────────────────────────────────────

    #[test]
    fn allrad_with_aligned_virtual_ring_equals_mode_matching() {
        // Real speakers at 0°, 90°, 180°, 270° and 4 virtual speakers at the same
        // angles: VBAP re-panning is the identity, so AllRAD = mode-matching:
        //   front source: 0° → ¼(1+√2) = 0.603553, ±90° → ¼, 180° → ¼(1−√2) = −0.103553
        let speakers = vec![
            speaker_at(1.0, 0.0, 0.0, 0),
            speaker_at(0.0, 1.0, 0.0, 1),
            speaker_at(-1.0, 0.0, 0.0, 2),
            speaker_at(0.0, -1.0, 0.0, 3),
        ];
        let decoder = AllRadDecoder::from_listener_with_count(&speakers, 4, &origin_listener(), 4);
        let gains = decoder.decode(&foa_encode(0.0, 0.0, 1.0));
        for (channel, expected) in [0.603553, 0.25, -0.103553, 0.25].iter().enumerate() {
            assert_close(gains.gains[channel], *expected, 1e-5, "aligned AllRAD");
        }
    }

    #[test]
    fn allrad_eight_virtual_on_aligned_quad_hand_derived() {
        // 8 virtual speakers (45° spacing) on real speakers at 0/90/180/270°.
        // Virtual decoder rows: dᵥ = (2/8)·[1/√2, sin φᵥ, cos φᵥ].
        // On-speaker virtuals map with weight 1; the 45° virtuals split
        // 1/√2 : 1/√2 (power-normalised 2D VBAP). Row for the 0° speaker:
        //   W = ¼·(1/√2)·(1 + 2/√2) = 0.426777,  Y = 0,  X = ¼·(1 + 2·½) = 0.5
        // Front source (W = X = 1/√2):
        //   0°: 0.426777/√2 + 0.5/√2 = 0.655330
        //   ±90°: 0.301777,   180°: 0.301777 − 0.353553 = −0.051777
        // The W row is (1+√2)/2 = 1.207× the mode-matching value 0.353553:
        // coherent summation in power-normalised VBAP lifts the omni part 1.64 dB.
        let speakers = vec![
            speaker_at(1.0, 0.0, 0.0, 0),
            speaker_at(0.0, 1.0, 0.0, 1),
            speaker_at(-1.0, 0.0, 0.0, 2),
            speaker_at(0.0, -1.0, 0.0, 3),
        ];
        let decoder = AllRadDecoder::from_listener_with_count(&speakers, 4, &origin_listener(), 8);
        assert_close(decoder.decode_matrix[0][0], 0.426777, 1e-5, "W row");
        assert_close(decoder.decode_matrix[0][1], 0.0, 1e-5, "Y row");
        assert_close(decoder.decode_matrix[0][2], 0.0, 1e-7, "Z row");
        assert_close(decoder.decode_matrix[0][3], 0.5, 1e-5, "X row");
        let gains = decoder.decode(&foa_encode(0.0, 0.0, 1.0));
        for (channel, expected) in [0.655330, 0.301777, -0.051777, 0.301777].iter().enumerate() {
            assert_close(gains.gains[channel], *expected, 1e-5, "8-virtual AllRAD");
        }
    }
}
