//! DDA compute pass: primary rays plus one sun shadow ray per hit, traced
//! through the two-level brickmap, one thread per pixel, writing shaded colors
//! into the frame's storage texture. Owns only what is specific to shading a
//! pixel — the camera uniform and the output texture; the brickmap/lighting
//! resources belong to [`super::world_bindings::WorldBindings`] (shared with the
//! E4 CAGI pass) and the light volume to [`super::cagi::LightVolume`].
//!
//! Bind group 0 = the shared world entries, the shared light-volume entries (minus the
//! writable back buffer, which belongs to the CA pass alone), plus this pass's own
//! [`WorldBinding::Camera`] and [`WorldBinding::Output`] — and
//! [`WorldBinding::PatternCache`], which is this pass's alone and deliberately outside the
//! shared layout. See [`super::binding`]; indices live there and nowhere else.

use crate::camera::CameraUniform;
use crate::variants::RenderQuality;
use voxel_material_graph::lowering::MaterialGraphShaderSet;

use super::binding::WorldBinding;
use super::cagi::LightVolume;
use super::composer::{Composition, FragmentEdit, ShaderProgram};
use super::world_bindings::{WorldBindings, PATTERN_CACHE_BINDING};
use super::ComputePipelineCache;
use crate::shader_consts::ShaderDefs;
use voxel_environment::{EnvironmentGpu, HillaireEnvironment, ENVIRONMENT_BIND_GROUP};

const WORKGROUP_SIZE: u32 = 8;

/// The shading pass's complete shader source, unpatched.
///
/// Which fragments it is made of now lives in [`super::composer`] — one table shared with the
/// CA pass, instead of the `concat!` block that used to sit here and a second one in
/// `passes::cagi`. Each pass had to name the other crates' WGSL to build that list; neither does
/// now.
///
/// Exposed so the headless benchmark (`examples/bench_dda.rs`) can build A/B pipeline variants by
/// patching the compile-time levers (see "A/B benchmark levers" in `shaders/world.wgsl`, the AO
/// block in `shaders/dda.wgsl` and the water levers in `shaders/water.wgsl`).
///
/// An empty def set means no lever is patched, so this is the raw concatenation — the shipped
/// default *is* the unpatched file, which is the property the `default_settings_match_shader_source`
/// tests in each lever module rely on.
pub static SHADER_SOURCE: std::sync::LazyLock<String> =
    std::sync::LazyLock::new(|| Composition::shading().joined_source(&ShaderDefs::default()));

/// [`SHADER_SOURCE`] with every experiment's compile-time levers patched in.
/// The app's preset path and the benchmark's variant builder both go through
/// this one function, so a new lever module cannot be forgotten at a call
/// site. Returns [`SHADER_SOURCE`] verbatim for the shipped (Balanced) quality.
///
/// The lever groups this applies are [`RenderQuality::declare_shading_consts`] — deliberately not
/// the same set as the CA pass's, since this pass shades and that one does not.
pub fn build_shader_source(quality: &RenderQuality) -> String {
    Composition::shading().joined_source(&quality.shading_shader_defs())
}

/// Build the shading source with the optional material-graph dispatch appended.
/// An empty set returns the ordinary source unchanged, preserving the shipped
/// renderer and all existing quality-preset cache keys.
pub fn build_shader_source_with_material_graphs(
    quality: &RenderQuality,
    graphs: &MaterialGraphShaderSet,
) -> String {
    Composition::shading()
        .joined_source_with_edits(&quality.shading_shader_defs(), &graph_edits(graphs))
}

/// The same, with the output storage-texture format patched in.
///
/// SEPARATE from the quality levers on purpose: output depth is a display property
/// rather than a quality tier (see [the `voxel-color` crate]), so it is not part of
/// [`RenderQuality`] and cannot ride its patch chain. Returns byte-identical source
/// for the shipped 8-bit path, so existing pipeline cache keys do not move.
pub fn build_shader_source_for_output(
    quality: &RenderQuality,
    graphs: &MaterialGraphShaderSet,
    output_format: voxel_color::OutputFormat,
) -> String {
    Composition::shading().joined_source_with_edits(
        &quality.shading_shader_defs(),
        &output_edits(graphs, output_format),
    )
}

/// The compiled shading program: joined source for the cache key, composed module for the device.
pub fn build_program_for_output(
    quality: &RenderQuality,
    graphs: &MaterialGraphShaderSet,
    output_format: voxel_color::OutputFormat,
) -> ShaderProgram {
    Composition::shading().build(
        &quality.shading_shader_defs(),
        &output_edits(graphs, output_format),
    )
}

/// The material-graph injection, aimed at the fragment that owns the marker.
///
/// `material_graph.wgsl` declares `GRAPH_DISPATCH_POINT` and the fallback function around it, and
/// the generated per-material functions are appended after it. They call the graph prelude's
/// helpers by bare name, which resolves because that fragment already imports
/// `vx::graph_prelude` — the generated import list covers every item the prelude declares, so
/// nothing has to predict which ones a graph will use.
fn graph_edits(graphs: &MaterialGraphShaderSet) -> Vec<FragmentEdit<'_>> {
    vec![FragmentEdit {
        file: "material_graph.wgsl",
        apply: Box::new(move |text| graphs.inject_into_dda(text)),
    }]
}

/// The graph injection plus the output-format patch, which belongs to `dda.wgsl` — the fragment
/// that declares the storage texture.
fn output_edits(
    graphs: &MaterialGraphShaderSet,
    output_format: voxel_color::OutputFormat,
) -> Vec<FragmentEdit<'_>> {
    let mut edits = graph_edits(graphs);
    edits.push(FragmentEdit {
        file: "dda.wgsl",
        apply: Box::new(move |text| output_format.patch_shader_source(text)),
    });
    edits
}

pub struct DdaPass {
    pipeline_cache: ComputePipelineCache,
    bind_group_layout: wgpu::BindGroupLayout,
    environment_bind_group_layout: wgpu::BindGroupLayout,
    environment_bind_group: wgpu::BindGroup,
    /// The format this pass's layout was built for, so
    /// [`DdaPass::set_shader`] can refuse a source that disagrees with it.
    output_format: voxel_color::OutputFormat,
    /// One bind group per ping-pong volume buffer: the CA pass leaves the newest
    /// values in whichever buffer it wrote last, so [`DdaPass::encode`] selects by
    /// [`LightVolume::front`] instead of rebuilding a bind group per frame.
    bind_groups: [wgpu::BindGroup; 2],
    camera_uniform_buffer: wgpu::Buffer,
}

impl DdaPass {
    pub fn new(
        device: &wgpu::Device,
        world_bindings: &WorldBindings,
        light_volume: &LightVolume,
        output_view: &wgpu::TextureView,
        output_format: voxel_color::OutputFormat,
    ) -> Self {
        let environment = HillaireEnvironment::new(device);
        Self::new_with_environment(
            device,
            world_bindings,
            light_volume,
            output_view,
            &environment,
            output_format,
        )
    }

    pub(crate) fn new_with_environment(
        device: &wgpu::Device,
        world_bindings: &WorldBindings,
        light_volume: &LightVolume,
        output_view: &wgpu::TextureView,
        environment: &dyn EnvironmentGpu,
        output_format: voxel_color::OutputFormat,
    ) -> Self {
        let program = build_program_for_output(
            &RenderQuality::default(),
            &MaterialGraphShaderSet::default(),
            output_format,
        );
        Self::new_with_environment_and_program(
            device,
            world_bindings,
            light_volume,
            output_view,
            environment,
            &program,
            output_format,
        )
    }

    /// Build the pass from an explicit program — the benchmark's entry point for A/B variants.
    /// Everything else (buffers, layout, bind groups) is identical to [`DdaPass::new`].
    /// Build the pass from an explicit program and a CALLER-OWNED environment.
    /// Public deliberately — see `CagiPass::new_with_environment_and_program`:
    /// a self-built environment is one whose LUTs nobody bakes, and the whole
    /// indirect term then shades BLACK.
    pub fn new_with_environment_and_program(
        device: &wgpu::Device,
        world_bindings: &WorldBindings,
        light_volume: &LightVolume,
        output_view: &wgpu::TextureView,
        environment: &dyn EnvironmentGpu,
        program: &ShaderProgram,
        output_format: voxel_color::OutputFormat,
    ) -> Self {
        // The source and the format must AGREE, and nothing about the signature
        // enforces it — so check here, where the message can name the problem,
        // rather than letting wgpu report it as a pipeline-layout mismatch four
        // layers down. This exact disagreement shipped twice.
        let wanted = output_format.wgsl_storage_declaration();
        assert!(
            program.source.contains(&wanted),
            "shading source does not declare `{wanted}` — it was not patched for \
             this OutputFormat, so the pipeline will not match its bind group layout"
        );
        let bind_group_layout = create_bind_group_layout(device, output_format.storage());
        let environment_bind_group_layout = environment.sample_bind_group_layout().clone();
        let pipeline_cache = ComputePipelineCache::new_with_layouts(
            device,
            "dda pass",
            "main",
            program,
            &[
                Some(&bind_group_layout),
                Some(&environment_bind_group_layout),
            ],
        );
        let camera_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dda camera uniform"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_groups = create_bind_groups(
            device,
            &bind_group_layout,
            world_bindings,
            light_volume,
            &camera_uniform_buffer,
            output_view,
        );

        Self {
            pipeline_cache,
            bind_group_layout,
            environment_bind_group_layout,
            environment_bind_group: environment.sample_bind_group().clone(),
            output_format,
            bind_groups,
            camera_uniform_buffer,
        }
    }

    /// Dispatch `shader_source` from now on, compiling it only on a cache miss
    /// (the overlay path: a compile-time lever or a preset changed). Buffers and
    /// bind groups are untouched — only the shader differs, so the existing bind
    /// groups stay valid against every cached pipeline.
    pub(crate) fn set_shader(
        &mut self,
        device: &wgpu::Device,
        source: &str,
        compose: impl FnOnce() -> naga::Module,
    ) {
        // The layout is fixed at construction; a source declaring a different
        // storage format cannot match it. Checked here because this is the path every
        // runtime rebuild takes, and the wgpu error it otherwise produces names a
        // "pipeline layout" mismatch rather than the unpatched source that caused it.
        let wanted = self.output_format.wgsl_storage_declaration();
        assert!(
            source.contains(&wanted),
            "shading source does not declare `{wanted}` — build it with \
             `build_shader_source_for_output` so it matches this pass's layout"
        );
        self.pipeline_cache.set_source_with_layouts(
            device,
            source,
            compose,
            &[
                Some(&self.bind_group_layout),
                Some(&self.environment_bind_group_layout),
            ],
        );
    }

    /// Precompile `shader_sources` (duplicates cost nothing) so that a later
    /// [`DdaPass::set_shader`] to any of them is a hash lookup instead of
    /// a shader compile — the reason a preset switch cannot stutter. Returns how
    /// many pipelines the cache holds afterwards.
    pub fn prewarm_pipelines(
        &mut self,
        device: &wgpu::Device,
        programs: &[ShaderProgram],
    ) -> usize {
        self.pipeline_cache.prewarm_with_layouts(
            device,
            programs,
            &[
                Some(&self.bind_group_layout),
                Some(&self.environment_bind_group_layout),
            ],
        )
    }

    /// Pipelines currently held (the memory the cache costs).
    pub fn cached_pipeline_count(&self) -> usize {
        self.pipeline_cache.len()
    }

    /// Refresh the bindings after the storage texture is recreated (a resize or a
    /// render-scale change) or the light volume was rebuilt (the CAGI resolution
    /// lever moved).
    pub(crate) fn rebind(
        &mut self,
        device: &wgpu::Device,
        world_bindings: &WorldBindings,
        light_volume: &LightVolume,
        output_view: &wgpu::TextureView,
    ) {
        self.bind_groups = create_bind_groups(
            device,
            &self.bind_group_layout,
            world_bindings,
            light_volume,
            &self.camera_uniform_buffer,
            output_view,
        );
    }

    /// Record the pass. `light_volume_front` selects the ping-pong buffer holding
    /// the CA's newest values ([`LightVolume::front`]); with CAGI off the shader
    /// never reads it.
    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera_uniform: &CameraUniform,
        light_volume_front: usize,
        output_width: u32,
        output_height: u32,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        queue.write_buffer(
            &self.camera_uniform_buffer,
            0,
            bytemuck::bytes_of(camera_uniform),
        );

        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("dda pass"),
            timestamp_writes,
        });
        compute_pass.set_pipeline(self.pipeline_cache.active());
        compute_pass.set_bind_group(0, &self.bind_groups[light_volume_front], &[]);
        compute_pass.set_bind_group(ENVIRONMENT_BIND_GROUP, &self.environment_bind_group, &[]);
        compute_pass.dispatch_workgroups(
            output_width.div_ceil(WORKGROUP_SIZE),
            output_height.div_ceil(WORKGROUP_SIZE),
            1,
        );
    }
}

/// Bind group layout only, separated from the pipeline so a shader-source
/// rebuild can reuse the ORIGINAL layout object — the existing bind groups must
/// stay valid against the new pipeline.
/// `storage_format` MUST match both the texture the pass writes and the
/// `texture_storage_2d<FORMAT, write>` type in `dda.wgsl`. wgpu wants a storage
/// texture's format in THREE places — the texture, this layout entry, and the WGSL
/// type — and validates all three against each other, so it is resolved once in
/// the `voxel-color` crate and threaded rather than restated.
fn create_bind_group_layout(
    device: &wgpu::Device,
    storage_format: wgpu::TextureFormat,
) -> wgpu::BindGroupLayout {
    let mut entries = WorldBindings::layout_entries();
    entries.extend(LightVolume::layout_entries(false));
    entries.push(WorldBindings::pattern_cache_layout_entry());
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: WorldBinding::Camera.index(),
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: WorldBinding::Output.index(),
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: storage_format,
            view_dimension: wgpu::TextureViewDimension::D2,
        },
        count: None,
    });
    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("dda bind group layout"),
        entries: &entries,
    })
}

fn create_bind_groups(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    world_bindings: &WorldBindings,
    light_volume: &LightVolume,
    camera_uniform_buffer: &wgpu::Buffer,
    output_view: &wgpu::TextureView,
) -> [wgpu::BindGroup; 2] {
    let bind_group = |volume_index: usize| {
        let mut entries = world_bindings.bind_group_entries();
        entries.extend(light_volume.bind_group_entries(volume_index, false));
        entries.push(wgpu::BindGroupEntry {
            binding: PATTERN_CACHE_BINDING,
            resource: world_bindings.pattern_cache_buffer().as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: WorldBinding::Camera.index(),
            resource: camera_uniform_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: WorldBinding::Output.index(),
            resource: wgpu::BindingResource::TextureView(output_view),
        });
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("dda bind group"),
            layout,
            entries: &entries,
        })
    };
    [bind_group(0), bind_group(1)]
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use crate::ao::{AoDirectionMode, AoMode, AoSettings};
    use crate::cagi::{CagiSampleMode, CagiSettings};
    use crate::passes::create_compute_pipeline_with_layouts;
    use crate::passes::world_bindings::PATTERN_CACHE_ENTRIES;
    use crate::traversal::TraversalSettings;
    use crate::variants::{QualityPreset, QUALITY_PRESETS};
    use crate::water::{WaterMode, WaterSettings};
    use std::collections::HashMap;

    #[test]
    fn default_settings_build_the_shipped_shader() {
        assert_eq!(
            build_shader_source(&RenderQuality::default()),
            SHADER_SOURCE.as_str()
        );
    }

    #[test]
    fn pattern_cache_buffer_size_matches_the_shader_mask() {
        let declaration = format!(
            "const PATTERN_CACHE_MASK: u32 = {}u - 1u;",
            PATTERN_CACHE_ENTRIES
        );
        assert!(
            SHADER_SOURCE.contains(&declaration),
            "Rust allocates {PATTERN_CACHE_ENTRIES} entries but WGSL does not declare `{declaration}`"
        );
    }

    /// Headless pipeline compile: prove the shading pass validates under wgpu
    /// 29's naga and that the compute pipeline accepts the bind group layout — no
    /// window, no world. Skips (with a note) when no GPU adapter exists, e.g.
    /// on a bare CI runner.
    #[test]
    fn dda_pipeline_compiles_headless() {
        let Some((device, _queue)) = headless_device() else {
            return;
        };
        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bind_group_layout =
            create_bind_group_layout(&device, voxel_color::OutputFormat::default().storage());
        let environment = HillaireEnvironment::new(&device);
        let _pipeline = create_compute_pipeline_with_layouts(
            &device,
            "dda test pipeline",
            "main",
            &build_program_for_output(
                &RenderQuality::default(),
                &MaterialGraphShaderSet::default(),
                voxel_color::OutputFormat::default(),
            ),
            &[
                Some(&bind_group_layout),
                Some(environment.sample_bind_group_layout()),
            ],
        );
        let validation_error = pollster::block_on(error_scope.pop());
        assert!(
            validation_error.is_none(),
            "the shading pass failed wgpu validation: {validation_error:?}"
        );
    }

    /// Headless pipeline compile of EVERY lever combination the overlay can
    /// select: each AO technique x each shadow mode, the cost-cutting levers,
    /// the CAGI sampling levers, and every traversal off-lever must validate under
    /// naga. Without this, a WGSL error on a non-default path only surfaces when
    /// someone clicks the radio button.
    /// The `voxel-color` crate unit-tests its patchers against a small WGSL fixture, so
    /// something has to check the REAL shader still carries what those patchers expect.
    /// This is that check: without it the fixture could stay green while `dda.wgsl`
    /// drifted, and the failure would only appear as a wgpu validation error at runtime.
    #[test]
    fn the_real_shading_source_carries_what_the_output_patchers_target() {
        use voxel_color::{OutputDepth, OutputFormat, OutputSupport};

        let support = OutputSupport {
            ten_bit_surface: true,
            float_surface: true,
            extended_srgb_presentation: true,
            sixteen_bit_norm_storage: true,
        };
        // The shipped file must be the 8-bit configuration verbatim.
        assert!(SHADER_SOURCE.contains("texture_storage_2d<rgba8unorm, write>"));
        assert_eq!(
            OutputFormat::default().patch_shader_source(&SHADER_SOURCE),
            SHADER_SOURCE.as_str()
        );
        // And every non-default depth must actually rewrite it.
        for depth in [OutputDepth::TenBit, OutputDepth::HdrFloat] {
            let format = OutputFormat::resolve(depth, support, wgpu::TextureFormat::Bgra8UnormSrgb);
            let patched = format.patch_shader_source(&SHADER_SOURCE);
            assert_ne!(
                patched,
                SHADER_SOURCE.as_str(),
                "{depth:?} did not patch the source"
            );
            assert!(
                patched.contains(&format.wgsl_storage_declaration()),
                "{depth:?} did not install its storage-texture type"
            );
        }
    }

    /// The compositor decodes the exact extended-sRGB transfer named by the surface tag,
    /// so restoring the old gamma-2.2 approximation would be a semantic mismatch that
    /// shader validation cannot see.
    #[test]
    fn the_real_shading_source_uses_the_exact_extended_srgb_transfer() {
        for anchor in ["0.04045", "0.0031308", "1.0 / 2.4", "12.92"] {
            assert!(
                SHADER_SOURCE.contains(anchor),
                "the exact sRGB transfer is missing `{anchor}`"
            );
        }
        assert!(
            !SHADER_SOURCE.contains("vec3<f32>(2.2, 2.2, 2.2)"),
            "the gamma-2.2 approximation must not feed an extended-sRGB-tagged surface"
        );
    }

    /// BOTH output depths must produce a shading pipeline that VALIDATES.
    ///
    /// The storage format has to agree in three places — the texture, this pass's
    /// bind group layout entry, and the `texture_storage_2d<...>` type inside the
    /// WGSL — and wgpu checks all three against each other. Every one of those
    /// disagreements shipped at least once while the output-depth toggle was being
    /// wired, each surfacing as a different error message several layers from its
    /// cause, and each discoverable only by running the app on a real display.
    ///
    /// This builds both depths headlessly so the next one is a `cargo test` failure
    /// instead of a panic on someone's machine. It exercises the pair that actually
    /// disagreed — the layout built from [`OutputFormat::storage`] against a
    /// pipeline built from [`OutputFormat::patch_shader_source`] — because both come
    /// from one resolved value and a drift between them is the whole failure class.
    #[test]
    fn both_output_depths_build_a_valid_shading_pipeline() {
        use voxel_color::{OutputDepth, OutputFormat, OutputSupport};

        let Some((device, _queue)) = headless_device() else {
            return;
        };
        // Claim surface support: the surface is irrelevant to a compute pipeline, and
        // the storage feature is the only half a headless device can veto.
        let support = OutputSupport {
            ten_bit_surface: true,
            // The surface is irrelevant to a compute pipeline, so claim both; the
            // storage feature below is the only half a headless device can veto.
            float_surface: true,
            extended_srgb_presentation: true,
            sixteen_bit_norm_storage: device
                .features()
                .contains(wgpu::Features::TEXTURE_FORMAT_16BIT_NORM),
        };

        for depth in OutputDepth::ALL {
            let format = OutputFormat::resolve(depth, support, wgpu::TextureFormat::Bgra8UnormSrgb);
            if format.depth() != depth {
                eprintln!("skipping {depth:?}: {}", support.ten_bit_diagnosis());
                continue;
            }
            let layout = create_bind_group_layout(&device, format.storage());
            let environment = HillaireEnvironment::new(&device);
            let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _pipeline = create_compute_pipeline_with_layouts(
                &device,
                "output depth test pipeline",
                "main",
                &build_program_for_output(
                    &RenderQuality::default(),
                    &MaterialGraphShaderSet::default(),
                    format,
                ),
                &[Some(&layout), Some(environment.sample_bind_group_layout())],
            );
            let validation_error = pollster::block_on(error_scope.pop());
            assert!(
                validation_error.is_none(),
                "{depth:?} ({:?} storage) failed wgpu validation — the layout and the \
                 shader disagree about the storage format: {validation_error:?}",
                format.storage()
            );
        }
    }

    #[test]
    fn every_lever_combination_compiles_headless() {
        let Some((device, _queue)) = headless_device() else {
            return;
        };
        let mut ao_settings_to_check = Vec::new();
        for mode in [
            AoMode::RayTraced,
            AoMode::AnalyticCorner,
            AoMode::AnalyticNeighborhood,
            AoMode::Off,
        ] {
            ao_settings_to_check.push(AoSettings {
                mode,
                ..AoSettings::default()
            });
        }
        ao_settings_to_check.push(AoSettings {
            brick_early_out: true,
            distance_fade: true,
            sun_aware_ray_budget: true,
            direction_mode: AoDirectionMode::BentUp,
            distance_falloff: false,
            ray_count: 4,
            max_distance_voxels: 32,
            ..AoSettings::default()
        });

        let bind_group_layout =
            create_bind_group_layout(&device, voxel_color::OutputFormat::default().storage());
        let environment = HillaireEnvironment::new(&device);
        let compile = |quality: &RenderQuality, description: String| {
            let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _pipeline = create_compute_pipeline_with_layouts(
                &device,
                "dda test pipeline",
                "main",
                &build_program_for_output(
                    quality,
                    &MaterialGraphShaderSet::default(),
                    voxel_color::OutputFormat::default(),
                ),
                &[
                    Some(&bind_group_layout),
                    Some(environment.sample_bind_group_layout()),
                ],
            );
            let validation_error = pollster::block_on(error_scope.pop());
            assert!(
                validation_error.is_none(),
                "{description} failed wgpu validation: {validation_error:?}"
            );
        };

        for ambient_occlusion in ao_settings_to_check {
            let quality = RenderQuality {
                ambient_occlusion,
                ..RenderQuality::default()
            };
            compile(&quality, format!("AO {:?}", ambient_occlusion.mode));
        }
        // Every traversal off-lever on its own, plus the all-on combination:
        // the column fast-forward paths are only reachable this way.
        for traversal in [
            TraversalSettings {
                column_fast_forward: true,
                ..TraversalSettings::default()
            },
            TraversalSettings {
                brick_bit_grid: true,
                ..TraversalSettings::default()
            },
            TraversalSettings {
                column_fast_forward: true,
                global_max_terminate: false,
                brick_bit_grid: true,
                distance_skip: false,
                directional_skip: false,
            },
        ] {
            let quality = RenderQuality {
                traversal,
                ..RenderQuality::default()
            };
            compile(&quality, format!("traversal {traversal:?}"));
        }
        // The E4 levers as the SHADING pass sees them (the propagation levers are
        // compiled by the CAGI pass's own test).
        for global_illumination in [
            CagiSettings {
                enabled: false,
                ..CagiSettings::default()
            },
            CagiSettings {
                sample_mode: CagiSampleMode::Nearest,
                ..CagiSettings::default()
            },
        ] {
            let quality = RenderQuality {
                global_illumination,
                ..RenderQuality::default()
            };
            compile(
                &quality,
                format!(
                    "CAGI enabled {} / {:?}",
                    global_illumination.enabled, global_illumination.sample_mode
                ),
            );
        }
        // E6: every water optics mode at both bounce budgets. The medium march,
        // the Snell branch and the two half-modes are only reachable this way, and
        // a WGSL error on one of them would otherwise surface as a black frame in
        // the bench rather than as a test failure.
        for mode in [
            WaterMode::Opaque,
            WaterMode::FresnelTint,
            WaterMode::Reflection,
            WaterMode::Refraction,
            WaterMode::Full,
        ] {
            for bounces in [1, 2] {
                let quality = RenderQuality {
                    water: WaterSettings {
                        mode,
                        bounces,
                        ..WaterSettings::default()
                    },
                    ..RenderQuality::default()
                };
                compile(&quality, format!("water {mode:?} x {bounces} bounces"));
            }
        }
    }

    /// The pipeline cache must dedupe by shader source and hold every preset's
    /// permutation after a prewarm, so a preset switch is a lookup.
    #[test]
    fn prewarming_the_presets_caches_one_pipeline_per_unique_source() {
        let Some((device, _queue)) = headless_device() else {
            return;
        };
        let bind_group_layout =
            create_bind_group_layout(&device, voxel_color::OutputFormat::default().storage());
        let environment = HillaireEnvironment::new(&device);
        let mut pass_pipelines: HashMap<u64, wgpu::ComputePipeline> = HashMap::new();
        let mut keys = Vec::new();
        for spec in QUALITY_PRESETS {
            if spec.preset == QualityPreset::Custom {
                continue;
            }
            let program = build_program_for_output(
                &spec.resolve(),
                &MaterialGraphShaderSet::default(),
                voxel_color::OutputFormat::default(),
            );
            let key = ComputePipelineCache::source_key(&program.source);
            keys.push(key);
            pass_pipelines.entry(key).or_insert_with(|| {
                create_compute_pipeline_with_layouts(
                    &device,
                    "dda test pipeline",
                    "main",
                    &program,
                    &[
                        Some(&bind_group_layout),
                        Some(environment.sample_bind_group_layout()),
                    ],
                )
            });
        }
        assert_eq!(keys.len(), 4, "four named presets");
        assert!(
            pass_pipelines.len() <= keys.len(),
            "the cache must dedupe presets that compile to the same source"
        );
    }

    /// A real GPU device, or `None` on a machine without an adapter (bare CI
    /// runner) so the GPU-dependent tests can skip with a note. Shared with the
    /// CAGI pass's tests.
    pub(crate) fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
        let instance = wgpu::Instance::default();
        let adapter = match pollster::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        ) {
            Ok(adapter) => adapter,
            Err(error) => {
                eprintln!("skipping GPU-dependent dda test: no adapter ({error})");
                return None;
            }
        };
        Some(
            pollster::block_on(adapter.request_device(&crate::gpu::device_descriptor(&adapter)))
                .expect("adapter exists but device creation failed"),
        )
    }

    use std::collections::BTreeMap;

    use voxel_graph::{InputPin, LinkId, NodeTypeId, OutputPin, PropertyValue, SocketKey};
    use voxel_material_graph::lowering::{compile, MaterialGraphShaderSet, MaterialSampleContext};
    use voxel_material_graph::test_support::{graph_driving_roughness, graph_with_output, node};

    // ---- moved from `voxel-material-graph` -------------------------------------
    // These exercise the ASSEMBLED shader, which is this crate's job: the lowering crate
    // generates a function and validates it standalone, but only here is it spliced into
    // `world.wgsl` + the binding prelude + the environment + the colour path. They were
    // the lowering crate's only reach back into the renderer.
    /// The highest-value test in this module: every animation node must survive
    /// injection into the WHOLE assembled DDA source, because a codegen slip is
    /// otherwise a runtime-only failure with no test to name it.
    #[test]
    fn animation_nodes_lower_to_naga_valid_wgsl_in_the_full_dda_source() {
        let registry = crate::graph::CATALOGUE;
        for (index, (node_type, socket)) in [
            ("material.time", "value"),
            ("material.oscillator", "value"),
            ("material.event_sensor", "signal"),
            ("material.event_sensor", "nearness"),
            ("material.event_sensor", "envelope"),
            ("material.multiply_scalar", "value"),
        ]
        .into_iter()
        .enumerate()
        {
            let (graph, _) = graph_driving_roughness(node_type, socket, &[]);
            let program = compile(&graph, &registry)
                .unwrap_or_else(|error| panic!("{node_type}.{socket} failed to compile: {error}"));
            let slot = 6 + index as u8;
            let mut set = MaterialGraphShaderSet::default();
            set.insert(slot, program);
            let source = super::build_shader_source_with_material_graphs(
                &crate::variants::RenderQuality::default(),
                &set,
            );
            let module = naga::front::wgsl::parse_str(&source).unwrap_or_else(|error| {
                panic!("{node_type}.{socket}: {}", error.emit_to_string(&source))
            });
            naga::valid::Validator::new(
                naga::valid::ValidationFlags::all(),
                naga::valid::Capabilities::all(),
            )
            .validate(&module)
            .unwrap_or_else(|error| panic!("{node_type}.{socket}: {error}"));
        }
    }

    /// A project with no animation nodes must generate byte-identical source.
    /// This is the ONLY byte-identity S3 claims — a frozen oscillator still
    /// returns a value, so an animated scene needs its own pixel baseline.
    #[test]
    fn a_project_without_animation_nodes_generates_unchanged_shader_source() {
        let quality = crate::variants::RenderQuality::default();
        let empty = MaterialGraphShaderSet::default();
        assert_eq!(
            super::build_shader_source_with_material_graphs(&quality, &empty),
            super::build_shader_source(&quality)
        );
    }

    #[test]
    fn graph_dispatch_injects_a_slot_branch_into_the_full_dda_source() {
        let registry = crate::graph::CATALOGUE;
        let (graph, _) = graph_with_output();
        let program = compile(&graph, &registry).unwrap();
        let mut set = MaterialGraphShaderSet::default();
        set.insert(6, program);
        let source = super::build_shader_source_with_material_graphs(
            &crate::variants::RenderQuality::default(),
            &set,
        );
        assert!(source.contains("if (material == 6u)"));
        assert!(source.contains("fn graph_material_6("));
        let module = naga::front::wgsl::parse_str(&source).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn procedural_catalog_lowers_noise_fbm_ramp_and_vectors_for_preview_and_gpu() {
        let registry = crate::graph::CATALOGUE;
        let (mut graph, output) = graph_with_output();
        let position = node("position");
        let noise = node("noise");
        let ramp = node("ramp");
        let fbm = node("fbm");
        graph.nodes.extend([
            (
                position.clone(),
                voxel_graph::NodeRecord {
                    node_type: NodeTypeId("material.position".into()),
                    node_type_version: 1,
                    properties: BTreeMap::new(),
                    socket_defaults: BTreeMap::new(),
                    unknown_payload: None,
                },
            ),
            (
                noise.clone(),
                voxel_graph::NodeRecord {
                    node_type: NodeTypeId("material.noise".into()),
                    node_type_version: 1,
                    properties: BTreeMap::new(),
                    socket_defaults: BTreeMap::from([
                        (SocketKey("scale".into()), PropertyValue::Scalar(1.5)),
                        (SocketKey("detail".into()), PropertyValue::Scalar(3.0)),
                        (SocketKey("roughness".into()), PropertyValue::Scalar(0.55)),
                    ]),
                    unknown_payload: None,
                },
            ),
            (
                ramp.clone(),
                voxel_graph::NodeRecord {
                    node_type: NodeTypeId("material.color_ramp".into()),
                    node_type_version: 1,
                    properties: BTreeMap::new(),
                    socket_defaults: BTreeMap::from([
                        (
                            SocketKey("color_a".into()),
                            PropertyValue::Color([0.02, 0.1, 0.01, 1.0]),
                        ),
                        (
                            SocketKey("color_b".into()),
                            PropertyValue::Color([0.5, 0.8, 0.08, 1.0]),
                        ),
                    ]),
                    unknown_payload: None,
                },
            ),
            (
                fbm.clone(),
                voxel_graph::NodeRecord {
                    node_type: NodeTypeId("material.fbm".into()),
                    node_type_version: 1,
                    properties: BTreeMap::new(),
                    socket_defaults: BTreeMap::new(),
                    unknown_payload: None,
                },
            ),
        ]);
        graph.links.extend([
            (
                LinkId("position-noise".into()),
                voxel_graph::LinkRecord {
                    from: OutputPin {
                        node: position.clone(),
                        socket: SocketKey("vector".into()),
                    },
                    to: InputPin {
                        node: noise.clone(),
                        socket: SocketKey("position".into()),
                    },
                    order: 0,
                },
            ),
            (
                LinkId("noise-ramp".into()),
                voxel_graph::LinkRecord {
                    from: OutputPin {
                        node: noise.clone(),
                        socket: SocketKey("factor".into()),
                    },
                    to: InputPin {
                        node: ramp.clone(),
                        socket: SocketKey("factor".into()),
                    },
                    order: 0,
                },
            ),
            (
                LinkId("ramp-base".into()),
                voxel_graph::LinkRecord {
                    from: OutputPin {
                        node: ramp.clone(),
                        socket: SocketKey("color".into()),
                    },
                    to: InputPin {
                        node: output.clone(),
                        socket: SocketKey("base_color".into()),
                    },
                    order: 0,
                },
            ),
            (
                LinkId("noise-emission".into()),
                voxel_graph::LinkRecord {
                    from: OutputPin {
                        node: noise.clone(),
                        socket: SocketKey("color".into()),
                    },
                    to: InputPin {
                        node: output.clone(),
                        socket: SocketKey("emission".into()),
                    },
                    order: 0,
                },
            ),
            (
                LinkId("position-fbm".into()),
                voxel_graph::LinkRecord {
                    from: OutputPin {
                        node: position,
                        socket: SocketKey("vector".into()),
                    },
                    to: InputPin {
                        node: fbm.clone(),
                        socket: SocketKey("position".into()),
                    },
                    order: 0,
                },
            ),
            (
                LinkId("fbm-roughness".into()),
                voxel_graph::LinkRecord {
                    from: OutputPin {
                        node: fbm,
                        socket: SocketKey("value".into()),
                    },
                    to: InputPin {
                        node: output,
                        socket: SocketKey("roughness".into()),
                    },
                    order: 0,
                },
            ),
        ]);
        let program = compile(&graph, &registry).unwrap();
        let sample = program.evaluate(MaterialSampleContext::still(
            [1.25, 2.5, -0.75],
            [0.0, 1.0, 0.0],
        ));
        assert!(sample.base_color.iter().all(|value| value.is_finite()));
        assert!((0.0..=1.0).contains(&sample.roughness));
        assert!(program.wgsl.contains("graph_fbm"));

        let mut shaders = MaterialGraphShaderSet::default();
        shaders.insert(3, program.clone());
        shaders.insert(4, program);
        let source = super::build_shader_source_with_material_graphs(
            &crate::variants::RenderQuality::default(),
            &shaders,
        );
        let module = naga::front::wgsl::parse_str(&source).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn pbr_surface_state_reaches_lighting_as_one_modifier_safe_payload() {
        let source = super::SHADER_SOURCE.as_str();
        assert!(source.contains("struct PbrSurface"));
        assert!(source.contains("surface.specular"));
        assert!(source.contains("surface.ambient_occlusion"));
        assert!(source.contains("surface.normal = normal"));
        assert!(source.contains("pbr_distribution_ggx"));
    }
}
