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

/// Edge length of the tileable cloud density field.
///
/// One texture, four channels, sampled at several frequencies for FBM detail — rather than a
/// low-frequency base plus a separate high-frequency erosion volume. `Rgba8Unorm` at 128³ is
/// 8 MB, and the alternative pair costs more memory *and* an extra fetch chain per sample.
pub const CLOUD_NOISE_EDGE: u32 = 128;

/// Number of 3D noise mip levels, including the authored 128³ level.
pub const CLOUD_NOISE_MIP_LEVELS: u32 = 6;

/// Edge length of the generated/authored Nubis Data Field.
pub const CLOUD_NDF_EDGE: u32 = 256;

/// World-space width and depth represented by the sky NDF, in metres.
///
/// Evolved's sky NDF is a 16 km x 16 km map. Keeping this scale explicit is what makes the
/// macro-weather field independent from the 128^3 up-res noise tile.
pub const CLOUD_NDF_EXTENT_WORLD: f32 = 16_384.0;

/// Resolution of the top-down cloud shadow map.
pub const CLOUD_SHADOW_EDGE: u32 = 512;

/// World-space edge length the shadow map covers, in METRES, centred on the camera column.
///
/// The map answers "how much sun reaches this ground point", so its extent bounds how far a cloud
/// shadow can be *seen* — not how far a cloud can be. Beyond it the lookup clamps to unshadowed,
/// which reads as distant sunlit ground rather than as an edge.
///
/// Sized against the world, not chosen: 512 m across [`CLOUD_SHADOW_EDGE`] = 512 texels is exactly
/// **one metre per texel**, i.e. one texel per world voxel, over a world that is 125 m across. The
/// first value here was 4096 m — 8 m per texel, so the entire world spanned about fifteen texels and
/// the ground received a smooth wash instead of cloud shadows. The feature was structurally right
/// and numerically useless, which no test caught because nothing related this constant to the
/// world's actual size. `cloud_shadow_extent_resolves_a_world_voxel_per_texel` now does.
pub const CLOUD_SHADOW_EXTENT_WORLD: f32 = 512.0;

/// The persistent lookup tables: Jolifanto's four, plus the NDF, noise field and shadow map the
/// cloud deck adds.
///
/// Cloud tables share this enum rather than getting their own because they share everything
/// that matters — the same uniform, the same bind group, the same "which of these did this
/// frame invalidate" question. A parallel type would have duplicated all three.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LutKind {
    Transmittance,
    MultipleScattering,
    SkyView,
    AerialPerspective,
    /// Tileable Perlin–Worley density field. Generated once; independent of sun and weather.
    CloudNoise,
    /// Nubis Data Field: world-scale 2D coverage/type/density layout.
    CloudNdf,
    /// Top-down transmittance of the deck along the active direct-light axis.
    CloudShadow,
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
    pub pad_a: f32,
    /// Direction from the sample toward the dominant sun.
    pub sun_direction: [f32; 3],
    pub pad_b: f32,
    /// Top-of-atmosphere sun illuminance in linear scene units.
    pub sun_illuminance: [f32; 3],
    pub pad_c: f32,
    /// Direction from a sample toward the physical moon.
    pub moon_direction: [f32; 3],
    pub pad_moon_direction: f32,
    /// Top-of-atmosphere moon illuminance in linear scene units.
    pub moon_illuminance: [f32; 3],
    pub pad_moon_illuminance: f32,
    /// Camera position in renderer world units.
    pub camera_position: [f32; 3],
    pub pad_d: f32,
    /// Sky-view LUT viewport in pixels.
    pub sky_view_size: [f32; 2],
    /// Explicit padding for WGSL's 16-byte alignment before the vec3 below.
    pub _pad_sky_view: [f32; 2],
    /// Aerial perspective LUT dimensions (x/y/z).
    pub aerial_size: [f32; 3],
    /// Scale for the diffuse hemisphere term — [`EnvironmentRequest::ambient_scale`].
    ///
    /// Deliberately placed in what was `_pad4`, the alignment slack after `aerial_size`. The
    /// uniform's size and every other field's offset are therefore unchanged, so this carries
    /// no ABI risk; a padding lane doing real work is what padding is for.
    pub ambient_scale: f32,
    /// Celestial presentation sun direction and daylight amount.
    pub visual_sun: [f32; 4],
    /// Celestial presentation moon direction and phase.
    pub visual_moon: [f32; 4],
    /// Compatibility zenith metadata and star rotation.
    pub visual_zenith: [f32; 4],
    /// Compatibility horizon metadata and moonlight amount.
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
    /// Deck geometry and optics: coverage, extinction σₜ, albedo σₛ/σₜ, deck bottom.
    pub cloud_shape: [f32; 4],
    /// Deck thickness, cloud type, detail-erosion strength, ambient extinction.
    pub cloud_detail: [f32; 4],
    /// Powder strength, forward HG eccentricity, back HG eccentricity, density scale.
    pub cloud_scatter: [f32; 4],
    /// Wind-advected deck offset (`xyz`) and primary step count (`w`, zero disables).
    pub cloud_wind: [f32; 4],
    /// Light-march tap count (`x`), shadow-map world extent (`y`), shadow texel count
    /// (`z`), weather-map variation (`w`).
    pub cloud_march: [f32; 4],
    /// Order-1 SH of upward ground radiance: `[0]` constant, `[1..4]` linear x/y/z.
    ///
    /// `xyz` is RGB, `w` is padding — so the WGSL side reads a plain
    /// `array<vec4<f32>, 4>` with no packing rules of its own.
    pub ground_bounce_sh: [[f32; 4]; 4],
    /// Weather-map channels used by the cloud density model: x variation, y precipitation.
    pub cloud_weather: [f32; 4],
    /// Direction of the active direct light (sun by day, moon by night).
    pub active_light_direction: [f32; 3],
    pub pad_active_light_direction: f32,
    /// Illuminance paired with `active_light_direction`; w is 1 for solar lighting and 0 for
    /// moonlight.
    pub active_light_illuminance: [f32; 3],
    pub active_light_is_sun: f32,
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
        self.cloud_march[2] = CLOUD_SHADOW_EDGE as f32;
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
        self.moon_direction = request.moon_direction;
        self.moon_illuminance = request.moon_illuminance;
        self.active_light_direction = request.active_light_direction;
        self.active_light_illuminance = request.active_light_illuminance;
        self.active_light_is_sun = request.active_light_is_sun;
        self.visual_sun = request.celestial_sun;
        self.visual_moon = request.celestial_moon;
        self.visual_zenith = request.sky_zenith;
        self.visual_horizon = request.sky_horizon;
        self.camera_position = request.camera_position;
        self.ambient_scale = request.ambient_scale;
        self.set_froxel_camera(request.camera);
        self.set_clouds(&request.clouds);
    }

    /// Apply the cloud deck. Separate from the sun fields it sits beside, because a deck
    /// change and a light change invalidate different things.
    pub fn set_clouds(&mut self, clouds: &crate::clouds::CloudRequest) {
        self.cloud_shape = [
            clouds.coverage,
            clouds.extinction,
            clouds.albedo,
            clouds.bottom_world,
        ];
        self.cloud_detail = [
            clouds.thickness_world,
            clouds.cloud_type,
            clouds.detail_strength,
            clouds.ambient_density,
        ];
        self.cloud_scatter = [
            clouds.powder_strength,
            clouds.forward_scatter,
            clouds.back_scatter,
            clouds.density_scale,
        ];
        self.cloud_wind = [
            clouds.wind_offset[0],
            clouds.wind_offset[1],
            clouds.wind_offset[2],
            clouds.primary_steps as f32,
        ];
        self.cloud_march = [
            clouds.light_steps as f32,
            CLOUD_SHADOW_EXTENT_WORLD,
            self.cloud_march[2],
            clouds.weather_variation,
        ];
        self.ground_bounce_sh = clouds.ground_bounce_sh;
        self.cloud_weather = [clouds.weather_variation, clouds.precipitation, 0.0, 0.0];
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
    cloud_noise_view: wgpu::TextureView,
    cloud_noise_mip_views: Vec<wgpu::TextureView>,
    cloud_ndf_view: wgpu::TextureView,
    cloud_shadow_view: wgpu::TextureView,
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
        // Rgba8Unorm rather than the LUTs' Rgba16Float: density is four values in 0..1, so
        // half floats would double the 8 MB for range this field cannot use.
        let cloud_noise = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cloud density field"),
            size: wgpu::Extent3d {
                width: CLOUD_NOISE_EDGE,
                height: CLOUD_NOISE_EDGE,
                depth_or_array_layers: CLOUD_NOISE_EDGE,
            },
            mip_level_count: CLOUD_NOISE_MIP_LEVELS,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D3,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let cloud_ndf = device.create_texture(&wgpu::TextureDescriptor {
            label: Some("cloud Nubis Data Field"),
            size: wgpu::Extent3d {
                width: CLOUD_NDF_EDGE,
                height: CLOUD_NDF_EDGE,
                depth_or_array_layers: 1,
            },
            mip_level_count: 1,
            sample_count: 1,
            dimension: wgpu::TextureDimension::D2,
            format: wgpu::TextureFormat::Rgba8Unorm,
            usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::STORAGE_BINDING,
            view_formats: &[],
        });
        let cloud_shadow = texture_2d("cloud shadow map", [CLOUD_SHADOW_EDGE, CLOUD_SHADOW_EDGE]);
        let transmittance_view = view_2d(&transmittance);
        let multiple_scattering_view = view_2d(&multiple_scattering);
        let sky_view_view = view_2d(&sky_view);
        let aerial_perspective_view = view_3d(&aerial_perspective);
        let cloud_noise_view = view_3d(&cloud_noise);
        let cloud_noise_mip_views = (0..CLOUD_NOISE_MIP_LEVELS)
            .map(|level| {
                cloud_noise.create_view(&wgpu::TextureViewDescriptor {
                    dimension: Some(wgpu::TextureViewDimension::D3),
                    base_mip_level: level,
                    mip_level_count: Some(1),
                    ..Default::default()
                })
            })
            .collect::<Vec<_>>();
        let cloud_ndf_view = view_2d(&cloud_ndf);
        let cloud_shadow_view = view_2d(&cloud_shadow);
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
        // A second sampler purely for the address mode: the density field is *tileable* and
        // is sampled at several frequencies, so it must repeat. Clamping it — which the LUT
        // sampler does, correctly, for tables indexed by angle — would smear the field's edge
        // texels across the whole sky.
        let cloud_sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("cloud density sampler"),
            address_mode_u: wgpu::AddressMode::Repeat,
            address_mode_v: wgpu::AddressMode::Repeat,
            address_mode_w: wgpu::AddressMode::Repeat,
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
                    sampled_texture_entry(6, wgpu::TextureViewDimension::D3),
                    sampled_texture_entry(7, wgpu::TextureViewDimension::D2),
                    wgpu::BindGroupLayoutEntry {
                        binding: 8,
                        visibility: wgpu::ShaderStages::COMPUTE | wgpu::ShaderStages::FRAGMENT,
                        ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::Filtering),
                        count: None,
                    },
                    sampled_texture_entry(9, wgpu::TextureViewDimension::D2),
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
                texture_entry(6, &cloud_noise_view),
                texture_entry(7, &cloud_shadow_view),
                wgpu::BindGroupEntry {
                    binding: 8,
                    resource: wgpu::BindingResource::Sampler(&cloud_sampler),
                },
                texture_entry(9, &cloud_ndf_view),
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
            cloud_noise_view,
            cloud_noise_mip_views,
            cloud_ndf_view,
            cloud_shadow_view,
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

    pub(crate) fn cloud_noise_mip_view(&self, level: u32) -> &wgpu::TextureView {
        &self.cloud_noise_mip_views[level as usize]
    }

    pub(crate) fn cloud_noise_sampling_view(&self) -> &wgpu::TextureView {
        &self.cloud_noise_view
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
            LutKind::CloudNoise => &self.cloud_noise_mip_views[0],
            LutKind::CloudNdf => &self.cloud_ndf_view,
            LutKind::CloudShadow => &self.cloud_shadow_view,
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
            moon_direction: [-0.3, 0.6, -0.7],
            moon_illuminance: [0.4, 0.3, 0.2],
            active_light_direction: [-0.1, 0.4, 0.8],
            active_light_illuminance: [0.4, 0.5, 0.6],
            active_light_is_sun: 0.0,
            celestial_sun: [0.7, 0.8, 0.9, 0.25],
            celestial_moon: [-0.7, -0.8, -0.9, 0.5],
            sky_zenith: [1.5, 2.5, 3.5, 1.25],
            sky_horizon: [2.0, 1.0, 0.5, 0.75],
            camera_position: [10.0, 20.0, 30.0],
            ambient_scale: 0.625,
            camera: FroxelCamera {
                forward: [0.0, 0.0, 1.0],
                right_scaled: [1.0, 0.0, 0.0],
                up_scaled: [0.0, 1.0, 0.0],
                near_world: 0.5,
                far_world: 4096.0,
            },
            // Deliberately none of the defaults, so a cloud field that never reaches
            // `set_clouds` fails here rather than rendering a deck that ignores its settings.
            clouds: crate::clouds::CloudSettings {
                coverage: 0.7,
                extinction: 0.11,
                albedo: 0.8,
                bottom_world: 300.0,
                thickness_world: 250.0,
                cloud_type: 0.25,
                detail_strength: 0.9,
                ambient_density: 0.5,
                powder_strength: 0.4,
                forward_scatter: 0.6,
                back_scatter: -0.4,
                primary_steps: 32,
                light_steps: 4,
                wind_offset: [11.0, 0.0, -7.0],
                ..crate::clouds::CloudSettings::default()
            }
            .request(),
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
        assert_eq!(std::mem::size_of::<AtmosphereUniform>(), 448);
        assert_eq!(std::mem::offset_of!(AtmosphereUniform, aerial_size), 112);
        assert_eq!(std::mem::offset_of!(AtmosphereUniform, visual_sun), 128);
        assert_eq!(std::mem::offset_of!(AtmosphereUniform, visual_horizon), 176);
        assert_eq!(std::mem::offset_of!(AtmosphereUniform, camera_forward), 192);
        assert_eq!(std::mem::offset_of!(AtmosphereUniform, camera_depth), 240);
        // The physical moon source sits beside the sun, so the appearance and cloud blocks
        // move together with the WGSL declarations in both sampling and LUT modules.
        assert_eq!(std::mem::offset_of!(AtmosphereUniform, cloud_shape), 256);
        assert_eq!(std::mem::offset_of!(AtmosphereUniform, cloud_march), 320);
        assert_eq!(
            std::mem::offset_of!(AtmosphereUniform, ground_bounce_sh),
            336
        );
        assert_eq!(std::mem::offset_of!(AtmosphereUniform, cloud_weather), 400);
        assert_eq!(
            std::mem::offset_of!(AtmosphereUniform, active_light_direction),
            416
        );
    }

    /// Both WGSL modules declare this uniform, and the shadow-map pass reads the cloud block
    /// through the LUT module's copy. If the two structs drift, the compute pass silently
    /// misreads every cloud field — no validation error, just wrong shadows.
    #[test]
    fn both_wgsl_modules_declare_the_same_cloud_block() {
        let sampling = crate::hillaire::shaders::WGSL;
        let lut = crate::hillaire::shaders::LUT_WGSL;
        for field in [
            "cloud_shape: vec4<f32>",
            "cloud_detail: vec4<f32>",
            "cloud_scatter: vec4<f32>",
            "cloud_wind: vec4<f32>",
            "cloud_march: vec4<f32>",
            "ground_bounce_sh: array<vec4<f32>, 4>",
        ] {
            assert!(sampling.contains(field), "sampling module missing {field}");
            assert!(lut.contains(field), "LUT module missing {field}");
        }
    }

    /// The Nubis cascade must be present in both consumers of the density model: the camera
    /// march and the cloud-shadow compute pass. A future asset-backed modeling field may replace
    /// the procedural fallback, but it must still flow through the same profile → up-res → density
    /// stage or the visible deck and its shadow will diverge.
    #[test]
    fn both_wgsl_consumers_use_the_nubis_density_cascade() {
        let sampling = crate::hillaire::shaders::WGSL;
        let lut = crate::hillaire::shaders::LUT_WGSL;
        for stage in [
            "fn cloud_ndf_at",
            "fn cloud_modeling_profile",
            "fn cloud_upres_noise",
            "fn cloud_value_erosion",
            "fn cloud_evolved_high_frequency_noise",
            "fn cloud_evolved_density",
        ] {
            assert!(sampling.contains(stage), "sampling module missing {stage}");
            assert!(lut.contains(stage), "LUT module missing {stage}");
        }
    }
}
