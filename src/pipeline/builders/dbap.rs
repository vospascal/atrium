use crate::pipeline::path_resolvers::ImageSourceResolver;
use crate::pipeline::render::RenderPipeline;
use crate::pipeline::renderers::dbap::DbapRenderer;
use crate::pipeline::stages::delay_comp::DelayCompStage;
use crate::pipeline::stages::fdn_reverb::{FdnConfig, FdnReverbStage};
use crate::pipeline::stages::lfe_crossover::LfeBassManagementStage;
use crate::pipeline::stages::master_gain::MasterGainStage;

use super::{image_source_path_effect_factories, PipelineParams};

pub(in crate::pipeline) fn build(p: &PipelineParams) -> RenderPipeline {
    RenderPipeline::new(
        Box::new(DbapRenderer::new(atrium_core::dbap::DbapParams {
            rolloff_db: p.dbap_rolloff_db,
            ..Default::default()
        })),
        Box::new(ImageSourceResolver),
        image_source_path_effect_factories(p),
        // FDN before LFE crossover: reverb bass gets redirected to LFE
        // alongside the dry signal's bass.
        vec![
            Box::new(FdnReverbStage::with_config(FdnConfig::speaker(
                p.reverb_rt60_seconds,
                p.reverb_pre_delay_ms,
            ))),
            Box::new(LfeBassManagementStage::new()),
            Box::new(DelayCompStage::static_calibration()),
            Box::new(MasterGainStage),
        ],
        0,
        p.reverb_rt60_seconds,
    )
}
