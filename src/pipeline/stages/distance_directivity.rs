//! Distance + directivity gain for WorldLocked mode.
//!
//! PathStage that computes per-speaker broadband gain:
//!   gain = distance_attenuation(source → speaker) × directivity(source → speaker)
//!
//! Per-speaker inside the WorldLockedRenderer rather than per-source.

use atrium_core::directivity::directivity_gain;
use atrium_core::panner::distance_gain_at_model;
use atrium_core::panner::DistanceModelType;

use crate::pipeline::path_stage::{PathContext, PathStage};

/// Per-path distance × directivity gain for WorldLocked rendering.
pub struct DistanceDirectivityPath {
    cached_gain: f32,
    ref_distance: f32,
    max_distance: f32,
    rolloff: f32,
    model: DistanceModelType,
}

impl DistanceDirectivityPath {
    pub fn new(
        ref_distance: f32,
        max_distance: f32,
        rolloff: f32,
        model: DistanceModelType,
    ) -> Self {
        Self {
            cached_gain: 0.0,
            ref_distance,
            max_distance,
            rolloff,
            model,
        }
    }
}

impl PathStage for DistanceDirectivityPath {
    fn update(&mut self, ctx: &PathContext) {
        let dist_gain = distance_gain_at_model(
            ctx.source_pos,
            ctx.target_pos,
            self.ref_distance,
            self.max_distance,
            self.rolloff,
            self.model,
        );
        let dir_gain = directivity_gain(
            ctx.source_pos,
            ctx.source_orientation,
            ctx.target_pos,
            ctx.source_directivity,
        );
        self.cached_gain = dist_gain * dir_gain;
    }

    fn gain_modifier(&self) -> f32 {
        self.cached_gain
    }

    fn name(&self) -> &str {
        "distance_directivity_path"
    }

    fn reset(&mut self) {
        self.cached_gain = 0.0;
    }
}
