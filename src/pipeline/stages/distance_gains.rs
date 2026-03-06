//! Scalar distance gain for HRTF rendering.
//!
//! HRTF uses FFT convolution for spatialization, so it doesn't need
//! per-channel gains. It only needs a scalar distance attenuation applied
//! before the HRTF convolver.

use atrium_core::panner::distance_gain_at_model;

use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Scalar distance attenuation for HRTF mode.
pub struct DistanceGainStage;

impl SourceStage for DistanceGainStage {
    fn process(&mut self, ctx: &SourceContext, output: &mut SourceOutput) {
        output.distance_gain = distance_gain_at_model(
            ctx.listener.position,
            ctx.source_pos,
            ctx.source_ref_distance,
            ctx.distance_model.max_distance,
            ctx.distance_model.rolloff,
            ctx.distance_model.model,
        );
    }

    fn name(&self) -> &str {
        "distance_gain"
    }
}
