use atrium_core::panner::DistanceModelType;

/// Distance model parameters for attenuation.
#[derive(Clone, Copy)]
pub struct DistanceModel {
    pub ref_distance: f32,
    pub max_distance: f32,
    pub rolloff: f32,
    pub model: DistanceModelType,
}

impl Default for DistanceModel {
    fn default() -> Self {
        Self {
            ref_distance: 1.0,
            max_distance: 20.0,
            rolloff: 1.0,
            model: DistanceModelType::Inverse,
        }
    }
}

impl DistanceModel {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_ref_distance_is_one_meter() {
        let dm = DistanceModel::default();
        assert_eq!(
            dm.ref_distance, 1.0,
            "ref_distance should be 1.0m (WebAudio/OpenAL standard)"
        );

        let dp = atrium_core::speaker::DistanceParams::default();
        assert_eq!(
            dp.ref_distance, 1.0,
            "DistanceParams ref_distance should match"
        );
    }

    // The formulas live in `atrium_core::panner::distance_gain_at_model`; these
    // tests pin them through the parameters a `DistanceModel` hands over.

    fn gain_at(model: &DistanceModel, distance: f32) -> f32 {
        atrium_core::panner::distance_gain_at_model(
            atrium_core::types::Vec3::ZERO,
            atrium_core::types::Vec3::new(distance, 0.0, 0.0),
            model.ref_distance,
            model.max_distance,
            model.rolloff,
            model.model,
        )
    }

    fn model_with(model: DistanceModelType, rolloff: f32) -> DistanceModel {
        DistanceModel {
            ref_distance: 1.0,
            max_distance: 20.0,
            rolloff,
            model,
        }
    }

    /// Every model is unity at ref_distance (WebAudio §6.1 distance models).
    #[test]
    fn every_model_is_unity_at_ref_distance() {
        for model_type in [
            DistanceModelType::Linear,
            DistanceModelType::Inverse,
            DistanceModelType::Exponential,
        ] {
            let gain = gain_at(&model_with(model_type, 1.0), 1.0);
            assert!((gain - 1.0).abs() < 1e-6, "{model_type:?}: {gain}");
        }
    }

    /// Inverse, rolloff 1: g = r_ref / d, so −6.02 dB per doubling of distance
    /// (1 → 2 → 4 → 8 m gives 1, 0.5, 0.25, 0.125).
    #[test]
    fn inverse_model_is_six_db_per_doubling() {
        let model = model_with(DistanceModelType::Inverse, 1.0);
        for distance in [1.0f32, 2.0, 4.0, 8.0] {
            let step_db =
                20.0 * (gain_at(&model, 2.0 * distance) / gain_at(&model, distance)).log10();
            assert!(
                (step_db + 6.0206).abs() < 1e-3,
                "{distance} m: {step_db} dB"
            );
        }
        assert!((gain_at(&model, 8.0) - 0.125).abs() < 1e-6);
    }

    /// Inverse with rolloff 2 (WebAudio): g = 1 / (1 + 2·(d − 1)); at 3 m that
    /// is 1/5 = 0.2, not the inverse-square 1/9 — rolloff is not an exponent.
    #[test]
    fn inverse_rolloff_follows_webaudio_formula() {
        let gain = gain_at(&model_with(DistanceModelType::Inverse, 2.0), 3.0);
        assert!((gain - 0.2).abs() < 1e-6, "{gain}");
    }

    /// Linear, ref 1 m, max 20 m, rolloff 1: g = 1 − (d − 1)/19. Halfway
    /// (10.5 m) is 0.5 and the far end (20 m) is 0.
    #[test]
    fn linear_model_pinned() {
        let model = model_with(DistanceModelType::Linear, 1.0);
        assert!((gain_at(&model, 10.5) - 0.5).abs() < 1e-6);
        assert!(gain_at(&model, 20.0).abs() < 1e-6);
    }

    /// Exponential: g = (d / r_ref)^(−rolloff). Rolloff 1 matches the inverse
    /// law; rolloff 2 is the energy (inverse-square) law, −12.04 dB per doubling.
    #[test]
    fn exponential_model_pinned() {
        let unit_rolloff = model_with(DistanceModelType::Exponential, 1.0);
        assert!((gain_at(&unit_rolloff, 4.0) - 0.25).abs() < 1e-6);
        let square_rolloff = model_with(DistanceModelType::Exponential, 2.0);
        assert!((gain_at(&square_rolloff, 2.0) - 0.25).abs() < 1e-6);
        assert!((gain_at(&square_rolloff, 5.0) - 0.04).abs() < 1e-6);
    }

    /// Beyond max_distance the gain is held at its max_distance value
    /// (OpenAL "clamped" semantics; WebAudio only clamps the linear model).
    /// Inverse at 20 m: 1/20 = 0.05 (−26 dB) and the same at 100 m.
    #[test]
    fn gain_is_held_beyond_max_distance() {
        let model = model_with(DistanceModelType::Inverse, 1.0);
        assert!((gain_at(&model, 20.0) - 0.05).abs() < 1e-6);
        assert_eq!(gain_at(&model, 100.0), gain_at(&model, 20.0));
    }

    /// Gain never increases with distance, for any model, from 1 cm to 2×max.
    #[test]
    fn gain_is_monotonic_non_increasing_in_distance() {
        for model_type in [
            DistanceModelType::Linear,
            DistanceModelType::Inverse,
            DistanceModelType::Exponential,
        ] {
            let model = model_with(model_type, 1.0);
            let mut previous = f32::INFINITY;
            let mut distance = 0.01f32;
            while distance < 40.0 {
                let gain = gain_at(&model, distance);
                assert!(
                    gain.is_finite() && gain <= previous,
                    "{model_type:?} {distance}"
                );
                previous = gain;
                distance *= 1.1;
            }
        }
    }
}
