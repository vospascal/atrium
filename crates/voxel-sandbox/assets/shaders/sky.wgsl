// Sky dome: atmospheric gradient + scattering glow, raymarched volumetric
// clouds, HDR sun disc, moon with phases, rotating star field.
//
// sun_direction:  xyz to sun, w = daylight (0..1)
// moon_direction: xyz to moon, w = moon phase (0 new, 0.5 full, 1 new)
// light_color:    rgb current light (linear), w = star rotation (radians)
// zenith_color:   rgb (linear), w = cloud coverage (0..1)
// horizon_color:  rgb (linear), w = cloud type (0 stratus, 1 cumulus, 2 cirrus)
// scroll:         xy cloud scroll (m), z = moonlight, w = fog (0..1)

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_view_bindings::{globals, view},
}

struct SkyUniform {
    sun_direction: vec4<f32>,
    moon_direction: vec4<f32>,
    light_color: vec4<f32>,
    zenith_color: vec4<f32>,
    horizon_color: vec4<f32>,
    scroll: vec4<f32>,
    // x = cloud march step count (live quality lever). Rest reserved.
    quality: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> sky: SkyUniform;

// ---------------------------------------------------------------- noise ---

fn hash31(p: vec3<f32>) -> f32 {
    var q = fract(p * vec3<f32>(0.1031, 0.1030, 0.0973));
    q += dot(q, q.yzx + 33.33);
    return fract((q.x + q.y) * q.z);
}

fn value_noise(p: vec3<f32>) -> f32 {
    let cell = floor(p);
    let t = fract(p);
    let u = t * t * (3.0 - 2.0 * t);
    let n000 = hash31(cell);
    let n100 = hash31(cell + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = hash31(cell + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = hash31(cell + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = hash31(cell + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = hash31(cell + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = hash31(cell + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = hash31(cell + vec3<f32>(1.0, 1.0, 1.0));
    let bottom = mix(mix(n000, n100, u.x), mix(n010, n110, u.x), u.y);
    let top = mix(mix(n001, n101, u.x), mix(n011, n111, u.x), u.y);
    return mix(bottom, top, u.z);
}

fn fbm(p: vec3<f32>) -> f32 {
    var total = 0.0;
    var amplitude = 0.5;
    var q = p;
    for (var octave = 0; octave < 4; octave++) {
        total += value_noise(q) * amplitude;
        q = q * 2.13 + vec3<f32>(11.7, 5.3, 7.1);
        amplitude *= 0.5;
    }
    return total / 0.9375;
}

fn fbm_cheap(p: vec3<f32>) -> f32 {
    return (value_noise(p) * 0.5 + value_noise(p * 2.13) * 0.25) / 0.75;
}

// ---------------------------------------------------------------- clouds ---

struct CloudLayer {
    base: f32,
    thickness: f32,
    // Frequency multipliers: y-squash flattens, x-stretch draws wisps.
    frequency: vec3<f32>,
    gain: f32,
}

fn cloud_layer(cloud_type: f32) -> CloudLayer {
    let w_stratus = clamp(1.0 - cloud_type, 0.0, 1.0);
    let w_cumulus = clamp(1.0 - abs(cloud_type - 1.0), 0.0, 1.0);
    let w_cirrus = clamp(cloud_type - 1.0, 0.0, 1.0);
    var layer: CloudLayer;
    layer.base = 240.0 * w_stratus + 320.0 * w_cumulus + 560.0 * w_cirrus;
    layer.thickness = 90.0 * w_stratus + 240.0 * w_cumulus + 70.0 * w_cirrus;
    layer.frequency = vec3<f32>(
        1.0 * w_stratus + 1.0 * w_cumulus + 0.22 * w_cirrus,
        3.0 * w_stratus + 1.3 * w_cumulus + 6.0 * w_cirrus,
        1.0 * w_stratus + 1.0 * w_cumulus + 1.2 * w_cirrus,
    );
    layer.gain = 2.2 * w_stratus + 1.5 * w_cumulus + 0.8 * w_cirrus;
    return layer;
}

fn cloud_density(position: vec3<f32>, layer: CloudLayer, coverage: f32) -> f32 {
    let sample = (position + vec3<f32>(sky.scroll.x, 0.0, sky.scroll.y))
        * layer.frequency * 0.0028;
    let shape = fbm(sample);
    // Fade toward the slab's floor and ceiling so clouds have bellies.
    let height01 = clamp((position.y - layer.base) / layer.thickness, 0.0, 1.0);
    let vertical = smoothstep(0.0, 0.18, height01) * smoothstep(1.0, 0.55, height01);
    let threshold = 0.98 - coverage * 0.92;
    return clamp((shape - threshold) * layer.gain, 0.0, 1.0) * vertical;
}

fn cloud_light_density(position: vec3<f32>, layer: CloudLayer, coverage: f32) -> f32 {
    let sample = (position + vec3<f32>(sky.scroll.x, 0.0, sky.scroll.y))
        * layer.frequency * 0.0028;
    let shape = fbm_cheap(sample);
    let height01 = clamp((position.y - layer.base) / layer.thickness, 0.0, 1.0);
    let vertical = smoothstep(0.0, 0.18, height01) * smoothstep(1.0, 0.55, height01);
    let threshold = 0.98 - coverage * 0.92;
    return clamp((shape - threshold) * layer.gain, 0.0, 1.0) * vertical;
}

struct CloudResult {
    color: vec3<f32>,
    transmittance: f32,
}

fn march_clouds(origin: vec3<f32>, direction: vec3<f32>) -> CloudResult {
    var result: CloudResult;
    result.color = vec3<f32>(0.0);
    result.transmittance = 1.0;

    let coverage = sky.zenith_color.w;
    if (direction.y < 0.02 || coverage < 0.015) {
        return result;
    }
    let layer = cloud_layer(sky.horizon_color.w);

    let t_enter = (layer.base - origin.y) / direction.y;
    let t_exit = (layer.base + layer.thickness - origin.y) / direction.y;
    if (t_enter > 14000.0) {
        return result;
    }

    let horizon_fade = smoothstep(0.02, 0.10, direction.y);
    let sun_direction = sky.sun_direction.xyz;
    let moon_direction = sky.moon_direction.xyz;
    let daylight = sky.sun_direction.w;
    let moonlight = sky.scroll.z;
    // Whichever body is up lights the clouds; ambient comes from the sky.
    let light_direction = normalize(mix(moon_direction, sun_direction, step(0.5, daylight)));
    let forward = pow(clamp(dot(direction, light_direction), 0.0, 1.0), 3.0);
    let light_energy = (daylight + moonlight * 0.30 + 0.02);
    let ambient = mix(sky.horizon_color.rgb, sky.zenith_color.rgb, 0.5)
        * (0.55 + 0.45 * daylight);

    // Live quality lever (P-overlay). Optical depth is conserved (sigma scales
    // with step_length) so cloud opacity is unchanged; a dithered start (like
    // the fog sea) turns coarser sampling into spatial churn rather than banding.
    let step_count = max(i32(sky.quality.x), 1);
    let step_length = (t_exit - t_enter) / f32(step_count);
    let extinction = 0.028;

    let dither = hash31(direction * 511.0);
    var t = t_enter + step_length * (0.25 + 0.5 * dither);
    for (var i = 0; i < step_count; i++) {
        let position = origin + direction * t;
        let density = cloud_density(position, layer, coverage);
        if (density > 0.005) {
            // Two shadow taps toward the light.
            let occlusion = cloud_light_density(
                position + light_direction * 35.0, layer, coverage,
            ) + cloud_light_density(position + light_direction * 90.0, layer, coverage);
            let light = exp(-occlusion * 2.2);
            let sample_color = sky.light_color.rgb * light_energy * light
                * (0.9 + 1.6 * forward) + ambient;
            let sigma = density * step_length * extinction;
            let alpha = 1.0 - exp(-sigma);
            result.color += result.transmittance * alpha * sample_color;
            result.transmittance *= exp(-sigma);
            if (result.transmittance < 0.02) {
                break;
            }
        }
        t += step_length;
    }

    // Distant clouds sink into the haze, and everything fades at the rim.
    let haze = 1.0 - exp(-t_enter * 0.00035);
    let opacity = (1.0 - result.transmittance) * horizon_fade;
    result.color = mix(result.color * horizon_fade, sky.horizon_color.rgb * opacity, haze);
    result.transmittance = 1.0 - opacity;
    return result;
}

// ------------------------------------------------------------ celestials ---

fn rotate_about_axis(v: vec3<f32>, axis: vec3<f32>, angle: f32) -> vec3<f32> {
    let c = cos(angle);
    let s = sin(angle);
    return v * c + cross(axis, v) * s + axis * dot(axis, v) * (1.0 - c);
}

fn star_field(direction: vec3<f32>, daylight: f32) -> vec3<f32> {
    let axis = normalize(vec3<f32>(0.0, 0.55, 0.84));
    let rotated = rotate_about_axis(direction, axis, -sky.light_color.w);
    let grid = 44.0;
    let cell = floor(rotated * grid);
    let seed = hash31(cell);
    if (seed < 0.955) {
        return vec3<f32>(0.0);
    }
    let offset = vec3<f32>(
        hash31(cell + 13.1),
        hash31(cell + 27.7),
        hash31(cell + 41.3),
    ) * 0.6 + 0.2;
    let local = fract(rotated * grid) - offset;
    let radius = length(local);
    let brightness = (seed - 0.955) / 0.045;
    let twinkle = 0.72 + 0.28 * sin(globals.time * (1.5 + seed * 3.0) + seed * 40.0);
    let core = smoothstep(0.22, 0.0, radius);
    let visibility = (1.0 - daylight) * (1.0 - daylight)
        * smoothstep(-0.02, 0.12, direction.y);
    // A slight blue-white tint spread; brightest stars go warm. HDR-bright
    // so stars survive depth-of-field blur and feed the bloom pass.
    let tint = mix(vec3<f32>(0.72, 0.80, 1.0), vec3<f32>(1.0, 0.92, 0.80),
        step(0.75, brightness));
    return tint * core * (0.6 + 5.5 * brightness * brightness) * twinkle * visibility;
}

fn moon_disc(direction: vec3<f32>) -> vec3<f32> {
    let moon_direction = sky.moon_direction.xyz;
    if (moon_direction.y < -0.10) {
        return vec3<f32>(0.0);
    }
    let angular_radius = 0.038;
    let toward = dot(direction, moon_direction);
    if (toward < 0.99) {
        return vec3<f32>(0.0);
    }
    // Local disc coordinates.
    let right = normalize(cross(vec3<f32>(0.0, 1.0, 0.0), moon_direction));
    let up = cross(moon_direction, right);
    let delta = direction - moon_direction * toward;
    let u = dot(delta, right) / angular_radius;
    let v = dot(delta, up) / angular_radius;
    let radial_sq = u * u + v * v;
    if (radial_sq > 1.15) {
        return vec3<f32>(0.0);
    }
    let edge = smoothstep(1.05, 0.92, radial_sq);
    // Shade the disc as a sphere lit from the phase angle: 0.5 = full
    // (light toward the viewer), 0 or 1 = new (light behind).
    let normal = vec3<f32>(u, v, sqrt(max(1.0 - min(radial_sq, 1.0), 0.0)));
    let phase_angle = (sky.moon_direction.w * 2.0 - 1.0) * 3.14159265;
    let light = vec3<f32>(sin(phase_angle), 0.0, -cos(phase_angle));
    let lit = clamp(dot(normal, light), 0.0, 1.0);
    // Faint earthshine keeps the dark limb barely visible.
    let surface = 3.6 * lit + 0.07;
    let daylight = sky.sun_direction.w;
    let mare = 0.75 + 0.25 * value_noise(normal * 6.0 + 3.0);
    return vec3<f32>(0.92, 0.95, 1.0) * surface * mare * edge * (1.0 - daylight * 0.85);
}

// -------------------------------------------------------------- fragment ---

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let direction = normalize(in.world_position.xyz - view.world_position);
    let sun_direction = sky.sun_direction.xyz;
    let daylight = sky.sun_direction.w;
    let fog = sky.scroll.w;

    // Base gradient: horizon → zenith above; below the horizon lies the
    // distant cloud sea the plateau floats on (the near field is the
    // raymarched fog volume — this is its horizon continuation).
    let up = clamp(direction.y, 0.0, 1.0);
    var color = mix(sky.horizon_color.rgb, sky.zenith_color.rgb, pow(up, 0.55));
    let below = smoothstep(0.0, 0.10, -direction.y);
    if (below > 0.0) {
        // Project onto a plane far under the camera for a receding texture.
        let sea_point = direction / max(-direction.y, 0.05) * 60.0;
        let sea_noise = fbm_cheap(vec3<f32>(sea_point.x, 0.0, sea_point.z) * 0.02
            + vec3<f32>(sky.scroll.x * 0.01, 0.0, sky.scroll.y * 0.01));
        let sea_shade = 0.78 + 0.34 * sea_noise;
        let sea_color = mix(sky.horizon_color.rgb, vec3<f32>(1.0), 0.32 * daylight)
            * sea_shade;
        color = mix(color, sea_color, below);
    }

    // Forward scattering: a broad glow plus a tighter halo around the sun,
    // strongest near the horizon and eaten by fog.
    let toward_sun = clamp(dot(direction, sun_direction), 0.0, 1.0);
    let glow = pow(toward_sun, 6.0) * 0.30 + pow(toward_sun, 40.0) * 0.45;
    color += sky.light_color.rgb * glow * (0.15 + 0.85 * daylight) * (1.0 - fog * 0.7);

    // Clouds occlude everything celestial behind them.
    let clouds = march_clouds(view.world_position, direction);

    var celestial = vec3<f32>(0.0);
    // HDR sun disc (blooms); fades out through fog.
    let disc = smoothstep(0.99955, 0.99985, toward_sun);
    if (sun_direction.y > -0.06) {
        celestial += sky.light_color.rgb * disc * 40.0 * (0.2 + 0.8 * daylight);
    }
    celestial += moon_disc(direction) * (1.0 + sky.scroll.z);
    celestial += star_field(direction, daylight) * 1.6;

    color += celestial * (1.0 - fog * 0.9);
    color = color * clouds.transmittance + clouds.color;

    // Fog swallows the sky near the horizon last.
    let fog_band = fog * (1.0 - smoothstep(0.0, 0.45, direction.y));
    color = mix(color, sky.horizon_color.rgb, fog_band * 0.85);

    return vec4<f32>(color, 1.0);
}
