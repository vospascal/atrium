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

/// Largest speaker ring an [`FoaDecoder`] can hold, and the largest virtual
/// ring [`AllRadDecoder::from_listener_with_count`] accepts.
///
/// Decoders keep their matrices in fixed-size stack arrays so that building and
/// using one never touches the heap — the decoder is rebuilt from the listener
/// pose on the audio thread. 32 virtual speakers is a 11.25° spacing, far past
/// the point where first-order material gains anything (FOA carries four
/// channels of spatial information). Speakers beyond this bound are ignored.
const MAX_VIRTUAL_SPEAKERS: usize = 32;

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
    decode_matrix: [[f32; 4]; MAX_VIRTUAL_SPEAKERS],
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

/// Apply a decode matrix (one `[w, y, z, x]` row per speaker) to a B-format
/// signal, writing each speaker's gain to its output channel. Rows without a
/// channel entry are skipped.
fn decode_rows(rows: &[[f32; 4]], channels: &[usize], bformat: &BFormat) -> ChannelGains {
    let mut gains = ChannelGains::silent(MAX_CHANNELS);
    for (i, row) in rows.iter().enumerate() {
        let g = row[0] * bformat.w + row[1] * bformat.y + row[2] * bformat.z + row[3] * bformat.x;
        if i < channels.len() {
            gains.gains[channels[i]] = g;
        }
    }
    gains
}

// ─────────────────────────────────────────────────────────────────────────────
// AllRAD Decoder (All-Round Ambisonic Decoding)
// ─────────────────────────────────────────────────────────────────────────────

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

/// Apply EPAD (Energy-Preserving AllRAD) correction to a decode matrix.
///
/// Replaces all singular values with their mean, equalizing energy across
/// source directions while preserving the spatial mapping.
///
/// For the N×4 decode matrix D:
///   1. Compute Dᵀ×D (4×4 symmetric positive semi-definite)
///   2. Eigendecompose via Jacobi rotations → V, λ
///   3. σᵢ = √λᵢ, σ_mean = mean of non-zero σ
///   4. D_EPAD = D × V × diag(σ_mean/σᵢ) × Vᵀ
fn apply_epad(decode_matrix: &mut [[f32; 4]]) {
    let n = decode_matrix.len();
    if n == 0 {
        return;
    }

    // Compute Dᵀ×D (4×4)
    let mut dtd = [[0.0f32; 4]; 4];
    for k in 0..4 {
        for l in k..4 {
            let mut sum = 0.0f32;
            for row in decode_matrix.iter() {
                sum += row[k] * row[l];
            }
            dtd[k][l] = sum;
            dtd[l][k] = sum;
        }
    }

    let (eigenvectors, eigenvalues) = jacobi_eigen_4x4(&dtd);

    // Singular values = √eigenvalues, compute mean of non-zero
    let mut sigmas = [0.0f32; 4];
    let mut sigma_sum = 0.0f32;
    let mut count = 0;
    for i in 0..4 {
        sigmas[i] = eigenvalues[i].max(0.0).sqrt();
        if sigmas[i] > 1e-6 {
            sigma_sum += sigmas[i];
            count += 1;
        }
    }
    if count == 0 {
        return;
    }
    let sigma_mean = sigma_sum / count as f32;

    // Scale factors: σ_mean / σᵢ
    let mut scales = [0.0f32; 4];
    for i in 0..4 {
        scales[i] = if sigmas[i] > 1e-6 {
            sigma_mean / sigmas[i]
        } else {
            0.0
        };
    }

    // Apply: D_EPAD = D × V × diag(scales) × Vᵀ
    // Row i of the result depends only on row i of the input, so rows are
    // rewritten in place.
    for row in decode_matrix.iter_mut() {
        let mut new_row = [0.0f32; 4];
        for k in 0..4 {
            let mut sum = 0.0f32;
            for j in 0..4 {
                // (D × V)[i][j] = Σ_l D[i][l] × V[l][j]
                let dv: f32 = (0..4).map(|l| row[l] * eigenvectors[l][j]).sum();
                // × scales[j] × Vᵀ[j][k] = V[k][j]
                sum += dv * scales[j] * eigenvectors[k][j];
            }
            new_row[k] = sum;
        }
        *row = new_row;
    }
}

/// Jacobi eigendecomposition of a 4×4 symmetric matrix.
///
/// Returns (eigenvectors, eigenvalues) where eigenvectors[col][row] contains
/// the eigenvector components. Converges in a few sweeps for small matrices.
#[allow(clippy::needless_range_loop)]
fn jacobi_eigen_4x4(matrix: &[[f32; 4]; 4]) -> ([[f32; 4]; 4], [f32; 4]) {
    let mut a = *matrix;
    let mut v = [[0.0f32; 4]; 4];
    for i in 0..4 {
        v[i][i] = 1.0;
    }

    // 20 sweeps is far more than needed for 4×4 convergence
    for _ in 0..20 {
        for p in 0..4 {
            for q in (p + 1)..4 {
                if a[p][q].abs() < 1e-12 {
                    continue;
                }

                // Compute Jacobi rotation to zero a[p][q]
                let tau = (a[q][q] - a[p][p]) / (2.0 * a[p][q]);
                let t = if tau >= 0.0 {
                    1.0 / (tau + (1.0 + tau * tau).sqrt())
                } else {
                    -1.0 / (-tau + (1.0 + tau * tau).sqrt())
                };
                let c = 1.0 / (1.0 + t * t).sqrt();
                let s = t * c;

                // Update diagonal and zero the (p,q) pair
                let app = a[p][p] - t * a[p][q];
                let aqq = a[q][q] + t * a[p][q];
                a[p][p] = app;
                a[q][q] = aqq;
                a[p][q] = 0.0;
                a[q][p] = 0.0;

                // Update off-diagonal elements in rows/columns p and q
                for r in 0..4 {
                    if r == p || r == q {
                        continue;
                    }
                    let arp = a[r][p];
                    let arq = a[r][q];
                    a[r][p] = c * arp - s * arq;
                    a[p][r] = a[r][p];
                    a[r][q] = s * arp + c * arq;
                    a[q][r] = a[r][q];
                }

                // Accumulate eigenvector rotations
                for r in 0..4 {
                    let vrp = v[r][p];
                    let vrq = v[r][q];
                    v[r][p] = c * vrp - s * vrq;
                    v[r][q] = s * vrp + c * vrq;
                }
            }
        }
    }

    let eigenvalues = [a[0][0], a[1][1], a[2][2], a[3][3]];
    (v, eigenvalues)
}

/// Angle difference wrapped to [-π, π].
fn angle_diff(a: f32, b: f32) -> f32 {
    let mut d = a - b;
    while d > std::f32::consts::PI {
        d -= std::f32::consts::TAU;
    }
    while d < -std::f32::consts::PI {
        d += std::f32::consts::TAU;
    }
    d
}

// ─────────────────────────────────────────────────────────────────────────────
// B-format rotation
// ─────────────────────────────────────────────────────────────────────────────

/// Rotate B-format around the Z (vertical) axis by `angle` radians.
///
/// Standard SN3D/AmbiX Z-rotation for FOA (Zotter & Frank 2019, §3.2):
///
/// ```text
/// [Y']   [ cos(α)  0  -sin(α) ] [Y]
/// [Z'] = [   0     1     0    ] [Z]
/// [X']   [ sin(α)  0   cos(α) ] [X]
/// ```
///
/// W (order 0) is omnidirectional → invariant under rotation.
/// Z is the vertical axis → invariant under yaw rotation.
/// Y and X encode sin(φ) and cos(φ) → rotate as a 2D vector.
/// This is an energy-preserving orthogonal transform (det R = 1).
///
/// **Why only Z rotation?** The ears are offset horizontally (left/right), so the
/// relevant parallax is in the azimuthal plane. Pitch/roll ear offsets are negligible
/// for a stationary listener. This keeps the operation to 2 multiplies + 2 adds.
pub fn foa_rotate_z(b: &BFormat, angle: f32) -> BFormat {
    let c = angle.cos();
    let s = angle.sin();
    BFormat {
        w: b.w,
        y: b.y * c - b.x * s,
        z: b.z,
        x: b.y * s + b.x * c,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Binaural Ambisonics Decoder (stereo headphone output)
// ─────────────────────────────────────────────────────────────────────────────

/// Bilateral ambisonics decoder for stereo headphone output.
///
/// Implements Bilateral Ambisonics (Ben-Hur et al. 2020): rotates B-format
/// by ±π/2 (90°) per ear, aligning each ear's ipsilateral hemisphere with
/// the "front" axis. The cardioid decode then naturally emphasizes sounds
/// on each ear's side.
///
/// Without HRTFs, we use forward-facing cardioid weights after rotation:
///   ear = W × √2 × 0.5 + X' × 0.5
/// (the √2 undoes the SN3D 1/√2 on W), where X' is the rotated forward axis
/// (was Y before the ±90° rotation).
///
/// Reference: Ben-Hur et al. 2020, "Binaural Reproduction Based on
/// Bilateral Ambisonics and Ear-Aligned HRTFs."
#[derive(Clone, Copy)]
pub struct BilateralDecoder;

impl BilateralDecoder {
    pub fn new() -> Self {
        Self
    }

    /// Decode B-format to stereo (left, right) for headphone output.
    pub fn decode_stereo(&self, bformat: &BFormat) -> (f32, f32) {
        let sqrt2 = std::f32::consts::SQRT_2;

        // ±90° rotation aligns each ear's ipsilateral direction with "front" (X axis).
        let b_left = foa_rotate_z(bformat, std::f32::consts::FRAC_PI_2);
        let b_right = foa_rotate_z(bformat, -std::f32::consts::FRAC_PI_2);

        // Forward-facing cardioid decode (X is primary after ±90° rotation):
        //   W × √2 × 0.5 — omnidirectional pressure
        //   X' × 0.5 — ipsilateral emphasis (was Y before rotation)
        let left = b_left.w * sqrt2 * 0.5 + b_left.x * 0.5;
        let right = b_right.w * sqrt2 * 0.5 + b_right.x * 0.5;
        (left, right)
    }
}

impl Default for BilateralDecoder {
    fn default() -> Self {
        Self::new()
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

    #[test]
    fn decoder_5_1_energy_variation() {
        // Baseline energy variation for mode-matching on an asymmetric 5.1 layout.
        // 5.1 has uneven angular spacing (60° front arc, 140° rear gap), so
        // mode-matching produces more variation than on symmetric quad.
        let azimuths = [
            30.0_f32.to_radians(),
            -30.0_f32.to_radians(),
            0.0_f32.to_radians(),
            110.0_f32.to_radians(),
            -110.0_f32.to_radians(),
        ];
        let dec = FoaDecoder::new(&azimuths, &[0.0; 5], &[0, 1, 2, 4, 5]);

        let mut energies = Vec::new();
        for deg in (0..360).step_by(10) {
            let az = (deg as f32).to_radians();
            let b = foa_encode(az, 0.0, 1.0);
            let g = dec.decode(&b);
            let energy: f32 = [0, 1, 2, 4, 5]
                .iter()
                .map(|&ch| g.gains[ch] * g.gains[ch])
                .sum();
            energies.push((deg, energy));
        }

        let min_e = energies.iter().map(|e| e.1).fold(f32::MAX, f32::min);
        let max_e = energies.iter().map(|e| e.1).fold(f32::MIN, f32::max);
        let ratio_db = 10.0 * (max_e / min_e.max(1e-10)).log10();

        // Mode-matching on 5.1 typically has ~6-9 dB energy variation.
        // This test documents the baseline; AllRAD (4.6) should improve it.
        assert!(max_e > 0.0, "decoder should produce nonzero energy");
        assert!(
            ratio_db < 12.0,
            "mode-matching 5.1 energy variation {ratio_db:.1} dB exceeds 12 dB cap \
             (min={min_e:.4} at {}°, max={max_e:.4} at {}°)",
            energies
                .iter()
                .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .unwrap()
                .0,
            energies
                .iter()
                .max_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
                .unwrap()
                .0,
        );

        // Print baseline for reference (visible with cargo test -- --nocapture)
        eprintln!(
            "Mode-matching 5.1 energy: min={min_e:.4}, max={max_e:.4}, variation={ratio_db:.1} dB"
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

    // -- B-format rotation tests --

    #[test]
    fn foa_rotate_z_identity() {
        let b = foa_encode(0.0, 0.0, 1.0);
        let rotated = foa_rotate_z(&b, 0.0);
        assert!((rotated.w - b.w).abs() < 1e-6);
        assert!((rotated.y - b.y).abs() < 1e-6);
        assert!((rotated.z - b.z).abs() < 1e-6);
        assert!((rotated.x - b.x).abs() < 1e-6);
    }

    #[test]
    fn foa_rotate_z_90_front_becomes_right() {
        // Front source (azimuth=0): X positive, Y zero.
        // foa_rotate_z turns the *head* by +angle (the field by −angle), so a
        // +90° turn puts a front source on the listener's right:
        //   Y' = Y·cos α − X·sin α = 0 − a·1 = −a  (negative Y = right).
        let b = foa_encode(0.0, 0.0, 1.0);
        let rotated = foa_rotate_z(&b, FRAC_PI_2);
        assert!((rotated.w - b.w).abs() < 1e-6, "W unchanged by rotation");
        assert!(
            (rotated.z - b.z).abs() < 1e-6,
            "Z unchanged by yaw rotation"
        );
        assert!(
            (rotated.y + MAX_RE_WEIGHT).abs() < 1e-6,
            "front source after +90° head turn should sit at the right (Y = −a): Y={}",
            rotated.y
        );
        assert!(
            rotated.x.abs() < 1e-5,
            "X should be ~0 after 90° rotation: X={}",
            rotated.x
        );
    }

    #[test]
    fn foa_rotate_z_preserves_energy() {
        let b = foa_encode(0.7, 0.3, 1.0); // arbitrary direction
        let energy_before = b.w * b.w + b.y * b.y + b.z * b.z + b.x * b.x;
        for deg in (0..360).step_by(30) {
            let angle = (deg as f32).to_radians();
            let r = foa_rotate_z(&b, angle);
            let energy_after = r.w * r.w + r.y * r.y + r.z * r.z + r.x * r.x;
            assert!(
                (energy_before - energy_after).abs() < 1e-5,
                "rotation should preserve energy at {}°: before={:.4}, after={:.4}",
                deg,
                energy_before,
                energy_after
            );
        }
    }

    // -- AllRAD decoder tests --

    fn test_5_1_speakers() -> Vec<Speaker> {
        use crate::types::Vec3;
        vec![
            Speaker {
                position: Vec3::new(0.0, 4.0, 0.0),
                channel: 0,
            }, // FL
            Speaker {
                position: Vec3::new(6.0, 4.0, 0.0),
                channel: 1,
            }, // FR
            Speaker {
                position: Vec3::new(3.0, 4.0, 0.0),
                channel: 2,
            }, // C
            Speaker {
                position: Vec3::new(3.0, 2.0, 0.0),
                channel: 3,
            }, // LFE (at listener)
            Speaker {
                position: Vec3::new(0.0, 0.0, 0.0),
                channel: 4,
            }, // RL
            Speaker {
                position: Vec3::new(6.0, 0.0, 0.0),
                channel: 5,
            }, // RR
        ]
    }

    fn test_listener() -> Listener {
        use crate::types::Vec3;
        Listener::new(Vec3::new(3.0, 2.0, 0.0), FRAC_PI_2) // facing +Y
    }

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

    // -- Bilateral decoder tests --

    #[test]
    fn bilateral_front_source_balanced() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(0.0, 0.0, 1.0); // front center
        let (l, r) = bilateral.decode_stereo(&b);
        assert!(
            (l - r).abs() < 0.05,
            "bilateral front: L={l:.4} and R={r:.4} should be roughly equal"
        );
    }

    #[test]
    fn bilateral_left_source_louder_left() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(FRAC_PI_2, 0.0, 1.0); // left
        let (l, r) = bilateral.decode_stereo(&b);
        assert!(
            l > r,
            "bilateral left source: L={l:.4} should be louder than R={r:.4}"
        );
    }

    #[test]
    fn bilateral_right_source_louder_right() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(-FRAC_PI_2, 0.0, 1.0); // right
        let (l, r) = bilateral.decode_stereo(&b);
        assert!(
            r > l,
            "bilateral right source: R={r:.4} should be louder than L={l:.4}"
        );
    }

    #[test]
    fn bilateral_nonzero_output() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(0.5, 0.0, 1.0);
        let (l, r) = bilateral.decode_stereo(&b);
        assert!(l.abs() > 0.01, "left output should be nonzero: {l}");
        assert!(r.abs() > 0.01, "right output should be nonzero: {r}");
    }

    #[test]
    fn bilateral_strong_ild_at_90_deg() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(FRAC_PI_2, 0.0, 1.0); // source at 90° left
        let (l, r) = bilateral.decode_stereo(&b);
        let ild = (l - r).abs();
        assert!(ild > 0.5, "bilateral ILD at 90° too low: {ild:.4}");
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

    // -- Gotcha: independent channel compression destroys spatial image --
    // (Zotter & Frank 2019: "NEVER compress individual Ambisonics channels
    // independently — use omnidirectional channel as sidechain for all channels.")

    #[test]
    fn independent_channel_compression_destroys_direction() {
        // Encode a source at 45° left.
        let b = foa_encode(FRAC_PI_4, 0.0, 1.0);

        // "Compress" only the Y channel (simulate independent dynamics).
        // A compressor hitting Y harder than W/X shifts the perceived direction.
        let b_compressed = BFormat {
            w: b.w,
            y: b.y * 0.5, // Y compressed by 6 dB independently
            z: b.z,
            x: b.x,
        };

        // Decode both with mode-matching on a 5.1 layout.
        let speakers = test_5_1_speakers();
        let listener = test_listener();
        let dec = FoaDecoder::from_listener(&speakers, 6, &listener);

        let g_original = dec.decode(&b);
        let g_compressed = dec.decode(&b_compressed);

        // The gain distributions should be measurably different — compressing
        // Y independently shifts the spatial image (Zotter & Frank 2019).
        let mut total_diff = 0.0_f32;
        for ch in 0..6 {
            total_diff += (g_original.gains[ch] - g_compressed.gains[ch]).abs();
        }
        assert!(
            total_diff > 0.1,
            "independent Y compression should change gain distribution, \
             but total diff was only {total_diff:.4}"
        );

        // FL gain (ch0) should change — it carries the Y component most.
        let fl_diff = (g_original.gains[0] - g_compressed.gains[0]).abs();
        assert!(
            fl_diff > 0.01,
            "FL gain should shift when Y is compressed: \
             original={:.4}, compressed={:.4}",
            g_original.gains[0],
            g_compressed.gains[0]
        );
    }

    #[test]
    fn epad_reduces_energy_variation() {
        // EPAD should produce significantly more uniform energy than plain AllRAD.
        let speakers = test_5_1_speakers();
        let listener = test_listener();
        let allrad = AllRadDecoder::from_listener(&speakers, 6, &listener);
        let epad = AllRadDecoder::from_listener_epad(&speakers, 6, &listener);

        let mut allrad_energies = Vec::new();
        let mut epad_energies = Vec::new();

        for deg in 0..360 {
            let az = (deg as f32).to_radians();
            let b = foa_encode(az, 0.0, 1.0);

            let g_allrad = allrad.decode(&b);
            let e_allrad: f32 = (0..6).map(|ch| g_allrad.gains[ch].powi(2)).sum();
            allrad_energies.push(e_allrad);

            let g_epad = epad.decode(&b);
            let e_epad: f32 = (0..6).map(|ch| g_epad.gains[ch].powi(2)).sum();
            epad_energies.push(e_epad);
        }

        let allrad_min = allrad_energies.iter().cloned().fold(f32::MAX, f32::min);
        let allrad_max = allrad_energies.iter().cloned().fold(f32::MIN, f32::max);
        let allrad_db = 10.0 * (allrad_max / allrad_min.max(1e-10)).log10();

        let epad_min = epad_energies.iter().cloned().fold(f32::MAX, f32::min);
        let epad_max = epad_energies.iter().cloned().fold(f32::MIN, f32::max);
        let epad_db = 10.0 * (epad_max / epad_min.max(1e-10)).log10();

        eprintln!("AllRAD energy variation: {allrad_db:.2} dB");
        eprintln!("EPAD   energy variation: {epad_db:.2} dB");

        // EPAD should be strictly better than AllRAD
        assert!(
            epad_db < allrad_db,
            "EPAD ({epad_db:.2} dB) should have less variation than AllRAD ({allrad_db:.2} dB)"
        );
        // EPAD should get energy variation under 1 dB
        assert!(
            epad_db < 1.0,
            "EPAD energy variation {epad_db:.2} dB exceeds 1 dB target"
        );
    }

    #[test]
    fn uniform_compression_preserves_direction() {
        // Same source at 45° left, but compress ALL channels equally.
        let b = foa_encode(FRAC_PI_4, 0.0, 1.0);
        let gain = 0.5; // 6 dB uniform compression
        let b_compressed = BFormat {
            w: b.w * gain,
            y: b.y * gain,
            z: b.z * gain,
            x: b.x * gain,
        };

        let speakers = test_5_1_speakers();
        let listener = test_listener();
        let dec = FoaDecoder::from_listener(&speakers, 6, &listener);

        let g_original = dec.decode(&b);
        let g_compressed = dec.decode(&b_compressed);

        // Per-channel gain ratios should be identical (direction preserved).
        for ch in 0..6 {
            let ratio_orig = g_original.gains[ch] / (g_original.gains[0] + 1e-10);
            let ratio_comp = g_compressed.gains[ch] / (g_compressed.gains[0] + 1e-10);
            assert!(
                (ratio_orig - ratio_comp).abs() < 0.01,
                "ch{ch}: uniform compression should preserve gain ratios: \
                 orig={ratio_orig:.4}, comp={ratio_comp:.4}"
            );
        }
    }

    // ─────────────────────────────────────────────────────────────────────
    // Audit 2026-11: hand-derived values and invariants
    // ─────────────────────────────────────────────────────────────────────

    fn assert_close(actual: f32, expected: f32, tolerance: f32, label: &str) {
        assert!(
            (actual - expected).abs() < tolerance,
            "{label}: got {actual}, expected {expected} (±{tolerance})"
        );
    }

    /// Unweighted first-order basis (max-rE removed) in ACN order W, Y, Z, X.
    fn unweighted_basis(azimuth: f32, elevation: f32) -> [f64; 4] {
        let bformat = foa_encode(azimuth, elevation, 1.0);
        [
            bformat.w as f64,
            (bformat.y / MAX_RE_WEIGHT) as f64,
            (bformat.z / MAX_RE_WEIGHT) as f64,
            (bformat.x / MAX_RE_WEIGHT) as f64,
        ]
    }

    /// Energy vector rE = Σ gᵢ² uᵢ / Σ gᵢ² for a horizontal ring.
    /// Returns (magnitude, direction in radians).
    fn energy_vector(gains: &[f32], azimuths: &[f32]) -> (f32, f32) {
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        let mut sum_energy = 0.0;
        for (gain, azimuth) in gains.iter().zip(azimuths) {
            let energy = gain * gain;
            sum_x += energy * azimuth.cos();
            sum_y += energy * azimuth.sin();
            sum_energy += energy;
        }
        let (x, y) = (sum_x / sum_energy, sum_y / sum_energy);
        ((x * x + y * y).sqrt(), y.atan2(x))
    }

    /// Velocity vector rV = Σ gᵢ uᵢ / Σ gᵢ for a horizontal ring.
    fn velocity_vector_magnitude(gains: &[f32], azimuths: &[f32]) -> f32 {
        let mut sum_x = 0.0;
        let mut sum_y = 0.0;
        let mut sum_gain = 0.0;
        for (gain, azimuth) in gains.iter().zip(azimuths) {
            sum_x += gain * azimuth.cos();
            sum_y += gain * azimuth.sin();
            sum_gain += gain;
        }
        ((sum_x * sum_x + sum_y * sum_y).sqrt()) / sum_gain
    }

    fn gains_for(decoded: &ChannelGains, channels: &[usize]) -> Vec<f32> {
        channels
            .iter()
            .map(|&channel| decoded.gains[channel])
            .collect()
    }

    fn speaker_at(x: f32, y: f32, z: f32, channel: usize) -> Speaker {
        use crate::types::Vec3;
        Speaker {
            position: Vec3::new(x, y, z),
            channel,
        }
    }

    /// ITU-R BS.775 5.0 ring (radius 2 m) around a listener at the origin
    /// facing +X: L 30°, R −30°, C 0°, Ls 110°, Rs −110°. Channels 0,1,2,4,5.
    fn itu_5_0_speakers() -> Vec<Speaker> {
        let place = |degrees: f32, channel: usize| {
            let azimuth = degrees.to_radians();
            speaker_at(2.0 * azimuth.cos(), 2.0 * azimuth.sin(), 0.0, channel)
        };
        vec![
            place(30.0, 0),
            place(-30.0, 1),
            place(0.0, 2),
            place(110.0, 4),
            place(-110.0, 5),
        ]
    }

    fn origin_listener() -> Listener {
        Listener::new(crate::types::Vec3::ZERO, 0.0)
    }

    #[test]
    fn spherical_harmonic_gram_matrix_is_fuma_w_with_sn3d_first_order() {
        // ∫ Yₙ Yₘ dΩ / 4π over the sphere (midpoint quadrature, cos-weighted).
        // SN3D first order: ‖Y‖² = ‖Z‖² = ‖X‖² = 1/3 (e.g. ∫cos²θ sin²φ dΩ/4π = 1/3).
        // W here is 1/√2 (FuMa scaling), so ‖W‖² = 1/2 — not SN3D/AmbiX's 1.
        // Off-diagonal terms vanish (orthogonality). Encoder and decoders share
        // W_NORM, so the convention is self-consistent inside the engine.
        let elevation_steps = 180;
        let azimuth_steps = 360;
        let mut gram = [[0.0f64; 4]; 4];
        let mut total_weight = 0.0f64;
        for elevation_index in 0..elevation_steps {
            let elevation = -std::f64::consts::FRAC_PI_2
                + (elevation_index as f64 + 0.5) * std::f64::consts::PI / elevation_steps as f64;
            let weight = elevation.cos();
            for azimuth_index in 0..azimuth_steps {
                let azimuth =
                    (azimuth_index as f64 + 0.5) * std::f64::consts::TAU / azimuth_steps as f64;
                let basis = unweighted_basis(azimuth as f32, elevation as f32);
                for row in 0..4 {
                    for column in 0..4 {
                        gram[row][column] += weight * basis[row] * basis[column];
                    }
                }
                total_weight += weight;
            }
        }
        let expected_diagonal = [0.5, 1.0 / 3.0, 1.0 / 3.0, 1.0 / 3.0];
        for (row, gram_row) in gram.iter().enumerate() {
            for (column, sum) in gram_row.iter().enumerate() {
                let value = sum / total_weight;
                let expected = if row == column {
                    expected_diagonal[row]
                } else {
                    0.0
                };
                assert!(
                    (value - expected).abs() < 1e-4,
                    "Gram[{row}][{column}] = {value}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn circular_harmonic_gram_matrix_on_horizon_is_uniform_one_half() {
        // On the horizontal circle the (W, Y, X) basis with W = 1/√2 is
        // orthogonal with equal norms: mean(W²) = 1/2, mean(sin²φ) = mean(cos²φ)
        // = 1/2. Equal norms are why EPAD energy is exactly azimuth-independent.
        let point_count = 36;
        let mut gram = [[0.0f64; 3]; 3];
        for index in 0..point_count {
            let azimuth = index as f32 * std::f32::consts::TAU / point_count as f32;
            let basis = unweighted_basis(azimuth, 0.0);
            let horizontal = [basis[0], basis[1], basis[3]];
            for row in 0..3 {
                for column in 0..3 {
                    gram[row][column] += horizontal[row] * horizontal[column];
                }
            }
        }
        for (row, gram_row) in gram.iter().enumerate() {
            for (column, value) in gram_row.iter().enumerate() {
                let mean = value / point_count as f64;
                let expected = if row == column { 0.5 } else { 0.0 };
                assert!(
                    (mean - expected).abs() < 1e-6,
                    "circular Gram[{row}][{column}] = {mean}, expected {expected}"
                );
            }
        }
    }

    #[test]
    fn max_re_weight_is_the_2d_first_order_optimum() {
        // 2D max-rE: a₁ = cos(π/(2N+2)) = cos(π/4) = 1/√2 for N = 1.
        // For a 2D panning function g(γ) = 1 + 2a·cos γ, |rE| = 2a/(1+2a²),
        // maximised at a = 1/√2 where |rE| = 1/√2.
        assert_close(
            MAX_RE_WEIGHT,
            (std::f32::consts::PI / 4.0).cos(),
            1e-7,
            "max-rE weight",
        );
        let energy_vector_magnitude = |weight: f32| 2.0 * weight / (1.0 + 2.0 * weight * weight);
        let at_optimum = energy_vector_magnitude(MAX_RE_WEIGHT);
        assert_close(at_optimum, std::f32::consts::FRAC_1_SQRT_2, 1e-6, "|rE|");
        assert!(energy_vector_magnitude(MAX_RE_WEIGHT - 0.02) < at_optimum);
        assert!(energy_vector_magnitude(MAX_RE_WEIGHT + 0.02) < at_optimum);
    }

    #[test]
    fn mode_matching_regular_quad_hand_derived_gains() {
        // Regular L-speaker ring: CCᵀ = (L/2)·I, so D = (2/L)·[1/√2, sin φᵢ, cos φᵢ]
        // and gᵢ = (1/L)·(1 + 2a·cos(φ − φᵢ)) with a = 1/√2, L = 4.
        //   front source:  FL = FR = ¼(1 + √2·cos45°) = ½,  RL = RR = ¼(1 − 1) = 0
        //   source at 45°: FL = ¼(1+√2) = 0.603553, FR = RL = ¼ = 0.25,
        //                  RR = ¼(1−√2) = −0.103553
        let azimuths = [
            45.0_f32.to_radians(),
            -45.0_f32.to_radians(),
            135.0_f32.to_radians(),
            -135.0_f32.to_radians(),
        ];
        let decoder = FoaDecoder::new(&azimuths, &[0.0; 4], &[0, 1, 2, 3]);

        let front = decoder.decode(&foa_encode(0.0, 0.0, 1.0));
        for (channel, expected) in [0.5, 0.5, 0.0, 0.0].iter().enumerate() {
            assert_close(front.gains[channel], *expected, 1e-5, "front source");
        }
        let at_speaker = decoder.decode(&foa_encode(FRAC_PI_4, 0.0, 1.0));
        for (channel, expected) in [0.603553, 0.25, 0.25, -0.103553].iter().enumerate() {
            assert_close(at_speaker.gains[channel], *expected, 1e-5, "45° source");
        }
    }

    #[test]
    fn mode_matching_regular_quad_energy_and_vectors_are_azimuth_independent() {
        // Energy: Σgᵢ² = (1/L²)·(L + 4a²·L/2) = (1 + 2a²)/L = 2/4 = 0.5 for every φ.
        // Velocity: |rV| = a = 1/√2. Energy: |rE| = 2a/(1+2a²) = 1/√2, pointing
        // at the source (the discrete sums are exact for L ≥ 4 up to order 3).
        let azimuths = [
            45.0_f32.to_radians(),
            -45.0_f32.to_radians(),
            135.0_f32.to_radians(),
            -135.0_f32.to_radians(),
        ];
        let decoder = FoaDecoder::new(&azimuths, &[0.0; 4], &[0, 1, 2, 3]);
        for degrees in (0..360).step_by(7) {
            let source_azimuth = (degrees as f32).to_radians();
            let gains = gains_for(
                &decoder.decode(&foa_encode(source_azimuth, 0.0, 1.0)),
                &[0, 1, 2, 3],
            );
            let energy: f32 = gains.iter().map(|gain| gain * gain).sum();
            assert_close(energy, 0.5, 1e-5, "quad energy");
            let (magnitude, direction) = energy_vector(&gains, &azimuths);
            assert_close(magnitude, std::f32::consts::FRAC_1_SQRT_2, 1e-5, "|rE|");
            assert!(
                angle_diff(direction, source_azimuth).abs() < 1e-3,
                "rE direction {direction} should equal source azimuth {source_azimuth}"
            );
            assert_close(
                velocity_vector_magnitude(&gains, &azimuths),
                std::f32::consts::FRAC_1_SQRT_2,
                1e-5,
                "|rV|",
            );
        }
    }

    #[test]
    fn mode_matching_octahedron_shows_2d_max_re_weight_in_3d() {
        // Octahedron (±X, ±Y, ±Z) is a spherical 3-design: CCᵀ = diag(3, 2, 2, 2),
        // so gᵢ = (1/6)·(1 + 3a·cos γᵢ). Source at +X with a = 1/√2:
        //   +X: (1 + 3a)/6 = 0.520220,  −X: (1 − 3a)/6 = −0.186887,  others 1/6.
        // |rE| = 2a/(1 + 3a²) = 0.565685. The 3D max-rE weight would be
        // a = 1/√3 (largest root of P₂), giving |rE| = 0.577350. The encoder's
        // 2D weight therefore costs 0.012 of |rE| on genuinely 3D layouts; the
        // production decoders are horizontal (AllRAD ring), where 1/√2 is optimal.
        let azimuths = [0.0, FRAC_PI_2, PI, -FRAC_PI_2, 0.0, 0.0];
        let elevations = [0.0, 0.0, 0.0, 0.0, FRAC_PI_2, -FRAC_PI_2];
        let decoder = FoaDecoder::new(&azimuths, &elevations, &[0, 1, 2, 3, 4, 5]);
        let gains = gains_for(
            &decoder.decode(&foa_encode(0.0, 0.0, 1.0)),
            &[0, 1, 2, 3, 4, 5],
        );
        assert_close(gains[0], 0.520220, 1e-4, "+X speaker");
        assert_close(gains[2], -0.186887, 1e-4, "−X speaker");
        for &index in &[1, 3, 4, 5] {
            assert_close(gains[index], 1.0 / 6.0, 1e-4, "orthogonal speaker");
        }
        let energy: f32 = gains.iter().map(|gain| gain * gain).sum();
        let energy_vector_x = (gains[0] * gains[0] - gains[2] * gains[2]) / energy;
        assert_close(energy_vector_x, 0.565685, 1e-4, "3D |rE| with 2D weight");
        let third = 1.0 / 3.0_f32.sqrt();
        let optimum = 2.0 * third / (1.0 + 3.0 * third * third);
        assert_close(optimum, 0.577350, 1e-5, "3D max-rE optimum");
    }

    #[test]
    fn foa_rotate_z_maps_source_azimuth_phi_to_phi_minus_angle() {
        // Y' = Y cos α − X sin α = a·sin(φ − α), X' = Y sin α + X cos α = a·cos(φ − α):
        // a head turn by +α. W and Z are invariant.
        for &source_degrees in &[0.0_f32, 30.0, 90.0, 170.0, -120.0] {
            for &angle_degrees in &[0.0_f32, 45.0, 90.0, -72.0, 200.0] {
                let source = source_degrees.to_radians();
                let angle = angle_degrees.to_radians();
                let rotated = foa_rotate_z(&foa_encode(source, 0.4, 0.8), angle);
                let expected = foa_encode(source - angle, 0.4, 0.8);
                for (actual, wanted) in [
                    (rotated.w, expected.w),
                    (rotated.y, expected.y),
                    (rotated.z, expected.z),
                    (rotated.x, expected.x),
                ] {
                    assert_close(actual, wanted, 1e-5, "rotated component");
                }
            }
        }
    }

    #[test]
    fn bilateral_hand_derived_ear_gains() {
        // Left ear  = ½(√2·W + X'_left)  with X'_left  = a·cos(φ − π/2) = a·sin φ·cos θ
        // Right ear = ½(√2·W + X'_right) with X'_right = −a·sin φ·cos θ
        // so L = ½(1 + a·sin φ·cos θ), R = ½(1 − a·sin φ·cos θ), L + R = 1.
        //   90° left: L = ½(1 + 0.707107) = 0.853553, R = 0.146447 → ILD 15.31 dB
        //   30° left: L = ½(1 + 0.353553) = 0.676777, R = 0.323223
        //   front / overhead: L = R = ½
        let bilateral = BilateralDecoder::new();
        let cases = [
            (FRAC_PI_2, 0.0, 0.853553, 0.146447),
            (30.0_f32.to_radians(), 0.0, 0.676777, 0.323223),
            (0.0, 0.0, 0.5, 0.5),
            (1.0, FRAC_PI_2, 0.5, 0.5),
        ];
        for (azimuth, elevation, expected_left, expected_right) in cases {
            let (left, right) = bilateral.decode_stereo(&foa_encode(azimuth, elevation, 1.0));
            assert_close(left, expected_left, 1e-5, "left ear");
            assert_close(right, expected_right, 1e-5, "right ear");
        }
        let (left, right) = bilateral.decode_stereo(&foa_encode(FRAC_PI_2, 0.0, 1.0));
        assert_close(20.0 * (left / right).log10(), 15.31, 0.01, "ILD at 90°");
    }

    #[test]
    fn bilateral_is_left_right_mirror_symmetric_with_constant_amplitude_sum() {
        let bilateral = BilateralDecoder::new();
        for degrees in (-180..=180).step_by(10) {
            let azimuth = (degrees as f32).to_radians();
            let (left, right) = bilateral.decode_stereo(&foa_encode(azimuth, 0.0, 1.0));
            let (mirror_left, mirror_right) =
                bilateral.decode_stereo(&foa_encode(-azimuth, 0.0, 1.0));
            assert_close(left, mirror_right, 1e-5, "mirror L↔R");
            assert_close(right, mirror_left, 1e-5, "mirror R↔L");
            assert_close(left + right, 1.0, 1e-5, "amplitude sum");
        }
    }

    #[test]
    fn bilateral_power_rises_1_76_db_from_front_to_side() {
        // L² + R² = ½(1 + a²·sin²φ) = ½ at the front and ¾ at ±90° (a² = ½):
        // 10·log10(1.5) = 1.761 dB louder at the sides. The decode is
        // amplitude-sum-preserving, not power-preserving; pinned so a change to
        // either is deliberate.
        let bilateral = BilateralDecoder::new();
        let power = |azimuth: f32| {
            let (left, right) = bilateral.decode_stereo(&foa_encode(azimuth, 0.0, 1.0));
            left * left + right * right
        };
        assert_close(power(0.0), 0.5, 1e-6, "front power");
        assert_close(power(FRAC_PI_2), 0.75, 1e-6, "side power");
        assert_close(
            10.0 * (power(FRAC_PI_2) / power(0.0)).log10(),
            1.761,
            1e-3,
            "front→side power rise",
        );
    }

    #[test]
    fn decoder_from_listener_uses_listener_local_azimuth() {
        // Listener facing +Y (yaw π/2): world +Y is local front, world −X is
        // local left (azimuth +π/2, counter-clockwise, right-handed Z-up).
        let listener = Listener::new(crate::types::Vec3::ZERO, FRAC_PI_2);
        let speakers = vec![
            speaker_at(0.0, 2.0, 0.0, 0),  // local front
            speaker_at(-2.0, 0.0, 0.0, 1), // local left
            speaker_at(0.0, -2.0, 0.0, 2), // local rear
            speaker_at(2.0, 0.0, 0.0, 3),  // local right
        ];
        let decoder = FoaDecoder::from_listener(&speakers, 4, &listener);
        let loudest = |azimuth: f32| {
            let gains = gains_for(
                &decoder.decode(&foa_encode(azimuth, 0.0, 1.0)),
                &[0, 1, 2, 3],
            );
            (0..4)
                .max_by(|&a, &b| gains[a].partial_cmp(&gains[b]).unwrap())
                .unwrap()
        };
        assert_eq!(loudest(0.0), 0, "front source → world +Y speaker");
        assert_eq!(loudest(FRAC_PI_2), 1, "left source → world −X speaker");
        assert_eq!(loudest(PI), 2, "rear source → world −Y speaker");
        assert_eq!(loudest(-FRAC_PI_2), 3, "right source → world +X speaker");
    }

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

    #[test]
    fn epad_on_itu_5_0_gives_exactly_flat_energy_and_mirror_symmetry() {
        // After EPAD, DᵀD = σ̄²·P(W,Y,X), so Σgᵢ² = σ̄²·(W² + Y² + X²)
        // = σ̄²·g²·(½ + a²) = σ̄²·g² — independent of azimuth.
        // An independent numpy re-implementation of AllRAD(12)+EPAD on this
        // layout gives σ̄² = 0.48951 and plain-AllRAD variation 4.63 dB.
        let speakers = itu_5_0_speakers();
        let listener = origin_listener();
        let channels = [0, 1, 2, 4, 5];
        let epad = AllRadDecoder::from_listener_epad(&speakers, 5, &listener);
        let allrad = AllRadDecoder::from_listener(&speakers, 5, &listener);

        let mut allrad_min = f32::MAX;
        let mut allrad_max = f32::MIN;
        for degrees in 0..360 {
            let azimuth = (degrees as f32).to_radians();
            let bformat = foa_encode(azimuth, 0.0, 1.0);
            let gains = gains_for(&epad.decode(&bformat), &channels);
            let energy: f32 = gains.iter().map(|gain| gain * gain).sum();
            assert_close(energy, 0.48951, 2e-4, "EPAD energy");

            let allrad_energy: f32 = gains_for(&allrad.decode(&bformat), &channels)
                .iter()
                .map(|gain| gain * gain)
                .sum();
            allrad_min = allrad_min.min(allrad_energy);
            allrad_max = allrad_max.max(allrad_energy);

            // L/R mirror: L(φ) = R(−φ), Ls(φ) = Rs(−φ), C(φ) = C(−φ).
            let mirror = gains_for(&epad.decode(&foa_encode(-azimuth, 0.0, 1.0)), &channels);
            assert_close(gains[0], mirror[1], 1e-4, "L↔R mirror");
            assert_close(gains[2], mirror[2], 1e-4, "C mirror");
            assert_close(gains[3], mirror[4], 1e-4, "Ls↔Rs mirror");
        }
        let allrad_variation_db = 10.0 * (allrad_max / allrad_min).log10();
        assert_close(
            allrad_variation_db,
            4.63,
            0.05,
            "AllRAD 5.0 energy variation",
        );
    }

    #[test]
    fn epad_on_itu_5_0_peaks_at_each_side_speaker_but_not_centre() {
        // Encode→decode at a speaker's own direction should make that speaker
        // the loudest. FOA cannot resolve C (0°) against L/R (±30°): a front
        // source decodes to L = R = 0.4499 > C = 0.2805 (numpy reference). This
        // is a first-order resolution limit, not a decoder defect.
        let speakers = itu_5_0_speakers();
        let decoder = AllRadDecoder::from_listener_epad(&speakers, 5, &origin_listener());
        let channels = [0, 1, 2, 4, 5];
        for (degrees, expected_channel) in [(30.0_f32, 0), (-30.0, 1), (110.0, 4), (-110.0, 5)] {
            let gains = decoder.decode(&foa_encode(degrees.to_radians(), 0.0, 1.0));
            let loudest = *channels
                .iter()
                .max_by(|&&a, &&b| gains.gains[a].partial_cmp(&gains.gains[b]).unwrap())
                .unwrap();
            assert_eq!(
                loudest, expected_channel,
                "source at {degrees}° should peak at channel {expected_channel}"
            );
        }
        let front = decoder.decode(&foa_encode(0.0, 0.0, 1.0));
        assert_close(front.gains[0], 0.4499, 2e-3, "front → L");
        assert_close(front.gains[2], 0.2805, 2e-3, "front → C");
    }

    #[test]
    fn epad_leaves_an_already_balanced_decoder_unchanged() {
        // Regular-quad mode-matching: DᵀD = ¼·Σ[½, s², 0, c²] = diag(½, ½, 0, ½),
        // so every non-zero singular value is already 1/√2 and EPAD is identity.
        let azimuths = [
            45.0_f32.to_radians(),
            -45.0_f32.to_radians(),
            135.0_f32.to_radians(),
            -135.0_f32.to_radians(),
        ];
        let decoder = FoaDecoder::new(&azimuths, &[0.0; 4], &[0, 1, 2, 3]);
        let mut equalised = decoder.decode_matrix.clone();
        apply_epad(&mut equalised);
        for (row, original) in equalised.iter().zip(&decoder.decode_matrix) {
            for column in 0..4 {
                assert_close(row[column], original[column], 1e-5, "EPAD identity");
            }
        }
    }

    #[test]
    fn jacobi_eigen_reconstructs_symmetric_matrix() {
        // [[2,1],[1,2]] ⊕ [3] ⊕ [0] has eigenvalues {1, 3, 3, 0}.
        let matrix = [
            [2.0, 1.0, 0.0, 0.0],
            [1.0, 2.0, 0.0, 0.0],
            [0.0, 0.0, 3.0, 0.0],
            [0.0, 0.0, 0.0, 0.0],
        ];
        let (eigenvectors, eigenvalues) = jacobi_eigen_4x4(&matrix);
        let mut sorted = eigenvalues;
        sorted.sort_by(|a, b| a.partial_cmp(b).unwrap());
        for (actual, expected) in sorted.iter().zip([0.0, 1.0, 3.0, 3.0]) {
            assert_close(*actual, expected, 1e-5, "eigenvalue");
        }
        for row in 0..4 {
            for column in 0..4 {
                // V·Λ·Vᵀ = A and Vᵀ·V = I
                let reconstructed: f32 = (0..4)
                    .map(|k| eigenvectors[row][k] * eigenvalues[k] * eigenvectors[column][k])
                    .sum();
                assert_close(reconstructed, matrix[row][column], 1e-5, "V·Λ·Vᵀ");
                let orthogonality: f32 = (0..4)
                    .map(|k| eigenvectors[k][row] * eigenvectors[k][column])
                    .sum();
                let identity = if row == column { 1.0 } else { 0.0 };
                assert_close(orthogonality, identity, 1e-5, "Vᵀ·V");
            }
        }
    }
}
