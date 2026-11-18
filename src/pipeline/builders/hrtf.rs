use crate::pipeline::path_resolvers::ImageSourceResolver;
use crate::pipeline::render::RenderPipeline;
use crate::pipeline::renderers::binaural::HrtfRenderer;
use crate::pipeline::stages::fdn_reverb::{FdnConfig, FdnReverbStage};
use crate::pipeline::stages::master_gain::MasterGainStage;

use super::{image_source_path_effect_factories, PipelineParams};

pub(in crate::pipeline) fn build(p: &PipelineParams) -> RenderPipeline {
    RenderPipeline::new(
        Box::new(HrtfRenderer::new(&p.hrtf_path)),
        Box::new(ImageSourceResolver),
        image_source_path_effect_factories(p),
        vec![
            Box::new(FdnReverbStage::with_config(FdnConfig::headphone(
                p.reverb_rt60_seconds,
                p.reverb_pre_delay_ms,
            ))),
            Box::new(MasterGainStage),
        ],
        2,
        p.reverb_rt60_seconds,
    )
}
