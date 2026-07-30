//! DDA compute pass: primary rays plus one sun shadow ray per hit, traced
//! through the two-level brickmap (`shaders/dda.wgsl`), one thread per pixel,
//! writing shaded colors into the frame's storage texture. Owns the brickmap
//! GPU buffers (uploaded once at startup — the world is static in Stage 1)
//! and the per-frame camera + lighting uniforms.
//!
//! Bind group 0 layout mirrors the table at the top of `dda.wgsl`:
//! camera uniform, brickmap metadata uniform, three read-only storage buffers
//! (brick pointers, occupancy bits, material bytes), the palette storage
//! buffer, the write-only rgba8unorm output texture, the lighting uniform,
//! the per-XZ-column max-brick-Y storage buffer (the traversal's
//! column-height early exit), and the two empty-space acceleration grids
//! (1-bit-per-brick occupancy, chebyshev skip distances).

use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};

use wgpu::util::DeviceExt;

use crate::brickmap::Brickmap;
use crate::camera::CameraUniform;
use crate::lighting::LightingUniform;
use crate::variants::RenderQuality;

const WORKGROUP_SIZE: u32 = 8;

/// The DDA shader source, exposed so the headless benchmark
/// (`examples/bench_dda.rs`) can build A/B pipeline variants by patching the
/// compile-time flags at the top of the file (see "A/B benchmark levers" in
/// `shaders/dda.wgsl`).
pub const SHADER_SOURCE: &str = include_str!("../../shaders/dda.wgsl");

/// [`SHADER_SOURCE`] with every experiment's compile-time levers patched in.
/// The app's preset path and the benchmark's variant builder both go through
/// this one function, so a new lever module cannot be forgotten at a call
/// site. Returns [`SHADER_SOURCE`] verbatim for the shipped (Balanced) quality.
pub fn build_shader_source(quality: &RenderQuality) -> String {
    let traversal_patched = quality.traversal.patch_shader_source(SHADER_SOURCE);
    let ao_patched = quality
        .ambient_occlusion
        .patch_shader_source(&traversal_patched);
    quality.shadows.patch_shader_source(&ao_patched)
}

/// Cache key of a shader source — pipelines are keyed by the hash rather than
/// the ~55 KB string itself.
fn shader_source_key(shader_source: &str) -> u64 {
    let mut hasher = DefaultHasher::new();
    shader_source.hash(&mut hasher);
    hasher.finish()
}

pub struct DdaPass {
    /// Compiled pipelines by shader-source key. Switching a compile-time lever
    /// (a preset change) is then a lookup, not a compile — the app prewarms
    /// every preset's permutation at startup so the switch never stutters.
    pipeline_cache: HashMap<u64, wgpu::ComputePipeline>,
    /// Key of the pipeline [`DdaPass::encode`] dispatches.
    active_pipeline_key: u64,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    camera_uniform_buffer: wgpu::Buffer,
    metadata_uniform_buffer: wgpu::Buffer,
    brick_indices_buffer: wgpu::Buffer,
    occupancy_words_buffer: wgpu::Buffer,
    material_words_buffer: wgpu::Buffer,
    palette_buffer: wgpu::Buffer,
    lighting_uniform_buffer: wgpu::Buffer,
    column_max_buffer: wgpu::Buffer,
    brick_occupancy_bits_buffer: wgpu::Buffer,
    skip_distance_buffer: wgpu::Buffer,
}

impl DdaPass {
    pub fn new(
        device: &wgpu::Device,
        brickmap: &Brickmap,
        output_view: &wgpu::TextureView,
    ) -> Self {
        Self::new_with_shader_source(device, brickmap, output_view, SHADER_SOURCE)
    }

    /// Build the pass from an explicit shader source string — the benchmark's
    /// entry point for A/B variants (patched copies of [`SHADER_SOURCE`]).
    /// Everything else (buffers, layout, bind group) is identical to
    /// [`DdaPass::new`].
    pub fn new_with_shader_source(
        device: &wgpu::Device,
        brickmap: &Brickmap,
        output_view: &wgpu::TextureView,
        shader_source: &str,
    ) -> Self {
        let bind_group_layout = create_bind_group_layout(device);
        let active_pipeline_key = shader_source_key(shader_source);
        let pipeline_cache = HashMap::from([(
            active_pipeline_key,
            create_pipeline(device, shader_source, &bind_group_layout),
        )]);

        let camera_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dda camera uniform"),
            size: std::mem::size_of::<CameraUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let lighting_uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("dda lighting uniform"),
            size: std::mem::size_of::<LightingUniform>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let metadata_uniform_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("dda brickmap metadata uniform"),
                contents: bytemuck::bytes_of(&brickmap.metadata()),
                usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            });
        let brick_indices_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dda brick indices"),
            contents: bytemuck::cast_slice(&brickmap.brick_indices),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let occupancy_words_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dda occupancy words"),
            contents: bytemuck::cast_slice(&brickmap.occupancy_words),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let material_words_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dda material words"),
            contents: bytemuck::cast_slice(&brickmap.material_words),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let palette_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dda palette"),
            contents: bytemuck::cast_slice(&crate::brickmap::palette()),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let column_max_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dda column max brick y"),
            contents: bytemuck::cast_slice(&brickmap.column_max_brick_y),
            usage: wgpu::BufferUsages::STORAGE,
        });
        let brick_occupancy_bits_buffer =
            device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
                label: Some("dda brick occupancy bits"),
                contents: bytemuck::cast_slice(&brickmap.brick_occupancy_bit_words),
                usage: wgpu::BufferUsages::STORAGE,
            });
        let skip_distance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("dda brick skip distances"),
            contents: bytemuck::cast_slice(&brickmap.brick_skip_distance_words),
            usage: wgpu::BufferUsages::STORAGE,
        });

        let bind_group = create_bind_group(
            device,
            &bind_group_layout,
            &camera_uniform_buffer,
            &metadata_uniform_buffer,
            &brick_indices_buffer,
            &occupancy_words_buffer,
            &material_words_buffer,
            &palette_buffer,
            output_view,
            &lighting_uniform_buffer,
            &column_max_buffer,
            &brick_occupancy_bits_buffer,
            &skip_distance_buffer,
        );

        Self {
            pipeline_cache,
            active_pipeline_key,
            bind_group_layout,
            bind_group,
            camera_uniform_buffer,
            metadata_uniform_buffer,
            brick_indices_buffer,
            occupancy_words_buffer,
            material_words_buffer,
            palette_buffer,
            lighting_uniform_buffer,
            column_max_buffer,
            brick_occupancy_bits_buffer,
            skip_distance_buffer,
        }
    }

    /// Dispatch `shader_source` from now on, compiling it only on a cache miss
    /// (the overlay path: a compile-time lever or a preset changed). Buffers,
    /// bind group layout and bind group are untouched — only the shader
    /// differs, so the existing bind group stays valid against every cached
    /// pipeline.
    pub fn set_shader_source(&mut self, device: &wgpu::Device, shader_source: &str) {
        self.active_pipeline_key = self.cache_pipeline(device, shader_source);
    }

    /// Precompile `shader_sources` (duplicates cost nothing) so that a later
    /// [`DdaPass::set_shader_source`] to any of them is a hash lookup instead of
    /// a shader compile — the reason a preset switch cannot stutter. Returns how
    /// many pipelines the cache holds afterwards.
    pub fn prewarm_pipelines(&mut self, device: &wgpu::Device, shader_sources: &[String]) -> usize {
        for shader_source in shader_sources {
            self.cache_pipeline(device, shader_source);
        }
        self.pipeline_cache.len()
    }

    /// Pipelines currently held (the memory the cache costs).
    pub fn cached_pipeline_count(&self) -> usize {
        self.pipeline_cache.len()
    }

    fn cache_pipeline(&mut self, device: &wgpu::Device, shader_source: &str) -> u64 {
        let key = shader_source_key(shader_source);
        self.pipeline_cache
            .entry(key)
            .or_insert_with(|| create_pipeline(device, shader_source, &self.bind_group_layout));
        key
    }

    fn active_pipeline(&self) -> &wgpu::ComputePipeline {
        self.pipeline_cache
            .get(&self.active_pipeline_key)
            .expect("the active DDA pipeline is always cached")
    }

    /// Refresh the output-texture binding after the storage texture is recreated.
    pub fn rebind(&mut self, device: &wgpu::Device, output_view: &wgpu::TextureView) {
        self.bind_group = create_bind_group(
            device,
            &self.bind_group_layout,
            &self.camera_uniform_buffer,
            &self.metadata_uniform_buffer,
            &self.brick_indices_buffer,
            &self.occupancy_words_buffer,
            &self.material_words_buffer,
            &self.palette_buffer,
            output_view,
            &self.lighting_uniform_buffer,
            &self.column_max_buffer,
            &self.brick_occupancy_bits_buffer,
            &self.skip_distance_buffer,
        );
    }

    #[allow(clippy::too_many_arguments)]
    pub fn encode(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        camera_uniform: &CameraUniform,
        lighting_uniform: &LightingUniform,
        output_width: u32,
        output_height: u32,
        timestamp_writes: Option<wgpu::ComputePassTimestampWrites<'_>>,
    ) {
        queue.write_buffer(
            &self.camera_uniform_buffer,
            0,
            bytemuck::bytes_of(camera_uniform),
        );
        queue.write_buffer(
            &self.lighting_uniform_buffer,
            0,
            bytemuck::bytes_of(lighting_uniform),
        );

        let mut compute_pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("dda pass"),
            timestamp_writes,
        });
        compute_pass.set_pipeline(self.active_pipeline());
        compute_pass.set_bind_group(0, &self.bind_group, &[]);
        compute_pass.dispatch_workgroups(
            output_width.div_ceil(WORKGROUP_SIZE),
            output_height.div_ceil(WORKGROUP_SIZE),
            1,
        );
    }
}

/// Bind group layout only, separated from the pipeline so a shader-source
/// rebuild ([`DdaPass::rebuild_pipeline`]) can reuse the ORIGINAL layout
/// object — the existing bind group must stay valid against the new pipeline.
fn create_bind_group_layout(device: &wgpu::Device) -> wgpu::BindGroupLayout {
    let uniform_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };
    let storage_entry = |binding: u32| wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    };

    device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
        label: Some("dda bind group layout"),
        entries: &[
            uniform_entry(0), // camera
            uniform_entry(1), // brickmap metadata
            storage_entry(2), // brick indices
            storage_entry(3), // occupancy words
            storage_entry(4), // material words
            storage_entry(5), // palette
            wgpu::BindGroupLayoutEntry {
                binding: 6,
                visibility: wgpu::ShaderStages::COMPUTE,
                ty: wgpu::BindingType::StorageTexture {
                    access: wgpu::StorageTextureAccess::WriteOnly,
                    format: wgpu::TextureFormat::Rgba8Unorm,
                    view_dimension: wgpu::TextureViewDimension::D2,
                },
                count: None,
            },
            uniform_entry(7),  // lighting
            storage_entry(8),  // per-XZ-column max occupied brick Y
            storage_entry(9),  // 1-bit-per-brick occupancy grid
            storage_entry(10), // chebyshev skip-distance bytes
        ],
    })
}

/// Shader module + compute pipeline against an existing layout, separated
/// from buffer upload so the headless pipeline test can validate the shader
/// and layout without building a world.
fn create_pipeline(
    device: &wgpu::Device,
    shader_source: &str,
    bind_group_layout: &wgpu::BindGroupLayout,
) -> wgpu::ComputePipeline {
    let shader_module = device.create_shader_module(wgpu::ShaderModuleDescriptor {
        label: Some("dda shader"),
        source: wgpu::ShaderSource::Wgsl(shader_source.into()),
    });

    let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
        label: Some("dda pipeline layout"),
        bind_group_layouts: &[Some(bind_group_layout)],
        immediate_size: 0,
    });

    device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
        label: Some("dda pipeline"),
        layout: Some(&pipeline_layout),
        module: &shader_module,
        entry_point: Some("main"),
        compilation_options: Default::default(),
        cache: None,
    })
}

#[allow(clippy::too_many_arguments)]
fn create_bind_group(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    camera_uniform_buffer: &wgpu::Buffer,
    metadata_uniform_buffer: &wgpu::Buffer,
    brick_indices_buffer: &wgpu::Buffer,
    occupancy_words_buffer: &wgpu::Buffer,
    material_words_buffer: &wgpu::Buffer,
    palette_buffer: &wgpu::Buffer,
    output_view: &wgpu::TextureView,
    lighting_uniform_buffer: &wgpu::Buffer,
    column_max_buffer: &wgpu::Buffer,
    brick_occupancy_bits_buffer: &wgpu::Buffer,
    skip_distance_buffer: &wgpu::Buffer,
) -> wgpu::BindGroup {
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("dda bind group"),
        layout,
        entries: &[
            wgpu::BindGroupEntry {
                binding: 0,
                resource: camera_uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 1,
                resource: metadata_uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 2,
                resource: brick_indices_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 3,
                resource: occupancy_words_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 4,
                resource: material_words_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 5,
                resource: palette_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 6,
                resource: wgpu::BindingResource::TextureView(output_view),
            },
            wgpu::BindGroupEntry {
                binding: 7,
                resource: lighting_uniform_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 8,
                resource: column_max_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 9,
                resource: brick_occupancy_bits_buffer.as_entire_binding(),
            },
            wgpu::BindGroupEntry {
                binding: 10,
                resource: skip_distance_buffer.as_entire_binding(),
            },
        ],
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::ao::{AoDirectionMode, AoMode, AoSettings};
    use crate::shadows::{ShadowMode, ShadowSettings};
    use crate::traversal::TraversalSettings;
    use crate::variants::{QualityPreset, QUALITY_PRESETS};

    #[test]
    fn default_settings_build_the_shipped_shader() {
        assert_eq!(
            build_shader_source(&RenderQuality::default()),
            SHADER_SOURCE
        );
    }

    /// Headless pipeline compile: prove `dda.wgsl` validates under wgpu 29's
    /// naga and that the compute pipeline accepts the bind group layout — no
    /// window, no world. Skips (with a note) when no GPU adapter exists, e.g.
    /// on a bare CI runner.
    #[test]
    fn dda_pipeline_compiles_headless() {
        let Some((device, _queue)) = headless_device() else {
            return;
        };
        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bind_group_layout = create_bind_group_layout(&device);
        let _pipeline = create_pipeline(&device, SHADER_SOURCE, &bind_group_layout);
        let validation_error = pollster::block_on(error_scope.pop());
        assert!(
            validation_error.is_none(),
            "dda.wgsl failed wgpu validation: {validation_error:?}"
        );
    }

    /// Headless pipeline compile of EVERY lever combination the overlay can
    /// select: each AO technique x each shadow mode, the cost-cutting levers,
    /// and every traversal off-lever must validate under naga. Without this, a
    /// WGSL error on a non-default path only surfaces when someone clicks the
    /// radio button.
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
            let _pipeline =
                create_pipeline(&device, &build_shader_source(quality), &bind_group_layout);
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
            let key = shader_source_key(&shader_source);
            keys.push(key);
            pass_pipelines
                .entry(key)
                .or_insert_with(|| create_pipeline(&device, &shader_source, &bind_group_layout));
        }
        assert_eq!(keys.len(), 4, "four named presets");
        assert_eq!(
            pass_pipelines.len(),
            3,
            "Quest and Balanced differ by render scale alone — same pipeline"
        );
    }

    /// A real GPU device, or `None` on a machine without an adapter (bare CI
    /// runner) so the GPU-dependent tests can skip with a note.
    fn headless_device() -> Option<(wgpu::Device, wgpu::Queue)> {
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
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("dda headless test device"),
                ..Default::default()
            }))
            .expect("adapter exists but device creation failed"),
        )
    }
}
