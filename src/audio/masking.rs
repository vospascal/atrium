//! Spreading function and masked threshold computation for psychoacoustic analysis.
//!
//! Implements a piecewise linear approximation of the Schroeder (1979) spreading
//! function used in MPEG Psychoacoustic Model 1. The spreading function describes
//! how masking energy from one Bark band affects neighboring bands.

use crate::audio::spectral_profile::BARK_BANDS;

/// Masking offset in dB for broadband noise sources (MPEG Psychoacoustic Model 1).
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
        for (i, entry) in table.iter_mut().enumerate() {
            let dz = i as f32 - SPREAD_CENTER as f32;
            *entry = if dz < -1.0 {
                27.0 * dz // steep lower slope (e.g. dz=-2 → -54 dB)
            } else if dz < 0.0 {
                6.5 * dz // shallow near-band below (e.g. dz=-0.5 → -3.25 dB)
            } else if dz == 0.0 {
                0.0 // same band
            } else {
                -24.0 * dz // upper slope (e.g. dz=+2 → -48 dB)
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
/// - `other_band_levels`: slice of band level arrays for all OTHER sources (each [f32; BARK_BANDS] in dB)
/// - `spreading`: the pre-computed spreading function
///
/// Returns the masked threshold in dB.
pub fn masked_threshold(
    target_band: usize,
    other_band_levels: &[[f32; BARK_BANDS]],
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
}
