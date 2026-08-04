// Shared environment bindings.
//
// The uniform layout and the `@group(1)` slots every environment implementation reads.
// Nothing here samples or shades: `hillaire.wgsl` owns the physical LUT reads,
// `appearance.wgsl` owns the camera-only backdrop, and `dispatch.wgsl` owns the four
// entry points the renderer actually calls.
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
};

// The group index is `ENVIRONMENT_BIND_GROUP` in `gpu.rs`. The two must agree; a
// consumer binding this group anywhere else gets a validation error, not a wrong image.
@group(1) @binding(0) var<uniform> atmosphere: AtmosphereUniform;
@group(1) @binding(1) var atmosphere_transmittance_lut: texture_2d<f32>;
@group(1) @binding(2) var atmosphere_multiple_scattering_lut: texture_2d<f32>;
@group(1) @binding(3) var atmosphere_sky_view_lut: texture_2d<f32>;
@group(1) @binding(4) var atmosphere_aerial_perspective_lut: texture_3d<f32>;
@group(1) @binding(5) var atmosphere_lut_sampler: sampler;
