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

use wgpu::util::DeviceExt;

use crate::brickmap::Brickmap;
use crate::camera::CameraUniform;
use crate::lighting::LightingUniform;

const WORKGROUP_SIZE: u32 = 8;

/// The DDA shader source, exposed so the headless benchmark
/// (`examples/bench_dda.rs`) can build A/B pipeline variants by patching the
/// compile-time flags at the top of the file (see "A/B benchmark levers" in
/// `shaders/dda.wgsl`).
pub const SHADER_SOURCE: &str = include_str!("../../shaders/dda.wgsl");

pub struct DdaPass {
    pipeline: wgpu::ComputePipeline,
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
        let pipeline = create_pipeline(device, shader_source, &bind_group_layout);

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
            pipeline,
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

    /// Swap in a new compute pipeline built from `shader_source` (the E1
    /// overlay path: AO compile-time levers changed). Buffers, bind group
    /// layout and bind group are untouched — only the shader differs, so the
    /// existing bind group stays valid against the new pipeline.
    pub fn rebuild_pipeline(&mut self, device: &wgpu::Device, shader_source: &str) {
        self.pipeline = create_pipeline(device, shader_source, &self.bind_group_layout);
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
        compute_pass.set_pipeline(&self.pipeline);
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

    /// Headless pipeline compile: prove `dda.wgsl` validates under wgpu 29's
    /// naga and that the compute pipeline accepts the bind group layout — no
    /// window, no world. Skips (with a note) when no GPU adapter exists,
    /// e.g. on a bare CI runner.
    #[test]
    fn dda_pipeline_compiles_headless() {
        let instance = wgpu::Instance::default();
        let adapter = match pollster::block_on(
            instance.request_adapter(&wgpu::RequestAdapterOptions::default()),
        ) {
            Ok(adapter) => adapter,
            Err(error) => {
                eprintln!("skipping dda_pipeline_compiles_headless: no GPU adapter ({error})");
                return;
            }
        };
        let (device, _queue) =
            pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
                label: Some("dda headless test device"),
                ..Default::default()
            }))
            .expect("adapter exists but device creation failed");

        let error_scope = device.push_error_scope(wgpu::ErrorFilter::Validation);
        let bind_group_layout = create_bind_group_layout(&device);
        let _pipeline = create_pipeline(&device, SHADER_SOURCE, &bind_group_layout);
        let validation_error = pollster::block_on(error_scope.pop());
        assert!(
            validation_error.is_none(),
            "dda.wgsl failed wgpu validation: {validation_error:?}"
        );
    }
}
