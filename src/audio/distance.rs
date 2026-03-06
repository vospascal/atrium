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
            ref_distance: 0.3,
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
