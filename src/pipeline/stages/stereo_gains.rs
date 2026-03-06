//! Equal-power stereo panning stage (Stereo mode).
//!
//! Wraps `SpeakerLayout::compute_gains_stereo()` as a SourceStage.
//! Writes L/R channel gains into `SourceOutput::channel_gains`.

use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Computes equal-power L/R stereo gains for headphone rendering.
pub struct StereoGainStage;

impl SourceStage for StereoGainStage {
    fn process(&mut self, ctx: &SourceContext, output: &mut SourceOutput) {
        let dist_params = atrium_core::speaker::DistanceParams {
            ref_distance: ctx.source_ref_distance,
            max_distance: ctx.distance_model.max_distance,
            rolloff: ctx.distance_model.rolloff,
            model: ctx.distance_model.model,
        };
        output.channel_gains = ctx.layout.compute_gains_stereo(
            ctx.listener,
            ctx.source_pos,
            ctx.source_orientation,
            ctx.source_directivity,
            &dist_params,
        );
        ctx.layout.apply_mask(&mut output.channel_gains);
    }

    fn name(&self) -> &str {
        "stereo_gains"
    }
}
