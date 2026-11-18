use crate::pipeline::path_resolvers::DirectPathResolver;
use crate::pipeline::render::RenderPipeline;
use crate::pipeline::renderers::world_locked::{WorldLockedParams, WorldLockedRenderer};
use crate::pipeline::stages::delay_comp::DelayCompStage;
use crate::pipeline::stages::lfe_crossover::LfeBassManagementStage;
use crate::pipeline::stages::master_gain::MasterGainStage;

use super::{longest_reflection_delay_seconds, PipelineParams};

pub(in crate::pipeline) fn build(p: &PipelineParams) -> RenderPipeline {
    RenderPipeline::new(
        Box::new(WorldLockedRenderer::new(WorldLockedParams {
            longest_delay_seconds: longest_reflection_delay_seconds(p),
        })),
        Box::new(DirectPathResolver),
        vec![],
        // No FDN: WorldLocked reverberates through its own per-speaker reflection
        // taps. Adding a shared FDN on top over-summed the tail and pushed
        // sustained tones into overload in reflective rooms.
        vec![
            Box::new(LfeBassManagementStage::new()),
            Box::new(DelayCompStage::static_calibration()),
            Box::new(MasterGainStage),
        ],
        0,
        // No late reverb in this mode, so no decay time to override.
        None,
    )
}
