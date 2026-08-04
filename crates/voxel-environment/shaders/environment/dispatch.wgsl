// The renderer-facing entry points.
//
// These four functions are the whole contract: DDA, water and CAGI call these names and
// nothing else in this module. Which implementation answers them — physical LUT reads,
// authored backdrop, or a blend of both — is decided here and only here, so a consumer
// never branches on the environment provider.
//
// One dependency runs the other way and is worth knowing about: `lighting.sky_ambient.w`
// below is the RENDERER's uniform, not this crate's. The ambient scale still lives in
// `LightingUniform`, so this file cannot be compiled without that binding in scope.

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
