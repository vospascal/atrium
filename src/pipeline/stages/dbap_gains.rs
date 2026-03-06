//! DBAP gain computation stage.
//!
//! Computes distance-based amplitude panning gains for all speakers.
//! Listener-independent: only source position and speaker positions matter.

use atrium_core::dbap::{dbap_gains, DbapParams};

use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Computes DBAP channel gains for listener-independent panning.
pub struct DbapGainStage {
    params: DbapParams,
    weights: Vec<f32>,
}

impl DbapGainStage {
    pub fn new(params: DbapParams) -> Self {
        Self {
            params,
            weights: Vec::new(),
        }
    }
}

impl SourceStage for DbapGainStage {
    fn process(&mut self, ctx: &SourceContext, output: &mut SourceOutput) {
        let speaker_count = ctx.layout.speaker_count();

        // Ensure weights vector is sized
        if self.weights.len() != speaker_count {
            self.weights = vec![1.0; speaker_count];
        }

        output.channel_gains = dbap_gains(
            ctx.source_pos,
            ctx.layout.speakers(),
            speaker_count,
            &self.weights,
            &self.params,
        );
        ctx.layout.apply_mask(&mut output.channel_gains);
    }

    fn name(&self) -> &str {
        "dbap_gains"
    }
}
