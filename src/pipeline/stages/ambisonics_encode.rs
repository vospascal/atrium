//! Ambisonics FOA encoding stage.
//!
//! Computes listener-relative azimuth, distance gain, and directivity,
//! then encodes into B-format (W, Y, X) stored in channel_gains[0..2].
//! The AmbisonicsRenderer decodes B-format to speaker gains.

use atrium_core::ambisonics::foa_encode;
use atrium_core::directivity::directivity_gain;
use atrium_core::panner::distance_gain_at_model;

use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Encodes source position into FOA B-format coefficients.
pub struct AmbisonicsEncodeStage;

impl SourceStage for AmbisonicsEncodeStage {
    fn process(&mut self, ctx: &SourceContext, output: &mut SourceOutput) {
        // Source direction in listener's local frame (matches VBAP convention)
        let dx = ctx.source_pos.x - ctx.listener.position.x;
        let dy = ctx.source_pos.y - ctx.listener.position.y;
        let cos_y = ctx.listener.yaw.cos();
        let sin_y = ctx.listener.yaw.sin();
        let local_x = dx * cos_y + dy * sin_y; // forward
        let local_y = -dx * sin_y + dy * cos_y; // left
        let azimuth = local_y.atan2(local_x);

        // Distance attenuation
        let dist_gain = distance_gain_at_model(
            ctx.listener.position,
            ctx.source_pos,
            ctx.source_ref_distance,
            ctx.distance_model.max_distance,
            ctx.distance_model.rolloff,
            ctx.distance_model.model,
        );

        // Source directivity
        let dir_gain = directivity_gain(
            ctx.source_pos,
            ctx.source_orientation,
            ctx.listener.position,
            ctx.source_directivity,
        );

        let gain = dist_gain * dir_gain;

        // Encode into B-format
        let bformat = foa_encode(azimuth, gain);

        // Store W, Y, X in channel_gains[0..3] for the renderer to decode
        output.channel_gains.gains[0] = bformat.w;
        output.channel_gains.gains[1] = bformat.y;
        output.channel_gains.gains[2] = bformat.x;
        output.channel_gains.count = 3;
        // No apply_mask here — channels hold B-format, not speaker gains.
        // The AmbisonicsRenderer applies the mask after decoding.
    }

    fn name(&self) -> &str {
        "ambisonics_encode"
    }
}
