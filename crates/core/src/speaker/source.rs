// What a renderer needs to know about a source: where it is, where it points,
// and how its level falls off with distance.

use crate::directivity::DirectivityPattern;
use crate::panner::DistanceModelType;
use crate::types::Vec3;

/// Distance model parameters for attenuation.
#[derive(Clone, Copy, Debug)]
pub struct DistanceParams {
    pub ref_distance: f32,
    pub max_distance: f32,
    pub rolloff: f32,
    pub model: DistanceModelType,
}

impl Default for DistanceParams {
    fn default() -> Self {
        Self {
            ref_distance: 1.0,
            max_distance: 20.0,
            rolloff: 1.0,
            model: DistanceModelType::Inverse,
        }
    }
}

/// Source spatial properties for gain computation.
#[derive(Clone, Copy, Debug)]
pub struct SourceSpatial<'a> {
    pub position: Vec3,
    pub orientation: Vec3,
    pub directivity: &'a DirectivityPattern,
}
