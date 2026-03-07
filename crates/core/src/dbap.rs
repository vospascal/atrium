//! Distance-Based Amplitude Panning (DBAP).
//!
//! Implements the improved DBAP algorithm from Ville Pukki & Trond Lossius (2021,
//! arXiv:2109.08704) which extends the original Lossius 2009 formulation with
//! speaker biasing and power scaling for well-behaved gains outside the speaker array.
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
/// `speaker_count` — how many entries in `speakers` are valid.
/// `weights` — per-speaker weight (typically 1.0 for all). Length must be ≥ speaker_count.
pub fn dbap_gains(
    source_pos: Vec3,
    speakers: &[Speaker],
    speaker_count: usize,
    weights: &[f32],
    params: &DbapParams,
) -> ChannelGains {
    let n = speaker_count;
    if n == 0 {
        return ChannelGains::silent(0);
    }

    let a = rolloff_coefficient(params.rolloff_db);

    // Compute speaker positions slice and blur
    let positions: Vec<Vec3> = speakers[..n].iter().map(|s| s.position).collect();
    let blur = compute_blur(&positions, params.blur_scalar);

    // Blurred distances (Eq 4)
    let distances: Vec<f32> = positions
        .iter()
        .map(|&p| blurred_distance(source_pos, p, blur))
        .collect();

    // Reference distance: max distance from centroid to any speaker (Eq 7: max(d_s))
    let centroid = {
        let mut c = Vec3::ZERO;
        for &p in &positions {
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

    let u_values: Vec<f32> = distances
        .iter()
        .map(|&d| {
            let diff = d_max - d;
            diff * diff + epsilon
        })
        .collect();

    // Median u value as pivot (Eq 10: u_m)
    let mut u_sorted = u_values.clone();
    u_sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let u_m = if n.is_multiple_of(2) {
        (u_sorted[n / 2 - 1] + u_sorted[n / 2]) / 2.0
    } else {
        u_sorted[n / 2]
    };
    let u_m = u_m.max(epsilon);

    // Biasing factors (Eq 10): b_i = (u_i / u_m * (1/p - 1))^2 + 1
    let biases: Vec<f32> = u_values
        .iter()
        .map(|&u_i| {
            if (p - 1.0).abs() < 1e-10 {
                1.0
            } else {
                let term = (u_i / u_m) * (1.0 / p - 1.0);
                term * term + 1.0
            }
        })
        .collect();

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
mod tests {
    use super::*;

    fn unit_weights(n: usize) -> Vec<f32> {
        vec![1.0; n]
    }

    fn square_speakers() -> Vec<Speaker> {
        vec![
            Speaker {
                position: Vec3::new(-1.0, 1.0, 0.0),
                channel: 0,
            },
            Speaker {
                position: Vec3::new(1.0, 1.0, 0.0),
                channel: 1,
            },
            Speaker {
                position: Vec3::new(1.0, -1.0, 0.0),
                channel: 2,
            },
            Speaker {
                position: Vec3::new(-1.0, -1.0, 0.0),
                channel: 3,
            },
        ]
    }

    #[test]
    fn rolloff_6db_free_field() {
        // 6 dB/doubling → a ≈ 1.0 (inverse distance law)
        let a = rolloff_coefficient(6.0);
        assert!((a - (6.0 / (20.0 * 2.0_f32.log10()))).abs() < 1e-6);
        // a ≈ 0.9966 ≈ 1.0
        assert!((a - 1.0).abs() < 0.01);
    }

    #[test]
    fn blur_prevents_infinity() {
        // Source exactly at speaker position — gain must be finite
        let speakers = square_speakers();
        let source = speakers[0].position;
        let params = DbapParams::default();
        let gains = dbap_gains(source, &speakers, speakers.len(), &unit_weights(4), &params);

        for ch in 0..4 {
            assert!(gains.gains[ch].is_finite(), "channel {ch} is not finite");
        }
        // The co-located speaker should have the highest gain
        assert!(gains.gains[0] > gains.gains[2]);
    }

    #[test]
    fn source_at_center_symmetric_gains() {
        let speakers = square_speakers();
        let source = Vec3::ZERO;
        let params = DbapParams::default();
        let gains = dbap_gains(source, &speakers, speakers.len(), &unit_weights(4), &params);

        // All 4 speakers equidistant from center → equal gains
        let g0 = gains.gains[0];
        for ch in 1..4 {
            assert!(
                (gains.gains[ch] - g0).abs() < 1e-5,
                "ch {ch} gain {} != ch 0 gain {g0}",
                gains.gains[ch]
            );
        }
    }

    #[test]
    fn constant_power_inside() {
        // Source inside speaker array → Σ v² ≈ 1.0
        let speakers = square_speakers();
        let source = Vec3::new(0.3, 0.2, 0.0);
        let params = DbapParams::default();
        let gains = dbap_gains(source, &speakers, speakers.len(), &unit_weights(4), &params);

        let power: f32 = (0..4).map(|ch| gains.gains[ch] * gains.gains[ch]).sum();
        assert!(
            (power - 1.0).abs() < 0.15,
            "power {power} not close to 1.0 for source inside array"
        );
    }

    #[test]
    fn power_falls_outside() {
        // Source far outside speaker array → total power < 1.0
        let speakers = square_speakers();
        let source_far = Vec3::new(10.0, 0.0, 0.0);
        let params = DbapParams::default();
        let gains = dbap_gains(
            source_far,
            &speakers,
            speakers.len(),
            &unit_weights(4),
            &params,
        );

        let power: f32 = (0..4).map(|ch| gains.gains[ch] * gains.gains[ch]).sum();
        assert!(power < 1.0, "power {power} should be < 1.0 outside array");
    }

    #[test]
    fn nearest_speaker_loudest() {
        let speakers = square_speakers();
        // Source near speaker 0 (FL at -1, 1)
        let source = Vec3::new(-0.8, 0.8, 0.0);
        let params = DbapParams::default();
        let gains = dbap_gains(source, &speakers, speakers.len(), &unit_weights(4), &params);

        // Speaker 0 should be loudest
        for ch in 1..4 {
            assert!(
                gains.gains[0] > gains.gains[ch],
                "speaker 0 ({}) should be louder than speaker {ch} ({})",
                gains.gains[0],
                gains.gains[ch]
            );
        }
    }

    #[test]
    fn higher_rolloff_sharper_focus() {
        let speakers = square_speakers();
        let source = Vec3::new(-0.5, 0.5, 0.0); // near speaker 0

        let params_low = DbapParams {
            rolloff_db: 3.0,
            blur_scalar: 0.3,
        };
        let gains_low = dbap_gains(
            source,
            &speakers,
            speakers.len(),
            &unit_weights(4),
            &params_low,
        );

        let params_high = DbapParams {
            rolloff_db: 12.0,
            blur_scalar: 0.3,
        };
        let gains_high = dbap_gains(
            source,
            &speakers,
            speakers.len(),
            &unit_weights(4),
            &params_high,
        );

        // Higher rolloff → nearest speaker gets proportionally more energy
        let ratio_low = gains_low.gains[0] / gains_low.gains[2]; // nearest / farthest
        let ratio_high = gains_high.gains[0] / gains_high.gains[2];
        assert!(
            ratio_high > ratio_low,
            "higher rolloff should focus more: ratio_high={ratio_high}, ratio_low={ratio_low}"
        );
    }

    #[test]
    fn dbap_5_1_gain_distribution() {
        // 5.1 layout matching the room in main.rs
        let speakers = vec![
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
                position: Vec3::new(0.0, 0.0, 0.0),
                channel: 4,
            }, // RL
            Speaker {
                position: Vec3::new(6.0, 0.0, 0.0),
                channel: 5,
            }, // RR
        ];
        let w = vec![1.0; 5];
        let params = DbapParams::default();

        // Source near front-left speaker
        let g = dbap_gains(Vec3::new(1.0, 3.0, 0.0), &speakers, 5, &w, &params);
        eprintln!(
            "DBAP near FL: FL={:.4} FR={:.4} C={:.4} RL={:.4} RR={:.4}",
            g.gains[0], g.gains[1], g.gains[2], g.gains[4], g.gains[5]
        );

        // FL should dominate
        assert!(g.gains[0] > g.gains[1], "FL should be louder than FR");
        assert!(g.gains[0] > g.gains[5], "FL should be louder than RR");

        // Source at center of room
        let g2 = dbap_gains(Vec3::new(3.0, 2.0, 0.0), &speakers, 5, &w, &params);
        eprintln!(
            "DBAP at center: FL={:.4} FR={:.4} C={:.4} RL={:.4} RR={:.4}",
            g2.gains[0], g2.gains[1], g2.gains[2], g2.gains[4], g2.gains[5]
        );

        // FL and FR should be symmetric (both at same distance from center)
        assert!(
            (g2.gains[0] - g2.gains[1]).abs() < 0.01,
            "FL and FR should be equal at center"
        );
        // RL and RR should be symmetric
        assert!(
            (g2.gains[4] - g2.gains[5]).abs() < 0.01,
            "RL and RR should be equal at center"
        );

        // Source near front-right
        let g3 = dbap_gains(Vec3::new(5.0, 3.0, 0.0), &speakers, 5, &w, &params);
        eprintln!(
            "DBAP near FR: FL={:.4} FR={:.4} C={:.4} RL={:.4} RR={:.4}",
            g3.gains[0], g3.gains[1], g3.gains[2], g3.gains[4], g3.gains[5]
        );

        // FR should dominate
        assert!(g3.gains[1] > g3.gains[0], "FR should be louder than FL");
    }
}
