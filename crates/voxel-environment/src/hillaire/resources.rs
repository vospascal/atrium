//! GPU resources shared by the Jolifanto/Hillaire LUT passes and samplers.

use super::LutConfig;
use crate::FroxelCamera;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// The bind-group slot used by the atmosphere sampler in DDA and CAGI shaders.
pub const ATMOSPHERE_BIND_GROUP: u32 = 1;

/// The four persistent lookup tables used by Jolifanto's LUT path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LutKind {
    Transmittance,
    MultipleScattering,
    SkyView,
    AerialPerspective,
}

impl LutKind {
    pub const ALL: [Self; 4] = [
        Self::Transmittance,
        Self::MultipleScattering,
        Self::SkyView,
        Self::AerialPerspective,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::Transmittance => "transmittance",
            Self::MultipleScattering => "multiple scattering",
            Self::SkyView => "sky view",
            Self::AerialPerspective => "aerial perspective",
        }
    }
}

/// Parameters consumed by all four LUT compute passes.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq, Pod, Zeroable)]
pub struct AtmosphereUniform {
    /// Planet bottom radius in kilometres.
    pub bottom_radius_km: f32,
    /// Atmosphere top radius in kilometres.
    pub top_radius_km: f32,
    /// Jolifanto's `fromKilometersScale` conversion at the renderer boundary.
    pub from_kilometers_scale: f32,
    pub _pad0: f32,
    /// Direction from the sample toward the dominant sun.
    pub sun_direction: [f32; 3],
    pub _pad1: f32,
    /// Top-of-atmosphere sun illuminance in linear scene units.
    pub sun_illuminance: [f32; 3],
    pub _pad2: f32,
    /// Camera position in renderer world units.
    pub camera_position: [f32; 3],
    pub _pad3: f32,
    /// Sky-view LUT viewport in pixels.
    pub sky_view_size: [f32; 2],
    /// Explicit padding for WGSL's 16-byte alignment before the vec3 below.
    pub _pad_sky_view: [f32; 2],
    /// Aerial perspective LUT dimensions (x/y/z).
    pub aerial_size: [f32; 3],
    pub _pad4: f32,
    /// Camera-only sun direction and daylight amount for visual decoration.
    pub visual_sun: [f32; 4],
    /// Camera-only moon direction and phase.
    pub visual_moon: [f32; 4],
    /// Camera-only zenith radiance and star rotation.
    pub visual_zenith: [f32; 4],
    /// Camera-only horizon radiance and moonlight amount.
    pub visual_horizon: [f32; 4],
    /// Camera forward basis for froxel ray reconstruction.
    pub camera_forward: [f32; 3],
    pub _pad_camera_forward: f32,
    /// Camera right basis with FOV/aspect scaling already applied.
    pub camera_right_scaled: [f32; 3],
    pub _pad_camera_right: f32,
    /// Camera up basis with vertical-FOV scaling already applied.
    pub camera_up_scaled: [f32; 3],
    pub _pad_camera_up: f32,
    /// Froxel near/far distances in renderer world units.
    pub camera_depth: [f32; 4],
}

impl Default for AtmosphereUniform {
    fn default() -> Self {
        Self {
            bottom_radius_km: 6360.0,
            top_radius_km: 6460.0,
            from_kilometers_scale: 1000.0,
            _pad0: 0.0,
            sun_direction: [0.55, 0.8, 0.35],
            _pad1: 0.0,
            sun_illuminance: [2.2, 2.112, 1.936],
            _pad2: 0.0,
            camera_position: [0.0, 0.0, 0.0],
            _pad3: 0.0,
            sky_view_size: [192.0, 108.0],
            _pad_sky_view: [0.0; 2],
            aerial_size: [32.0, 32.0, 32.0],
            _pad4: 0.0,
            visual_sun: [0.55, 0.8, 0.35, 1.0],
            visual_moon: [-0.55, -0.8, -0.35, 0.85],
            visual_zenith: [0.08, 0.31, 2.55, 0.0],
            visual_horizon: [2.55, 1.37, 0.63, 0.0],
            camera_forward: [1.0, 0.0, 0.0],
            _pad_camera_forward: 0.0,
            camera_right_scaled: [0.57735026, 0.0, 0.0],
            _pad_camera_right: 0.0,
            camera_up_scaled: [0.0, 0.57735026, 0.0],
            _pad_camera_up: 0.0,
            camera_depth: [0.1, 32_000.0, 0.0, 0.0],
        }
    }
}

impl AtmosphereUniform {
    /// Apply camera-relative froxel projection data without exposing the GPU
    /// uniform layout to camera or platform code.
    pub fn set_froxel_camera(&mut self, camera: FroxelCamera) {
        self.camera_forward = camera.forward;
        self.camera_right_scaled = camera.right_scaled;
        self.camera_up_scaled = camera.up_scaled;
        self.camera_depth = [camera.near_world, camera.far_world, 0.0, 0.0];
    }
}

/// CPU-side resource state. The concrete `wgpu` textures and bind groups belong to the
/// renderer adapter; this type keeps invalidation and sizing independent of that API.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AtmosphereResources {
    pub lut_config: LutConfig,
    pub generation: u64,
}

/// Persistent textures and the sampling bind group consumed by renderer passes.
pub struct AtmosphereBindings {
    pub resources: AtmosphereResources,
    pub uniform: AtmosphereUniform,
    uniform_buffer: wgpu::Buffer,
    transmittance: wgpu::Texture,
    multiple_scattering: wgpu::Texture,
    sky_view: wgpu::Texture,
    aerial_perspective: wgpu::Texture,
    transmittance_view: wgpu::TextureView,
    multiple_scattering_view: wgpu::TextureView,
    sky_view_view: wgpu::TextureView,
    aerial_perspective_view: wgpu::TextureView,
    sample_bind_group_layout: wgpu::BindGroupLayout,
    sample_bind_group: wgpu::BindGroup,
}

impl AtmosphereBindings {
    pub fn new(device: &wgpu::Device, lut_config: LutConfig) -> Self {
        let uniform = AtmosphereUniform {
            sky_view_size: [lut_config.sky_view[0] as f32, lut_config.sky_view[1] as f32],
            aerial_size: [
                lut_config.aerial_perspective[0] as f32,
                lut_config.aerial_perspective[1] as f32,
                lut_config.aerial_perspective[2] as f32,
            ],
            ..AtmosphereUniform::default()
        };
        let texture_2d = |label: &str, size: [u32; 2]| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: 1,
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D2,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            })
        };
        let texture_3d = |label: &str, size: [u32; 3]| {
            device.create_texture(&wgpu::TextureDescriptor {
                label: Some(label),
                size: wgpu::Extent3d {
                    width: size[0],
                    height: size[1],
                    depth_or_array_layers: size[2],
                },
                mip_level_count: 1,
                sample_count: 1,
                dimension: wgpu::TextureDimension::D3,
                format: wgpu::TextureFormat::Rgba16Float,
                usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
                view_formats: &[],
            })
        };
        let transmittance = texture_2d("atmosphere transmittance LUT", lut_config.transmittance);
        let multiple_scattering = texture_2d(
            "atmosphere multiple-scattering LUT",
            lut_config.multiple_scattering,
        );
        let sky_view = texture_2d("atmosphere sky-view LUT", lut_config.sky_view);
        let aerial_perspective = texture_3d(
            "atmosphere aerial-perspective LUT",
            lut_config.aerial_perspective,
        );
        let view_2d = |texture: &wgpu::Texture| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D2),
                ..Default::default()
            })
        };
        let view_3d = |texture: &wgpu::Texture| {
            texture.create_view(&wgpu::TextureViewDescriptor {
                dimension: Some(wgpu::TextureViewDimension::D3),
                ..Default::default()
            })
        };
        let transmittance_view = view_2d(&transmittance);
        let multiple_scattering_view = view_2d(&multiple_scattering);
        let sky_view_view = view_2d(&sky_view);
        let aerial_perspective_view = view_3d(&aerial_perspective);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("atmosphere LUT sampler"),
            address_mode_u: wgpu::AddressMode::ClampToEdge,
            address_mode_v: wgpu::AddressMode::ClampToEdge,
            address_mode_w: wgpu::AddressMode::ClampToEdge,
            mag_filter: wgpu::FilterMode::Linear,
            min_filter: wgpu::FilterMode::Linear,
            mipmap_filter: wgpu::MipmapFilterMode::Nearest,
            ..Default::default()
        });
        let sample_bind_group_layout =
            device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
                label: Some("atmosphere sample bind group layout"),
                entries: &[
                    uniform_entry(0),
                    sampled_texture_entry(1, wgpu::TextureViewDimension::D2),
                    sampled_texture_entry(2, wgpu::TextureViewDimension::D2),
                    sampled_texture_entry(3, wgpu::TextureViewDimension::D2),
                    sampled_texture_entry(4, wgpu::TextureViewDimension::D3),
                    wgpu::BindGroupLayoutEntry {
                        binding: 5,
                        visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                ],
            });
        let uniform_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("atmosphere uniform"),
            contents: bytemuck::bytes_of(&uniform),
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
        });
        let sample_bind_group = device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("atmosphere sample bind group"),
            layout: &sample_bind_group_layout,
            entries: &[
                buffer_entry(0, &uniform_buffer),
                texture_entry(1, &transmittance_view),
                texture_entry(2, &multiple_scattering_view),
                texture_entry(3, &sky_view_view),
                texture_entry(4, &aerial_perspective_view),
                wgpu::BindGroupEntry {
                    binding: 5,
                    resource: wgpu::BindingResource::Sampler(&sampler),
                },
            ],
        });
        Self {
            resources: AtmosphereResources {
                lut_config,
                generation: 0,
            },
            uniform,
            uniform_buffer,
            transmittance,
            multiple_scattering,
            sky_view,
            aerial_perspective,
            transmittance_view,
            multiple_scattering_view,
            sky_view_view,
            aerial_perspective_view,
            sample_bind_group_layout,
            sample_bind_group,
        }
    }

    pub fn sample_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        &self.sample_bind_group_layout
    }

    pub fn sample_bind_group(&self) -> &wgpu::BindGroup {
        &self.sample_bind_group
    }

    pub(crate) fn uniform_buffer(&self) -> &wgpu::Buffer {
        &self.uniform_buffer
    }

    pub fn update_uniform(&mut self, queue: &wgpu::Queue, uniform: AtmosphereUniform) {
        self.uniform = uniform;
        self.resources.generation = self.resources.generation.wrapping_add(1);
        queue.write_buffer(&self.uniform_buffer, 0, bytemuck::bytes_of(&uniform));
    }

    pub fn texture(&self, kind: LutKind) -> &wgpu::Texture {
        match kind {
            LutKind::Transmittance => &self.transmittance,
            LutKind::MultipleScattering => &self.multiple_scattering,
            LutKind::SkyView => &self.sky_view,
            LutKind::AerialPerspective => &self.aerial_perspective,
        }
    }

    pub fn view(&self, kind: LutKind) -> &wgpu::TextureView {
        match kind {
            LutKind::Transmittance => &self.transmittance_view,
            LutKind::MultipleScattering => &self.multiple_scattering_view,
            LutKind::SkyView => &self.sky_view_view,
            LutKind::AerialPerspective => &self.aerial_perspective_view,
        }
    }
}

fn uniform_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Uniform,
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn sampled_texture_entry(
    binding: u32,
    dimension: wgpu::TextureViewDimension,
) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Texture {
            sample_type: wgpu::TextureSampleType::Float { filterable: true },
            view_dimension: dimension,
            multisampled: false,
        },
        count: None,
    }
}

fn buffer_entry<'a>(binding: u32, buffer: &'a wgpu::Buffer) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: buffer.as_entire_binding(),
    }
}

fn texture_entry<'a>(binding: u32, view: &'a wgpu::TextureView) -> wgpu::BindGroupEntry<'a> {
    wgpu::BindGroupEntry {
        binding,
        resource: wgpu::BindingResource::TextureView(view),
    }
}

impl Default for AtmosphereResources {
    fn default() -> Self {
        Self {
            lut_config: LutConfig::default(),
            generation: 0,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::AtmosphereUniform;

    #[test]
    fn atmosphere_uniform_matches_wgsl_alignment() {
        assert_eq!(std::mem::size_of::<AtmosphereUniform>(), 224);
        assert_eq!(std::mem::offset_of!(AtmosphereUniform, aerial_size), 80);
        assert_eq!(std::mem::offset_of!(AtmosphereUniform, visual_sun), 96);
        assert_eq!(std::mem::offset_of!(AtmosphereUniform, visual_horizon), 144);
        assert_eq!(std::mem::offset_of!(AtmosphereUniform, camera_forward), 160);
        assert_eq!(std::mem::offset_of!(AtmosphereUniform, camera_depth), 208);
    }
}
