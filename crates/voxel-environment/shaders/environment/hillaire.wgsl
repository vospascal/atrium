// LUT-backed Hillaire/Jolifanto environment sampling.
//
// The expensive atmosphere integration happens in the four compute passes under
// shaders/lut/. DDA, water and CAGI only perform filtered LUT reads here; there is no
// per-camera-pixel atmosphere ray march in this module.
//
// Everything in this file is PHYSICAL: it is the radiance CAGI and diffuse lighting
// share. Visual-only decoration belongs in `appearance.wgsl`, so that improving
// transport here cannot change the authored backdrop, and vice versa.

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
