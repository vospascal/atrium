use atrium_core::panner::DistanceModelType;
use atrium_core::speaker::DistanceParams;

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

impl DistanceModel {
    /// Convert to core DistanceParams for the speaker gain computation.
    pub fn as_params(&self) -> DistanceParams {
        DistanceParams {
            ref_distance: self.ref_distance,
            max_distance: self.max_distance,
            rolloff: self.rolloff,
            model: self.model,
        }
    }
}

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

        let dp = DistanceParams::default();
        assert_eq!(
            dp.ref_distance, 1.0,
            "DistanceParams ref_distance should match"
        );
    }

    #[test]
    fn as_params_preserves_ref_distance() {
        let dm = DistanceModel::default();
        let dp = dm.as_params();
        assert_eq!(dm.ref_distance, dp.ref_distance);
        assert_eq!(dm.max_distance, dp.max_distance);
        assert_eq!(dm.rolloff, dp.rolloff);
    }
}
