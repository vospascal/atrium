//! Composable render pipeline.
//!
//! A render pipeline defines the complete audio processing chain for a given
//! render mode. Each pipeline consists of two categories of stages:
//!
//! 1. **SourceStages** — per-source, before routing (envelopes, listener-relative
//!    propagation for VBAP/HRTF)
//! 2. **MixStages** — post-mix, whole-buffer (LFE crossover, delay comp,
//!    reverb, master gain)
//!
//! Plus a **Renderer** that handles mode-specific spatialization:
//! multichannel gain ramp, per-speaker propagation, or HRTF convolution.
//!
//! # Pipeline definitions
//!
//! Each render mode is defined as a function returning a `RenderPipeline`.
//! Reading the definition tells you exactly what stages run:
//!
//! ```text
//! WorldLocked {
//!     source_stages: []
//!     renderer: WorldLockedRenderer (per-speaker air absorption, ground effect, reflections, distance+directivity)
//!     mix_stages: [LfeCrossover, DelayComp(static), MasterGain]
//! }
//!
//! Vbap {
//!     resolver: ImageSourceResolver (1 direct + up to 6 reflections)
//!     path_effects: [PropagationDelay, AirAbsorption, GroundEffect, WallAbsorption]
//!     renderer: MultichannelRenderer (per-path VBAP gains)
//!     mix_stages: [LfeCrossover, DelayComp(listener), FdnReverb, MasterGain]
//! }
//!
//! Hrtf {
//!     resolver: ImageSourceResolver (1 direct + up to 6 reflections)
//!     path_effects: [PropagationDelay, AirAbsorption, GroundEffect, WallAbsorption]
//!     renderer: HrtfRenderer (per-path HRTF convolution to stereo)
//!     mix_stages: [FdnReverb, MasterGain]
//! }
//!
//! Dbap {
//!     resolver: ImageSourceResolver (1 direct + up to 6 reflections)
//!     path_effects: [PropagationDelay, AirAbsorption, GroundEffect, WallAbsorption]
//!     renderer: DbapRenderer (per-path DBAP gains)
//!     mix_stages: [LfeCrossover, DelayComp(static), MasterGain]
//! }
//!
//! Ambisonics {
//!     resolver: ImageSourceResolver (1 direct + up to 6 reflections)
//!     path_effects: [PropagationDelay, AirAbsorption, GroundEffect, WallAbsorption]
//!     renderer: AmbisonicsRenderer (per-path FOA encode + decode)
//!     mix_stages: [AmbiMultiDelay, AmbiDecode, LfeCrossover, DelayComp(listener), FdnReverb, MasterGain]
//! }
//! ```

pub mod mix_stage;
pub mod path;
pub mod path_effects;
pub mod path_resolvers;
pub mod renderer;
pub mod renderers;
pub mod source_stage;
pub mod stages;

use atrium_core::speaker::SpeakerLayout;

use crate::audio::atmosphere::AtmosphericParams;
use crate::audio::distance::DistanceModel;
use crate::audio::propagation::GroundProperties;

use self::mix_stage::{MixContext, MixStage};
use self::path::{PathEffect, PathEffectChain, PathEffectFactory, PathResolver, MAX_PATHS};
use self::renderer::Renderer;
use self::source_stage::{SourceContext, SourceOutput, SourceStage};

use self::path_resolvers::{DirectPathResolver, ImageSourceResolver};
use self::renderers::ambisonics::AmbisonicsRenderer;
use self::renderers::binaural::HrtfRenderer;
use self::renderers::dbap::DbapRenderer;
use self::renderers::multichannel::MultichannelRenderer;
use self::renderers::world_locked::WorldLockedRenderer;
use self::stages::ambi_decode::AmbisonicsDecodeStage;
use self::stages::ambi_multi_delay::AmbiMultiDelayStage;
use self::stages::delay_comp::DelayCompStage;
use self::stages::fdn_reverb::FdnReverbStage;
use self::stages::lfe_crossover::LfeCrossoverStage;
use self::stages::master_gain::MasterGainStage;

// ─────────────────────────────────────────────────────────────────────────────
// Pipeline mode — rendering approach (separate from channel layout)
// ─────────────────────────────────────────────────────────────────────────────

// Rendering approach. Determines which pipeline runs.
// RenderMode (atrium_core::speaker): WorldLocked, Vbap, Hrtf, Dbap, Ambisonics.
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
    pub resolver: Box<dyn PathResolver>,
    /// Factories for creating per-path effects (air absorption, ground effect, etc.).
    pub path_effect_factories: Vec<PathEffectFactory>,
    /// Per-source × per-path effect chains. Grown by ensure_topology.
    /// Indexed: [source_idx][path_idx].
    path_effects: Vec<[PathEffectChain; MAX_PATHS]>,
    /// How many output channels the renderer uses. 0 = use ctx.channels (default).
    /// HRTF sets this to 2 so post-mix stages like FDN reverb don't spread
    /// wet signal to channels the renderer never wrote to.
    pub render_channels: usize,
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
        for chains in &mut self.path_effects {
            for chain in chains.iter_mut() {
                chain.reset();
            }
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

        // Grow per-source path effect chains.
        while self.path_effects.len() < source_count {
            let chains: [PathEffectChain; MAX_PATHS] = std::array::from_fn(|_| {
                let effects: Vec<Box<dyn PathEffect>> = self
                    .path_effect_factories
                    .iter()
                    .map(|f| f(sample_rate))
                    .collect();
                PathEffectChain::new(effects)
            });
            self.path_effects.push(chains);
        }
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
    pub er_wall_reflectivity: f32,
    pub fdn_wet_gain: f32,
    pub fdn_rt60_low: f32,
    pub fdn_rt60_high: f32,
    pub distance_model: DistanceModel,
    /// DBAP rolloff in dB per doubling of distance.
    /// 6.0 = free-field inverse distance law, 3–5 dB for reverberant spaces.
    pub dbap_rolloff_db: f32,
    /// Ambisonics multi-delay wet gain (0.0 = dry only, 1.0 = full wet).
    pub ambi_wet_gain: f32,
}

impl Default for PipelineParams {
    fn default() -> Self {
        Self {
            sample_rate: 48000.0,
            hrtf_path: "assets/hrtf/default.sofa".into(),
            er_wet_gain: 0.4,
            er_wall_reflectivity: 0.9,
            fdn_wet_gain: 0.2,
            fdn_rt60_low: 0.8,
            fdn_rt60_high: 0.3,
            distance_model: DistanceModel::default(),
            dbap_rolloff_db: 6.0,
            ambi_wet_gain: 0.3,
        }
    }
}

/// Build all 5 pipelines. Pre-allocated at startup, zero allocation on mode switch.
pub fn build_all_pipelines(params: &PipelineParams) -> [RenderPipeline; 5] {
    [
        build_world_locked(params),
        build_vbap(params),
        build_hrtf(params),
        build_dbap(params),
        build_ambisonics(params),
    ]
}

fn build_world_locked(p: &PipelineParams) -> RenderPipeline {
    let sample_rate = p.sample_rate;
    let distance_model = &p.distance_model;

    RenderPipeline {
        // WorldLocked: no source stages — propagation lives in the renderer
        source_stages: SourceStageBank::new(vec![], sample_rate),
        renderer: Box::new(WorldLockedRenderer::new(
            self::renderers::world_locked::WorldLockedParams {
                ref_distance: distance_model.ref_distance,
                max_distance: distance_model.max_distance,
                rolloff: distance_model.rolloff,
                model: distance_model.model,
                wet_gain: p.er_wet_gain,
                wall_reflectivity: p.er_wall_reflectivity,
            },
        )),
        mix_stages: vec![
            Box::new(LfeCrossoverStage::new()),
            Box::new(DelayCompStage::static_calibration()),
            Box::new(MasterGainStage),
        ],
        resolver: Box::new(DirectPathResolver),
        path_effect_factories: vec![],
        path_effects: Vec::new(),
        render_channels: 0,
    }
}

fn build_vbap(p: &PipelineParams) -> RenderPipeline {
    let sample_rate = p.sample_rate;
    let wall_reflectivity = p.er_wall_reflectivity;

    RenderPipeline {
        // VBAP: no source stages — air absorption and ground effect are per-path PathEffects.
        source_stages: SourceStageBank::new(vec![], sample_rate),
        renderer: Box::new(MultichannelRenderer::new()),
        mix_stages: vec![
            Box::new(LfeCrossoverStage::new()),
            Box::new(DelayCompStage::listener_relative()),
            Box::new(FdnReverbStage::new(
                p.fdn_wet_gain,
                p.fdn_rt60_low,
                p.fdn_rt60_high,
            )),
            Box::new(MasterGainStage),
        ],
        resolver: Box::new(ImageSourceResolver::new(wall_reflectivity)),
        path_effect_factories: vec![
            Box::new(|sample_rate| {
                Box::new(path_effects::PropagationDelayEffect::new(sample_rate))
                    as Box<dyn PathEffect>
            }),
            Box::new(|sample_rate| {
                Box::new(path_effects::AirAbsorptionEffect::new(sample_rate)) as Box<dyn PathEffect>
            }),
            Box::new(|sample_rate| {
                Box::new(path_effects::GroundEffectFilter::new(sample_rate)) as Box<dyn PathEffect>
            }),
            Box::new(|sample_rate| {
                Box::new(path_effects::WallAbsorptionEffect::new(sample_rate))
                    as Box<dyn PathEffect>
            }),
        ],
        path_effects: Vec::new(),
        render_channels: 0,
    }
}

fn build_hrtf(p: &PipelineParams) -> RenderPipeline {
    let sample_rate = p.sample_rate;
    let wall_reflectivity = p.er_wall_reflectivity;

    RenderPipeline {
        // HRTF: no source stages — air absorption and ground effect are per-path PathEffects.
        source_stages: SourceStageBank::new(vec![], sample_rate),
        renderer: Box::new(HrtfRenderer::new(&p.hrtf_path, sample_rate)),
        mix_stages: vec![
            Box::new(FdnReverbStage::new(
                p.fdn_wet_gain,
                p.fdn_rt60_low,
                p.fdn_rt60_high,
            )),
            Box::new(MasterGainStage),
        ],
        resolver: Box::new(ImageSourceResolver::new(wall_reflectivity)),
        path_effect_factories: vec![
            Box::new(|sample_rate| {
                Box::new(path_effects::PropagationDelayEffect::new(sample_rate))
                    as Box<dyn PathEffect>
            }),
            Box::new(|sample_rate| {
                Box::new(path_effects::AirAbsorptionEffect::new(sample_rate)) as Box<dyn PathEffect>
            }),
            Box::new(|sample_rate| {
                Box::new(path_effects::GroundEffectFilter::new(sample_rate)) as Box<dyn PathEffect>
            }),
            Box::new(|sample_rate| {
                Box::new(path_effects::WallAbsorptionEffect::new(sample_rate))
                    as Box<dyn PathEffect>
            }),
        ],
        path_effects: Vec::new(),
        render_channels: 2,
    }
}

fn build_dbap(p: &PipelineParams) -> RenderPipeline {
    let sample_rate = p.sample_rate;
    let wall_reflectivity = p.er_wall_reflectivity;

    RenderPipeline {
        // DBAP: no source stages — air absorption and ground effect are per-path PathEffects.
        source_stages: SourceStageBank::new(vec![], sample_rate),
        renderer: Box::new(DbapRenderer::new(atrium_core::dbap::DbapParams {
            rolloff_db: p.dbap_rolloff_db,
            ..Default::default()
        })),
        mix_stages: vec![
            Box::new(LfeCrossoverStage::new()),
            Box::new(DelayCompStage::static_calibration()),
            Box::new(MasterGainStage),
        ],
        resolver: Box::new(ImageSourceResolver::new(wall_reflectivity)),
        path_effect_factories: vec![
            Box::new(|sample_rate| {
                Box::new(path_effects::PropagationDelayEffect::new(sample_rate))
                    as Box<dyn PathEffect>
            }),
            Box::new(|sample_rate| {
                Box::new(path_effects::AirAbsorptionEffect::new(sample_rate)) as Box<dyn PathEffect>
            }),
            Box::new(|sample_rate| {
                Box::new(path_effects::GroundEffectFilter::new(sample_rate)) as Box<dyn PathEffect>
            }),
            Box::new(|sample_rate| {
                Box::new(path_effects::WallAbsorptionEffect::new(sample_rate))
                    as Box<dyn PathEffect>
            }),
        ],
        path_effects: Vec::new(),
        render_channels: 0,
    }
}

fn build_ambisonics(p: &PipelineParams) -> RenderPipeline {
    let sample_rate = p.sample_rate;
    let wall_reflectivity = p.er_wall_reflectivity;

    RenderPipeline {
        // Ambisonics: no source stages — air absorption and ground effect are per-path PathEffects.
        source_stages: SourceStageBank::new(vec![], sample_rate),
        renderer: Box::new(AmbisonicsRenderer::new()),
        mix_stages: vec![
            Box::new(AmbiMultiDelayStage::new(p.ambi_wet_gain)),
            Box::new(AmbisonicsDecodeStage::new()),
            Box::new(LfeCrossoverStage::new()),
            Box::new(DelayCompStage::listener_relative()),
            Box::new(FdnReverbStage::new(
                p.fdn_wet_gain,
                p.fdn_rt60_low,
                p.fdn_rt60_high,
            )),
            Box::new(MasterGainStage),
        ],
        resolver: Box::new(ImageSourceResolver::new(wall_reflectivity)),
        path_effect_factories: vec![
            Box::new(|sample_rate| {
                Box::new(path_effects::PropagationDelayEffect::new(sample_rate))
                    as Box<dyn PathEffect>
            }),
            Box::new(|sample_rate| {
                Box::new(path_effects::AirAbsorptionEffect::new(sample_rate)) as Box<dyn PathEffect>
            }),
            Box::new(|sample_rate| {
                Box::new(path_effects::GroundEffectFilter::new(sample_rate)) as Box<dyn PathEffect>
            }),
            Box::new(|sample_rate| {
                Box::new(path_effects::WallAbsorptionEffect::new(sample_rate))
                    as Box<dyn PathEffect>
            }),
        ],
        path_effects: Vec::new(),
        render_channels: 0,
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// Pipeline render dispatch
// ─────────────────────────────────────────────────────────────────────────────

/// Scene-level parameters for rendering a single buffer.
pub struct RenderParams<'a> {
    pub listener: &'a atrium_core::listener::Listener,
    pub channels: usize,
    pub sample_rate: f32,
    pub master_gain: f32,
    pub distance_model: &'a DistanceModel,
    pub layout: &'a SpeakerLayout,
    pub atmosphere: &'a AtmosphericParams,
    pub ground: &'a GroundProperties,
    pub room_min: atrium_core::types::Vec3,
    pub room_max: atrium_core::types::Vec3,
    pub barriers: &'a [crate::audio::propagation::Barrier],
    pub wall_materials: &'a [path::WallMaterial; 6],
}

/// Render one buffer through the active pipeline.
///
/// This replaces the monolithic `mix_sources()` + `HrtfMixer::mix()` path.
pub fn render_pipeline(
    pipeline: &mut RenderPipeline,
    sources: &mut [Box<dyn atrium_core::source::SoundSource>],
    params: &RenderParams,
    output: &mut [f32],
) {
    use self::path::{PathSet, ResolveContext};
    use crate::profile_span;

    let num_frames = output.len() / params.channels;

    // Split borrow: source_stages, renderer, resolver, path_effects are independent fields
    let RenderPipeline {
        source_stages,
        renderer,
        mix_stages,
        resolver,
        path_effect_factories,
        path_effects,
        render_channels,
    } = pipeline;

    // Ensure topology
    source_stages.ensure_sources(sources.len());
    renderer.ensure_topology(sources.len(), params.layout, params.sample_rate);

    // Grow per-source path effect chains if needed.
    while path_effects.len() < sources.len() {
        let chains: [PathEffectChain; MAX_PATHS] = std::array::from_fn(|_| {
            let effects: Vec<Box<dyn PathEffect>> = path_effect_factories
                .iter()
                .map(|f| f(params.sample_rate))
                .collect();
            PathEffectChain::new(effects)
        });
        path_effects.push(chains);
    }

    // Zero output
    output.fill(0.0);

    // Per-source pipeline
    for (i, source) in sources.iter_mut().enumerate() {
        if !source.is_active() {
            continue;
        }

        let pos = source.position();
        let dist_to_listener = params.listener.position.distance_to(pos);

        let ctx = SourceContext {
            listener: params.listener,
            source_pos: pos,
            source_orientation: source.orientation(),
            source_directivity: &source.directivity(),
            source_spread: source.spread(),
            source_ref_distance: source.ref_distance(),
            dist_to_listener,
            atmosphere: params.atmosphere,
            room_min: params.room_min,
            room_max: params.room_max,
            ground: params.ground,
            sample_rate: params.sample_rate,
            distance_model: params.distance_model,
            layout: params.layout,
        };

        // Resolve propagation paths for this source
        let mut paths = PathSet::new();
        {
            let resolve_ctx = ResolveContext {
                source_pos: pos,
                target_pos: params.listener.position,
                room_min: params.room_min,
                room_max: params.room_max,
                barriers: params.barriers,
            };
            resolver.resolve(&resolve_ctx, &mut paths);
        }

        // Buffer-rate source stages
        let mut src_out = SourceOutput::default_for(params.layout.total_channels());
        {
            let _s = profile_span!("source_stages", src = i).entered();
            source_stages.process_all(i, &ctx, &mut src_out);
        }

        // Collect source stage refs for the inner loop
        let mut stage_refs = source_stages.for_source(i);

        // Update per-path effect chains at buffer rate
        if let Some(chains) = path_effects.get_mut(i) {
            for (pi, path) in paths.as_slice().iter().enumerate() {
                let effect_ctx = path::PathEffectContext {
                    path,
                    atmosphere: params.atmosphere,
                    ground: params.ground,
                    sample_rate: params.sample_rate,
                    source_pos: pos,
                    target_pos: params.listener.position,
                    wall_materials: params.wall_materials,
                };
                chains[pi].update(&effect_ctx);
            }
        }

        // Renderer: mode-specific spatialization
        {
            let _s = profile_span!("renderer", src = i).entered();
            let mut out = renderer::OutputBuffer {
                buffer: output,
                channels: params.channels,
                num_frames,
                sample_rate: params.sample_rate,
            };
            let effect_chains = if let Some(chains) = path_effects.get_mut(i) {
                &mut chains[..]
            } else {
                &mut []
            };
            renderer.render_source(
                i,
                source.as_mut(),
                &mut stage_refs,
                &ctx,
                &src_out,
                &paths,
                effect_chains,
                &mut out,
            );
        }
    }

    // Post-mix chain
    let effective_render_channels = if *render_channels > 0 {
        *render_channels
    } else {
        params.layout.total_channels()
    };
    let mix_ctx = MixContext {
        listener: params.listener,
        layout: params.layout,
        sample_rate: params.sample_rate,
        channels: params.channels,
        room_min: params.room_min,
        room_max: params.room_max,
        master_gain: params.master_gain,
        render_channels: effective_render_channels,
    };
    {
        let _s = profile_span!("mix_stages").entered();
        for stage in mix_stages.iter_mut() {
            stage.process(output, &mix_ctx);
        }
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
    use crate::pipeline::stages::ground_effect::GroundEffectStage;

    fn default_wall_materials() -> [path::WallMaterial; 6] {
        std::array::from_fn(|_| path::WallMaterial::default())
    }

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

        let room_min = Vec3::new(0.0, 0.0, 0.0);
        let room_max = Vec3::new(6.0, 4.0, 3.0);

        // Listener at center of room
        let listener_a = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
        let rp_a = RenderParams {
            listener: &listener_a,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            room_min,
            room_max,
            barriers: &[],
            wall_materials: &default_wall_materials(),
        };
        render_pipeline(&mut pipeline, &mut sources, &rp_a, &mut buf_a);

        // Reset for second pass (clear gain ramp state)
        pipeline.reset();

        // Listener at a completely different position
        let listener_b = Listener::new(Vec3::new(5.0, 0.5, 0.0), 0.0);
        let rp_b = RenderParams {
            listener: &listener_b,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            room_min,
            room_max,
            barriers: &[],
            wall_materials: &default_wall_materials(),
        };
        render_pipeline(&mut pipeline, &mut sources, &rp_b, &mut buf_b);

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
        let rp = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            room_min: Vec3::ZERO,
            room_max: Vec3::new(6.0, 4.0, 3.0),
            barriers: &[],
            wall_materials: &default_wall_materials(),
        };
        render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

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
        let rp = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            room_min: Vec3::ZERO,
            room_max: Vec3::new(6.0, 4.0, 3.0),
            barriers: &[],
            wall_materials: &default_wall_materials(),
        };
        render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

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
        let rp = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            room_min: Vec3::ZERO,
            room_max: Vec3::new(6.0, 4.0, 3.0),
            barriers: &[],
            wall_materials: &default_wall_materials(),
        };
        render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

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
                Box::new(|_sr| Box::new(GroundEffectStage) as Box<dyn SourceStage>),
                Box::new(|_sr| Box::new(GroundEffectStage) as Box<dyn SourceStage>),
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
        assert_eq!(pipelines.len(), 5);
        assert_eq!(
            pipelines[RenderMode::WorldLocked.index()].renderer.name(),
            "world_locked"
        );
        assert_eq!(
            pipelines[RenderMode::Vbap.index()].renderer.name(),
            "multichannel"
        );
        assert_eq!(pipelines[RenderMode::Hrtf.index()].renderer.name(), "hrtf");
        assert_eq!(pipelines[RenderMode::Dbap.index()].renderer.name(), "dbap");
        assert_eq!(
            pipelines[RenderMode::Ambisonics.index()].renderer.name(),
            "ambisonics"
        );
    }

    // ── Ambisonics: all speakers get signal ─────────────────────────────

    #[test]
    fn ambisonics_all_speakers_active() {
        let layout = layout_5_1();
        let dm = default_distance_model();
        let atm = default_atmosphere();
        let ground = default_ground();

        let source_pos = Vec3::new(1.0, 3.0, 0.0);
        let params = PipelineParams::default();
        let mut pipeline = build_ambisonics(&params);
        pipeline.ensure_topology(1, &layout, 48000.0);

        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: source_pos,
            val: 1.0,
        })];

        let channels = 6;
        let frames = 2048;
        let mut buffer = vec![0.0f32; frames * channels];

        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
        let rp = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            room_min: Vec3::new(-20.0, -20.0, -5.0),
            room_max: Vec3::new(20.0, 20.0, 5.0),
            barriers: &[],
            wall_materials: &default_wall_materials(),
        };
        render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

        // All spatial speakers (not LFE ch3) should have signal
        let quarter = frames * 3 / 4;
        let has_signal = |ch: usize| -> bool {
            (quarter..frames).any(|f| buffer[f * channels + ch].abs() > 1e-10)
        };
        assert!(has_signal(0), "FL should have signal");
        assert!(has_signal(1), "FR should have signal");
        assert!(has_signal(2), "C should have signal");
        assert!(has_signal(4), "RL should have signal");
        assert!(has_signal(5), "RR should have signal");
    }

    #[test]
    fn ambisonics_differs_from_vbap() {
        let layout = layout_5_1();
        let dm = default_distance_model();
        let atm = default_atmosphere();
        let ground = default_ground();

        let source_pos = Vec3::new(1.0, 3.0, 0.0);
        let params = PipelineParams::default();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);

        let channels = 6;
        let frames = 2048;

        let render = |pipeline: &mut RenderPipeline| -> Vec<f32> {
            pipeline.ensure_topology(1, &layout, 48000.0);
            let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
                pos: source_pos,
                val: 1.0,
            })];
            let mut buffer = vec![0.0f32; frames * channels];
            let rp = RenderParams {
                listener: &listener,
                channels,
                sample_rate: 48000.0,
                master_gain: 1.0,
                distance_model: &dm,
                layout: &layout,
                atmosphere: &atm,
                ground: &ground,
                room_min: Vec3::new(-20.0, -20.0, -5.0),
                room_max: Vec3::new(20.0, 20.0, 5.0),
                barriers: &[],
                wall_materials: &default_wall_materials(),
            };
            render_pipeline(pipeline, &mut sources, &rp, &mut buffer);
            buffer
        };

        let vbap_buf = render(&mut build_vbap(&params));
        let ambi_buf = render(&mut build_ambisonics(&params));

        let quarter = frames * 3 / 4;
        let rms = |buf: &[f32], ch: usize| -> f32 {
            let sum: f32 = (quarter..frames)
                .map(|f| buf[f * channels + ch].powi(2))
                .sum();
            (sum / (frames - quarter) as f32).sqrt()
        };

        let mut total_diff = 0.0f32;
        for ch in [0, 1, 2, 4, 5] {
            let v = rms(&vbap_buf, ch);
            let a = rms(&ambi_buf, ch);
            total_diff += (v - a).abs();
        }
        assert!(
            total_diff > 0.01,
            "Ambisonics and VBAP should produce different output (total_diff={total_diff})"
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
        let rp = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            room_min: Vec3::new(-20.0, -20.0, -5.0),
            room_max: Vec3::new(20.0, 20.0, 5.0),
            barriers: &[],
            wall_materials: &default_wall_materials(),
        };
        render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

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

    // ── DBAP: nearest speaker gets loudest output ───────────────────────

    #[test]
    fn dbap_nearest_speaker_loudest() {
        let layout = layout_5_1();
        let dm = default_distance_model();
        let atm = default_atmosphere();
        let ground = default_ground();

        // Source near front-left speaker (0,4)
        let source_pos = Vec3::new(0.5, 3.5, 0.0);
        let params = PipelineParams::default();
        let mut pipeline = build_dbap(&params);
        pipeline.ensure_topology(1, &layout, 48000.0);

        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: source_pos,
            val: 1.0,
        })];

        let channels = 6;
        let frames = 2048;
        let mut buffer = vec![0.0f32; frames * channels];

        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
        let rp = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            room_min: Vec3::new(-20.0, -20.0, -5.0),
            room_max: Vec3::new(20.0, 20.0, 5.0),
            barriers: &[],
            wall_materials: &default_wall_materials(),
        };
        render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

        // RMS per channel (last quarter, filters settled)
        let quarter = frames * 3 / 4;
        let rms = |ch: usize| -> f32 {
            let sum: f32 = (quarter..frames)
                .map(|f| buffer[f * channels + ch].powi(2))
                .sum();
            (sum / (frames - quarter) as f32).sqrt()
        };

        let fl_rms = rms(0);
        let fr_rms = rms(1);
        let c_rms = rms(2);
        let rl_rms = rms(4);
        let rr_rms = rms(5);
        eprintln!(
            "DBAP RMS: FL={fl_rms:.6} FR={fr_rms:.6} C={c_rms:.6} RL={rl_rms:.6} RR={rr_rms:.6}"
        );

        // FL is nearest speaker — it should be loudest
        assert!(
            fl_rms > fr_rms,
            "FL ({fl_rms}) should be louder than FR ({fr_rms})"
        );
        assert!(
            fl_rms > rr_rms,
            "FL ({fl_rms}) should be louder than RR ({rr_rms})"
        );

        // All speakers should have signal (DBAP sends to all)
        assert!(fr_rms > 1e-6, "FR should have signal");
        assert!(rl_rms > 1e-6, "RL should have signal");
        assert!(rr_rms > 1e-6, "RR should have signal");
    }

    #[test]
    fn dbap_differs_from_world_locked() {
        let layout = layout_5_1();
        let dm = default_distance_model();
        let atm = default_atmosphere();
        let ground = default_ground();

        let source_pos = Vec3::new(1.0, 3.0, 0.0);
        let params = PipelineParams::default();
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);

        let channels = 6;
        let frames = 2048;

        let render = |pipeline: &mut RenderPipeline| -> Vec<f32> {
            pipeline.ensure_topology(1, &layout, 48000.0);
            let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
                pos: source_pos,
                val: 1.0,
            })];
            let mut buffer = vec![0.0f32; frames * channels];
            let rp = RenderParams {
                listener: &listener,
                channels,
                sample_rate: 48000.0,
                master_gain: 1.0,
                distance_model: &dm,
                layout: &layout,
                atmosphere: &atm,
                ground: &ground,
                room_min: Vec3::new(-20.0, -20.0, -5.0),
                room_max: Vec3::new(20.0, 20.0, 5.0),
                barriers: &[],
                wall_materials: &default_wall_materials(),
            };
            render_pipeline(pipeline, &mut sources, &rp, &mut buffer);
            buffer
        };

        let wl_buf = render(&mut build_world_locked(&params));
        let dbap_buf = render(&mut build_dbap(&params));

        // Compare RMS per channel (last quarter)
        let quarter = frames * 3 / 4;
        let rms = |buf: &[f32], ch: usize| -> f32 {
            let sum: f32 = (quarter..frames)
                .map(|f| buf[f * channels + ch].powi(2))
                .sum();
            (sum / (frames - quarter) as f32).sqrt()
        };

        // They must differ on at least some channels
        let mut total_diff = 0.0f32;
        for ch in [0, 1, 2, 4, 5] {
            let wl = rms(&wl_buf, ch);
            let db = rms(&dbap_buf, ch);
            eprintln!(
                "ch{ch}: WorldLocked={wl:.6} DBAP={db:.6} diff={:.6}",
                (wl - db).abs()
            );
            total_diff += (wl - db).abs();
        }
        assert!(
            total_diff > 0.01,
            "DBAP and WorldLocked should produce different output (total_diff={total_diff})"
        );
    }

    // ── HRTF: only stereo channels have signal on 5.1 layout ─────────

    #[test]
    fn hrtf_no_bleed_to_surround_channels() {
        let layout = layout_5_1();
        let dm = default_distance_model();
        let atm = default_atmosphere();
        let ground = default_ground();

        let source_pos = Vec3::new(1.0, 3.0, 0.0);
        let params = PipelineParams::default();
        let mut pipeline = build_hrtf(&params);
        pipeline.ensure_topology(1, &layout, 48000.0);

        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: source_pos,
            val: 1.0,
        })];

        let channels = 6;
        let frames = 2048;
        let mut buffer = vec![0.0f32; frames * channels];

        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
        let rp = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            room_min: Vec3::new(-20.0, -20.0, -5.0),
            room_max: Vec3::new(20.0, 20.0, 5.0),
            barriers: &[],
            wall_materials: &default_wall_materials(),
        };
        render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

        // Channels 2-5 (C, LFE, RL, RR) must be silent — HRTF only writes to L/R
        for ch in 2..channels {
            let max_abs = (0..frames)
                .map(|f| buffer[f * channels + ch].abs())
                .fold(0.0f32, f32::max);
            assert_eq!(
                max_abs, 0.0,
                "HRTF should not bleed to channel {ch}, but got max abs {max_abs}"
            );
        }

        // Stereo channels should have signal
        let has_signal =
            |ch: usize| -> bool { (0..frames).any(|f| buffer[f * channels + ch].abs() > 1e-10) };
        assert!(has_signal(0), "L should have signal in HRTF mode");
        assert!(has_signal(1), "R should have signal in HRTF mode");
    }

    // ── VBAP: FDN reverb stays within speaker channels ───────────────

    #[test]
    fn vbap_fdn_no_bleed_beyond_channels() {
        // Stereo layout on a 2-channel device — FDN should not bleed beyond ch 0-1
        let layout = SpeakerLayout::stereo(Vec3::new(-1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0));
        let dm = default_distance_model();
        let atm = default_atmosphere();
        let ground = default_ground();

        let source_pos = Vec3::new(0.0, 2.0, 0.0);
        let params = PipelineParams::default();
        let mut pipeline = build_vbap(&params);
        pipeline.ensure_topology(1, &layout, 48000.0);

        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: source_pos,
            val: 1.0,
        })];

        let channels = 2;
        let frames = 2048;
        let mut buffer = vec![0.0f32; frames * channels];

        let listener = Listener::new(Vec3::ZERO, 0.0);
        let rp = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            room_min: Vec3::new(-5.0, -5.0, -5.0),
            room_max: Vec3::new(5.0, 5.0, 5.0),
            barriers: &[],
            wall_materials: &default_wall_materials(),
        };
        render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

        // Both stereo channels should have signal (renderer + FDN wet)
        let has_signal =
            |ch: usize| -> bool { (0..frames).any(|f| buffer[f * channels + ch].abs() > 1e-10) };
        assert!(has_signal(0), "L should have signal");
        assert!(has_signal(1), "R should have signal");
    }

    // ── Mode × Layout channel output matrix (12 combinations) ─────────

    /// Run a single render mode through the full pipeline on a 5.1 layout
    /// with the given channel mode (active channels), using a 6-channel buffer.
    ///
    /// `active_channels`: which channels should have signal (e.g. [0,1] for stereo).
    /// `silent_channels`: which channels must be silent (e.g. [2,3,4,5] for stereo).
    fn assert_mode_channel_output(
        mode: RenderMode,
        active_channels: &[usize],
        silent_channels: &[usize],
        label: &str,
    ) {
        let dm = default_distance_model();
        let atm = default_atmosphere();
        let ground = default_ground();
        let params = PipelineParams::default();

        let mut pipeline = match mode {
            RenderMode::WorldLocked => build_world_locked(&params),
            RenderMode::Vbap => build_vbap(&params),
            RenderMode::Hrtf => build_hrtf(&params),
            RenderMode::Dbap => build_dbap(&params),
            RenderMode::Ambisonics => build_ambisonics(&params),
        };

        // Always 5.1 layout (6 hardware channels) with active mask applied
        let mut layout = layout_5_1();
        layout.set_active_channels(active_channels);
        let hardware_channels = 6;
        pipeline.ensure_topology(1, &layout, 48000.0);

        let source_pos = Vec3::new(1.0, 3.0, 0.0);
        let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
        let frames = 2048;

        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: source_pos,
            val: 1.0,
        })];
        let mut buffer = vec![0.0f32; frames * hardware_channels];

        let rp = RenderParams {
            listener: &listener,
            channels: hardware_channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            atmosphere: &atm,
            ground: &ground,
            room_min: Vec3::new(-20.0, -20.0, -5.0),
            room_max: Vec3::new(20.0, 20.0, 5.0),
            barriers: &[],
            wall_materials: &default_wall_materials(),
        };
        render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

        // Silent channels must have no signal
        for &ch in silent_channels {
            let max_abs = (0..frames)
                .map(|f| buffer[f * hardware_channels + ch].abs())
                .fold(0.0f32, f32::max);
            assert_eq!(
                max_abs, 0.0,
                "{label}: channel {ch} should be silent, got max {max_abs}"
            );
        }

        // At least one active channel should have signal
        let has_any_signal = active_channels
            .iter()
            .any(|&ch| (0..frames).any(|f| buffer[f * hardware_channels + ch].abs() > 1e-10));
        assert!(
            has_any_signal,
            "{label}: should produce signal in at least one active channel"
        );
    }

    // Channel modes on 5.1 hardware:
    //   Stereo = [0, 1] active, [2, 3, 4, 5] silent
    //   Quad   = [0, 1, 4, 5] active, [2, 3] silent
    //   5.1    = [0, 1, 2, 4, 5] active, [3] silent (LFE gets crossover only)

    const STEREO_ACTIVE: &[usize] = &[0, 1];
    const STEREO_SILENT: &[usize] = &[2, 3, 4, 5];
    const QUAD_ACTIVE: &[usize] = &[0, 1, 4, 5];
    const QUAD_SILENT: &[usize] = &[2, 3];
    const SURROUND_ACTIVE: &[usize] = &[0, 1, 2, 3, 4, 5];
    const SURROUND_SILENT: &[usize] = &[];

    // ── WorldLocked: Stereo, Quad, 5.1 ──────────────────────────────

    #[test]
    fn world_locked_stereo_output() {
        assert_mode_channel_output(
            RenderMode::WorldLocked,
            STEREO_ACTIVE,
            STEREO_SILENT,
            "WorldLocked×Stereo",
        );
    }

    #[test]
    fn world_locked_quad_output() {
        assert_mode_channel_output(
            RenderMode::WorldLocked,
            QUAD_ACTIVE,
            QUAD_SILENT,
            "WorldLocked×Quad",
        );
    }

    #[test]
    fn world_locked_5_1_output() {
        assert_mode_channel_output(
            RenderMode::WorldLocked,
            SURROUND_ACTIVE,
            SURROUND_SILENT,
            "WorldLocked×5.1",
        );
    }

    // ── VBAP: Quad, 5.1 (no stereo — needs ≥3 speakers) ────────────

    #[test]
    fn vbap_quad_output() {
        assert_mode_channel_output(RenderMode::Vbap, QUAD_ACTIVE, QUAD_SILENT, "Vbap×Quad");
    }

    #[test]
    fn vbap_5_1_output() {
        assert_mode_channel_output(
            RenderMode::Vbap,
            SURROUND_ACTIVE,
            SURROUND_SILENT,
            "Vbap×5.1",
        );
    }

    // ── HRTF: always stereo (channels 0-1) ──────────────────────────

    #[test]
    fn hrtf_stereo_output() {
        assert_mode_channel_output(
            RenderMode::Hrtf,
            STEREO_ACTIVE,
            STEREO_SILENT,
            "Hrtf×Stereo",
        );
    }

    // ── DBAP: Stereo, Quad, 5.1 ─────────────────────────────────────

    #[test]
    fn dbap_stereo_output() {
        assert_mode_channel_output(
            RenderMode::Dbap,
            STEREO_ACTIVE,
            STEREO_SILENT,
            "Dbap×Stereo",
        );
    }

    #[test]
    fn dbap_quad_output() {
        assert_mode_channel_output(RenderMode::Dbap, QUAD_ACTIVE, QUAD_SILENT, "Dbap×Quad");
    }

    #[test]
    fn dbap_5_1_output() {
        assert_mode_channel_output(
            RenderMode::Dbap,
            SURROUND_ACTIVE,
            SURROUND_SILENT,
            "Dbap×5.1",
        );
    }

    // ── Ambisonics: Stereo, Quad, 5.1 ───────────────────────────────

    #[test]
    fn ambisonics_stereo_output() {
        assert_mode_channel_output(
            RenderMode::Ambisonics,
            STEREO_ACTIVE,
            STEREO_SILENT,
            "Ambisonics×Stereo",
        );
    }

    #[test]
    fn ambisonics_quad_output() {
        assert_mode_channel_output(
            RenderMode::Ambisonics,
            QUAD_ACTIVE,
            QUAD_SILENT,
            "Ambisonics×Quad",
        );
    }

    #[test]
    fn ambisonics_5_1_output() {
        assert_mode_channel_output(
            RenderMode::Ambisonics,
            SURROUND_ACTIVE,
            SURROUND_SILENT,
            "Ambisonics×5.1",
        );
    }
}
