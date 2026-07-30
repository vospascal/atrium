//! DDA compute pass: primary rays plus one sun shadow ray per hit, traced
//! through the two-level brickmap, one thread per pixel, writing shaded colors
//! into the frame's storage texture. Owns only what is specific to shading a
//! pixel — the camera uniform and the output texture; the brickmap/lighting
//! resources belong to [`super::world_bindings::WorldBindings`] (shared with the
//! E4 CAGI pass) and the light volume to [`super::cagi::LightVolume`].
//!
//! Bind group 0 = the shared world entries (bindings 1-5, 7-10), the shared light
//! volume entries (11, 13, 14 — the writable back buffer at 12 belongs to the CA
//! pass alone), plus this pass's own camera uniform (0) and output texture (6).
//! The tables at the top of `shaders/world.wgsl`, `shaders/cagi_volume.wgsl` and
//! `shaders/dda.wgsl` document their own thirds.

use crate::camera::CameraUniform;
use crate::variants::RenderQuality;

use super::cagi::LightVolume;
use super::world_bindings::WorldBindings;
use super::ComputePipelineCache;

const WORKGROUP_SIZE: u32 = 8;

/// The shading pass's shader source: the shared traversal core, the shared
/// light-volume half, then the shading pass itself. Exposed so the headless
/// benchmark (`examples/bench_dda.rs`) can build A/B pipeline variants by
/// patching the compile-time levers (see "A/B benchmark levers" in
/// `shaders/world.wgsl` and the AO block in `shaders/dda.wgsl`).
pub const SHADER_SOURCE: &str = concat!(
    include_str!("../../shaders/world.wgsl"),
    include_str!("../../shaders/cagi_volume.wgsl"),
    include_str!("../../shaders/dda.wgsl"),
);

/// [`SHADER_SOURCE`] with every experiment's compile-time levers patched in.
/// The app's preset path and the benchmark's variant builder both go through
/// this one function, so a new lever module cannot be forgotten at a call
/// site. Returns [`SHADER_SOURCE`] verbatim for the shipped (Balanced) quality.
///
/// Only the CAGI levers of the SHARED volume half are patched here; the
/// propagation levers live in `cagi.wgsl`, which this source does not contain
/// (`super::cagi::build_shader_source` patches those).
pub fn build_shader_source(quality: &RenderQuality) -> String {
    let traversal_patched = quality.traversal.patch_shader_source(SHADER_SOURCE);
    let ao_patched = quality
        .ambient_occlusion
        .patch_shader_source(&traversal_patched);
    let shadows_patched = quality.shadows.patch_shader_source(&ao_patched);
    quality
        .global_illumination
        .patch_volume_consts(&shadows_patched)
}

pub struct DdaPass {
    pipeline_cache: ComputePipelineCache,
    bind_group_layout: wgpu::BindGroupLayout,
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
    ) -> Self {
        Self::new_with_shader_source(
            device,
            world_bindings,
            light_volume,
            output_view,
            SHADER_SOURCE,
        )
    }

    /// Build the pass from an explicit shader source string — the benchmark's
    /// entry point for A/B variants (patched copies of [`SHADER_SOURCE`]).
    /// Everything else (buffers, layout, bind groups) is identical to
    /// [`DdaPass::new`].
    pub fn new_with_shader_source(
        device: &wgpu::Device,
        world_bindings: &WorldBindings,
        light_volume: &LightVolume,
        output_view: &wgpu::TextureView,
        shader_source: &str,
    ) -> Self {
        let bind_group_layout = create_bind_group_layout(device);
        let pipeline_cache = ComputePipelineCache::new(
            device,
            "dda pass",
            "main",
            shader_source,
            &bind_group_layout,
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
            bind_groups,
            camera_uniform_buffer,
        }
    }

    /// Dispatch `shader_source` from now on, compiling it only on a cache miss
    /// (the overlay path: a compile-time lever or a preset changed). Buffers and
    /// bind groups are untouched — only the shader differs, so the existing bind
    /// groups stay valid against every cached pipeline.
    pub fn set_shader_source(&mut self, device: &wgpu::Device, shader_source: &str) {
        self.pipeline_cache
            .set_shader_source(device, shader_source, &self.bind_group_layout);
    }

    /// Precompile `shader_sources` (duplicates cost nothing) so that a later
    /// [`DdaPass::set_shader_source`] to any of them is a hash lookup instead of
    /// a shader compile — the reason a preset switch cannot stutter. Returns how
    /// many pipelines the cache holds afterwards.
    pub fn prewarm_pipelines(&mut self, device: &wgpu::Device, shader_sources: &[String]) -> usize {
        self.pipeline_cache
            .prewarm(device, shader_sources, &self.bind_group_layout)
    }

    /// Pipelines currently held (the memory the cache costs).
    pub fn cached_pipeline_count(&self) -> usize {
        self.pipeline_cache.len()
    }

    /// Refresh the bindings after the storage texture is recreated (a resize or a
    /// render-scale change) or the light volume was rebuilt (the CAGI resolution
    /// lever moved).
    pub fn rebind(
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
fn create_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let mut entries = WorldBindings::layout_entries();
    entries.extend(LightVolume::layout_entries(false));
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 0,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    });
    entries.push(wgpu::BindGroupLayoutEntry {
        binding: 6,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::StorageTexture {
            access: wgpu::StorageTextureAccess::WriteOnly,
            format: wgpu::TextureFormat::Rgba8Unorm,
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
            binding: 0,
            resource: camera_uniform_buffer.as_entire_binding(),
        });
        entries.push(wgpu::BindGroupEntry {
            binding: 6,
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
    use crate::passes::create_compute_pipeline;
    use crate::shadows::{ShadowMode, ShadowSettings};
    use crate::traversal::TraversalSettings;
    use crate::variants::{QualityPreset, QUALITY_PRESETS};
    use std::collections::HashMap;

    #[test]
    fn default_settings_build_the_shipped_shader() {
        assert_eq!(
            build_shader_source(&RenderQuality::default()),
            SHADER_SOURCE
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
        let bind_group_layout = create_bind_group_layout(&device);
        let _pipeline = create_compute_pipeline(
            &device,
            "dda test pipeline",
            "main",
            SHADER_SOURCE,
            &bind_group_layout,
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

        let bind_group_layout = create_bind_group_layout(&device);
        let compile = |quality: &RenderQuality, description: String| {
            let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
            let _pipeline = create_compute_pipeline(
                &device,
                "dda test pipeline",
                "main",
                &build_shader_source(quality),
                &bind_group_layout,
            );
            let validation_error = pollster::block_on(error_scope.pop());
            assert!(
                validation_error.is_none(),
                "{description} failed wgpu validation: {validation_error:?}"
            );
        };

        for ambient_occlusion in ao_settings_to_check {
            for shadow_mode in [ShadowMode::Hard, ShadowMode::SoftDistanceField] {
                let quality = RenderQuality {
                    ambient_occlusion,
                    shadows: ShadowSettings {
                        mode: shadow_mode,
                        ..ShadowSettings::default()
                    },
                    ..RenderQuality::default()
                };
                compile(
                    &quality,
                    format!("AO {:?} + shadows {shadow_mode:?}", ambient_occlusion.mode),
                );
            }
        }
        // Every traversal off-lever on its own, plus the all-on combination:
        // the column fast-forward paths are only reachable this way.
        for traversal in [
            TraversalSettings {
                column_fast_forward: true,
                ..TraversalSettings::default()
            },
            TraversalSettings {
                descend_fast_forward: true,
                ..TraversalSettings::default()
            },
            TraversalSettings {
                any_hit_shadow: true,
                ..TraversalSettings::default()
            },
            TraversalSettings {
                brick_bit_grid: true,
                ..TraversalSettings::default()
            },
            TraversalSettings {
                column_fast_forward: true,
                descend_fast_forward: true,
                global_max_terminate: false,
                any_hit_shadow: true,
                brick_bit_grid: true,
                distance_skip: false,
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
    }

    /// The pipeline cache must dedupe by shader source and hold every preset's
    /// permutation after a prewarm, so a preset switch is a lookup.
    #[test]
    fn prewarming_the_presets_caches_one_pipeline_per_unique_source() {
        let Some((device, _queue)) = headless_device() else {
            return;
        };
        let bind_group_layout = create_bind_group_layout(&device);
        let mut pass_pipelines: HashMap<u64, wgpu::ComputePipeline> = HashMap::new();
        let mut keys = Vec::new();
        for spec in QUALITY_PRESETS {
            if spec.preset == QualityPreset::Custom {
                continue;
            }
            let shader_source = build_shader_source(&spec.resolve());
            let key = ComputePipelineCache::source_key(&shader_source);
            keys.push(key);
            pass_pipelines.entry(key).or_insert_with(|| {
                create_compute_pipeline(
                    &device,
                    "dda test pipeline",
                    "main",
                    &shader_source,
                    &bind_group_layout,
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
}
