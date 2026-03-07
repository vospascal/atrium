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
//! decode weights. This gives ITD from rotation and ILD from asymmetric weights.
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

// ─────────────────────────────────────────────────────────────────────────────
// AllRAD Decoder (All-Round Ambisonic Decoding)
// ─────────────────────────────────────────────────────────────────────────────

/// Default number of virtual speakers for AllRAD horizontal ring.
/// 12 = 30° spacing. FOA (order 1) only needs ≥4 mathematically (t-design order 2),
/// but 12 gives smoother VBAP interpolation between virtual→real re-panning.
/// Going higher (24, 50) has diminishing returns since FOA only carries 4 channels
/// of spatial information. 12 is the standard choice in literature (Zotter & Frank).
///
/// This is just the default — `from_listener_with_count()` accepts any value ≥4.
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
pub struct AllRadDecoder {
    /// N_real × 4 combined decode matrix.
    decode_matrix: Vec<[f32; 4]>,
    /// Output channel index for each real speaker.
    channels: Vec<usize>,
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
    /// `virtual_count` must be ≥4 (minimum for FOA t-design). Typical values:
    /// 12 (default, 30° spacing), 16 (22.5°), 24 (15°). Higher values give
    /// marginally smoother re-panning but only affect build time, not runtime.
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
        let virtual_count = virtual_count.max(4);
        let n_real = speaker_count;
        if n_real == 0 {
            return Self {
                decode_matrix: Vec::new(),
                channels: Vec::new(),
            };
        }

        let channels: Vec<usize> = speakers[..n_real].iter().map(|s| s.channel).collect();

        // Step 1: Virtual speakers at equal angular intervals (listener-local frame)
        let mut virt_azimuths = vec![0.0f32; virtual_count];
        for (i, az) in virt_azimuths.iter_mut().enumerate() {
            *az = (i as f32) * std::f32::consts::TAU / virtual_count as f32;
        }
        let virt_elevations = vec![0.0f32; virtual_count];
        let virt_channels: Vec<usize> = (0..virtual_count).collect();

        // Step 2: FOA mode-matching decoder for the virtual ring
        let virt_decoder = FoaDecoder::new(&virt_azimuths, &virt_elevations, &virt_channels);

        // Step 3: VBAP re-panning matrix — for each virtual speaker, compute
        // gains to real speakers. Virtual speakers are in local frame, so we
        // need real speaker angles in local frame too.
        let cos_y = listener.yaw.cos();
        let sin_y = listener.yaw.sin();

        // Compute real speaker angles in listener-local frame + sort by angle
        let mut real_angles: Vec<(f32, usize)> = Vec::with_capacity(n_real);
        let mut real_dists: Vec<f32> = Vec::with_capacity(n_real);
        for (i, s) in speakers[..n_real].iter().enumerate() {
            let dx = s.position.x - listener.position.x;
            let dy = s.position.y - listener.position.y;
            let local_x = dx * cos_y + dy * sin_y;
            let local_y = -dx * sin_y + dy * cos_y;
            real_angles.push((local_y.atan2(local_x), i));
            real_dists.push((dx * dx + dy * dy).sqrt().max(0.1));
        }
        real_angles.sort_by(|a, b| a.0.partial_cmp(&b.0).unwrap());

        // For each virtual speaker, find its VBAP gains to real speakers
        let mut vbap_matrix = vec![[0.0f32; MAX_CHANNELS]; virtual_count];
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
                for &(angle, idx) in &real_angles {
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
        let mut decode_matrix = vec![[0.0f32; 4]; n_real];
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
        }
    }

    /// Decode B-format to per-channel speaker gains (same interface as FoaDecoder).
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

    /// Number of real speakers in this decoder.
    pub fn speaker_count(&self) -> usize {
        self.decode_matrix.len()
    }
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
// Bilateral Ambisonics Decoder (binaural / stereo headphone output)
// ─────────────────────────────────────────────────────────────────────────────

/// Default ear offset from head center: ~8.75cm (half of average head width ~17.5cm).
/// Source: Algazi et al. (2001) KEMAR head model. This is used to compute the
/// distance-dependent rotation angle δ = arctan(offset / distance), matching how
/// acoustic parallax works: close sources → large δ → strong ITD, distant sources
/// → δ ≈ 0 → both ears hear nearly the same angle.
const DEFAULT_EAR_OFFSET_M: f32 = 0.0875;

/// Bilateral ambisonics decoder for stereo headphone output.
///
/// For each ear:
/// 1. Rotate B-format by ±ear_angle (accounts for ear offset → ITD)
/// 2. Decode using a cardioid pickup pattern pointing in each ear's direction
///
/// The cardioid decode gives: mono = W/√2 + Y·sin(φ_ear) + X·cos(φ_ear)
/// where φ_ear = ±π/2 (left/right). This produces:
///   Left  ear: W/√2 + Y  (cardioid toward left)
///   Right ear: W/√2 - Y  (cardioid toward right)
///
/// The ear rotation before decoding introduces a subtle per-ear perspective
/// shift that creates natural ITD cues.
pub struct BilateralDecoder {
    /// Rotation angle for ear offset (radians). Positive = left ear.
    ear_angle: f32,
}

impl BilateralDecoder {
    pub fn new() -> Self {
        Self {
            ear_angle: DEFAULT_EAR_OFFSET_M,
        }
    }

    /// Create with a specific ear offset in meters.
    pub fn with_ear_offset(ear_offset_m: f32) -> Self {
        Self {
            ear_angle: ear_offset_m,
        }
    }

    /// Compute the ear rotation angle for a source at the given distance.
    /// δ = arctan(ear_offset / distance), clamped for very close sources.
    fn ear_rotation(&self, source_distance: f32) -> f32 {
        let dist = source_distance.max(0.1);
        (self.ear_angle / dist).atan()
    }

    /// Decode B-format to stereo (left, right) for headphone output.
    ///
    /// `source_distance` is used to scale the ear rotation angle — closer
    /// sources produce larger ITD, matching natural acoustics.
    pub fn decode_stereo(&self, bformat: &BFormat, source_distance: f32) -> (f32, f32) {
        let delta = self.ear_rotation(source_distance);

        // Rotate B-format to each ear's perspective
        let b_left = foa_rotate_z(bformat, delta);
        let b_right = foa_rotate_z(bformat, -delta);

        // Cardioid decode weights, derived from virtual microphone pickup patterns:
        //
        // W × √2 × 0.5 — omnidirectional (pressure) component. The √2 undoes
        //   SN3D normalization (W was encoded as g/√2). The 0.5 prevents clipping
        //   when W and directional components align.
        //
        // ±Y × 0.5 — left-right component. +Y for left ear, -Y for right ear.
        //   This is the primary ILD mechanism: a source to the left has positive Y,
        //   so the left ear gets a boost and the right ear gets attenuation.
        //
        // X × 0.25 — frontal bias. Without this term, sources directly in front
        //   and directly behind would produce identical L/R output (front-back
        //   ambiguity). The X component breaks this symmetry — front sources get
        //   a subtle boost in both ears. Weighted at 0.25 (half of Y) to keep it
        //   secondary to the L/R separation.
        let sqrt2 = std::f32::consts::SQRT_2;
        let left = b_left.w * sqrt2 * 0.5 + b_left.y * 0.5 + b_left.x * 0.25;
        let right = b_right.w * sqrt2 * 0.5 - b_right.y * 0.5 + b_right.x * 0.25;

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
    fn foa_rotate_z_90_front_becomes_left() {
        // Front source (azimuth=0): X positive, Y zero.
        // Rotating +90° should move energy from X into Y (front → left).
        let b = foa_encode(0.0, 0.0, 1.0);
        let rotated = foa_rotate_z(&b, FRAC_PI_2);
        assert!((rotated.w - b.w).abs() < 1e-6, "W unchanged by rotation");
        assert!(
            (rotated.z - b.z).abs() < 1e-6,
            "Z unchanged by yaw rotation"
        );
        assert!(
            rotated.y.abs() > 0.3,
            "Y should be significant after 90° rotation: Y={}",
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
        let (l, r) = bilateral.decode_stereo(&b, 2.0);
        assert!(
            (l - r).abs() < 0.05,
            "bilateral front: L={l:.4} and R={r:.4} should be roughly equal"
        );
    }

    #[test]
    fn bilateral_left_source_louder_left() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(FRAC_PI_2, 0.0, 1.0); // left
        let (l, r) = bilateral.decode_stereo(&b, 2.0);
        assert!(
            l > r,
            "bilateral left source: L={l:.4} should be louder than R={r:.4}"
        );
    }

    #[test]
    fn bilateral_right_source_louder_right() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(-FRAC_PI_2, 0.0, 1.0); // right
        let (l, r) = bilateral.decode_stereo(&b, 2.0);
        assert!(
            r > l,
            "bilateral right source: R={r:.4} should be louder than L={l:.4}"
        );
    }

    #[test]
    fn bilateral_closer_source_wider_itd() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(FRAC_PI_2, 0.0, 1.0); // left source

        let (l_far, r_far) = bilateral.decode_stereo(&b, 5.0);
        let (l_near, r_near) = bilateral.decode_stereo(&b, 0.5);

        let ild_far = l_far - r_far;
        let ild_near = l_near - r_near;
        assert!(
            ild_near.abs() > ild_far.abs(),
            "closer source should have larger ILD: near={:.4}, far={:.4}",
            ild_near,
            ild_far
        );
    }

    #[test]
    fn bilateral_nonzero_output() {
        let bilateral = BilateralDecoder::new();
        let b = foa_encode(0.5, 0.0, 1.0);
        let (l, r) = bilateral.decode_stereo(&b, 2.0);
        assert!(l.abs() > 0.01, "left output should be nonzero: {l}");
        assert!(r.abs() > 0.01, "right output should be nonzero: {r}");
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
}
