//! VBAP gain computation stage.
//!
//! Wraps `SpeakerLayout::compute_gains_with_spread()` as a SourceStage.
//! Writes target channel gains into `SourceOutput::channel_gains`.

use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Computes VBAP channel gains (with MDAP spread) for listener-relative panning.
pub struct VbapGainStage;

impl SourceStage for VbapGainStage {
    fn process(&mut self, ctx: &SourceContext, output: &mut SourceOutput) {
        let dist_params = atrium_core::speaker::DistanceParams {
            ref_distance: ctx.source_ref_distance,
            max_distance: ctx.distance_model.max_distance,
            rolloff: ctx.distance_model.rolloff,
            model: ctx.distance_model.model,
        };
        let source = atrium_core::speaker::SourceSpatial {
            position: ctx.source_pos,
            orientation: ctx.source_orientation,
            directivity: ctx.source_directivity,
        };
        output.channel_gains = ctx.layout.compute_gains_with_spread(
            atrium_core::speaker::RenderMode::Vbap,
            ctx.listener,
            &source,
            &dist_params,
            ctx.source_spread,
        );
        ctx.layout.apply_mask(&mut output.channel_gains);
    }

    fn name(&self) -> &str {
        "vbap_gains"
    }
}
