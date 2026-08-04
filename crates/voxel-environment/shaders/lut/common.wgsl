// Jolifanto/Hillaire LUT generation helpers.

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

@group(0) @binding(0) var<uniform> atmosphere: AtmosphereUniform;

const PI: f32 = 3.141592653589793;

fn atmosphere_height(uv_y: f32) -> f32 {
    return uv_y * (atmosphere.top_radius_km - atmosphere.bottom_radius_km);
}

fn rayleigh_density(height_km: f32) -> f32 {
    return exp(-max(height_km, 0.0) / 8.0);
}

fn mie_density(height_km: f32) -> f32 {
    return exp(-max(height_km, 0.0) / 1.2);
}

fn extinction(height_km: f32) -> vec3<f32> {
    let rayleigh = rayleigh_density(height_km) * vec3<f32>(0.005802, 0.013558, 0.033100);
    let mie = mie_density(height_km) * vec3<f32>(0.004440);
    let ozone = smoothstep(10.0, 30.0, height_km) * vec3<f32>(0.000650, 0.001881, 0.000085);
    return rayleigh + mie + ozone;
}

fn rayleigh_phase(mu: f32) -> f32 {
    return 3.0 / (16.0 * PI) * (1.0 + mu * mu);
}

fn mie_phase(mu: f32, eccentricity: f32) -> f32 {
    let g2 = eccentricity * eccentricity;
    let denominator = pow(1.0 + g2 - 2.0 * eccentricity * mu, 1.5);
    return 3.0 / (8.0 * PI) * (1.0 - g2) / (2.0 + g2)
        * (1.0 + mu * mu) / denominator;
}

fn atmosphere_origin_at_height(uv_y: f32) -> vec3<f32> {
    let height = atmosphere_height(clamp(uv_y, 0.0, 1.0));
    return vec3<f32>(0.0, atmosphere.bottom_radius_km + height, 0.0);
}

fn atmosphere_ray_sphere_distance(
    origin: vec3<f32>,
    direction: vec3<f32>,
    radius: f32,
) -> f32 {
    let b = dot(origin, direction);
    let c = dot(origin, origin) - radius * radius;
    let discriminant = b * b - c;
    if (discriminant < 0.0) {
        return -1.0;
    }
    let root = sqrt(discriminant);
    let near = -b - root;
    let far = -b + root;
    if (near >= 0.0) {
        return near;
    }
    if (far >= 0.0) {
        return far;
    }
    return -1.0;
}

fn atmosphere_ray_distance(origin: vec3<f32>, direction: vec3<f32>) -> f32 {
    let top = atmosphere_ray_sphere_distance(origin, direction, atmosphere.top_radius_km);
    let ground = atmosphere_ray_sphere_distance(origin, direction, atmosphere.bottom_radius_km);
    if (ground >= 0.0) {
        return min(top, ground);
    }
    return max(top, 0.0);
}

fn atmosphere_transmittance_segment(
    origin: vec3<f32>,
    direction: vec3<f32>,
    distance: f32,
) -> vec3<f32> {
    if (distance <= 0.0) {
        return vec3<f32>(1.0);
    }
    let step_length = distance / 16.0;
    var transmittance = vec3<f32>(1.0);
    for (var sample = 0u; sample < 16u; sample = sample + 1u) {
        let point = origin + direction * ((f32(sample) + 0.5) * step_length);
        let height = max(length(point) - atmosphere.bottom_radius_km, 0.0);
        transmittance *= exp(-extinction(height) * step_length);
    }
    return transmittance;
}

fn atmosphere_view_direction(uv: vec2<f32>) -> vec3<f32> {
    let elevation = uv.y * 2.0 - 1.0;
    let horizontal = sqrt(max(1.0 - elevation * elevation, 0.0));
    let azimuth = uv.x * 2.0 * PI;
    return normalize(vec3<f32>(
        cos(azimuth) * horizontal,
        elevation,
        sin(azimuth) * horizontal,
    ));
}

fn atmosphere_froxel_direction(uv: vec2<f32>) -> vec3<f32> {
    let ndc = uv * 2.0 - vec2<f32>(1.0);
    return normalize(
        atmosphere.camera_forward
            + ndc.x * atmosphere.camera_right_scaled
            + ndc.y * atmosphere.camera_up_scaled,
    );
}

fn atmosphere_froxel_distance(uv_z: f32) -> f32 {
    let near_distance = max(atmosphere.camera_depth.x, 0.001);
    let far_distance = max(atmosphere.camera_depth.y, near_distance + 0.001);
    return near_distance * pow(far_distance / near_distance, clamp(uv_z, 0.0, 1.0));
}

fn atmosphere_scattering(
    origin: vec3<f32>,
    direction: vec3<f32>,
    distance: f32,
) -> vec3<f32> {
    if (distance <= 0.0) {
        return vec3<f32>(0.0);
    }
    let sun_direction = normalize(atmosphere.sun_direction);
    let mu = dot(direction, sun_direction);
    let rayleigh_phase_value = rayleigh_phase(mu);
    let mie_phase_value = mie_phase(mu, 0.8);
    let step_length = distance / 24.0;
    var result = vec3<f32>(0.0);
    var view_transmittance = vec3<f32>(1.0);
    for (var sample = 0u; sample < 24u; sample = sample + 1u) {
        let point = origin + direction * ((f32(sample) + 0.5) * step_length);
        let height = max(length(point) - atmosphere.bottom_radius_km, 0.0);
        let rayleigh = rayleigh_density(height) * vec3<f32>(0.005802, 0.013558, 0.033100);
        let mie = mie_density(height) * vec3<f32>(0.003996);
        let light_distance = atmosphere_ray_distance(point, sun_direction);
        let light_transmittance = atmosphere_transmittance_segment(
            point,
            sun_direction,
            light_distance,
        );
        let source = atmosphere.sun_illuminance * light_transmittance
            * (rayleigh * rayleigh_phase_value + mie * mie_phase_value);
        result += view_transmittance * source * step_length;
        view_transmittance *= exp(-extinction(height) * step_length);
    }
    return max(result, vec3<f32>(0.0));
}
