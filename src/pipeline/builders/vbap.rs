use crate::pipeline::path_resolvers::ImageSourceResolver;
use crate::pipeline::render::RenderPipeline;
use crate::pipeline::renderers::multichannel::MultichannelRenderer;
use crate::pipeline::stages::delay_comp::DelayCompStage;
use crate::pipeline::stages::fdn_reverb::{FdnConfig, FdnReverbStage};
use crate::pipeline::stages::lfe_crossover::LfeBassManagementStage;
use crate::pipeline::stages::master_gain::MasterGainStage;

use super::{image_source_path_effect_factories, PipelineParams};

// VBAP vs Ambisonics decision rationale (BBC WHP254, Gandemer 2018, Malecki 2024):
//
//   Off-centre listening: VBAP wins (graceful degradation; Ambisonics needs 4th+ order to match)
//   Trajectory smoothness: Ambisonics wins (VBAP has magnetization effect — sources snap to speakers)
//   Localization accuracy: VBAP wins at center with good layout; comparable at 3rd+ order Ambisonics
//   Computational cost: VBAP lower (2–3 speakers active vs all speakers)
//   Arbitrary layouts: VBAP wins natively; Ambisonics needs AllRAD for irregular arrays
//   Source width control: VBAP manual (MDAP); Ambisonics natural (order-dependent)
//   5.1 without height: No significant difference (Malecki 2024); Atmos only wins on 7.1.4
//
// We support both because the use cases differ: VBAP for multi-listener 5.1 speaker
// arrays (off-centre robustness matters), Ambisonics for FOA scenes and spatial effects
// (multi-delay, rotation operate natively in B-format domain).

pub(in crate::pipeline) fn build(p: &PipelineParams) -> RenderPipeline {
    RenderPipeline::new(
        Box::new(MultichannelRenderer::new()),
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
            Box::new(DelayCompStage::listener_relative()),
            Box::new(MasterGainStage),
        ],
        0,
        p.er_wall_reflectivity,
    )
}
