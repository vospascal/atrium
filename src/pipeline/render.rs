//! One pipeline per render mode, and the per-buffer render call that drives it.
//!
//! `RenderPipeline` owns the chain (resolver, per-path effects, renderer, mix
//! stages); `render_pipeline` runs one buffer through it. The per-mode
//! definitions that fill a `RenderPipeline` live in `builders`.

use atrium_core::speaker::SpeakerLayout;

use super::environment::Environment;
use super::mix_stage::{MixContext, MixStage};
use super::path::{
    PathEffect, PathEffectChain, PathEffectContext, PathEffectFactory, PathResolver, MAX_PATHS,
};
use super::renderer::{OutputBuffer, Renderer, SourceContext};
use super::renderers::field::{FieldEncoding, FieldRenderer};
use super::room_acoustics;
use crate::audio::distance::DistanceModel;

// ─────────────────────────────────────────────────────────────────────────────
// RenderPipeline — one per mode
// ─────────────────────────────────────────────────────────────────────────────

/// Largest callback, in frames, that renders without growing a buffer. Output
/// devices ask for 64 to 4096 frames; a larger request still renders
/// correctly but allocates once, in that callback.
const LARGEST_EXPECTED_CALLBACK_FRAMES: usize = 8192;

/// A complete render pipeline for one mode.
pub struct RenderPipeline {
    pub renderer: Box<dyn Renderer>,
    /// Dedicated path for environmental fields. Fields do not enter the
    /// point-source resolver/renderer at all.
    field_renderer: FieldRenderer,
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
    /// Pre-allocated late-reverb send buffer (same interleaved layout as output).
    /// For generic FDN modes (HRTF, VBAP, DBAP), channel 0 carries a mono send bus.
    /// For Ambisonics, the renderer writes a full FOA B-format layout.
    /// Read by the FDN stage as its delay line injection input.
    pub reverb_send_buffer: Vec<f32>,
    /// Explicit decay time of this pipeline's late reverb, if the environment
    /// sets one. The reverb send uses the same decay as the reverb itself.
    pub reverb_rt60_override: Option<f32>,
}

impl RenderPipeline {
    /// A pipeline with no sources yet: per-source state grows in
    /// `ensure_topology` once the source count and sample rate are known.
    pub(super) fn new(
        renderer: Box<dyn Renderer>,
        resolver: Box<dyn PathResolver>,
        path_effect_factories: Vec<PathEffectFactory>,
        mix_stages: Vec<Box<dyn MixStage>>,
        render_channels: usize,
        reverb_rt60_override: Option<f32>,
    ) -> Self {
        Self {
            renderer,
            field_renderer: FieldRenderer::default(),
            mix_stages,
            resolver,
            path_effect_factories,
            path_effects: Vec::new(),
            render_channels,
            reverb_send_buffer: Vec::new(),
            reverb_rt60_override,
        }
    }

    /// Initialize all mix stages (called when audio params are known).
    pub fn init(&mut self, ctx: &MixContext) {
        for stage in &mut self.mix_stages {
            stage.init(ctx);
        }
        // The reverb send is resized to the callback's buffer on every render.
        // Reserve it here so that resize never reaches the allocator on the
        // audio thread.
        self.reverb_send_buffer
            .reserve(LARGEST_EXPECTED_CALLBACK_FRAMES * ctx.channels);
    }

    /// Reset all state (called on mode switch).
    pub fn reset(&mut self) {
        self.renderer.reset();
        self.field_renderer.reset();
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
        self.renderer
            .ensure_topology(source_count, layout, sample_rate);
        self.field_renderer
            .ensure_topology(source_count, sample_rate);

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
    pub environment: &'a Environment,
    /// Bypass soft clipping and gain clamping for acoustic measurement.
    pub measurement_mode: bool,
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
    use super::path::{PathSet, ResolveContext};
    use crate::profile_span;

    let num_frames = output.len() / params.channels;

    // Split borrow: renderer, resolver, path_effects are independent fields
    let RenderPipeline {
        renderer,
        field_renderer,
        mix_stages,
        resolver,
        path_effect_factories,
        path_effects,
        render_channels,
        reverb_send_buffer,
        reverb_rt60_override,
    } = pipeline;

    // Ensure topology
    renderer.ensure_topology(sources.len(), params.layout, params.sample_rate);
    field_renderer.ensure_topology(sources.len(), params.sample_rate);

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

    // Room geometry + RT60 for per-source reverb send (critical distance).
    let (volume, _surface_area) =
        room_acoustics::room_geometry(params.environment.min, params.environment.max);
    let rt60 = room_acoustics::reverb_decay_seconds(params.environment, *reverb_rt60_override);

    // Prepare reverb send buffer (same size as output, zeroed).
    reverb_send_buffer.resize(output.len(), 0.0);
    reverb_send_buffer.fill(0.0);

    // Per-source pipeline
    for (i, source) in sources.iter_mut().enumerate() {
        if !source.is_active() {
            continue;
        }

        // Environmental fields are already the surrounding acoustic energy.
        // Render them directly as decorrelated channels: no point position,
        // distance law, image-source paths, propagation filters, or room send.
        if source.emitter_kind() == atrium_core::source::EmitterKind::Field {
            let encoding: FieldEncoding = renderer.field_encoding(params.layout, params.channels);
            // A field may only use the channels this pipeline renders to: the
            // binaural pair in HRTF mode, even when the layout is 5.1.
            let field_channels = if *render_channels > 0 {
                (*render_channels).min(params.channels)
            } else {
                params.channels
            };
            field_renderer.render_source(
                i,
                source.as_mut(),
                encoding,
                params.layout,
                params.sample_rate,
                params.channels,
                field_channels,
                num_frames,
                output,
            );
            continue;
        }

        let pos = source.position();
        // Where this pipeline measures propagation to. Everything below that
        // depends on a receiver position (paths, delays, air absorption, the
        // reverb send) uses this one point, never the listener directly.
        let path_target = renderer.path_target(params.listener, params.layout);
        let dist_to_target = path_target.distance_to(pos);

        // Per-source reverb send: critical distance scales by √γ (directivity factor).
        // Diffuse sources (high MDAP spread — wind, rain, waves) are the ambient
        // field itself, not a point that echoes off walls, so their send is
        // scaled down; a localized bird keeps its full reverb.
        let gamma = atrium_core::directivity::directivity_factor(&source.directivity());
        let critical_dist = room_acoustics::critical_distance(volume, rt60, gamma);
        let reverb_send = room_acoustics::reverb_send(dist_to_target, critical_dist)
            * room_acoustics::diffuse_reverb_scale(source.spread());

        let ctx = SourceContext {
            listener: params.listener,
            source_pos: pos,
            source_orientation: source.orientation(),
            source_directivity: &source.directivity(),
            source_spread: source.spread(),
            source_ref_distance: source.ref_distance(),
            path_target,
            environment: params.environment,
            sample_rate: params.sample_rate,
            distance_model: params.distance_model,
            layout: params.layout,
            reverb_send,
        };

        // Resolve propagation paths for this source
        let mut paths = PathSet::new();
        {
            let resolve_ctx = ResolveContext {
                source_pos: pos,
                target_pos: path_target,
                environment: params.environment,
            };
            resolver.resolve(&resolve_ctx, &mut paths);
        }

        // Update per-path effect chains at buffer rate
        if let Some(chains) = path_effects.get_mut(i) {
            for (pi, path) in paths.as_slice().iter().enumerate() {
                let effect_ctx = PathEffectContext {
                    path,
                    environment: params.environment,
                    sample_rate: params.sample_rate,
                    source_pos: pos,
                    target_pos: path_target,
                };
                chains[pi].update(&effect_ctx);
            }
        }

        // Renderer: mode-specific spatialization
        {
            let _s = profile_span!("renderer", src = i).entered();
            let mut out = OutputBuffer {
                buffer: output,
                reverb_send: Some(reverb_send_buffer),
                channels: params.channels,
                num_frames,
                sample_rate: params.sample_rate,
            };
            let effect_chains = if let Some(chains) = path_effects.get_mut(i) {
                &mut chains[..]
            } else {
                &mut []
            };
            renderer.render_source(i, source.as_mut(), &ctx, &paths, effect_chains, &mut out);
        }
    }

    // Post-mix chain
    let effective_render_channels = if *render_channels > 0 {
        (*render_channels).min(params.channels)
    } else {
        params.layout.total_channels().min(params.channels)
    };
    let mix_ctx = MixContext {
        listener: params.listener,
        layout: params.layout,
        sample_rate: params.sample_rate,
        channels: params.channels,
        environment: params.environment,
        master_gain: params.master_gain,
        render_channels: effective_render_channels,
        reverb_input: Some(reverb_send_buffer),
        measurement_mode: params.measurement_mode,
    };
    {
        let _s = profile_span!("mix_stages").entered();
        for stage in mix_stages.iter_mut() {
            stage.process(output, &mix_ctx);
        }
    }
}
