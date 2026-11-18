use crate::pipeline::path_resolvers::ImageSourceResolver;
use crate::pipeline::render::RenderPipeline;
use crate::pipeline::renderers::ambisonics::AmbisonicsRenderer;
use crate::pipeline::stages::ambi_decode::AmbisonicsDecodeStage;
use crate::pipeline::stages::ambi_multi_delay::AmbiMultiDelayStage;
use crate::pipeline::stages::delay_comp::DelayCompStage;
use crate::pipeline::stages::fdn_reverb::{FdnConfig, FdnReverbStage};
use crate::pipeline::stages::lfe_crossover::LfeBassManagementStage;
use crate::pipeline::stages::master_gain::MasterGainStage;

use super::{image_source_path_effect_factories, PipelineParams};

// Why Ambisonics exists alongside VBAP: see the decision rationale in `vbap.rs`.

pub(in crate::pipeline) fn build(p: &PipelineParams) -> RenderPipeline {
    RenderPipeline::new(
        Box::new(AmbisonicsRenderer::new()),
        Box::new(ImageSourceResolver),
        image_source_path_effect_factories(p),
        vec![
            Box::new(AmbiMultiDelayStage::new()),
            Box::new(AmbisonicsDecodeStage::new()),
            Box::new(FdnReverbStage::with_config(FdnConfig::speaker(
                p.reverb_rt60_seconds,
                p.reverb_pre_delay_ms,
            ))),
            Box::new(LfeBassManagementStage::new()),
            Box::new(DelayCompStage::listener_relative()),
            Box::new(MasterGainStage),
        ],
        0,
        p.er_wall_reflectivity,
    )
}
