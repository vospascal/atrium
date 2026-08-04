//! GPU resources shared by the Jolifanto/Hillaire LUT passes and samplers.

use super::LutConfig;
use crate::api::{EnvironmentRequest, FroxelCamera};
use crate::scale::FROM_KILOMETERS_SCALE;

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt;

/// Jolifanto's Earth-like planet surface radius, in kilometres.
pub const EARTH_BOTTOM_RADIUS_KM: f32 = 6360.0;
/// Jolifanto's Earth-like atmosphere top radius, in kilometres — 100 km of air.
pub const EARTH_TOP_RADIUS_KM: f32 = 6460.0;

/// The four persistent lookup tables used by Jolifanto's LUT path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LutKind {
    Transmittance,
    MultipleScattering,
    SkyView,
    AerialPerspective,
}

impl LutKind {}

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
        Self::new(LutConfig::default(), &EnvironmentRequest::default())
    }
}

impl AtmosphereUniform {
    /// Build the uniform from the two things that determine it: the LUT sizes chosen at
    /// allocation, and the frame the renderer asked for.
    ///
    /// Assembled from [`Zeroable::zeroed`] rather than written out field by field, which
    /// zeroes the six `_pad*` members for free and — more importantly — means the planet
    /// constants, the LUT dimensions and the per-frame values each appear exactly once in
    /// the crate. The previous hand-written `Default` restated Jolifanto's LUT sizes and
    /// the whole default frame, so three lists had to be kept in step by hand.
    pub fn new(lut_config: LutConfig, request: &EnvironmentRequest) -> Self {
        let mut uniform = Self::zeroed();
        uniform.bottom_radius_km = EARTH_BOTTOM_RADIUS_KM;
        uniform.top_radius_km = EARTH_TOP_RADIUS_KM;
        uniform.from_kilometers_scale = FROM_KILOMETERS_SCALE;
        uniform.set_lut_config(lut_config);
        uniform.apply_request(request);
        uniform
    }

    /// Record the allocated LUT dimensions the compute passes index against.
    pub fn set_lut_config(&mut self, lut_config: LutConfig) {
        self.sky_view_size = [lut_config.sky_view[0] as f32, lut_config.sky_view[1] as f32];
        self.aerial_size = [
            lut_config.aerial_perspective[0] as f32,
            lut_config.aerial_perspective[1] as f32,
            lut_config.aerial_perspective[2] as f32,
        ];
    }

    /// Overwrite every per-frame field from the renderer's request, leaving the fields
    /// fixed at construction (planet radii, world scale, LUT dimensions) untouched.
    ///
    /// This is the *only* place the GPU uniform layout meets renderer-supplied values, and
    /// keeping it to one function is what lets [`EnvironmentRequest`] be the boundary: a
    /// field added to the request without a line here is a field that silently does
    /// nothing, and it is visible in one place rather than spread across a call site.
    pub fn apply_request(&mut self, request: &EnvironmentRequest) {
        self.sun_direction = request.sun_direction;
        self.sun_illuminance = request.sun_illuminance;
        self.visual_sun = request.celestial_sun;
        self.visual_moon = request.celestial_moon;
        self.visual_zenith = request.sky_zenith;
        self.visual_horizon = request.sky_horizon;
        self.camera_position = request.camera_position;
        self.set_froxel_camera(request.camera);
    }

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
///
/// Only the `TextureView`s are held: a view keeps its texture's GPU resource alive on its own,
/// and the `Texture` handles were kept solely for an introspection accessor that had no
/// callers. A LUT dump would want them back — three lines, when something actually needs one.
pub struct AtmosphereBindings {
    pub resources: AtmosphereResources,
    pub uniform: AtmosphereUniform,
    uniform_buffer: wgpu::Buffer,
    transmittance_view: wgpu::TextureView,
    multiple_scattering_view: wgpu::TextureView,
    sky_view_view: wgpu::TextureView,
    aerial_perspective_view: wgpu::TextureView,
    sample_bind_group_layout: wgpu::BindGroupLayout,
    sample_bind_group: wgpu::BindGroup,
}

impl AtmosphereBindings {
    pub fn new(device: &wgpu::Device, lut_config: LutConfig) -> Self {
        let uniform = AtmosphereUniform::new(lut_config, &EnvironmentRequest::default());
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
    use super::*;
    use crate::api::FroxelCamera;

    /// Every per-frame field of the request must reach the uniform. Written as a whole-
    /// struct comparison rather than field-by-field asserts so that adding a field to
    /// [`EnvironmentRequest`] and forgetting the line in `apply_request` fails here — the
    /// failure mode is a value that silently never reaches the GPU, which no image makes
    /// obvious.
    #[test]
    fn apply_request_carries_every_per_frame_field() {
        let request = EnvironmentRequest {
            sun_direction: [0.1, 0.2, 0.3],
            sun_illuminance: [4.0, 5.0, 6.0],
            celestial_sun: [0.7, 0.8, 0.9, 0.25],
            celestial_moon: [-0.7, -0.8, -0.9, 0.5],
            sky_zenith: [1.5, 2.5, 3.5, 1.25],
            sky_horizon: [2.0, 1.0, 0.5, 0.75],
            camera_position: [10.0, 20.0, 30.0],
            camera: FroxelCamera {
                forward: [0.0, 0.0, 1.0],
                right_scaled: [1.0, 0.0, 0.0],
                up_scaled: [0.0, 1.0, 0.0],
                near_world: 0.5,
                far_world: 4096.0,
            },
        };
        let mut uniform = AtmosphereUniform::default();
        uniform.apply_request(&request);
        assert_eq!(
            uniform,
            AtmosphereUniform::new(LutConfig::default(), &request)
        );
    }

    /// The fields fixed at allocation must survive every frame update. A request that
    /// reset the planet radii or the world scale would put the LUT domains and the
    /// sampling code into different coordinate systems.
    #[test]
    fn applying_a_request_leaves_the_allocation_time_fields_alone() {
        let config = LutConfig {
            sky_view: [96, 54],
            ..LutConfig::default()
        };
        let mut uniform = AtmosphereUniform::new(config, &EnvironmentRequest::default());
        uniform.apply_request(&EnvironmentRequest {
            camera_position: [1.0, 2.0, 3.0],
            ..EnvironmentRequest::default()
        });
        assert_eq!(uniform.bottom_radius_km, EARTH_BOTTOM_RADIUS_KM);
        assert_eq!(uniform.top_radius_km, EARTH_TOP_RADIUS_KM);
        assert_eq!(uniform.from_kilometers_scale, FROM_KILOMETERS_SCALE);
        assert_eq!(uniform.sky_view_size, [96.0, 54.0]);
    }

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
