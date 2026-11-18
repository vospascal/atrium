//! ISO 9613-2:1996 — Sound propagation outdoors.
//!
//! Provides the ground-effect term A_ground (§7.3) of the ISO 9613-2 total
//! attenuation A = A_div + A_atm + A_ground + A_bar + A_misc. The other terms
//! live elsewhere or are not modelled:
//!
//!   A_div    geometric divergence: the per-source distance model (distance.rs)
//!   A_atm    atmospheric absorption: ISO 9613-1, see atmosphere.rs
//!   A_bar    screening by obstacles: not modelled, the scene has no obstacles
//!   A_misc   vegetation, housing, industrial sites: not modelled
//!
//! Reference: ISO 9613-2:1996, "Acoustics — Attenuation of sound during
//!            propagation outdoors — Part 2: General method of calculation"

// ─────────────────────────────────────────────────────────────────────────────
// Ground Effect — ISO 9613-2 §7.3
// ─────────────────────────────────────────────────────────────────────────────

/// Ground surface properties for ISO 9613-2 ground effect calculation.
///
/// The ground factor G ranges from 0.0 (hard/reflective) to 1.0 (porous/soft):
///   - G = 0.0: concrete, water, stone, tile, asphalt
///   - G = 0.5: mixed or compacted soil
///   - G = 1.0: grass, soil, carpet, vegetation
///
/// For indoor use, typical values:
///   - Hard floor (tile, concrete): G = 0.0
///   - Carpet / rugs: G ≈ 0.7–1.0
///   - Wooden floor: G ≈ 0.3
#[derive(Clone, Copy, Debug)]
pub struct GroundProperties {
    /// Ground factor near the source (0.0 = hard, 1.0 = porous).
    pub g_source: f32,
    /// Ground factor in the middle region between source and receiver.
    pub g_middle: f32,
    /// Ground factor near the receiver.
    pub g_receiver: f32,
}

impl Default for GroundProperties {
    /// Default: hard reflective surface (concrete/tile floor).
    fn default() -> Self {
        Self {
            g_source: 0.0,
            g_middle: 0.0,
            g_receiver: 0.0,
        }
    }
}

impl GroundProperties {
    /// Hard reflective surface (concrete, tile, stone).
    pub fn hard() -> Self {
        Self {
            g_source: 0.0,
            g_middle: 0.0,
            g_receiver: 0.0,
        }
    }

    /// Soft porous surface (grass, carpet, soil).
    pub fn soft() -> Self {
        Self {
            g_source: 1.0,
            g_middle: 1.0,
            g_receiver: 1.0,
        }
    }

    /// Mixed surface (e.g., wooden floor with rugs).
    pub fn mixed(g: f32) -> Self {
        let g = g.clamp(0.0, 1.0);
        Self {
            g_source: g,
            g_middle: g,
            g_receiver: g,
        }
    }
}

/// Octave-band centre frequencies of ISO 9613-2 Table 3 (Hz).
pub const GROUND_BAND_FREQUENCIES: [f32; 8] =
    [63.0, 125.0, 250.0, 500.0, 1000.0, 2000.0, 4000.0, 8000.0];

/// Ground attenuation per octave band, ISO 9613-2 §7.3.1 (Table 3).
///
///   A_ground = A_s + A_r + A_m      (dB, positive = loss)
///
/// Source and receiver regions (same expressions, own height and G):
///
/// | Band        | A_s or A_r            |
/// |-------------|-----------------------|
/// | 63 Hz       | −1.5                  |
/// | 125 Hz      | −1.5 + G · a′(h)      |
/// | 250 Hz      | −1.5 + G · b′(h)      |
/// | 500 Hz      | −1.5 + G · c′(h)      |
/// | 1 kHz       | −1.5 + G · d′(h)      |
/// | 2, 4, 8 kHz | −1.5 · (1 − G)        |
///
/// Every height function is ≥ 1.5, so porous ground (G = 1) never amplifies:
/// the −1.5 dB reflection gain is cancelled or exceeded in every band. Hard
/// ground (G = 0) is −1.5 dB per region in every band: a flat +3 dB boost.
///
/// Middle region: A_m = −3q at 63 Hz and −3q·(1 − G_m) above, with
/// q = 0 while d_p ≤ 30·(h_s + h_r), else q = 1 − 30·(h_s + h_r)/d_p.
/// The standard's own q is continuous, so no extra smoothing is needed.
///
/// `distance` is the source→receiver distance projected on the ground (m).
/// Returned values are indexed like [`GROUND_BAND_FREQUENCIES`].
pub fn ground_effect_bands(
    distance: f32,
    h_source: f32,
    h_receiver: f32,
    ground: &GroundProperties,
) -> [f32; 8] {
    if distance < 0.01 {
        return [0.0; 8];
    }
    let h_source = h_source.max(0.0);
    let h_receiver = h_receiver.max(0.0);

    let h_sum = h_source + h_receiver;
    let q = if distance <= 30.0 * h_sum {
        0.0
    } else {
        1.0 - 30.0 * h_sum / distance
    };

    std::array::from_fn(|band| {
        let middle = if band == 0 {
            -3.0 * q
        } else {
            -3.0 * q * (1.0 - ground.g_middle)
        };
        ground_region_band(ground.g_source, h_source, distance, band)
            + ground_region_band(ground.g_receiver, h_receiver, distance, band)
            + middle
    })
}

/// Source- or receiver-region term of ISO 9613-2 Table 3 for one octave band.
fn ground_region_band(g: f32, height: f32, distance: f32, band: usize) -> f32 {
    // (1 − e^(−d_p/50)): the height functions grow in over the first ~100 m.
    let range_term = 1.0 - (-distance / 50.0).exp();
    let height_squared = height * height;
    match band {
        0 => -1.5,
        1 => {
            let a_prime = 1.5
                + 3.0 * (-0.12 * (height - 5.0) * (height - 5.0)).exp() * range_term
                + 5.7
                    * (-0.09 * height_squared).exp()
                    * (1.0 - (-2.8e-6 * distance * distance).exp());
            -1.5 + g * a_prime
        }
        2 => -1.5 + g * (1.5 + 8.6 * (-0.09 * height_squared).exp() * range_term),
        3 => -1.5 + g * (1.5 + 14.0 * (-0.46 * height_squared).exp() * range_term),
        4 => -1.5 + g * (1.5 + 5.0 * (-0.9 * height_squared).exp() * range_term),
        _ => -1.5 * (1.0 - g),
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Ground Effect — Real-time linear gain
// ─────────────────────────────────────────────────────────────────────────────

/// Broadband ground effect as a linear gain multiplier for real-time use.
///
/// Averages the ISO 9613-2 ground attenuation (in dB) over the 125 Hz–4 kHz
/// octave bands and converts to an amplitude factor. Used where a path has no
/// per-band processing (the per-speaker WorldLocked renderer).
///
/// Hard ground gives +3 dB (×1.41) at short range and up to +6 dB once the
/// middle region opens; porous ground gives ≤ 0 dB (never a boost).
pub fn ground_effect_gain(
    distance: f32,
    h_source: f32,
    h_receiver: f32,
    ground: &GroundProperties,
) -> f32 {
    if distance < 0.01 {
        return 1.0;
    }
    let bands = ground_effect_bands(distance, h_source, h_receiver, ground);
    // Bands 1..=6 are 125 Hz–4 kHz.
    let average_db = bands[1..=6].iter().sum::<f32>() / 6.0;
    // Positive dB = loss, so gain = 10^(−dB/20).
    10.0_f32.powf(-average_db / 20.0)
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;

    // ── Ground Effect ────────────────────────────────────────────────────

    // The pinned values below are worked by hand from ISO 9613-2 Table 3.

    /// Hard ground (G = 0) is −1.5 dB per region in every band, and the middle
    /// region is empty while d_p ≤ 30·(h_s + h_r): a flat −3 dB (a +3 dB boost).
    #[test]
    fn hard_ground_is_flat_minus_three_db_at_short_range() {
        let bands = ground_effect_bands(10.0, 1.5, 1.5, &GroundProperties::hard());
        for (band, value) in bands.iter().enumerate() {
            assert!(
                (value + 3.0).abs() < 1e-4,
                "band {band}: expected -3.0 dB, got {value}"
            );
        }
        let gain = ground_effect_gain(10.0, 1.5, 1.5, &GroundProperties::hard());
        assert!((20.0 * gain.log10() - 3.0).abs() < 1e-3, "gain {gain}");
    }

    /// Hard ground with source and receiver on the ground: the middle region
    /// covers the whole path (q = 1), adding −3 dB: −6 dB in total.
    #[test]
    fn hard_ground_at_zero_height_is_minus_six_db() {
        let bands = ground_effect_bands(20.0, 0.0, 0.0, &GroundProperties::hard());
        for value in bands {
            assert!((value + 6.0).abs() < 1e-4, "expected -6.0 dB, got {value}");
        }
    }

    /// Middle region factor q = 1 − 30·(h_s + h_r)/d_p. Heights 1 m + 1 m at
    /// 120 m: q = 0.5, so hard ground gives −3 − 1.5 = −4.5 dB.
    #[test]
    fn middle_region_uses_iso_q() {
        let bands = ground_effect_bands(120.0, 1.0, 1.0, &GroundProperties::hard());
        assert!((bands[3] + 4.5).abs() < 1e-4, "got {}", bands[3]);
        // At exactly the threshold (60 m) the middle region is still empty.
        let at_threshold = ground_effect_bands(60.0, 1.0, 1.0, &GroundProperties::hard());
        assert!(
            (at_threshold[3] + 3.0).abs() < 1e-4,
            "got {}",
            at_threshold[3]
        );
    }

    /// Porous ground, source and receiver on the ground, 1000 m (so the range
    /// term 1 − e^(−d/50) ≈ 1). Per region:
    ///   125 Hz: a′(0) − 1.5 = 3.0·e^(−3) + 5.7·(1 − e^(−2.8)) = 0.149 + 5.353 = 5.50
    ///   250 Hz: b′(0) − 1.5 = 8.6
    ///   500 Hz: c′(0) − 1.5 = 14.0
    ///   1 kHz:  d′(0) − 1.5 = 5.0
    ///   2 kHz+: 0
    /// Two regions double each; the middle region is 0 for G = 1 above 63 Hz,
    /// and −3 dB at 63 Hz on top of 2 × −1.5.
    #[test]
    fn porous_ground_matches_table_three_height_functions() {
        let bands = ground_effect_bands(1000.0, 0.0, 0.0, &GroundProperties::soft());
        let expected = [-6.0, 11.0, 17.2, 28.0, 10.0, 0.0, 0.0, 0.0];
        for (band, (value, want)) in bands.iter().zip(expected).enumerate() {
            assert!(
                (value - want).abs() < 0.02,
                "band {band} ({} Hz): expected {want} dB, got {value}",
                GROUND_BAND_FREQUENCIES[band]
            );
        }
    }

    /// The bug this replaced: grass came out +3.8 dB louder than free field,
    /// louder than concrete. Porous ground must never amplify from 125 Hz up,
    /// at any geometry.
    #[test]
    fn porous_ground_never_amplifies() {
        let soft = GroundProperties::soft();
        let hard = GroundProperties::hard();
        for distance in [0.5f32, 2.0, 10.0, 20.0, 60.0, 300.0] {
            for h_source in [0.0f32, 0.3, 1.2, 6.0, 20.0] {
                for h_receiver in [0.0f32, 1.2, 1.7] {
                    let bands = ground_effect_bands(distance, h_source, h_receiver, &soft);
                    for (band, value) in bands.iter().enumerate().skip(1) {
                        assert!(
                            *value >= -1e-4,
                            "d={distance} hs={h_source} hr={h_receiver} band {band}: {value} dB"
                        );
                    }
                    let soft_gain = ground_effect_gain(distance, h_source, h_receiver, &soft);
                    let hard_gain = ground_effect_gain(distance, h_source, h_receiver, &hard);
                    assert!(soft_gain <= 1.0 + 1e-5, "soft gain {soft_gain}");
                    assert!(
                        hard_gain > soft_gain,
                        "hard {hard_gain} vs soft {soft_gain}"
                    );
                }
            }
        }
    }

    #[test]
    fn ground_effect_gain_hard_boosts() {
        // Hard ground: constructive interference → gain > 1.0
        let ground = GroundProperties::hard();
        let g = ground_effect_gain(10.0, 1.5, 1.5, &ground);
        assert!(g > 1.0, "hard ground should boost: got {g}");
    }

    #[test]
    fn ground_effect_gain_soft_attenuates_relative() {
        let hard = GroundProperties::hard();
        let soft = GroundProperties::soft();
        let g_hard = ground_effect_gain(10.0, 1.5, 1.5, &hard);
        let g_soft = ground_effect_gain(10.0, 1.5, 1.5, &soft);
        assert!(
            g_soft < g_hard,
            "soft ({g_soft}) should be less than hard ({g_hard})"
        );
    }

    #[test]
    fn ground_effect_gain_zero_distance_is_unity() {
        let ground = GroundProperties::soft();
        assert_eq!(ground_effect_gain(0.0, 1.5, 1.5, &ground), 1.0);
    }

    // ── Pinned values and invariants (audit 2026-11) ─────────────────────

    /// Soft ground (G = 1), h_s = h_r = 1.5 m, d_p = 10 m (q = 0, range term
    /// 1 − e^(−0.2) = 0.1813). Per region, worked from Table 3:
    ///   125 Hz: −1.5 + 1.5 + 3·e^(−0.12·12.25)·0.1813 + 5.7·e^(−0.2025)·(1 − e^(−2.8e−4)) = 0.1263
    ///   250 Hz: 8.6·e^(−0.2025)·0.1813 = 1.2731
    ///   500 Hz: 14·e^(−1.035)·0.1813  = 0.9015
    ///   1 kHz:  5·e^(−2.025)·0.1813   = 0.1196
    /// Two regions: [−3, 0.2527, 2.5463, 1.8030, 0.2393, 0, 0, 0] dB.
    /// Broadband: mean of 125 Hz–4 kHz = 0.8069 dB → gain 10^(−0.8069/20) = 0.9113.
    #[test]
    fn soft_ground_at_ear_height_pinned() {
        let soft = GroundProperties::soft();
        let bands = ground_effect_bands(10.0, 1.5, 1.5, &soft);
        let expected = [-3.0, 0.2527, 2.5463, 1.8030, 0.2393, 0.0, 0.0, 0.0];
        for (band, (value, want)) in bands.iter().zip(expected).enumerate() {
            assert!(
                (value - want).abs() < 1e-3,
                "band {band}: {value} vs {want}"
            );
        }
        let gain = ground_effect_gain(10.0, 1.5, 1.5, &soft);
        assert!((gain - 0.9113).abs() < 1e-3, "{gain}");
    }

    /// Every Table 3 term is linear in G, so a uniform G = 0.5 must be the
    /// exact mean of hard and soft ground in every band and geometry.
    #[test]
    fn ground_effect_is_linear_in_ground_factor() {
        let hard = GroundProperties::hard();
        let soft = GroundProperties::soft();
        let mixed = GroundProperties::mixed(0.5);
        for (distance, h_source, h_receiver) in [(10.0f32, 1.5f32, 1.5f32), (200.0, 0.5, 1.5)] {
            let hard_bands = ground_effect_bands(distance, h_source, h_receiver, &hard);
            let soft_bands = ground_effect_bands(distance, h_source, h_receiver, &soft);
            let mixed_bands = ground_effect_bands(distance, h_source, h_receiver, &mixed);
            for band in 0..8 {
                let mean = (hard_bands[band] + soft_bands[band]) / 2.0;
                assert!(
                    (mixed_bands[band] - mean).abs() < 1e-4,
                    "d={distance} band {band}: {} vs {mean}",
                    mixed_bands[band]
                );
            }
        }
        // G outside [0, 1] is clamped, not extrapolated.
        let clamped = GroundProperties::mixed(3.0);
        assert_eq!(clamped.g_middle, 1.0);
        assert_eq!(GroundProperties::mixed(-1.0).g_source, 0.0);
    }

    /// Reciprocity: with the same G under both ends, swapping source and
    /// receiver heights cannot change the result (A_s and A_r share Table 3).
    #[test]
    fn ground_effect_is_reciprocal() {
        let ground = GroundProperties::mixed(0.7);
        for distance in [5.0f32, 40.0, 250.0] {
            let forward = ground_effect_bands(distance, 0.4, 2.2, &ground);
            let reverse = ground_effect_bands(distance, 2.2, 0.4, &ground);
            for band in 0..8 {
                assert!((forward[band] - reverse[band]).abs() < 1e-5);
            }
        }
    }

    /// A source below the ground plane is treated as lying on it (h clamped
    /// to 0), never producing NaN from the height functions.
    #[test]
    fn below_ground_source_is_treated_as_on_ground() {
        let soft = GroundProperties::soft();
        let below = ground_effect_bands(50.0, -2.0, 1.5, &soft);
        let on_ground = ground_effect_bands(50.0, 0.0, 1.5, &soft);
        assert_eq!(below, on_ground);
        assert!(below.iter().all(|value| value.is_finite()));
    }

    /// The middle-region factor q = 1 − 30(h_s + h_r)/d_p is continuous at
    /// d_p = 30(h_s + h_r) (q = 0 on both sides), so a source walking past the
    /// threshold produces no level step.
    #[test]
    fn middle_region_onset_is_continuous() {
        let hard = GroundProperties::hard();
        let threshold = 30.0 * (1.0 + 1.0);
        let just_inside = ground_effect_bands(threshold - 0.01, 1.0, 1.0, &hard);
        let just_outside = ground_effect_bands(threshold + 0.01, 1.0, 1.0, &hard);
        for band in 0..8 {
            assert!((just_inside[band] - just_outside[band]).abs() < 1e-3);
        }
    }
}
