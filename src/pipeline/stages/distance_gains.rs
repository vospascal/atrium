//! Scalar distance gain for HRTF rendering.
//!
//! HRTF uses FFT convolution for spatialization, so it doesn't need
//! per-channel gains. It only needs a scalar distance × directivity
//! attenuation applied before the HRTF convolver.

use atrium_core::directivity::directivity_gain;
use atrium_core::panner::distance_gain_at_model;

use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Scalar distance × directivity attenuation for HRTF mode.
pub struct DistanceGainStage;

impl SourceStage for DistanceGainStage {
    fn process(&mut self, ctx: &SourceContext, output: &mut SourceOutput) {
        let dist_gain = distance_gain_at_model(
            ctx.listener.position,
            ctx.source_pos,
            ctx.source_ref_distance,
            ctx.distance_model.max_distance,
            ctx.distance_model.rolloff,
            ctx.distance_model.model,
        );
        let dir_gain = directivity_gain(
            ctx.source_pos,
            ctx.source_orientation,
            ctx.listener.position,
            ctx.source_directivity,
        );
        output.distance_gain = dist_gain * dir_gain;
    }

    fn name(&self) -> &str {
        "distance_gain"
    }
}
