use super::super::allrad::angle_diff;
use super::super::encode::foa_encode;
use super::super::test_support::{
    assert_close, gains_for, speaker_at, test_5_1_speakers, test_listener,
};
use super::*;
use std::f32::consts::{FRAC_PI_2, FRAC_PI_4, PI};

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
