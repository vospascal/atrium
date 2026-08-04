//! Jolifanto LUT dimensions and update policy.

use super::resources::{AtmosphereBindings, LutKind};

/// Starting LUT sizes from Jolifanto's WebGPU implementation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LutConfig {
    pub transmittance: [u32; 2],
    pub multiple_scattering: [u32; 2],
    pub sky_view: [u32; 2],
    pub aerial_perspective: [u32; 3],
}

impl Default for LutConfig {
    fn default() -> Self {
        Self {
            transmittance: [256, 64],
            multiple_scattering: [32, 32],
            sky_view: [192, 108],
            aerial_perspective: [32, 32, 32],
        }
    }
}

/// Which LUTs need regeneration for the current frame.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct LutUpdate {
    pub atmosphere: bool,
    pub sun: bool,
    pub camera: bool,
}

impl LutUpdate {
    pub const fn all() -> Self {
        Self {
            atmosphere: true,
            sun: true,
            camera: true,
        }
    }

    pub const fn constant_luts(self) -> bool {
        self.atmosphere || self.sun
    }

    pub const fn view_luts(self) -> bool {
        self.atmosphere || self.sun || self.camera
    }
}

/// The four compute pipelines that populate the persistent atmosphere LUTs.
pub struct AtmosphereLutPasses {
    transmittance: LutPipeline,
    multiple_scattering: LutPipeline,
    sky_view: LutPipeline,
    aerial_perspective: LutPipeline,
}

struct LutPipeline {
    pipeline: wgpu::ComputePipeline,
    bind_group: wgpu::BindGroup,
    workgroups: [u32; 3],
}

impl AtmosphereLutPasses {
    pub fn new(device: &wgpu::Device, bindings: &AtmosphereBindings) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("jolifanto atmosphere LUT shaders"),
            source: wgpu::ShaderSource::Wgsl(super::shaders::LUT_WGSL.into()),
        });
        let make = |label: &str, entry: &str, kind: LutKind, dimension, workgroups| {
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &[
                    wgpu::BindGroupLayoutEntry {
                        binding: 0,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Buffer {
                            ty: wgpu::BufferBindingType::Uniform,
                            has_dynamic_offset: false,
                            min_binding_size: None,
                        },
                        count: None,
                    },
                    wgpu::BindGroupLayoutEntry {
                        binding: 1,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::StorageTexture {
                            access: wgpu::StorageTextureAccess::WriteOnly,
                            format: wgpu::TextureFormat::Rgba16Float,
                            view_dimension: dimension,
                        },
                        count: None,
                    },
                ],
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: bindings.uniform_buffer().as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(bindings.view(kind)),
                    },
                ],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some(label),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some(label),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some(entry),
                compilation_options: Default::default(),
                cache: None,
            });
            LutPipeline {
                pipeline,
                bind_group,
                workgroups,
            }
        };
        Self {
            transmittance: make(
                "atmosphere transmittance pass",
                "atmosphere_transmittance_main",
                LutKind::Transmittance,
                wgpu::TextureViewDimension::D2,
                [32, 8, 1],
            ),
            multiple_scattering: make(
                "atmosphere multiple-scattering pass",
                "atmosphere_multiple_scattering_main",
                LutKind::MultipleScattering,
                wgpu::TextureViewDimension::D2,
                [4, 4, 1],
            ),
            sky_view: make(
                "atmosphere sky-view pass",
                "atmosphere_sky_view_main",
                LutKind::SkyView,
                wgpu::TextureViewDimension::D2,
                [24, 14, 1],
            ),
            aerial_perspective: make(
                "atmosphere aerial-perspective pass",
                "atmosphere_aerial_perspective_main",
                LutKind::AerialPerspective,
                wgpu::TextureViewDimension::D3,
                [8, 8, 8],
            ),
        }
    }

    /// Encode only the LUTs invalidated by the atmosphere/sun/camera update.
    pub fn encode(&self, encoder: &mut wgpu::CommandEncoder, update: LutUpdate) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("jolifanto atmosphere LUT generation"),
            timestamp_writes: None,
        });
        if update.constant_luts() {
            dispatch(&mut pass, &self.transmittance);
            dispatch(&mut pass, &self.multiple_scattering);
        }
        if update.view_luts() {
            dispatch(&mut pass, &self.sky_view);
            dispatch(&mut pass, &self.aerial_perspective);
        }
    }
}

fn dispatch(pass: &mut wgpu::ComputePass<'_>, pipeline: &LutPipeline) {
    pass.set_pipeline(&pipeline.pipeline);
    pass.set_bind_group(0, &pipeline.bind_group, &[]);
    pass.dispatch_workgroups(
        pipeline.workgroups[0],
        pipeline.workgroups[1],
        pipeline.workgroups[2],
    );
}

#[cfg(test)]
mod tests {
    #[test]
    fn all_lut_compute_variants_parse_with_naga() {
        naga::front::wgsl::parse_str(super::super::shaders::LUT_WGSL)
            .expect("Jolifanto LUT compute WGSL must parse with naga");
    }

    #[test]
    fn lut_contract_pins_jolifanto_scale_and_local_up_parameterization() {
        let source = super::super::shaders::LUT_WGSL;
        assert!(source.contains("from_kilometers_scale"));
        assert!(source.contains("atmosphere_origin_at_height"));
        assert!(source.contains("atmosphere_ray_sphere_distance"));

        let sampling = crate::WGSL;
        assert!(sampling.contains("planet_center_world"));
        assert!(sampling.contains("dot(normalize(direction), local_up)"));
    }
}
