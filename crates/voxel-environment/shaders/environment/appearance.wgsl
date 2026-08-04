// The camera-only appearance layer: horizon/zenith palette, stars, moon, sun glow.
//
// None of this enters CAGI, diffuse lighting or the transmittance LUT. That separation
// is deliberate and load-bearing — it is what lets the physical transport in
// `hillaire.wgsl` improve without flattening the authored backdrop, and it is why the
// `visual_*` uniform fields exist alongside the physical `sun_*` ones.

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
