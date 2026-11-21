//! Distance-Based Amplitude Panning (DBAP).
//!
//! Implements the improved DBAP algorithm from Jacob Sundstrom (UC San Diego,
//! 2021), "Speaker Placement Agnosticism: Improving the Distance-based Amplitude
//! Panning Algorithm" (arXiv:2109.08704), which extends the original Lossius et al.
//! 2009 formulation with speaker biasing and power scaling for well-behaved gains
//! outside the speaker array.
//!
//! Key equations (paper numbering):
//!   Eq 3:  a = R / (20 * log10(2))           — rolloff coefficient
//!   Eq 4:  d_i = √(|s - p_i|² + r²)         — blurred distance
//!   Eq 8:  v_i = k * w_i * b_i / d_i^a       — per-speaker gain
//!   Eq 9:  k = p^(2a) / √(Σ b²w²/d^(2a))    — normalization
//!   Eq 10: b_i = (u_i/u_m * (1/p - 1))² + 1  — biasing factor
//!   Eq 12: r = mean(d_ic) * blur_scalar        — spatial blur

use crate::speaker::{ChannelGains, Speaker, MAX_CHANNELS};
use crate::types::Vec3;

/// Floor (metres) for the blurred distance d_i. Only reachable when the blur is
/// zero (single speaker, coincident speakers, or `blur_scalar = 0`) and the
/// source sits on a speaker; keeps d_i^(−a) finite there.
const MIN_BLURRED_DISTANCE: f32 = 1e-3;

/// DBAP configuration parameters.
#[derive(Clone, Copy, Debug)]
pub struct DbapParams {
    /// Rolloff in dB per doubling of distance. 6.0 = free-field inverse distance law.
    pub rolloff_db: f32,
    /// Spatial blur as fraction of mean centroid-speaker distance (range 0.2–0.5).
    pub blur_scalar: f32,
}

impl Default for DbapParams {
    fn default() -> Self {
        Self {
            rolloff_db: 6.0,
            blur_scalar: 0.3,
        }
    }
}

/// Rolloff coefficient from dB per doubling (Eq 3).
/// a = R / (20 * log10(2))
pub fn rolloff_coefficient(rolloff_db: f32) -> f32 {
    rolloff_db / (20.0 * 2.0_f32.log10())
}

/// Blurred distance preventing singularity at speaker positions (Eq 4).
/// d = √(|source - speaker|² + blur²)
pub fn blurred_distance(source: Vec3, speaker: Vec3, blur: f32) -> f32 {
    let d = source - speaker;
    (d.x * d.x + d.y * d.y + d.z * d.z + blur * blur).sqrt()
}

/// Compute spatial blur radius from speaker layout (Eq 12).
/// r = mean distance from centroid to each speaker × blur_scalar
pub fn compute_blur(speaker_positions: &[Vec3], blur_scalar: f32) -> f32 {
    if speaker_positions.is_empty() {
        return 0.0;
    }
    let n = speaker_positions.len() as f32;
    let centroid = {
        let mut c = Vec3::ZERO;
        for &p in speaker_positions {
            c = c + p;
        }
        c * (1.0 / n)
    };
    let mean_dist: f32 = speaker_positions
        .iter()
        .map(|&p| p.distance_to(centroid))
        .sum::<f32>()
        / n;
    mean_dist * blur_scalar
}

/// Compute per-speaker gains using improved DBAP (Eq 8–12).
///
/// All speakers receive some signal, weighted by inverse distance. The improved
/// formulation adds biasing `b_i` so that sources outside the speaker array
/// decay naturally without requiring convex hull computation.
///
/// `speakers` — spatial speakers (position + channel index).
/// `speaker_count` — how many entries in `speakers` are valid (at most
/// `MAX_CHANNELS`, the capacity of a `ChannelGains`; extra entries are ignored).
/// `weights` — per-speaker weight (typically 1.0 for all). Length must be ≥ speaker_count.
///
/// Degenerate layouts stay finite: a single speaker, or speakers that all
/// coincide, have zero blur, so every blurred distance is floored at
/// `MIN_BLURRED_DISTANCE` and the bias pivot u_m = 0 is treated as "all
/// speakers equally biased". A single speaker therefore receives all the
/// power, v = p^(2a) (the bias cancels in Eq 9).
pub fn dbap_gains(
    source_pos: Vec3,
    speakers: &[Speaker],
    speaker_count: usize,
    weights: &[f32],
    params: &DbapParams,
) -> ChannelGains {
    let n = speaker_count.min(MAX_CHANNELS);
    if n == 0 {
        return ChannelGains::silent(0);
    }

    let a = rolloff_coefficient(params.rolloff_db);

    // Speaker positions and blur (fixed-size scratch: no allocation per call).
    let mut position_storage = [Vec3::ZERO; MAX_CHANNELS];
    for (position, speaker) in position_storage.iter_mut().zip(&speakers[..n]) {
        *position = speaker.position;
    }
    let positions = &position_storage[..n];
    let blur = compute_blur(positions, params.blur_scalar);

    // Blurred distances (Eq 4), floored so a source on a speaker of a
    // zero-blur layout cannot divide by zero.
    let mut distances = [0.0_f32; MAX_CHANNELS];
    for (distance, &position) in distances.iter_mut().zip(positions) {
        *distance = blurred_distance(source_pos, position, blur).max(MIN_BLURRED_DISTANCE);
    }
    let distances = &distances[..n];

    // Reference distance: max distance from centroid to any speaker (Eq 7: max(d_s))
    let centroid = {
        let mut c = Vec3::ZERO;
        for &p in positions {
            c = c + p;
        }
        c * (1.0 / n as f32)
    };
    let d_rs: f32 = positions
        .iter()
        .map(|&p| p.distance_to(centroid))
        .fold(0.0_f32, f32::max);
    let d_rs = d_rs.max(0.01); // prevent division by zero

    // Distance from source to centroid
    let d_source_centroid = source_pos.distance_to(centroid);

    // Power scaling factor p (Eq 7): p < 1 when source is outside reference circle
    let q = d_source_centroid / d_rs;
    let p = if q > 1.0 { 1.0 / q } else { 1.0 };

    // Biasing factors b_i (Eq 10/11).
    // u_i = (d_max - d_i)^2: closer speakers to source get larger u_i → more bias.
    // u_m = median speaker's u value (pivot).
    let d_max = distances.iter().cloned().fold(f32::MIN, f32::max);
    let epsilon = blur / n as f32;

    let mut u_values = [0.0_f32; MAX_CHANNELS];
    for (u_value, &d) in u_values.iter_mut().zip(distances) {
        let diff = d_max - d;
        *u_value = diff * diff + epsilon;
    }
    let u_values = &u_values[..n];

    // Median u value as pivot (Eq 10: u_m). `sort_unstable_by` never allocates.
    let mut u_sorted = [0.0_f32; MAX_CHANNELS];
    u_sorted[..n].copy_from_slice(u_values);
    let u_sorted = &mut u_sorted[..n];
    u_sorted.sort_unstable_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let u_m = if n.is_multiple_of(2) {
        (u_sorted[n / 2 - 1] + u_sorted[n / 2]) / 2.0
    } else {
        u_sorted[n / 2]
    };
    let u_m = u_m.max(epsilon);

    // Biasing factors (Eq 10): b_i = (u_i / u_m * (1/p - 1))^2 + 1.
    // u_m = 0 only when the blur (and so ε) is zero and at least half the
    // speakers sit at d_max: a single speaker, coincident speakers, or
    // `blur_scalar = 0` with a symmetric layout. There is no pivot to scale
    // against, so u_i / u_m is taken as 1 (uniform bias, which Eq 9's
    // normalisation cancels) instead of evaluating 0/0.
    let mut biases = [0.0_f32; MAX_CHANNELS];
    for (bias, &u_i) in biases.iter_mut().zip(u_values) {
        *bias = if (p - 1.0).abs() < 1e-10 {
            1.0
        } else {
            let bias_ratio = if u_m > 0.0 { u_i / u_m } else { 1.0 };
            let term = bias_ratio * (1.0 / p - 1.0);
            term * term + 1.0
        };
    }

    // Normalization factor k (Eq 9): k = p^(2a) / √(Σ b²w²/d^(2a))
    let two_a = 2.0 * a;
    let p_2a = p.powf(two_a);
    let sum: f32 = (0..n)
        .map(|i| {
            let bw = biases[i] * weights[i];
            bw * bw / distances[i].powf(two_a)
        })
        .sum();

    let k = if sum > 0.0 { p_2a / sum.sqrt() } else { 0.0 };

    // Per-speaker gains (Eq 8): v_i = k * w_i * b_i / d_i^a
    let mut gains = ChannelGains::silent(MAX_CHANNELS);
    for i in 0..n {
        let v = k * weights[i] * biases[i] / distances[i].powf(a);
        gains.gains[speakers[i].channel] = v;
    }
    gains
}

#[cfg(test)]
mod tests;
