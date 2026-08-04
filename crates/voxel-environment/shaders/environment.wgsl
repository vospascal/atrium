// LUT-backed Hillaire/Jolifanto environment sampling.
//
// The expensive atmosphere integration happens in the four compute passes under
// shaders/lut/. DDA, water and CAGI only perform filtered LUT reads here; there is no
// per-camera-pixel atmosphere ray march in this module.

struct AtmosphereUniform {
    bottom_radius_km: f32,
    top_radius_km: f32,
    from_kilometers_scale: f32,
    _pad0: f32,
    sun_direction: vec3<f32>,
    _pad1: f32,
    sun_illuminance: vec3<f32>,
    _pad2: f32,
    camera_position: vec3<f32>,
    _pad3: f32,
    sky_view_size: vec2<f32>,
    aerial_size: vec3<f32>,
    _pad4: f32,
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

@group(1) @binding(0) var<uniform> atmosphere: AtmosphereUniform;
@group(1) @binding(1) var atmosphere_transmittance_lut: texture_2d<f32>;
@group(1) @binding(2) var atmosphere_multiple_scattering_lut: texture_2d<f32>;
@group(1) @binding(3) var atmosphere_sky_view_lut: texture_2d<f32>;
@group(1) @binding(4) var atmosphere_aerial_perspective_lut: texture_3d<f32>;
@group(1) @binding(5) var atmosphere_lut_sampler: sampler;

fn atmosphere_sky_view_uv(direction: vec3<f32>) -> vec2<f32> {
    let azimuth = atan2(direction.z, direction.x) / (2.0 * 3.141592653589793) + 0.5;
    let elevation = direction.y * 0.5 + 0.5;
    return vec2<f32>(azimuth, elevation);
}

fn atmosphere_transmittance_uv(direction: vec3<f32>) -> vec2<f32> {
    // Atrium positions are local metres above the terrain, while the LUT's
    // radius domain is expressed as altitude above the planet surface.
    let altitude_km = atmosphere.camera_position.y / atmosphere.from_kilometers_scale;
    let height = clamp(
        altitude_km / (atmosphere.top_radius_km - atmosphere.bottom_radius_km),
        0.0,
        1.0,
    );
    let planet_center_world = vec3<f32>(
        0.0,
        -atmosphere.bottom_radius_km * atmosphere.from_kilometers_scale,
        0.0,
    );
    let local_up = normalize(atmosphere.camera_position - planet_center_world);
    return vec2<f32>(dot(normalize(direction), local_up) * 0.5 + 0.5, height);
}

fn environment_sun_transmittance(direction: vec3<f32>) -> vec3<f32> {
    return environment_sun_transmittance_at(atmosphere.camera_position, direction);
}

// CAGI source cells are expressed in renderer world metres rather than camera
// space. The LUT stores optical depth by altitude, so use the same
// `fromKilometersScale` conversion as the host-side adapter instead of tracing
// a world shadow ray for every candidate cell.
fn environment_sun_transmittance_at(position_world: vec3<f32>, direction: vec3<f32>) -> vec3<f32> {
    let altitude_km = position_world.y / atmosphere.from_kilometers_scale;
    let height = clamp(
        altitude_km / (atmosphere.top_radius_km - atmosphere.bottom_radius_km),
        0.0,
        1.0,
    );
    let planet_center_world = vec3<f32>(
        0.0,
        -atmosphere.bottom_radius_km * atmosphere.from_kilometers_scale,
        0.0,
    );
    let local_up = normalize(position_world - planet_center_world);
    let uv = vec2<f32>(dot(normalize(direction), local_up) * 0.5 + 0.5, height);
    return textureSampleLevel(
        atmosphere_transmittance_lut,
        atmosphere_lut_sampler,
        uv,
        0.0,
    ).rgb;
}

fn environment_hillaire_sky(direction: vec3<f32>) -> vec3<f32> {
    let view = normalize(direction);
    let sky = textureSampleLevel(
        atmosphere_sky_view_lut,
        atmosphere_lut_sampler,
        atmosphere_sky_view_uv(view),
        0.0,
    ).rgb;
    let multiple = textureSampleLevel(
        atmosphere_multiple_scattering_lut,
        atmosphere_lut_sampler,
        vec2<f32>(
            dot(view, normalize(atmosphere.sun_direction)) * 0.5 + 0.5,
            clamp(
                atmosphere.camera_position.y / atmosphere.from_kilometers_scale
                    / (atmosphere.top_radius_km - atmosphere.bottom_radius_km),
                0.0,
                1.0,
            ),
        ),
        0.0,
    ).rgb;
    let toward_sun = max(dot(view, normalize(atmosphere.sun_direction)), 0.0);
    let sun_disk = smoothstep(0.99955, 0.99985, toward_sun);
    return max(sky + multiple + atmosphere.sun_illuminance * sun_disk * 4.0, vec3<f32>(0.0));
}

fn atmosphere_froxel_uv(direction: vec3<f32>) -> vec2<f32> {
    let view = normalize(direction);
    let forward = normalize(atmosphere.camera_forward);
    let right = normalize(atmosphere.camera_right_scaled);
    let up = normalize(atmosphere.camera_up_scaled);
    let forward_component = max(dot(view, forward), 0.0001);
    let ndc = vec2<f32>(
        dot(view, right) / (forward_component * length(atmosphere.camera_right_scaled)),
        dot(view, up) / (forward_component * length(atmosphere.camera_up_scaled)),
    );
    return ndc * 0.5 + vec2<f32>(0.5);
}

fn atmosphere_froxel_depth(distance_world: f32) -> f32 {
    let near_distance = max(atmosphere.camera_depth.x, 0.001);
    let far_distance = max(atmosphere.camera_depth.y, near_distance + 0.001);
    return log(max(distance_world, near_distance) / near_distance)
        / log(far_distance / near_distance);
}

// Sample aerial perspective for a finite camera-ray segment. Sky-view already
// contains the infinite-atmosphere result, so this is kept separate and is only
// applied by callers that know the ray distance.
fn environment_aerial_perspective(direction: vec3<f32>, distance_world: f32) -> vec3<f32> {
    let view = normalize(direction);
    let distance_uv = clamp(atmosphere_froxel_depth(distance_world), 0.0, 1.0);
    let view_uv = atmosphere_froxel_uv(view);
    let uvw = vec3<f32>(view_uv, distance_uv);
    let aerial = textureSampleLevel(
        atmosphere_aerial_perspective_lut,
        atmosphere_lut_sampler,
        uvw,
        0.0,
    );
    // RGB is inscattered radiance; alpha stores the red-channel transmittance
    // from the compact LUT representation.
    return aerial.rgb + environment_hillaire_sky(view) * aerial.a;
}

fn sky_hash(point: vec3<f32>) -> f32 {
    var q = fract(point * vec3<f32>(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yzx + vec3<f32>(33.33));
    return fract((q.x + q.y) * q.z);
}

fn rotate_sky_y(direction: vec3<f32>, angle: f32) -> vec3<f32> {
    let sine = sin(angle);
    let cosine = cos(angle);
    return vec3<f32>(
        direction.x * cosine - direction.z * sine,
        direction.y,
        direction.x * sine + direction.z * cosine,
    );
}

fn environment_star_radiance(direction: vec3<f32>, daylight: f32) -> vec3<f32> {
    if (direction.y <= -0.02 || daylight >= 0.98) {
        return vec3<f32>(0.0);
    }
    let rotated = rotate_sky_y(direction, -atmosphere.visual_zenith.w);
    let cell = floor(rotated * 180.0);
    let seed = sky_hash(cell);
    if (seed < 0.992) {
        return vec3<f32>(0.0);
    }
    let offset = vec3<f32>(
        sky_hash(cell + vec3<f32>(13.1)),
        sky_hash(cell + vec3<f32>(27.7)),
        sky_hash(cell + vec3<f32>(41.3)),
    ) * 0.55 + vec3<f32>(0.225);
    let radius = length(fract(rotated * 180.0) - offset);
    let core = smoothstep(0.24, 0.0, radius);
    let brightness = (seed - 0.992) / 0.008;
    let visibility = (1.0 - daylight) * (1.0 - daylight)
        * smoothstep(-0.02, 0.12, direction.y);
    let tint = mix(vec3<f32>(0.55, 0.72, 1.0), vec3<f32>(1.0, 0.82, 0.58),
        step(0.72, brightness));
    return tint * core * (0.3 + 5.0 * brightness * brightness) * visibility;
}

fn environment_moon_radiance(direction: vec3<f32>, daylight: f32) -> vec3<f32> {
    let moon_direction = atmosphere.visual_moon.xyz;
    if (moon_direction.y < -0.06) {
        return vec3<f32>(0.0);
    }
    let disc = smoothstep(0.99915, 0.99972, dot(direction, moon_direction));
    let lit_fraction = 0.5 - 0.5 * cos(atmosphere.visual_moon.w * 6.28318530718);
    return vec3<f32>(0.48, 0.62, 1.0) * disc
        * (0.035 + 4.0 * lit_fraction) * (1.0 - daylight * 0.9);
}

fn environment_celestial_detail(direction: vec3<f32>) -> vec3<f32> {
    let daylight = atmosphere.visual_sun.w;
    let view = normalize(direction);
    // Visual-only celestial detail restored from the previous implementation.
    // These terms never enter CAGI or the atmosphere transmittance LUT.
    let toward_sun = max(dot(view, atmosphere.visual_sun.xyz), 0.0);
    let sun_glow = pow(toward_sun, 6.0) * 0.22 + pow(toward_sun, 48.0) * 0.55;
    let sun_disc = smoothstep(0.99955, 0.99985, toward_sun);
    var detail = vec3<f32>(1.0, 0.72, 0.42) * sun_glow * daylight;
    detail += vec3<f32>(12.0, 8.5, 5.0) * sun_disc * daylight;
    detail += environment_moon_radiance(view, daylight) * (1.0 + atmosphere.visual_horizon.w);
    detail += environment_star_radiance(view, daylight);
    return detail;
}

fn environment_sky_radiance(direction: vec3<f32>) -> vec3<f32> {
    let view = normalize(direction);
    // Camera appearance layer restored from the previous implementation. This
    // authored palette is intentionally independent from the LUT radiance used
    // by CAGI, so improving physical transport cannot flatten the backdrop.
    let up = clamp(view.y, 0.0, 1.0);
    let palette = mix(
        atmosphere.visual_horizon.rgb,
        atmosphere.visual_zenith.rgb,
        pow(up, 0.55),
    );
    return palette + environment_celestial_detail(view);
}

fn environment_diffuse_radiance(normal: vec3<f32>) -> vec3<f32> {
    let up = normalize(vec3<f32>(
        normal.x * 0.35,
        0.35 + max(normal.y, 0.0) * 0.65,
        normal.z * 0.35,
    ));
    // Keep celestial decoration out of diffuse lighting; this is the physical
    // environment contribution shared with CAGI, not the camera-only backdrop.
    let sky = environment_hillaire_sky(up);
    let ground = vec3<f32>(0.45, 0.36, 0.28)
        * (0.12 + 0.25 * atmosphere.visual_sun.w);
    return (sky * 0.12 + ground) * lighting.sky_ambient.w;
}

fn sky_color(direction: vec3<f32>) -> vec3<f32> {
    return environment_sky_radiance(direction);
}

fn sky_color_at_distance(direction: vec3<f32>, distance_world: f32) -> vec3<f32> {
    if (distance_world <= 0.0) {
        return sky_color(direction);
    }
    // Near misses retain the authored backdrop; farther rays progressively use
    // the aerial-perspective LUT for atmospheric depth. Celestial decoration is
    // added after the physical LUT so it remains a visual-only layer.
    let visual = sky_color(direction);
    let aerial = environment_aerial_perspective(direction, distance_world);
    let physical = aerial + environment_celestial_detail(direction);
    let distance_km = distance_world / atmosphere.from_kilometers_scale;
    let atmosphere_blend = 1.0 - exp(-distance_km * 0.035);
    return mix(visual, physical, clamp(atmosphere_blend, 0.0, 1.0));
}

fn ambient_light(normal: vec3<f32>) -> vec3<f32> {
    return environment_diffuse_radiance(normal);
}
