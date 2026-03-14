/// Maps real-world sound levels to digital amplitude and distance parameters.
///
/// MP3 files are mastered at arbitrary digital levels — a quiet recording of a
/// loud djembe might have higher RMS than a hot recording of a soft cat purring.
/// SoundProfile first RMS-normalizes the raw audio, then applies a gain based
/// on the sound's real-world SPL normalized to the loudest source in the scene.
///
/// Distance attenuation uses a fixed reference distance (IEC 61672: 1m standard
/// measurement distance). SPL only controls amplitude — louder sources are louder
/// at every distance, but the inverse-square rolloff curve is identical for all.
pub struct SoundProfile {
    /// Approximate SPL at 1 meter in dB (IEC 61672 measurement distance).
    pub reference_spl: f32,
}

impl SoundProfile {
    /// Compute linear amplitude for a source.
    ///
    /// Maps `reference_spl` to digital amplitude using `max_source_spl` (the
    /// loudest source's SPL in the scene) as the 0 dBFS calibration point.
    /// The loudest source gets gain 1.0; quieter sources scale down by
    /// 10^((spl - max) / 20) per dB below the maximum.
    pub fn amplitude(&self, buffer_rms: f32, target_rms: f32, max_source_spl: f32) -> f32 {
        // Step 1: RMS normalization — correct for mastering differences
        let rms_correction = if buffer_rms > 1e-6 {
            target_rms / buffer_rms
        } else {
            1.0
        };

        // Step 2: SPL-to-amplitude mapping (gain = 10^((spl - max) / 20), capped at 1.0)
        let db_below = self.reference_spl - max_source_spl;
        let spl_gain = 10.0_f32.powf(db_below / 20.0).min(1.0);

        rms_correction * spl_gain
    }

    /// Return the reference distance for distance attenuation.
    ///
    /// Per IEC 61672, this is the standard measurement distance (typically 1m)
    /// and is constant for all sources. SPL differences are handled solely by
    /// amplitude scaling — the inverse-square rolloff curve (ISO 9613) is
    /// identical regardless of source loudness.
    pub fn ref_distance(&self, global_ref: f32) -> f32 {
        global_ref
    }

    /// Compute the audible radius — maximum distance at which this source is heard.
    ///
    /// Uses ISO 9613 free-field spherical spreading (inverse square law):
    ///
    ///   SPL(d) = reference_spl - 20·log₁₀(d / d_ref)
    ///
    /// The audible radius is where SPL drops to `spl_threshold`:
    ///
    ///   d_audible = d_ref × 10^((reference_spl - spl_threshold) / 20)
    ///
    /// With d_ref=1.0 m (IEC 61672), spl_threshold=20 dB:
    ///   - Djembe (75 dB) → 562 m (capped to max_distance)
    ///   - Campfire (55 dB) → 56 m (capped to max_distance)
    ///   - Cat (25 dB) → 1.78 m
    pub fn audible_radius(&self, spl_threshold: f32, max_distance: f32) -> f32 {
        let db_above = self.reference_spl - spl_threshold;
        if db_above <= 0.0 {
            return 0.0; // source is already below hearing threshold at 1m
        }
        let radius = 10.0_f32.powf(db_above / 20.0);
        radius.min(max_distance)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn loudest_source_has_gain_one() {
        let profile = SoundProfile {
            reference_spl: 75.0,
        };
        // When buffer_rms == target_rms, RMS correction is 1.0
        let gain = profile.amplitude(0.1, 0.1, 75.0);
        assert!((gain - 1.0).abs() < 1e-6);
    }

    #[test]
    fn loud_source_has_higher_gain_than_quiet() {
        let djembe = SoundProfile {
            reference_spl: 75.0,
        };
        let campfire = SoundProfile {
            reference_spl: 55.0,
        };
        let purring = SoundProfile {
            reference_spl: 25.0,
        };
        let max_spl = 75.0;
        let rms = 0.1;
        let target = 0.1;
        let gain_djembe = djembe.amplitude(rms, target, max_spl);
        let gain_campfire = campfire.amplitude(rms, target, max_spl);
        let gain_purring = purring.amplitude(rms, target, max_spl);
        assert!(gain_djembe > gain_campfire);
        assert!(gain_campfire > gain_purring);
    }

    #[test]
    fn gain_ordering_survives_rms_normalization() {
        // Even if the loud source has a quieter recording, SPL ordering is preserved
        let djembe = SoundProfile {
            reference_spl: 75.0,
        };
        let campfire = SoundProfile {
            reference_spl: 55.0,
        };
        let max_spl = 75.0;
        let target = 0.1;
        // Djembe recording is quieter (lower RMS), campfire is hotter
        let gain_djembe = djembe.amplitude(0.05, target, max_spl);
        let gain_campfire = campfire.amplitude(0.2, target, max_spl);
        assert!(
            gain_djembe > gain_campfire,
            "djembe ({}) should beat campfire ({}) despite lower recording RMS",
            gain_djembe,
            gain_campfire
        );
    }

    #[test]
    fn db_scaling_20db_is_factor_10() {
        // 20 dB below max → gain = 10^(-20/20) = 0.1
        let profile = SoundProfile {
            reference_spl: 55.0,
        };
        let gain = profile.amplitude(0.1, 0.1, 75.0);
        assert!(
            (gain - 0.1).abs() < 0.001,
            "20 dB below max should give gain 0.1, got {gain}"
        );
    }

    #[test]
    fn db_scaling_50db_below_max() {
        // 50 dB below max → gain = 10^(-50/20) = 0.00316
        let profile = SoundProfile {
            reference_spl: 25.0,
        };
        let gain = profile.amplitude(0.1, 0.1, 75.0);
        assert!(
            (gain - 0.00316).abs() < 0.001,
            "50 dB below max should give gain ~0.00316, got {gain}"
        );
    }

    #[test]
    fn amplitude_stable_at_extreme_spl_gap() {
        let profile = SoundProfile {
            reference_spl: 10.0,
        };
        let gain = profile.amplitude(0.1, 0.1, 120.0);
        assert!(gain.is_finite());
        assert!(gain > 0.0);
        assert!(gain < 0.001);
    }

    #[test]
    fn default_ref_distance_is_one_meter() {
        let profile = SoundProfile {
            reference_spl: 75.0,
        };
        assert_eq!(profile.ref_distance(1.0), 1.0);
    }

    #[test]
    fn audible_radius_cat_is_short() {
        let cat = SoundProfile {
            reference_spl: 25.0,
        };
        let radius = cat.audible_radius(20.0, 100.0);
        assert!((radius - 1.778).abs() < 0.1, "cat radius = {radius}");
    }

    #[test]
    fn scene_gain_distribution() {
        // Verify the expected gain distribution with max_source_spl = 75 (djembe)
        let djembe = SoundProfile {
            reference_spl: 75.0,
        };
        let campfire = SoundProfile {
            reference_spl: 55.0,
        };
        let purring = SoundProfile {
            reference_spl: 25.0,
        };
        let max_spl = 75.0;
        let rms = 0.1;
        let target = 0.1;

        let gain_djembe = djembe.amplitude(rms, target, max_spl);
        let gain_campfire = campfire.amplitude(rms, target, max_spl);
        let gain_purring = purring.amplitude(rms, target, max_spl);

        assert!(
            (gain_djembe - 1.0).abs() < 1e-6,
            "djembe should be 1.0, got {gain_djembe}"
        );
        assert!(
            (gain_campfire - 0.1).abs() < 0.01,
            "campfire should be ~0.1, got {gain_campfire}"
        );
        assert!(
            (gain_purring - 0.00316).abs() < 0.001,
            "purring should be ~0.00316, got {gain_purring}"
        );
    }
}
