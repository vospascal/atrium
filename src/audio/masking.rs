//! Spreading function and masked threshold computation for psychoacoustic analysis.
//!
//! The spreading function is the two-slope triangle of Terhardt (1979) with
//! level-independent slopes: masking falls 27 dB per Bark below the masker and
//! 24 dB per Bark above it (Terhardt's upper slope is level- and
//! frequency-dependent, 24 + 230/f − 0.2·L with f in Hz and L in dB; the
//! constant 24 is its high-frequency, low-level limit). The upper slope is the shallower one, so a masker
//! spreads more toward higher bands than lower ones. It describes how masking
//! energy from one Bark band affects neighbouring bands.

use crate::audio::spectral_profile::BARK_BANDS;

/// Signal-to-mask offset in dB: the masked threshold sits this far below the
/// spread masker energy. A fixed, band- and tonality-independent value chosen
/// for this module — roughly Johnston's (1988) 5.5 dB offset for noise-like
/// maskers, rounded. MPEG-1 Psychoacoustic Model 1 does not use a constant
/// here; its masking index depends on the Bark band and on whether the masker
/// is tonal or noise-like.
pub const MASKING_OFFSET: f32 = 6.0;

/// Spreading function lookup table size (dz from -23 to +23).
const SPREAD_TABLE_SIZE: usize = 47;

/// Center index of the spreading table (dz = 0).
const SPREAD_CENTER: usize = 23;

/// Pre-computed spreading function for inter-band masking analysis.
pub struct SpreadingFunction {
    /// Spreading attenuation in dB, indexed by (dz + SPREAD_CENTER).
    /// dz = target_band - masker_band, range [-23, +23].
    table: [f32; SPREAD_TABLE_SIZE],
}

impl SpreadingFunction {
    pub fn new() -> Self {
        let mut table = [0.0f32; SPREAD_TABLE_SIZE];
        // Two straight slopes meeting at 0 dB for dz = 0, so the function is
        // continuous everywhere, and at every distance (|dz| = 1 included) the
        // band above the masker is masked at least as strongly as the band below.
        for (i, entry) in table.iter_mut().enumerate() {
            let dz = i as f32 - SPREAD_CENTER as f32;
            *entry = if dz < 0.0 {
                27.0 * dz // lower slope: masker above the target (dz=-1 → -27 dB)
            } else {
                -24.0 * dz // upper slope: masker below the target (dz=+1 → -24 dB)
            };
        }
        Self { table }
    }

    /// Get spreading attenuation in dB for a given Bark distance.
    /// `dz` = target_band - masker_band (can be negative).
    pub fn spread_db(&self, dz: i32) -> f32 {
        let idx = (dz + SPREAD_CENTER as i32).clamp(0, SPREAD_TABLE_SIZE as i32 - 1) as usize;
        self.table[idx]
    }
}

impl Default for SpreadingFunction {
    fn default() -> Self {
        Self::new()
    }
}

/// Compute the masked threshold for a single source in a single band.
///
/// Uses power-sum of spreading contributions from all other sources:
///   T[b] = 10·log10( sum_{t!=s} sum_{b'} 10^((band_db[t][b'] + spread(b-b') - offset) / 10) )
///
/// Arguments:
/// - `target_band`: the Bark band index to compute threshold for
/// - `other_band_levels`: band level arrays for all OTHER sources (each
///   [f32; BARK_BANDS] in dB). An iterator, so callers can skip the target
///   source without copying; a band at −∞ dB contributes nothing.
/// - `spreading`: the pre-computed spreading function
///
/// Returns the masked threshold in dB (−∞ when nothing masks the band).
pub fn masked_threshold<'a>(
    target_band: usize,
    other_band_levels: impl IntoIterator<Item = &'a [f32; BARK_BANDS]>,
    spreading: &SpreadingFunction,
) -> f32 {
    let mut linear_sum = 0.0f32;

    for other_bands in other_band_levels {
        for (masker_band, &level) in other_bands.iter().enumerate() {
            let dz = target_band as i32 - masker_band as i32;
            let spread = spreading.spread_db(dz);
            let contribution_db = level + spread - MASKING_OFFSET;
            // Convert to linear power and accumulate
            linear_sum += 10.0_f32.powf(contribution_db / 10.0);
        }
    }

    if linear_sum > 0.0 {
        10.0 * linear_sum.log10()
    } else {
        f32::NEG_INFINITY // no masking
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn spread_zero_at_center() {
        let spreading_function = SpreadingFunction::new();
        assert_eq!(spreading_function.spread_db(0), 0.0);
    }

    #[test]
    fn spread_always_negative_off_center() {
        let spreading_function = SpreadingFunction::new();
        for dz in -23..=23i32 {
            if dz != 0 {
                assert!(
                    spreading_function.spread_db(dz) < 0.0,
                    "spread({dz}) = {} should be negative",
                    spreading_function.spread_db(dz)
                );
            }
        }
    }

    #[test]
    fn upward_masking_stronger_than_downward() {
        // Low frequencies mask high more effectively than high masks low.
        // |spread(+2)| should be LESS than |spread(-2)|
        // because upward masking (dz > 0) has a gentler slope.
        let spreading_function = SpreadingFunction::new();
        let upward = spreading_function.spread_db(2).abs(); // low masking high: 48 dB
        let downward = spreading_function.spread_db(-2).abs(); // high masking low: 54 dB
        assert!(
            downward > upward,
            "downward masking ({downward} dB) should be steeper than upward ({upward} dB)"
        );
    }

    #[test]
    fn spread_is_monotonically_decreasing_from_center() {
        let spreading_function = SpreadingFunction::new();
        for dz in 1..=23i32 {
            assert!(
                spreading_function.spread_db(dz) <= spreading_function.spread_db(dz - 1),
                "spread should decrease: spread({}) = {} > spread({}) = {}",
                dz,
                spreading_function.spread_db(dz),
                dz - 1,
                spreading_function.spread_db(dz - 1)
            );
            assert!(
                spreading_function.spread_db(-dz) <= spreading_function.spread_db(-(dz - 1)),
                "spread should decrease: spread({}) = {} > spread({}) = {}",
                -dz,
                spreading_function.spread_db(-dz),
                -(dz - 1),
                spreading_function.spread_db(-(dz - 1))
            );
        }
    }

    #[test]
    fn masked_threshold_no_maskers_is_neg_infinity() {
        let spreading_function = SpreadingFunction::new();
        let threshold = masked_threshold(10, &[], &spreading_function);
        assert!(threshold.is_infinite() && threshold < 0.0);
    }

    #[test]
    fn masked_threshold_increases_with_masker_level() {
        let spreading_function = SpreadingFunction::new();
        let quiet_masker = [[0.0f32; BARK_BANDS]; 1];
        let loud_masker = {
            let mut masker = [[0.0f32; BARK_BANDS]; 1];
            masker[0] = [40.0; BARK_BANDS];
            masker
        };
        let threshold_quiet = masked_threshold(10, &quiet_masker, &spreading_function);
        let threshold_loud = masked_threshold(10, &loud_masker, &spreading_function);
        assert!(
            threshold_loud > threshold_quiet,
            "louder masker should produce higher threshold: {threshold_loud} vs {threshold_quiet}"
        );
    }

    #[test]
    fn same_band_masking_stronger_than_distant() {
        let spreading_function = SpreadingFunction::new();
        // Masker in band 10 only
        let mut masker_bands = [f32::NEG_INFINITY; BARK_BANDS];
        masker_bands[10] = 60.0; // 60 dB in band 10
        let maskers = [masker_bands];

        let threshold_same = masked_threshold(10, &maskers, &spreading_function); // same band
        let threshold_far = masked_threshold(20, &maskers, &spreading_function); // 10 bands away
        assert!(
            threshold_same > threshold_far,
            "same-band masking ({threshold_same}) should be stronger than distant ({threshold_far})"
        );
    }

    // ── Hand-derived pins (audit 2026-11) ───────────────────────────────

    /// Beyond one Bark the table uses the Terhardt (1979) level-independent
    /// slopes: lower side 27 dB/Bark, upper side 24 dB/Bark.
    /// dz = +2 → −48 dB, dz = +5 → −120 dB, dz = −2 → −54 dB, dz = −5 → −135 dB.
    #[test]
    fn spreading_slopes_are_27_and_24_db_per_bark() {
        let spreading_function = SpreadingFunction::new();
        assert_eq!(spreading_function.spread_db(2), -48.0);
        assert_eq!(spreading_function.spread_db(5), -120.0);
        assert_eq!(spreading_function.spread_db(-2), -54.0);
        assert_eq!(spreading_function.spread_db(-5), -135.0);
        // Out-of-table distances clamp to the ±23 edge instead of panicking.
        assert_eq!(
            spreading_function.spread_db(40),
            spreading_function.spread_db(23)
        );
        assert_eq!(
            spreading_function.spread_db(-40),
            spreading_function.spread_db(-23)
        );
    }

    /// Masked threshold is a power sum minus the 6 dB offset.
    /// One 60 dB masker in the target band: 60 − 6 = 54 dB.
    /// Two such maskers: 54 + 10·log10(2) = 57.0103 dB.
    #[test]
    fn masked_threshold_is_power_sum_minus_offset() {
        let spreading_function = SpreadingFunction::new();
        let mut masker = [f32::NEG_INFINITY; BARK_BANDS];
        masker[10] = 60.0;
        let one = masked_threshold(10, &[masker], &spreading_function);
        assert!((one - 54.0).abs() < 1e-4, "one masker: {one}");
        let two = masked_threshold(10, &[masker, masker], &spreading_function);
        assert!((two - 57.0103).abs() < 1e-3, "two maskers: {two}");
    }

    /// Spread of masking is asymmetric toward HIGHER frequencies: a masker
    /// masks the band above it more than the band below. Every reference the
    /// module could mean agrees at one Bark — Schroeder (1979):
    /// SF(+1) = −4.3 dB, SF(−1) = −7.9 dB; MPEG-1 Model 1: −17 dB vs ≥ −17 dB;
    /// Terhardt: −24 vs −27 dB. The table used to give SF(−1) = −6.5 dB from a
    /// `−1 ≤ dz < 0` branch (6.5 dB/Bark, meant for fractional dz that integer
    /// bands never produce), inverting the asymmetry at the neighbour band.
    /// Now SF(+1) = −24 dB, SF(−1) = −27 dB (Terhardt).
    #[test]
    fn spreading_one_bark_upward_is_at_least_as_strong_as_downward() {
        let spreading_function = SpreadingFunction::new();
        let upward = spreading_function.spread_db(1); // masker one band below target
        let downward = spreading_function.spread_db(-1); // masker one band above target
        assert!(
            upward >= downward,
            "spread(+1) = {upward} dB should be ≥ spread(−1) = {downward} dB"
        );
        assert_eq!(upward, -24.0);
        assert_eq!(downward, -27.0);
    }

    /// At every Bark distance the upper side is at least as strong as the
    /// lower side, and both sides are straight lines through 0 dB at dz = 0
    /// (no step between |dz| = 1 and |dz| = 2).
    #[test]
    fn spreading_is_two_straight_slopes_through_zero() {
        let spreading_function = SpreadingFunction::new();
        for distance in 1..=23i32 {
            let upward = spreading_function.spread_db(distance);
            let downward = spreading_function.spread_db(-distance);
            assert!(
                upward >= downward,
                "dz = ±{distance}: {upward} vs {downward}"
            );
            assert_eq!(upward, -24.0 * distance as f32);
            assert_eq!(downward, -27.0 * distance as f32);
        }
    }

    /// A masker one band ABOVE the target (dz = −1) at 60 dB: 60 − 27 − 6 = 27 dB;
    /// one band BELOW (dz = +1): 60 − 24 − 6 = 30 dB.
    #[test]
    fn neighbour_band_thresholds_follow_the_slopes() {
        let spreading_function = SpreadingFunction::new();
        let mut masker = [f32::NEG_INFINITY; BARK_BANDS];
        masker[10] = 60.0;
        let below_masker = masked_threshold(9, &[masker], &spreading_function);
        let above_masker = masked_threshold(11, &[masker], &spreading_function);
        assert!((below_masker - 27.0).abs() < 1e-4, "{below_masker}");
        assert!((above_masker - 30.0).abs() < 1e-4, "{above_masker}");
    }
}
