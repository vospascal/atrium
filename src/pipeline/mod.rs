//! Composable render pipeline.
//!
//! A render pipeline defines the complete audio processing chain for a given
//! render mode. Each pipeline consists of three categories of stages:
//!
//! 1. **SourceStages** — per-source, before routing (envelopes, listener-relative
//!    propagation for VBAP/Stereo/Binaural)
//! 2. **PathStages** — per source × output path, inside the renderer
//!    (per-speaker propagation for WorldLocked)
//! 3. **MixStages** — post-mix, whole-buffer (LFE crossover, delay comp,
//!    reverb, master gain)
//!
//! Plus a **Renderer** that handles mode-specific spatialization:
//! multichannel gain ramp, per-speaker path processing, or HRTF convolution.
//!
//! # Pipeline definitions
//!
//! Each render mode is defined as a function returning a `RenderPipeline`.
//! Reading the definition tells you exactly what stages run:
//!
//! ```text
//! WorldLocked {
//!     source_stages: []
//!     renderer: WorldLockedRenderer { path_stages: [AirAbsorption, Ground, Reflections, DistanceDirectivity] }
//!     mix_stages: [LfeCrossover, DelayComp(static), MasterGain]
//! }
//!
//! Vbap {
//!     source_stages: [AirAbsorption, GroundEffect, Reflections, VbapGains]
//!     renderer: MultichannelRenderer
//!     mix_stages: [LfeCrossover, DelayComp(listener), EarlyReflections, FdnReverb, MasterGain]
//! }
//! ```

pub mod mix_stage;
pub mod path_stage;
pub mod renderer;
pub mod renderers;
pub mod source_stage;
pub mod stages;

use atrium_core::speaker::SpeakerLayout;

use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::distance::DistanceModel;
use crate::audio::propagation::GroundProperties;

use self::mix_stage::{MixContext, MixStage};
use self::renderer::Renderer;
use self::source_stage::{SourceContext, SourceOutput, SourceStage};

use self::renderers::binaural::BinauralRenderer;
use self::renderers::multichannel::MultichannelRenderer;
use self::renderers::world_locked::WorldLockedRenderer;
use self::stages::air_absorption::{AirAbsorptionPath, AirAbsorptionStage};
use self::stages::delay_comp::DelayCompStage;
use self::stages::distance_directivity::DistanceDirectivityPath;
use self::stages::distance_gains::DistanceGainStage;
use self::stages::early_reflections::EarlyReflectionsStage;
use self::stages::fdn_reverb::FdnReverbStage;
use self::stages::ground_effect::{GroundEffectPath, GroundEffectStage};
use self::stages::lfe_crossover::LfeCrossoverStage;
use self::stages::master_gain::MasterGainStage;
use self::stages::reflections::{ReflectionsPath, ReflectionsStage};
use self::stages::stereo_gains::StereoGainStage;
use self::stages::vbap_gains::VbapGainStage;

// ─────────────────────────────────────────────────────────────────────────────
// Pipeline mode — rendering approach (separate from channel layout)
// ─────────────────────────────────────────────────────────────────────────────

/// Rendering approach. Determines which pipeline runs.
///
// RenderMode (atrium_core::speaker): WorldLocked, Vbap, Stereo, Binaural.
// index() and ALL defined on RenderMode. Used as pipeline array index.

// ─────────────────────────────────────────────────────────────────────────────
// SourceStageBank — factory-managed per-source instances
// ─────────────────────────────────────────────────────────────────────────────

/// Factory function that creates one SourceStage instance given the sample rate.
type SourceStageFactory = Box<dyn Fn(f32) -> Box<dyn SourceStage> + Send>;

/// A column of per-source stage instances, grown by a factory.
struct StageColumn {
    factory: SourceStageFactory,
    instances: Vec<Box<dyn SourceStage>>,
}

/// Manages per-source SourceStage instances via factories.
///
/// Each "column" is one stage type (e.g., AirAbsorption). Each column has
/// N instances, one per source. When sources are added, new instances are
/// created via the factory function.
pub struct SourceStageBank {
    columns: Vec<StageColumn>,
    sample_rate: f32,
}

impl SourceStageBank {
    fn new(factories: Vec<SourceStageFactory>, sample_rate: f32) -> Self {
        Self {
            columns: factories
                .into_iter()
                .map(|f| StageColumn {
                    factory: f,
                    instances: Vec::new(),
                })
                .collect(),
            sample_rate,
        }
    }

    /// Ensure we have enough per-source instances.
    pub fn ensure_sources(&mut self, count: usize) {
        for col in &mut self.columns {
            while col.instances.len() < count {
                col.instances.push((col.factory)(self.sample_rate));
            }
        }
    }

    /// Run all stages' `process()` for a given source index.
    pub fn process_all(
        &mut self,
        source_idx: usize,
        ctx: &SourceContext,
        output: &mut SourceOutput,
    ) {
        for col in &mut self.columns {
            if let Some(stage) = col.instances.get_mut(source_idx) {
                stage.process(ctx, output);
            }
        }
    }

    /// Collect mutable references to all stages for a given source,
    /// for passing to the renderer's inner loop (`process_sample`).
    pub fn for_source(&mut self, source_idx: usize) -> Vec<&mut dyn SourceStage> {
        let mut refs = Vec::with_capacity(self.columns.len());
        for col in &mut self.columns {
            if let Some(stage) = col.instances.get_mut(source_idx) {
                refs.push(&mut **stage as &mut dyn SourceStage);
            }
        }
        refs
    }

    /// Reset all stage instances.
    pub fn reset(&mut self) {
        for col in &mut self.columns {
            for instance in &mut col.instances {
                instance.reset();
            }
        }
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// RenderPipeline — one per mode
// ─────────────────────────────────────────────────────────────────────────────

/// A complete render pipeline for one mode.
pub struct RenderPipeline {
    pub source_stages: SourceStageBank,
    pub renderer: Box<dyn Renderer>,
    pub mix_stages: Vec<Box<dyn MixStage>>,
}

impl RenderPipeline {
    /// Initialize all mix stages (called when audio params are known).
    pub fn init(&mut self, ctx: &MixContext) {
        for stage in &mut self.mix_stages {
            stage.init(ctx);
        }
    }

    /// Reset all state (called on mode switch).
    pub fn reset(&mut self) {
        self.source_stages.reset();
        self.renderer.reset();
        for stage in &mut self.mix_stages {
            stage.reset();
        }
    }

    /// Ensure topology matches current source count and layout.
    pub fn ensure_topology(
        &mut self,
        source_count: usize,
        layout: &SpeakerLayout,
        sample_rate: f32,
    ) {
        self.source_stages.ensure_sources(source_count);
        self.renderer
            .ensure_topology(source_count, layout, sample_rate);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pipeline constructors
// ─────────────────────────────────────────────────────────────────────────────

/// Parameters for building pipelines (extracted from scene config).
pub struct PipelineParams {
    pub sample_rate: f32,
    pub hrtf_path: String,
    pub er_wet_gain: f32,
    pub er_wall_absorption: f32,
    pub fdn_wet_gain: f32,
    pub fdn_rt60_low: f32,
    pub fdn_rt60_high: f32,
    pub distance_model: DistanceModel,
}

impl Default for PipelineParams {
    fn default() -> Self {
        Self {
            sample_rate: 48000.0,
            hrtf_path: "assets/hrtf/default.sofa".into(),
            er_wet_gain: 0.4,
            er_wall_absorption: 0.9,
            fdn_wet_gain: 0.2,
            fdn_rt60_low: 0.8,
            fdn_rt60_high: 0.3,
            distance_model: DistanceModel::default(),
        }
    }
}

/// Build all 4 pipelines. Pre-allocated at startup, zero allocation on mode switch.
pub fn build_all_pipelines(params: &PipelineParams) -> [RenderPipeline; 4] {
    [
        build_world_locked(params),
        build_vbap(params),
        build_stereo(params),
        build_binaural(params),
    ]
}

fn build_world_locked(p: &PipelineParams) -> RenderPipeline {
    let sr = p.sample_rate;
    let dm = &p.distance_model;
    let wet = p.er_wet_gain;
    let abs = p.er_wall_absorption;
    let ref_dist = dm.ref_distance;
    let max_dist = dm.max_distance;
    let rolloff = dm.rolloff;
    let model = dm.model;

    RenderPipeline {
        // WorldLocked: no source stages — propagation lives in the renderer
        source_stages: SourceStageBank::new(vec![], sr),
        renderer: Box::new(WorldLockedRenderer::new(vec![
            Box::new(move |sr| {
                Box::new(AirAbsorptionPath::new(sr))
                    as Box<dyn crate::pipeline::path_stage::PathStage>
            }),
            Box::new(move |_sr| {
                Box::new(GroundEffectPath::new()) as Box<dyn crate::pipeline::path_stage::PathStage>
            }),
            Box::new(move |_sr| {
                Box::new(ReflectionsPath::new(wet, abs))
                    as Box<dyn crate::pipeline::path_stage::PathStage>
            }),
            Box::new(move |_sr| {
                Box::new(DistanceDirectivityPath::new(
                    ref_dist, max_dist, rolloff, model,
                )) as Box<dyn crate::pipeline::path_stage::PathStage>
            }),
        ])),
        mix_stages: vec![
            Box::new(LfeCrossoverStage::new()),
            Box::new(DelayCompStage::static_calibration()),
            Box::new(MasterGainStage),
        ],
    }
}

fn build_vbap(p: &PipelineParams) -> RenderPipeline {
    let sr = p.sample_rate;
    let wet = p.er_wet_gain;
    let abs = p.er_wall_absorption;

    RenderPipeline {
        source_stages: SourceStageBank::new(
            vec![
                Box::new(move |sr| Box::new(AirAbsorptionStage::new(sr)) as Box<dyn SourceStage>),
                Box::new(move |_sr| Box::new(GroundEffectStage) as Box<dyn SourceStage>),
                Box::new(move |_sr| {
                    Box::new(ReflectionsStage::new(wet, abs)) as Box<dyn SourceStage>
                }),
                Box::new(move |_sr| Box::new(VbapGainStage) as Box<dyn SourceStage>),
            ],
            sr,
        ),
        renderer: Box::new(MultichannelRenderer::new()),
        mix_stages: vec![
            Box::new(LfeCrossoverStage::new()),
            Box::new(DelayCompStage::listener_relative()),
            Box::new(EarlyReflectionsStage::new(wet, abs)),
            Box::new(FdnReverbStage::new(
                p.fdn_wet_gain,
                p.fdn_rt60_low,
                p.fdn_rt60_high,
            )),
            Box::new(MasterGainStage),
        ],
    }
}

fn build_stereo(p: &PipelineParams) -> RenderPipeline {
    let sr = p.sample_rate;
    let wet = p.er_wet_gain;
    let abs = p.er_wall_absorption;

    RenderPipeline {
        source_stages: SourceStageBank::new(
            vec![
                Box::new(move |sr| Box::new(AirAbsorptionStage::new(sr)) as Box<dyn SourceStage>),
                Box::new(move |_sr| Box::new(GroundEffectStage) as Box<dyn SourceStage>),
                Box::new(move |_sr| {
                    Box::new(ReflectionsStage::new(wet, abs)) as Box<dyn SourceStage>
                }),
                Box::new(move |_sr| Box::new(StereoGainStage) as Box<dyn SourceStage>),
            ],
            sr,
        ),
        renderer: Box::new(MultichannelRenderer::new()),
        mix_stages: vec![
            Box::new(EarlyReflectionsStage::new(wet, abs)),
            Box::new(FdnReverbStage::new(
                p.fdn_wet_gain,
                p.fdn_rt60_low,
                p.fdn_rt60_high,
            )),
            Box::new(MasterGainStage),
        ],
    }
}

fn build_binaural(p: &PipelineParams) -> RenderPipeline {
    let sr = p.sample_rate;
    let wet = p.er_wet_gain;
    let abs = p.er_wall_absorption;

    RenderPipeline {
        source_stages: SourceStageBank::new(
            vec![
                Box::new(move |sr| Box::new(AirAbsorptionStage::new(sr)) as Box<dyn SourceStage>),
                Box::new(move |_sr| Box::new(GroundEffectStage) as Box<dyn SourceStage>),
                Box::new(move |_sr| {
                    Box::new(ReflectionsStage::new(wet, abs)) as Box<dyn SourceStage>
                }),
                Box::new(move |_sr| Box::new(DistanceGainStage) as Box<dyn SourceStage>),
            ],
            sr,
        ),
        renderer: Box::new(BinauralRenderer::new(&p.hrtf_path, sr)),
        mix_stages: vec![
            Box::new(EarlyReflectionsStage::new(wet, abs)),
            Box::new(FdnReverbStage::new(
                p.fdn_wet_gain,
                p.fdn_rt60_low,
                p.fdn_rt60_high,
            )),
            Box::new(MasterGainStage),
        ],
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Tests
// ─────────────────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use atrium_core::listener::Listener;
    use atrium_core::source::SoundSource;
    use atrium_core::speaker::RenderMode;
    use atrium_core::types::Vec3;

    use crate::audio::atmosphere::AtmosphericParams;
    use crate::audio::distance::DistanceModel;
    use crate::audio::propagation::GroundProperties;

    /// Constant-value test source.
    struct ConstSource {
        pos: Vec3,
        val: f32,
    }
    impl SoundSource for ConstSource {
        fn next_sample(&mut self, _sr: f32) -> f32 {
            self.val
        }
        fn position(&self) -> Vec3 {
            self.pos
        }
        fn tick(&mut self, _dt: f32) {}
        fn is_active(&self) -> bool {
            true
        }
    }

    fn default_atmosphere() -> AtmosphericParams {
        AtmosphericParams {
            temperature_c: 20.0,
            humidity_pct: 50.0,
            pressure_kpa: 101.325,
        }
    }

    fn default_ground() -> GroundProperties {
        GroundProperties::default()
    }

    fn default_distance_model() -> DistanceModel {
        DistanceModel::default()
    }

    fn layout_5_1() -> SpeakerLayout {
        SpeakerLayout::surround_5_1(
            Vec3::new(0.0, 4.0, 0.0), // FL
            Vec3::new(6.0, 4.0, 0.0), // FR
            Vec3::new(3.0, 4.0, 0.0), // C
            Vec3::new(0.0, 0.0, 0.0), // RL
            Vec3::new(6.0, 0.0, 0.0), // RR
        )
    }

    // ── WorldLocked: gains independent of listener position ──────────────

    #[test]
    fn world_locked_gains_independent_of_listener() {
        let layout = layout_5_1();
        let dm = default_distance_model();
        let atm = default_atmosphere();
        let ground = default_ground();
        let source_pos = Vec3::new(1.0, 1.0, 0.0);

        // Build a minimal WorldLocked pipeline (only DistanceDirectivity path stage)
        let params = PipelineParams {
            sample_rate: 48000.0,
            ..PipelineParams::default()
        };
        let mut pipeline = build_world_locked(&params);

        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: source_pos,
            val: 1.0,
        })];

        let channels = 6;
        let frames = 512;
        let mut buf_a = vec![0.0f32; frames * channels];
        let mut buf_b = vec![0.0f32; frames * channels];

        // Listener at center of room
        let listener_a = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
        render_pipeline(
            &mut pipeline,
            &mut sources,
            &listener_a,
            &mut buf_a,
            channels,
            48000.0,
            1.0,
            &dm,
            &layout,
            &atm,
            &ground,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(6.0, 4.0, 3.0),
        );

        // Reset for second pass (clear gain ramp state)
        pipeline.reset();

        // Listener at a completely different position
        let listener_b = Listener::new(Vec3::new(5.0, 0.5, 0.0), 0.0);
        render_pipeline(
            &mut pipeline,
            &mut sources,
            &listener_b,
            &mut buf_b,
            channels,
            48000.0,
            1.0,
            &dm,
            &layout,
            &atm,
            &ground,
            Vec3::new(0.0, 0.0, 0.0),
            Vec3::new(6.0, 4.0, 3.0),
        );

        // Gains should be identical (WorldLocked ignores listener position).
        // We compare per-channel energy (RMS) — the first buffer ramps from 0,
        // so compare the second half where ramps have settled.
        let half = frames / 2;
        for ch in 0..channels {
            let rms_a: f32 = (half..frames)
                .map(|f| buf_a[f * channels + ch].powi(2))
                .sum::<f32>()
                / half as f32;
            let rms_b: f32 = (half..frames)
                .map(|f| buf_b[f * channels + ch].powi(2))
                .sum::<f32>()
                / half as f32;
            assert!(
                (rms_a - rms_b).abs() < 1e-6,
                "WorldLocked ch{} RMS differs: {:.6} vs {:.6}",
                ch,
                rms_a,
                rms_b,
            );
        }
    }

    // ── WorldLocked: nearest speaker gets highest gain ───────────────────

    #[test]
    fn world_locked_nearest_speaker_highest_gain() {
        let layout = layout_5_1();
        let dm = default_distance_model();
        let atm = default_atmosphere();
        let ground = default_ground();

        // Source very close to rear-left speaker (0,0,0)
        let source_pos = Vec3::new(0.1, 0.1, 0.0);

        let params = PipelineParams::default();
        let mut pipeline = build_world_locked(&params);

        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: source_pos,
            val: 1.0,
        })];

        let channels = 6;
        let frames = 1024;
        let mut buffer = vec![0.0f32; frames * channels];

        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
        render_pipeline(
            &mut pipeline,
            &mut sources,
            &listener,
            &mut buffer,
            channels,
            48000.0,
            1.0,
            &dm,
            &layout,
            &atm,
            &ground,
            Vec3::ZERO,
            Vec3::new(6.0, 4.0, 3.0),
        );

        // Measure RMS of last quarter (gain ramp settled)
        let quarter = frames * 3 / 4;
        let mut ch_rms = [0.0f32; 6];
        for ch in 0..channels {
            ch_rms[ch] = (quarter..frames)
                .map(|f| buffer[f * channels + ch].powi(2))
                .sum::<f32>()
                / (frames - quarter) as f32;
        }

        // RL is channel 4 — it should have the highest gain
        let rl_rms = ch_rms[4];
        for (ch, &rms) in ch_rms.iter().enumerate() {
            if ch == 4 || ch == 3 {
                continue;
            } // skip RL itself and LFE (no speaker)
            assert!(
                rl_rms >= rms,
                "WorldLocked: RL (ch4) RMS {:.6} should be >= ch{} RMS {:.6}",
                rl_rms,
                ch,
                rms,
            );
        }
    }

    // ── Speaker mask: masked channels get zero output ────────────────────

    #[test]
    fn speaker_mask_zeroes_masked_channels() {
        let mut layout = layout_5_1();
        // Mask center channel (2) and LFE (3)
        layout.set_active_channels(&[0, 1, 4, 5]);

        let dm = default_distance_model();
        let atm = default_atmosphere();
        let ground = default_ground();

        // Source at center of room
        let source_pos = Vec3::new(3.0, 2.0, 0.0);

        let params = PipelineParams::default();
        let mut pipeline = build_world_locked(&params);

        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: source_pos,
            val: 1.0,
        })];

        let channels = 6;
        let frames = 512;
        let mut buffer = vec![0.0f32; frames * channels];

        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
        render_pipeline(
            &mut pipeline,
            &mut sources,
            &listener,
            &mut buffer,
            channels,
            48000.0,
            1.0,
            &dm,
            &layout,
            &atm,
            &ground,
            Vec3::ZERO,
            Vec3::new(6.0, 4.0, 3.0),
        );

        // Channels 2 and 3 should be completely silent
        for frame in 0..frames {
            assert_eq!(
                buffer[frame * channels + 2],
                0.0,
                "Center ch should be masked"
            );
            assert_eq!(buffer[frame * channels + 3], 0.0, "LFE ch should be masked");
        }

        // Active channels should have some signal
        let has_signal =
            |ch: usize| -> bool { (0..frames).any(|f| buffer[f * channels + ch].abs() > 1e-10) };
        assert!(has_signal(0), "FL should have signal");
        assert!(has_signal(1), "FR should have signal");
        assert!(has_signal(4), "RL should have signal");
        assert!(has_signal(5), "RR should have signal");
    }

    // ── VBAP mask: masked channels zeroed via apply_mask ─────────────────

    #[test]
    fn vbap_mask_zeroes_masked_channels() {
        let mut layout = layout_5_1();
        // Quad profile: mask center (2) and LFE (3)
        layout.set_active_channels(&[0, 1, 4, 5]);

        let dm = default_distance_model();
        let atm = default_atmosphere();
        let ground = default_ground();

        let source_pos = Vec3::new(3.0, 3.0, 0.0);

        let params = PipelineParams::default();
        let mut pipeline = build_vbap(&params);

        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: source_pos,
            val: 1.0,
        })];

        let channels = 6;
        let frames = 512;
        let mut buffer = vec![0.0f32; frames * channels];

        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
        render_pipeline(
            &mut pipeline,
            &mut sources,
            &listener,
            &mut buffer,
            channels,
            48000.0,
            1.0,
            &dm,
            &layout,
            &atm,
            &ground,
            Vec3::ZERO,
            Vec3::new(6.0, 4.0, 3.0),
        );

        // Masked channels 2 and 3 should be silent
        for frame in 0..frames {
            assert_eq!(
                buffer[frame * channels + 2],
                0.0,
                "Center ch masked in VBAP"
            );
            assert_eq!(buffer[frame * channels + 3], 0.0, "LFE ch masked in VBAP");
        }
    }

    // ── RenderMode index ──────────────────────────────────────────────────

    #[test]
    fn render_mode_index_roundtrip() {
        for mode in RenderMode::ALL {
            assert_eq!(RenderMode::ALL[mode.index()], mode);
        }
    }

    // ── SourceStageBank: factory creates per-source instances ────────────

    #[test]
    fn source_stage_bank_grows_with_sources() {
        let mut bank = SourceStageBank::new(
            vec![
                Box::new(|_sr| Box::new(VbapGainStage) as Box<dyn SourceStage>),
                Box::new(|_sr| Box::new(StereoGainStage) as Box<dyn SourceStage>),
            ],
            48000.0,
        );

        bank.ensure_sources(3);
        assert_eq!(bank.columns.len(), 2);
        assert_eq!(bank.columns[0].instances.len(), 3);
        assert_eq!(bank.columns[1].instances.len(), 3);

        // Growing doesn't shrink
        bank.ensure_sources(5);
        assert_eq!(bank.columns[0].instances.len(), 5);

        // Smaller count is a no-op
        bank.ensure_sources(2);
        assert_eq!(bank.columns[0].instances.len(), 5);
    }

    // ── Pipeline reset clears gain ramps ─────────────────────────────────

    #[test]
    fn pipeline_reset_clears_state() {
        let params = PipelineParams::default();
        let mut pipeline = build_vbap(&params);
        let layout = layout_5_1();

        pipeline.ensure_topology(2, &layout, 48000.0);

        // After reset, renderer should have zeroed prev_gains
        // (can't inspect directly, but calling reset should not panic)
        pipeline.reset();
    }

    // ── Build all pipelines without panic ────────────────────────────────

    #[test]
    fn build_all_pipelines_smoke() {
        let params = PipelineParams::default();
        let pipelines = build_all_pipelines(&params);
        assert_eq!(pipelines.len(), 4);
        assert_eq!(
            pipelines[RenderMode::WorldLocked.index()].renderer.name(),
            "world_locked"
        );
        assert_eq!(
            pipelines[RenderMode::Vbap.index()].renderer.name(),
            "multichannel"
        );
        assert_eq!(
            pipelines[RenderMode::Stereo.index()].renderer.name(),
            "multichannel"
        );
        assert_eq!(
            pipelines[RenderMode::Binaural.index()].renderer.name(),
            "binaural"
        );
    }

    // ── WorldLocked: per-speaker air absorption differs with distance ─────

    #[test]
    fn world_locked_air_absorption_differs_by_distance() {
        // Two speakers at very different distances from source.
        // The nearer speaker should have higher high-frequency content
        // (less air absorption) than the far speaker.
        use atrium_core::speaker::{Speaker, SpeakerLayout};

        let near_pos = Vec3::new(1.0, 0.0, 0.0); // 1m from source
        let far_pos = Vec3::new(15.0, 0.0, 0.0); // 15m from source
        let layout = SpeakerLayout::new(
            &[
                Speaker {
                    position: near_pos,
                    channel: 0,
                },
                Speaker {
                    position: far_pos,
                    channel: 1,
                },
            ],
            None,
            2,
        );

        let dm = default_distance_model();
        let atm = default_atmosphere();
        let ground = default_ground();
        let source_pos = Vec3::ZERO;

        let params = PipelineParams::default();
        let mut pipeline = build_world_locked(&params);
        pipeline.ensure_topology(1, &layout, 48000.0);

        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: source_pos,
            val: 1.0,
        })];

        let channels = 2;
        let frames = 2048; // enough for filter to settle
        let mut buffer = vec![0.0f32; frames * channels];

        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
        render_pipeline(
            &mut pipeline,
            &mut sources,
            &listener,
            &mut buffer,
            channels,
            48000.0,
            1.0,
            &dm,
            &layout,
            &atm,
            &ground,
            Vec3::new(-20.0, -20.0, -5.0),
            Vec3::new(20.0, 20.0, 5.0),
        );

        // Compare RMS of last quarter (filters settled)
        let quarter = frames * 3 / 4;
        let rms = |ch: usize| -> f32 {
            (quarter..frames)
                .map(|f| buffer[f * channels + ch].powi(2))
                .sum::<f32>()
                / (frames - quarter) as f32
        };

        let near_rms = rms(0);
        let far_rms = rms(1);

        // Near speaker should have significantly more signal than far
        // (less distance attenuation + less air absorption filtering)
        assert!(
            near_rms > far_rms * 2.0,
            "Near speaker RMS ({near_rms:.6}) should be significantly louder than far ({far_rms:.6})"
        );

        // Both should have nonzero signal (WorldLocked renders to both)
        assert!(near_rms > 1e-10, "Near speaker should have signal");
        assert!(far_rms > 1e-10, "Far speaker should have signal");
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pipeline render dispatch
// ─────────────────────────────────────────────────────────────────────────────

/// Render one buffer through the active pipeline.
///
/// This replaces the monolithic `mix_sources()` + `BinauralMixer::mix()` path.
pub fn render_pipeline(
    pipeline: &mut RenderPipeline,
    sources: &mut [Box<dyn atrium_core::source::SoundSource>],
    listener: &atrium_core::listener::Listener,
    output: &mut [f32],
    channels: usize,
    sample_rate: f32,
    master_gain: f32,
    distance_model: &DistanceModel,
    layout: &SpeakerLayout,
    atmosphere: &AtmosphericParams,
    ground: &GroundProperties,
    room_min: atrium_core::types::Vec3,
    room_max: atrium_core::types::Vec3,
) {
    use crate::profile_span;
    let num_frames = output.len() / channels;

    // Split borrow: source_stages and renderer are independent fields
    let RenderPipeline {
        source_stages,
        renderer,
        mix_stages,
    } = pipeline;

    // Ensure topology
    source_stages.ensure_sources(sources.len());
    renderer.ensure_topology(sources.len(), layout, sample_rate);

    // Zero output
    output.fill(0.0);

    // Per-source pipeline
    for (i, source) in sources.iter_mut().enumerate() {
        if !source.is_active() {
            continue;
        }

        let pos = source.position();
        let dist_to_listener = listener.position.distance_to(pos);

        let ctx = SourceContext {
            listener,
            source_pos: pos,
            source_orientation: source.orientation(),
            source_directivity: &source.directivity(),
            source_spread: source.spread(),
            source_ref_distance: source.ref_distance(),
            dist_to_listener,
            atmosphere,
            room_min,
            room_max,
            ground,
            sample_rate,
            distance_model,
            layout,
        };

        // Buffer-rate source stages
        let mut src_out = SourceOutput::default_for(layout.total_channels());
        {
            let _s = profile_span!("source_stages", src = i).entered();
            source_stages.process_all(i, &ctx, &mut src_out);
        }

        // Collect source stage refs for the inner loop
        let mut stage_refs = source_stages.for_source(i);

        // Renderer: mode-specific spatialization
        {
            let _s = profile_span!("renderer", src = i).entered();
            renderer.render_source(
                i,
                source.as_mut(),
                &mut stage_refs,
                &ctx,
                &src_out,
                output,
                channels,
                num_frames,
                sample_rate,
            );
        }
    }

    // Post-mix chain
    let mix_ctx = MixContext {
        listener,
        layout,
        sample_rate,
        channels,
        room_min,
        room_max,
        master_gain,
    };
    {
        let _s = profile_span!("mix_stages").entered();
        for stage in mix_stages.iter_mut() {
            stage.process(output, &mix_ctx);
        }
    }
}
