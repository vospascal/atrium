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

    fn total_power(gains: &ChannelGains) -> f32 {
        gains.gains.iter().map(|gain| gain * gain).sum()
    }

    #[test]
    fn rolloff_3db_is_half_of_6db() {
        // a = R / (20·log10 2) = R / 6.0206: 3 dB → 0.49829, linear in R.
        assert!((rolloff_coefficient(3.0) - 0.498_29).abs() < 1e-5);
        assert!((rolloff_coefficient(6.0) - 2.0 * rolloff_coefficient(3.0)).abs() < 1e-6);
        // 20·log10(2) dB per doubling is exactly a = 1.
        assert!((rolloff_coefficient(20.0 * 2.0_f32.log10()) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn blurred_distance_hand_values() {
        // Eq 4: √(|s − p|² + r²). (3,4,0) from origin, r = 0 → 5; r = 12 → √(25+144) = 13.
        let source = Vec3::new(3.0, 4.0, 0.0);
        assert!((blurred_distance(source, Vec3::ZERO, 0.0) - 5.0).abs() < 1e-5);
        assert!((blurred_distance(source, Vec3::ZERO, 12.0) - 13.0).abs() < 1e-5);
        // Source on the speaker: distance is exactly the blur radius.
        assert!((blurred_distance(source, source, 0.3) - 0.3).abs() < 1e-6);
    }

    #[test]
    fn compute_blur_square_layout() {
        // Eq 12: centroid (0,0); every corner is √2 from it → r = 0.3·√2 = 0.424264.
        let positions: Vec<Vec3> = square_speakers().iter().map(|s| s.position).collect();
        assert!((compute_blur(&positions, 0.3) - 0.424_264).abs() < 1e-5);
        assert_eq!(compute_blur(&[], 0.3), 0.0);
    }

    #[test]
    fn two_speaker_inside_gains_hand_computed() {
        // Speakers at x = ±1, source at x = 0.5 (inside reference circle → p = 1, b = 1).
        // Blur r = 0.3 · mean(1, 1) = 0.3.
        // d_left = √(1.5² + 0.09) = 1.52971, d_right = √(0.5² + 0.09) = 0.58310.
        // v ∝ d^(−a), a = 0.99658, normalised to Σv² = 1 → left 0.35721, right 0.93402.
        let speakers = vec![
            Speaker {
                position: Vec3::new(-1.0, 0.0, 0.0),
                channel: 0,
            },
            Speaker {
                position: Vec3::new(1.0, 0.0, 0.0),
                channel: 1,
            },
        ];
        let gains = dbap_gains(
            Vec3::new(0.5, 0.0, 0.0),
            &speakers,
            2,
            &unit_weights(2),
            &DbapParams::default(),
        );
        assert!(
            (gains.gains[0] - 0.357_21).abs() < 1e-4,
            "left={}",
            gains.gains[0]
        );
        assert!(
            (gains.gains[1] - 0.934_02).abs() < 1e-4,
            "right={}",
            gains.gains[1]
        );
    }

    #[test]
    fn source_at_center_of_square_gives_half_per_speaker() {
        // Four equidistant speakers, Σv² = 1 → each v = 1/√4 = 0.5.
        let speakers = square_speakers();
        let gains = dbap_gains(
            Vec3::ZERO,
            &speakers,
            4,
            &unit_weights(4),
            &DbapParams::default(),
        );
        for channel in 0..4 {
            assert!((gains.gains[channel] - 0.5).abs() < 1e-5);
        }
    }

    #[test]
    fn power_is_exactly_one_inside_reference_circle() {
        // Inside the reference circle (|s − centroid| ≤ max centroid–speaker
        // distance = √2) p = 1, so Eq 9 gives Σv² = 1 exactly — not "≈".
        let speakers = square_speakers();
        for source in [
            Vec3::new(0.3, 0.2, 0.0),
            Vec3::new(-0.9, 0.9, 0.0),
            Vec3::new(1.0, -1.0, 0.0), // on a speaker
            Vec3::new(1.4, 0.0, 0.0),  // outside the square, inside the circle
        ] {
            let gains = dbap_gains(
                source,
                &speakers,
                4,
                &unit_weights(4),
                &DbapParams::default(),
            );
            let power = total_power(&gains);
            assert!(
                (power - 1.0).abs() < 1e-5,
                "source {source:?} power {power}"
            );
        }
    }

    #[test]
    fn power_outside_follows_p_to_the_4a() {
        // Eq 7/9: outside the reference circle Σv² = k²·Σ(b²w²/d^2a) = p^(4a),
        // independent of the bias b. Square: reference radius √2. Source at
        // 2√2 → p = 0.5 → Σv² = 0.5^(4·0.99658) = 0.063096 (−12.0 dB).
        // NOTE: per Sundstrom Eq 9 the amplitude falls as p^(2a), i.e. twice the
        // nominal rolloff (12 dB per doubling for R = 6 dB).
        let speakers = square_speakers();
        let a = rolloff_coefficient(6.0);
        for (source, p) in [
            (Vec3::new(2.0 * 2.0_f32.sqrt(), 0.0, 0.0), 0.5_f32),
            (Vec3::new(0.0, -4.0 * 2.0_f32.sqrt(), 0.0), 0.25),
        ] {
            let gains = dbap_gains(
                source,
                &speakers,
                4,
                &unit_weights(4),
                &DbapParams::default(),
            );
            let expected = p.powf(4.0 * a);
            let power = total_power(&gains);
            assert!(
                (power - expected).abs() < 1e-4 * expected.max(1e-3) + 1e-6,
                "source {source:?}: power {power}, expected {expected}"
            );
        }
        let gains_at_2_root_2 = dbap_gains(
            Vec3::new(2.0 * 2.0_f32.sqrt(), 0.0, 0.0),
            &speakers,
            4,
            &unit_weights(4),
            &DbapParams::default(),
        );
        assert!((total_power(&gains_at_2_root_2) - 0.063_096).abs() < 1e-4);
    }

    #[test]
    fn power_is_continuous_across_reference_circle() {
        // p and b are both continuous at q = 1 (b → 1 as p → 1), so crossing the
        // circle must not step the level. Radius √2 = 1.414214.
        let speakers = square_speakers();
        let radius = 2.0_f32.sqrt();
        let inside = dbap_gains(
            Vec3::new(radius - 1e-3, 0.0, 0.0),
            &speakers,
            4,
            &unit_weights(4),
            &DbapParams::default(),
        );
        let outside = dbap_gains(
            Vec3::new(radius + 1e-3, 0.0, 0.0),
            &speakers,
            4,
            &unit_weights(4),
            &DbapParams::default(),
        );
        for channel in 0..4 {
            assert!(
                (inside.gains[channel] - outside.gains[channel]).abs() < 5e-3,
                "channel {channel}: inside {} outside {}",
                inside.gains[channel],
                outside.gains[channel]
            );
        }
    }

    #[test]
    fn far_source_is_biased_toward_nearest_speakers() {
        // Source far to +X: the two +X speakers (channels 1, 2) must dominate,
        // and the mirror pairs must be equal (source on the x axis).
        let speakers = square_speakers();
        let gains = dbap_gains(
            Vec3::new(20.0, 0.0, 0.0),
            &speakers,
            4,
            &unit_weights(4),
            &DbapParams::default(),
        );
        assert!((gains.gains[1] - gains.gains[2]).abs() < 1e-6);
        assert!((gains.gains[0] - gains.gains[3]).abs() < 1e-6);
        assert!(
            gains.gains[1] > 2.0 * gains.gains[0],
            "near {} vs far {}",
            gains.gains[1],
            gains.gains[0]
        );
    }

    #[test]
    fn mirror_symmetric_layout_gives_mirrored_gains() {
        // Square is symmetric about x = 0: source (x, y) on speaker i equals
        // source (−x, y) on its mirror (0 ↔ 1, 3 ↔ 2), inside and outside.
        let speakers = square_speakers();
        for (x, y) in [(0.4_f32, 0.7_f32), (3.0, -1.5)] {
            let right = dbap_gains(
                Vec3::new(x, y, 0.0),
                &speakers,
                4,
                &unit_weights(4),
                &DbapParams::default(),
            );
            let left = dbap_gains(
                Vec3::new(-x, y, 0.0),
                &speakers,
                4,
                &unit_weights(4),
                &DbapParams::default(),
            );
            for (channel, mirror) in [(0, 1), (3, 2)] {
                assert!((right.gains[channel] - left.gains[mirror]).abs() < 1e-5);
            }
        }
    }

    #[test]
    fn weight_zero_silences_speaker_and_keeps_power() {
        // w_i = 0 removes speaker i; normalisation redistributes so Σv² stays 1.
        let speakers = square_speakers();
        let weights = vec![1.0, 1.0, 0.0, 1.0];
        let gains = dbap_gains(Vec3::ZERO, &speakers, 4, &weights, &DbapParams::default());
        assert_eq!(gains.gains[2], 0.0);
        // Three equidistant remaining speakers → each 1/√3 = 0.57735.
        for channel in [0, 1, 3] {
            assert!((gains.gains[channel] - 0.577_35).abs() < 1e-4);
        }
    }

    /// A single-speaker layout has blur r = 0 and ε = r/N = 0. Off the speaker
    /// the source is outside the (0.01 m) reference circle, so p < 1 and
    /// Eq 10 used to evaluate u_i / u_m = 0 / 0 → NaN on every channel. Not
    /// reachable from scene config (stereo/quad/5.1 only), but `dbap_gains` and
    /// `SpeakerLayout::new` are public.
    #[test]
    fn single_speaker_gains_are_finite() {
        let speakers = vec![Speaker {
            position: Vec3::ZERO,
            channel: 0,
        }];
        let gains = dbap_gains(
            Vec3::new(1.0, 0.0, 0.0),
            &speakers,
            1,
            &unit_weights(1),
            &DbapParams::default(),
        );
        assert!(gains.gains[0].is_finite(), "gain = {}", gains.gains[0]);
    }

    /// One speaker takes all the power. Eq 9 with N = 1: v = k·b/d^a and
    /// k = p^(2a)·d^a / b, so v = p^(2a) — the bias cancels. Reference radius
    /// is the 0.01 m floor; source at 1 m → q = 100, p = 0.01,
    /// v = 0.01^(2·0.996578) = 1.03202e-4 (−79.7 dB). Inside the reference
    /// circle (source on the speaker, distance floored) p = 1 → v = 1.
    #[test]
    fn single_speaker_gets_the_normalised_power_for_its_distance() {
        let speakers = vec![Speaker {
            position: Vec3::ZERO,
            channel: 0,
        }];
        let a = rolloff_coefficient(6.0);
        let off_speaker = dbap_gains(
            Vec3::new(1.0, 0.0, 0.0),
            &speakers,
            1,
            &unit_weights(1),
            &DbapParams::default(),
        );
        let expected = 0.01_f32.powf(2.0 * a);
        assert!(
            (off_speaker.gains[0] - expected).abs() < 1e-3 * expected,
            "gain {} expected {expected}",
            off_speaker.gains[0]
        );
        assert!((expected - 1.032_02e-4).abs() < 1e-8);

        let on_speaker = dbap_gains(
            Vec3::ZERO,
            &speakers,
            1,
            &unit_weights(1),
            &DbapParams::default(),
        );
        assert!(
            (on_speaker.gains[0] - 1.0).abs() < 1e-5,
            "gain {}",
            on_speaker.gains[0]
        );
    }

    /// Coincident speakers (zero blur, every u_i = 0) share the power equally
    /// and stay finite, including with the source exactly on them:
    /// three speakers, p = 1 on the speakers → each 1/√3 = 0.57735.
    #[test]
    fn coincident_speakers_share_power_and_stay_finite() {
        let position = Vec3::new(2.0, -1.0, 0.5);
        let speakers: Vec<Speaker> = (0..3)
            .map(|channel| Speaker { position, channel })
            .collect();
        for source in [position, Vec3::new(5.0, 3.0, 0.0)] {
            let gains = dbap_gains(
                source,
                &speakers,
                3,
                &unit_weights(3),
                &DbapParams::default(),
            );
            for channel in 0..3 {
                assert!(gains.gains[channel].is_finite(), "{source:?} {channel}");
                assert!((gains.gains[channel] - gains.gains[0]).abs() < 1e-7);
            }
        }
        let on_speakers = dbap_gains(
            position,
            &speakers,
            3,
            &unit_weights(3),
            &DbapParams::default(),
        );
        assert!((on_speakers.gains[0] - 0.577_35).abs() < 1e-4);
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
