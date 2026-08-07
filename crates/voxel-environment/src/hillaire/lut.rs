//! Jolifanto LUT dimensions and the compute passes that populate them.
//!
//! The *decision* about what needs recomputing is not here — it is
//! [`EnvironmentInvalidation`], stated in renderer-neutral terms. This file only maps
//! that answer onto which of the four tables it affects, which is the one part of it that
//! is genuinely Hillaire-specific: transmittance and multiple scattering depend on the
//! atmosphere and the sun, sky view and aerial perspective additionally depend on where
//! the camera is.

use super::resources::{AtmosphereBindings, LutKind};
use crate::api::EnvironmentInvalidation;

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

/// The compute pipelines that populate the persistent tables.
///
/// Seven, not four: the cloud deck adds an NDF, density field and shadow map. They sit here rather
/// than in a parallel type because they answer the same question this file exists to answer —
/// which tables did this frame's invalidation reach — and a second encoder pass would have had
/// to duplicate it.
pub struct AtmosphereLutPasses {
    transmittance: LutPipeline,
    multiple_scattering: LutPipeline,
    sky_view: LutPipeline,
    aerial_perspective: LutPipeline,
    /// Pure noise: generated once, invalidated by nothing.
    cloud_noise: LutPipeline,
    /// Downsampled 3D noise levels used by distance-based cloud LOD.
    cloud_noise_mips: Vec<LutPipeline>,
    /// World-scale Nubis Data Field, generated once until an authored asset is uploaded.
    cloud_ndf: LutPipeline,
    /// Redrawn whenever the deck or the sun moves — most frames while the wind blows.
    cloud_shadow: LutPipeline,
    /// Whether [`Self::encode`] has generated the density field yet.
    ///
    /// `Cell` rather than a flag on the caller because "the noise exists" is this type's own
    /// state, and `encode` takes `&self` to keep the adapter's borrow shape unchanged.
    cloud_noise_generated: std::cell::Cell<bool>,
    cloud_ndf_generated: std::cell::Cell<bool>,
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
        let uniform_layout_entry = wgpu::BindGroupLayoutEntry {
            binding: 0,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::Buffer {
                ty: wgpu::BufferBindingType::Uniform,
                has_dynamic_offset: false,
                min_binding_size: None,
            },
            count: None,
        };
        let storage_layout_entry = |format, view_dimension| wgpu::BindGroupLayoutEntry {
            binding: 1,
            visibility: wgpu::ShaderStages::COMPUTE,
            ty: wgpu::BindingType::StorageTexture {
                access: wgpu::StorageTextureAccess::WriteOnly,
                format,
                view_dimension,
            },
            count: None,
        };
        let make_with = |label: &str,
                         entry: &str,
                         kind: LutKind,
                         format,
                         dimension,
                         workgroups,
                         reads_density: bool| {
            let mut layout_entries = vec![
                uniform_layout_entry,
                storage_layout_entry(format, dimension),
            ];
            if reads_density {
                // The shadow pass integrates the density field, so unlike the four atmosphere
                // tables it needs a sampled texture as well as a write target.
                layout_entries.push(wgpu::BindGroupLayoutEntry {
                    binding: 2,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D3,
                        multisampled: false,
                    },
                    count: None,
                });
                layout_entries.push(wgpu::BindGroupLayoutEntry {
                    binding: 4,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: true },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                });
                layout_entries.push(wgpu::BindGroupLayoutEntry {
                    binding: 3,
                    visibility: wgpu::ShaderStages::COMPUTE,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                    count: None,
                });
            }
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some(label),
                entries: &layout_entries,
            });
            let density_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
                label: Some("cloud shadow density sampler"),
                address_mode_u: wgpu::AddressMode::Repeat,
                address_mode_v: wgpu::AddressMode::Repeat,
                address_mode_w: wgpu::AddressMode::Repeat,
                mag_filter: wgpu::FilterMode::Linear,
                min_filter: wgpu::FilterMode::Linear,
                mipmap_filter: wgpu::MipmapFilterMode::Nearest,
                ..Default::default()
            });
            let mut group_entries = vec![
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: bindings.uniform_buffer().as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(bindings.view(kind)),
                },
            ];
            if reads_density {
                group_entries.push(wgpu::BindGroupEntry {
                    binding: 2,
                    resource: wgpu::BindingResource::TextureView(
                        bindings.cloud_noise_sampling_view(),
                    ),
                });
                group_entries.push(wgpu::BindGroupEntry {
                    binding: 3,
                    resource: wgpu::BindingResource::Sampler(&density_sampler),
                });
                group_entries.push(wgpu::BindGroupEntry {
                    binding: 4,
                    resource: wgpu::BindingResource::TextureView(bindings.view(LutKind::CloudNdf)),
                });
            }
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some(label),
                layout: &layout,
                entries: &group_entries,
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
        let make = |label: &str, entry: &str, kind: LutKind, dimension, workgroups| {
            make_with(
                label,
                entry,
                kind,
                wgpu::TextureFormat::Rgba16Float,
                dimension,
                workgroups,
                false,
            )
        };
        let make_noise_mip = |level: u32| {
            let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("cloud noise mip layout"),
                entries: &[
                    uniform_layout_entry,
                    storage_layout_entry(
                        wgpu::TextureFormat::Rgba8Unorm,
                        wgpu::TextureViewDimension::D3,
                    ),
                    wgpu::BindGroupLayoutEntry {
                        binding: 2,
                        visibility: wgpu::ShaderStages::COMPUTE,
                        ty: wgpu::BindingType::Texture {
                            sample_type: wgpu::TextureSampleType::Float { filterable: true },
                            view_dimension: wgpu::TextureViewDimension::D3,
                            multisampled: false,
                        },
                        count: None,
                    },
                ],
            });
            let bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
                label: Some("cloud noise mip bind group"),
                layout: &layout,
                entries: &[
                    wgpu::BindGroupEntry {
                        binding: 0,
                        resource: bindings.uniform_buffer().as_entire_binding(),
                    },
                    wgpu::BindGroupEntry {
                        binding: 1,
                        resource: wgpu::BindingResource::TextureView(
                            bindings.cloud_noise_mip_view(level),
                        ),
                    },
                    wgpu::BindGroupEntry {
                        binding: 2,
                        resource: wgpu::BindingResource::TextureView(
                            bindings.cloud_noise_mip_view(level - 1),
                        ),
                    },
                ],
            });
            let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
                label: Some("cloud noise mip pipeline layout"),
                bind_group_layouts: &[Some(&layout)],
                immediate_size: 0,
            });
            let pipeline = device.create_compute_pipeline(&wgpu::ComputePipelineDescriptor {
                label: Some("cloud noise mip pipeline"),
                layout: Some(&pipeline_layout),
                module: &shader,
                entry_point: Some("cloud_noise_mip_main"),
                compilation_options: Default::default(),
                cache: None,
            });
            let edge = super::resources::CLOUD_NOISE_EDGE >> level;
            LutPipeline {
                pipeline,
                bind_group,
                workgroups: [edge / 4, edge / 4, edge / 4],
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
            // 128^3 at 4^3 per workgroup. Rgba8Unorm because density is four values in 0..1.
            cloud_noise: make_with(
                "cloud density field pass",
                "cloud_noise_main",
                LutKind::CloudNoise,
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureViewDimension::D3,
                [
                    super::resources::CLOUD_NOISE_EDGE / 4,
                    super::resources::CLOUD_NOISE_EDGE / 4,
                    super::resources::CLOUD_NOISE_EDGE / 4,
                ],
                false,
            ),
            cloud_noise_mips: (1..super::resources::CLOUD_NOISE_MIP_LEVELS)
                .map(make_noise_mip)
                .collect(),
            cloud_ndf: make_with(
                "cloud Nubis Data Field pass",
                "cloud_ndf_main",
                LutKind::CloudNdf,
                wgpu::TextureFormat::Rgba8Unorm,
                wgpu::TextureViewDimension::D2,
                [
                    super::resources::CLOUD_NDF_EDGE / 8,
                    super::resources::CLOUD_NDF_EDGE / 8,
                    1,
                ],
                false,
            ),
            // 512^2 at 8x8 per workgroup, and the one pass that reads the density field.
            cloud_shadow: make_with(
                "cloud shadow map pass",
                "cloud_shadow_main",
                LutKind::CloudShadow,
                wgpu::TextureFormat::Rgba16Float,
                wgpu::TextureViewDimension::D2,
                [
                    super::resources::CLOUD_SHADOW_EDGE / 8,
                    super::resources::CLOUD_SHADOW_EDGE / 8,
                    1,
                ],
                true,
            ),
            cloud_noise_generated: std::cell::Cell::new(false),
            cloud_ndf_generated: std::cell::Cell::new(false),
        }
    }

    /// Encode only the LUTs this frame's invalidation reaches.
    pub fn encode(
        &self,
        encoder: &mut wgpu::CommandEncoder,
        invalidation: EnvironmentInvalidation,
    ) {
        let mut pass = encoder.begin_compute_pass(&wgpu::ComputePassDescriptor {
            label: Some("jolifanto atmosphere LUT generation"),
            timestamp_writes: None,
        });
        if invalidation.view_independent() {
            dispatch(&mut pass, &self.transmittance);
            dispatch(&mut pass, &self.multiple_scattering);
        }
        if invalidation.view_dependent() {
            dispatch(&mut pass, &self.sky_view);
            dispatch(&mut pass, &self.aerial_perspective);
        }
        // The density field is pure noise: nothing in a frame can invalidate it, so it is
        // generated exactly once. Regenerating it per frame would be 2 M texels of Worley for a
        // result that never changes, and it is the one table with no invalidation bit.
        if !self.cloud_noise_generated.get() {
            dispatch(&mut pass, &self.cloud_noise);
            for mip in &self.cloud_noise_mips {
                dispatch(&mut pass, mip);
            }
            self.cloud_noise_generated.set(true);
        }
        if !self.cloud_ndf_generated.get() {
            dispatch(&mut pass, &self.cloud_ndf);
            self.cloud_ndf_generated.set(true);
        }
        // The shadow map depends on the deck AND the active direct light, but NOT the camera — it is
        // world-anchored, so turning the head must not redraw it. That is exactly
        // `cloud_dependent`, and it is why `weather` is its own bit rather than folded into
        // `sun`: the wind moves the deck every frame, and folding it in would re-integrate the
        // transmittance table every frame with it.
        if invalidation.cloud_dependent() {
            dispatch(&mut pass, &self.cloud_shadow);
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

    /// The sampling module must parse on its own too.
    ///
    /// It is spliced into a consumer's shader, so a syntax error here surfaces as a compile
    /// failure pointing at `voxel-rt` — the crate that did nothing wrong. Parsing it in this
    /// crate's own tests is what keeps the blame local, and it matters more now that the module
    /// carries a cloud marcher rather than only LUT reads.
    #[test]
    fn the_sampling_module_parses_with_naga() {
        naga::front::wgsl::parse_str(super::super::shaders::WGSL)
            .expect("environment sampling WGSL must parse with naga");
    }

    /// The sky-view LUT's write mapping must be the exact inverse of its read mapping.
    ///
    /// Generation lives in `lut/common.wgsl` (`atmosphere_view_direction`, texel -> direction) and
    /// reading in `environment/hillaire.wgsl` (`atmosphere_sky_view_uv`, direction -> texel). Two
    /// functions in two files, in two different WGSL modules, that must agree — and nothing checked
    /// that they did.
    ///
    /// They did not. Generation used `azimuth = uv.x * TAU` while the reader used
    /// `atan2(z, x) / TAU + 0.5`, so every sky lookup was **rotated 180 degrees in azimuth**. The
    /// sunward glow was written sunward and read anti-sunward. It was easy to miss while the
    /// authored backdrop hid most of the physical sky, but it is what lit the clouds, which is
    /// why they stayed grey under an orange sunset while the sun moved.
    ///
    /// Ported to Rust rather than asserted as a string match: the defect was a half-texture offset,
    /// and only a round trip catches that.
    #[test]
    fn the_sky_view_lut_round_trips_direction_through_its_own_uv() {
        use std::f32::consts::TAU;

        // `atmosphere_sky_view_uv` — the reader, in environment/hillaire.wgsl.
        let to_uv = |direction: [f32; 3]| -> [f32; 2] {
            let azimuth = direction[2].atan2(direction[0]) / TAU + 0.5;
            [azimuth, direction[1] * 0.5 + 0.5]
        };
        // `atmosphere_view_direction` — the writer, in lut/common.wgsl.
        let to_direction = |uv: [f32; 2]| -> [f32; 3] {
            let elevation = uv[1] * 2.0 - 1.0;
            let horizontal = (1.0f32 - elevation * elevation).max(0.0).sqrt();
            let azimuth = (uv[0] - 0.5) * TAU;
            [
                azimuth.cos() * horizontal,
                elevation,
                azimuth.sin() * horizontal,
            ]
        };

        // Sweep the sphere, deliberately including the sunward and anti-sunward horizons that
        // `cloud_ambient_light` reads, and the poles where the horizontal term collapses.
        for azimuth_step in 0..16 {
            for elevation_step in 0..9 {
                let azimuth = (azimuth_step as f32 / 16.0) * TAU - std::f32::consts::PI;
                let elevation = elevation_step as f32 / 8.0 * 2.0 - 1.0;
                let horizontal = (1.0f32 - elevation * elevation).max(0.0).sqrt();
                let direction = [
                    azimuth.cos() * horizontal,
                    elevation,
                    azimuth.sin() * horizontal,
                ];
                let recovered = to_direction(to_uv(direction));
                // At the poles the azimuth is degenerate, so compare directions, not angles.
                let error = (0..3)
                    .map(|axis| (recovered[axis] - direction[axis]).abs())
                    .fold(0.0f32, f32::max);
                assert!(
                    error < 1.0e-4,
                    "direction {direction:?} round-tripped to {recovered:?} (error {error}): the \
                     sky-view LUT is written and read with different parameterizations"
                );
            }
        }

        // And pin the pairing in the shader text, so moving one without the other fails here rather
        // than in the image.
        assert!(
            super::super::shaders::LUT_WGSL.contains("let azimuth = (uv.x - 0.5) * 2.0 * PI;"),
            "the sky-view write mapping must stay centred on uv.x = 0.5"
        );
        assert!(
            super::super::shaders::WGSL
                .contains("atan2(direction.z, direction.x) / (2.0 * 3.141592653589793) + 0.5"),
            "the sky-view read mapping must stay the inverse of the write mapping"
        );
    }

    #[test]
    fn lut_contract_pins_jolifanto_scale_and_local_up_parameterization() {
        let source = super::super::shaders::LUT_WGSL;
        assert!(source.contains("from_kilometers_scale"));
        assert!(source.contains("atmosphere_origin_at_height"));
        assert!(source.contains("atmosphere_ray_sphere_distance"));

        let sampling = super::super::shaders::WGSL;
        assert!(sampling.contains("planet_center_world"));
        assert!(sampling.contains("dot(normalize(direction), local_up)"));
    }
}
