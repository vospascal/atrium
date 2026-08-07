// Shared environment bindings.
//
// The uniform layout and the `@group(1)` slots every environment implementation reads.
// Nothing here samples or shades: `hillaire.wgsl` owns the physical LUT reads,
// `hillaire.wgsl` owns physical atmosphere reads, `appearance.wgsl` adds resolved celestial
// sources, and `dispatch.wgsl` owns the four entry points the renderer actually calls.
//
// This struct's Rust counterpart is `AtmosphereUniform`, and
// `atmosphere_uniform_matches_wgsl_alignment` pins the offsets. Adding a field here
// without adding it there silently misreads every field after it.

struct AtmosphereUniform {
    bottom_radius_km: f32,
    top_radius_km: f32,
    from_kilometers_scale: f32,
    pad_a: f32,
    sun_direction: vec3<f32>,
    pad_b: f32,
    sun_illuminance: vec3<f32>,
    pad_c: f32,
    moon_direction: vec3<f32>,
    pad_moon_direction: f32,
    moon_illuminance: vec3<f32>,
    pad_moon_illuminance: f32,
    camera_position: vec3<f32>,
    pad_d: f32,
    sky_view_size: vec2<f32>,
    aerial_size: vec3<f32>,
    ambient_scale: f32,
    visual_sun: vec4<f32>,
    visual_moon: vec4<f32>,
    visual_zenith: vec4<f32>,
    visual_horizon: vec4<f32>,
    camera_forward: vec3<f32>,
    _pad_camera_forward: f32,
    camera_right_scaled: vec3<f32>,
    _pad_camera_right: f32,
    camera_up_scaled: vec3<f32>,
    _pad_camera_up: f32,
    camera_depth: vec4<f32>,
    // coverage, extinction sigma_t, albedo sigma_s/sigma_t, deck bottom (world)
    cloud_shape: vec4<f32>,
    // deck thickness (world), cloud type, detail erosion, ambient extinction
    cloud_detail: vec4<f32>,
    // powder strength, forward HG g, back HG g, density scale
    cloud_scatter: vec4<f32>,
    // wind offset xyz, primary step count (0 disables the march)
    cloud_wind: vec4<f32>,
    // light taps, shadow world extent, shadow texel count, weather variation
    cloud_march: vec4<f32>,
    // order-1 SH of upward ground radiance; xyz is RGB, w is padding
    ground_bounce_sh: array<vec4<f32>, 4>,
    // weather variation and precipitation; z/w reserved
    cloud_weather: vec4<f32>,
    // active direct-light direction and illuminance; the final scalar says sun (1) or moon (0)
    active_light_direction: vec3<f32>,
    _pad_active_light_direction: f32,
    active_light_illuminance: vec3<f32>,
    active_light_is_sun: f32,
};

// The group index is `ENVIRONMENT_BIND_GROUP` in `gpu.rs`. The two must agree; a
// consumer binding this group anywhere else gets a validation error, not a wrong image.
@group(1) @binding(0) var<uniform> atmosphere: AtmosphereUniform;
@group(1) @binding(1) var atmosphere_transmittance_lut: texture_2d<f32>;
@group(1) @binding(2) var atmosphere_multiple_scattering_lut: texture_2d<f32>;
@group(1) @binding(3) var atmosphere_sky_view_lut: texture_2d<f32>;
@group(1) @binding(4) var atmosphere_aerial_perspective_lut: texture_3d<f32>;
@group(1) @binding(5) var atmosphere_lut_sampler: sampler;
// Tileable Perlin-Worley density: R low-frequency base, GBA Worley erosion octaves.
@group(1) @binding(6) var cloud_density_field: texture_3d<f32>;
// Top-down transmittance of the deck along the active direct-light axis. R is the transmittance.
@group(1) @binding(7) var cloud_shadow_map: texture_2d<f32>;
// Repeating sampler: the density field tiles, so it must not clamp.
@group(1) @binding(8) var cloud_field_sampler: sampler;
// Nubis Data Field: world-scale coverage/type/density layout.
@group(1) @binding(9) var cloud_ndf_field: texture_2d<f32>;
