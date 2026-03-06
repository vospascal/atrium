/// Maps real-world sound levels to digital amplitude and distance parameters.
///
/// MP3 files are mastered at arbitrary digital levels — a quiet recording of a
/// loud djembe might have higher RMS than a hot recording of a soft cat purring.
/// SoundProfile first RMS-normalizes the raw audio, then applies a gain based
/// on the sound's real-world SPL mapped to a configurable reference level.
///
/// It also computes per-source `ref_distance` so louder sources project further.
pub struct SoundProfile {
    /// Approximate SPL at 1 meter in dB (IEC 61672 measurement distance).
    pub reference_spl: f32,
}

impl SoundProfile {
    /// Compute linear amplitude for a source.
    ///
    /// Maps `reference_spl` to digital amplitude using `spl_reference` as the
    /// 0 dBFS calibration point. Sources at or above `spl_reference` are capped
    /// at gain 1.0; sources below scale down by 20·log₁₀ per dB.
    ///
    /// Real-world standard for `spl_reference`: 94 dB (IEC 61672).
    /// Lower values make quiet sources louder (loud sources cap at 1.0).
    pub fn amplitude(
        &self,
        buffer_rms: f32,
        target_rms: f32,
        spl_reference: f32,
    ) -> f32 {
        // Step 1: RMS normalization — correct for mastering differences
        let rms_correction = if buffer_rms > 1e-6 {
            target_rms / buffer_rms
        } else {
            1.0
        };

        // Step 2: SPL-to-amplitude mapping (gain = 10^((spl - ref) / 20), capped at 1.0)
        let db_below = self.reference_spl - spl_reference;
        let spl_gain = 10.0_f32.powf(db_below / 20.0).min(1.0);

        rms_correction * spl_gain
    }

    /// Compute per-source reference distance.
    ///
    /// Scales the global `ref_distance` proportionally to SPL: louder sources
    /// get a larger ref_distance, so their gain stays at 1.0 over a wider radius.
    ///
    /// Formula: `global_ref × (reference_spl / spl_reference)`
    ///
    /// With spl_reference=40, global_ref=1.0:
    ///   - Djembe (100 dB) → 2.5 m (fills room)
    ///   - Campfire (35 dB) → 0.88 m (nearby)
    ///   - Cat (25 dB) → 0.63 m (close range only)
    pub fn ref_distance(&self, global_ref: f32, spl_reference: f32) -> f32 {
        global_ref * (self.reference_spl / spl_reference.max(1.0))
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
    ///   - Djembe (100 dB) → 10000 m (capped to max_distance)
    ///   - Campfire (35 dB) → 5.6 m
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
