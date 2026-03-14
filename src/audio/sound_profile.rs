/// Maps real-world sound levels to digital amplitude and distance parameters.
///
/// MP3 files are mastered at arbitrary digital levels — a quiet recording of a
/// loud djembe might have higher RMS than a hot recording of a soft cat purring.
/// SoundProfile first RMS-normalizes the raw audio, then applies a gain based
/// on the sound's real-world SPL relative to the IEC 61672 calibration level.
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
    /// Maps `reference_spl` to digital amplitude using `spl_reference` as the
    /// calibration point. A source at `spl_reference` gets gain 1.0. Louder
    /// sources get gain > 1.0, quieter sources get gain < 1.0.
    ///
    /// No cap — the distance model's `MAX_NEAR_FIELD_GAIN` handles clipping
    /// protection. With spl_reference = 94 dB (IEC 61672):
    ///   djembe (75 dB) → 0.112,  campfire (55 dB) → 0.0112,  cat (25 dB) → 0.000355
    pub fn amplitude(&self, buffer_rms: f32, target_rms: f32, spl_reference: f32) -> f32 {
        // Step 1: RMS normalization — correct for mastering differences
        let rms_correction = if buffer_rms > 1e-6 {
            target_rms / buffer_rms
        } else {
            1.0
        };

        // Step 2: SPL-to-amplitude mapping (absolute, no cap)
        let db_diff = self.reference_spl - spl_reference;
        let spl_gain = 10.0_f32.powf(db_diff / 20.0);

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

    // IEC 61672 calibration level: 94 dB SPL = 0 dBFS
    const SPL_REF: f32 = 94.0;

    #[test]
    fn source_at_reference_has_gain_one() {
        let profile = SoundProfile {
            reference_spl: 94.0,
        };
        let gain = profile.amplitude(0.1, 0.1, SPL_REF);
        assert!((gain - 1.0).abs() < 1e-6);
    }

    #[test]
    fn djembe_gain_below_one() {
        // 75 dB is 19 dB below spl_reference 94 → gain = 10^(-19/20) ≈ 0.112
        let profile = SoundProfile {
            reference_spl: 75.0,
        };
        let gain = profile.amplitude(0.1, 0.1, SPL_REF);
        let expected = 10.0_f32.powf(-19.0 / 20.0); // ≈ 0.112
        assert!(
            (gain - expected).abs() < 0.001,
            "75 dB source should get gain ~{expected}, got {gain}"
        );
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
        let rms = 0.1;
        let target = 0.1;
        let gain_djembe = djembe.amplitude(rms, target, SPL_REF);
        let gain_campfire = campfire.amplitude(rms, target, SPL_REF);
        let gain_purring = purring.amplitude(rms, target, SPL_REF);
        assert!(gain_djembe > gain_campfire);
        assert!(gain_campfire > gain_purring);
    }

    #[test]
    fn gain_ordering_survives_rms_normalization() {
        let djembe = SoundProfile {
            reference_spl: 75.0,
        };
        let campfire = SoundProfile {
            reference_spl: 55.0,
        };
        let target = 0.1;
        // Djembe recording is quieter (lower RMS), campfire is hotter
        let gain_djembe = djembe.amplitude(0.05, target, SPL_REF);
        let gain_campfire = campfire.amplitude(0.2, target, SPL_REF);
        assert!(
            gain_djembe > gain_campfire,
            "djembe ({}) should beat campfire ({}) despite lower recording RMS",
            gain_djembe,
            gain_campfire
        );
    }

    #[test]
    fn db_scaling_20db_below_reference() {
        // 74 dB is 20 dB below spl_reference 94 → gain = 10^(-20/20) = 0.1
        let profile = SoundProfile {
            reference_spl: 74.0,
        };
        let gain = profile.amplitude(0.1, 0.1, SPL_REF);
        assert!(
            (gain - 0.1).abs() < 0.001,
            "20 dB below reference should give gain 0.1, got {gain}"
        );
    }

    #[test]
    fn cat_gain_with_iec_reference() {
        // Cat at 25 dB with spl_reference 94: gain = 10^((25-94)/20) = 10^(-3.45) ≈ 0.000355
        let profile = SoundProfile {
            reference_spl: 25.0,
        };
        let gain = profile.amplitude(0.1, 0.1, SPL_REF);
        let expected = 10.0_f32.powf(-69.0 / 20.0);
        assert!(
            (gain - expected).abs() < 0.0001,
            "cat should get gain ~{expected}, got {gain}"
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
        // With spl_reference = 94 (IEC 61672):
        //   djembe   75 dB → 10^(-19/20) ≈ 0.112
        //   campfire 55 dB → 10^(-39/20) ≈ 0.0112
        //   purring  25 dB → 10^(-69/20) ≈ 0.000355
        let djembe = SoundProfile {
            reference_spl: 75.0,
        };
        let campfire = SoundProfile {
            reference_spl: 55.0,
        };
        let purring = SoundProfile {
            reference_spl: 25.0,
        };
        let rms = 0.1;
        let target = 0.1;

        let gain_djembe = djembe.amplitude(rms, target, SPL_REF);
        let gain_campfire = campfire.amplitude(rms, target, SPL_REF);
        let gain_purring = purring.amplitude(rms, target, SPL_REF);

        let expected_djembe = 10.0_f32.powf(-19.0 / 20.0);
        let expected_campfire = 10.0_f32.powf(-39.0 / 20.0);
        let expected_purring = 10.0_f32.powf(-69.0 / 20.0);

        assert!(
            (gain_djembe - expected_djembe).abs() < 0.001,
            "djembe should be ~{expected_djembe}, got {gain_djembe}"
        );
        assert!(
            (gain_campfire - expected_campfire).abs() < 0.001,
            "campfire should be ~{expected_campfire}, got {gain_campfire}"
        );
        assert!(
            (gain_purring - expected_purring).abs() < 0.0001,
            "purring should be ~{expected_purring}, got {gain_purring}"
        );
    }
}
