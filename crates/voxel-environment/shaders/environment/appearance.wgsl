// Celestial presentation on top of the Hillaire atmosphere.
//
// The atmosphere LUTs own the sky's physical in-scattered radiance. This module only adds
// distant sources that are not represented by those LUTs: stars and the resolved sun/moon
// discs. They are still evaluated in physical order: source radiance is attenuated by the
// atmosphere here, and the cloud march in `dispatch.wgsl` attenuates the complete backdrop.

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
    let view = normalize(direction);
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
    let visibility = smoothstep(0.18, 0.98, 1.0 - daylight)
        * smoothstep(-0.02, 0.12, direction.y);
    let tint = mix(vec3<f32>(0.55, 0.72, 1.0), vec3<f32>(1.0, 0.82, 0.58),
        step(0.72, brightness));
    return tint * core * (0.3 + 5.0 * brightness * brightness) * visibility
        * environment_view_transmittance(view);
}

fn environment_moon_radiance(direction: vec3<f32>, daylight: f32) -> vec3<f32> {
    let moon_direction = normalize(atmosphere.moon_direction);
    if (moon_direction.y < -0.06) {
        return vec3<f32>(0.0);
    }
    let disc = smoothstep(0.99915, 0.99972, dot(direction, moon_direction));
    let visibility = smoothstep(0.02, 0.45, 1.0 - daylight);
    // The moon source is already phase-scaled by the CPU environment model. Keeping the disc
    // tied to that same source prevents a bright visual moon from lighting clouds and terrain
    // with a different phase or colour.
    return atmosphere.moon_illuminance * disc * 6.0 * visibility
        * environment_view_transmittance(direction);
}

fn environment_celestial_detail(direction: vec3<f32>) -> vec3<f32> {
    let daylight = atmosphere.visual_sun.w;
    let view = normalize(direction);
    // Resolved celestial detail is presentation-only and does not enter CAGI or the LUT-generated
    // atmospheric in-scattered radiance. Its distant-source radiance does use the transmittance
    // LUT below, so the camera sees the same atmosphere that attenuates the physical sky.
    let toward_sun = max(dot(view, normalize(atmosphere.sun_direction)), 0.0);
    let sun_glow = pow(toward_sun, 6.0) * 0.12 + pow(toward_sun, 48.0) * 0.30;
    let sun_disc = smoothstep(0.99955, 0.99985, toward_sun);
    let sun_transmittance = environment_view_transmittance(view);
    var detail = atmosphere.sun_illuminance * sun_glow * sun_transmittance * daylight;
    detail += atmosphere.sun_illuminance * sun_disc * 6.0 * sun_transmittance * daylight;
    detail += environment_moon_radiance(view, daylight);
    detail += environment_star_radiance(view, daylight);
    return detail;
}

fn environment_sky_radiance(direction: vec3<f32>) -> vec3<f32> {
    let view = normalize(direction);
    // Hillaire is the primary sky model. The authored zenith/horizon fields remain available
    // as metadata for compatibility and tuning, but they no longer replace physical sky
    // radiance in the default presentation path.
    return environment_hillaire_sky(view) + environment_celestial_detail(view);
}
