#import bevy_pbr::forward_io::VertexOutput

// Uniforms passed from GrassMaterial on the Rust side.
struct GrassUniforms {
    time: f32,
    wind_speed: f32,
    variation_strength: f32,
    wetness: f32,
    wind_strength: f32,
    wind_direction_x: f32,
    wind_direction_y: f32,
    _padding: f32,
};

@group(2) @binding(0)
var<storage, read> material: GrassUniforms;

// ── Simplex 2D noise (Ashima/webgl-noise port) ─────────────────────────────

fn mod289_3(x: vec3<f32>) -> vec3<f32> {
    return x - floor(x * (1.0 / 289.0)) * 289.0;
}

fn mod289_2(x: vec2<f32>) -> vec2<f32> {
    return x - floor(x * (1.0 / 289.0)) * 289.0;
}

fn permute(x: vec3<f32>) -> vec3<f32> {
    return mod289_3(((x * 34.0) + 10.0) * x);
}

fn simplex2d(v: vec2<f32>) -> f32 {
    let C = vec4<f32>(
        0.211324865405187,   // (3.0 - sqrt(3.0)) / 6.0
        0.366025403784439,   // 0.5 * (sqrt(3.0) - 1.0)
        -0.577350269189626,  // -1.0 + 2.0 * C.x
        0.024390243902439,   // 1.0 / 41.0
    );

    // First corner
    var i = floor(v + dot(v, C.yy));
    let x0 = v - i + dot(i, C.xx);

    // Other corners
    var i1: vec2<f32>;
    if x0.x > x0.y {
        i1 = vec2<f32>(1.0, 0.0);
    } else {
        i1 = vec2<f32>(0.0, 1.0);
    }
    let x12 = x0.xyxy + C.xxzz - vec4<f32>(i1.x, i1.y, 0.0, 0.0);

    // Permutations
    i = mod289_2(i);
    let p = permute(permute(i.y + vec3<f32>(0.0, i1.y, 1.0)) + i.x + vec3<f32>(0.0, i1.x, 1.0));

    var m = max(vec3<f32>(0.5) - vec3<f32>(dot(x0, x0), dot(x12.xy, x12.xy), dot(x12.zw, x12.zw)), vec3<f32>(0.0));
    m = m * m;
    m = m * m;

    // Gradients
    let x = 2.0 * fract(p * C.www) - 1.0;
    let h = abs(x) - 0.5;
    let ox = floor(x + 0.5);
    let a0 = x - ox;

    m = m * (1.79284291400159 - 0.85373472095314 * (a0 * a0 + h * h));

    let g = vec3<f32>(
        a0.x * x0.x + h.x * x0.y,
        a0.y * x12.x + h.y * x12.y,
        a0.z * x12.z + h.z * x12.w,
    );

    return 130.0 * dot(m, g);
}

// ── Fractional Brownian Motion (layered noise) ──────────────────────────────

fn fbm(position: vec2<f32>, octaves: i32) -> f32 {
    var value = 0.0;
    var amplitude = 0.5;
    var frequency = 1.0;
    var p = position;

    for (var i = 0; i < octaves; i++) {
        value += amplitude * simplex2d(p * frequency);
        frequency *= 2.0;
        amplitude *= 0.5;
    }
    return value;
}

// ── Main fragment shader ────────────────────────────────────────────────────
//
// TODO(user): This is where you shape the garden's look!
// Tweak the colors, noise scales, and blending to get the aesthetic you want.
// The simplex2d() and fbm() functions above give you smooth procedural noise.
// `material.time` gives you animation, `world_pos` gives you spatial variation.

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let world_pos = in.world_position.xz;

    // Wind animation: shift the sampling position over time
    let wind_dir = vec2<f32>(material.wind_direction_x, material.wind_direction_y);
    let wind_offset = wind_dir * material.time * material.wind_speed * material.wind_strength;
    let animated_pos = world_pos + wind_offset;

    // ── Layer 1: broad terrain patches (~2-3m) ──────────────────────
    let large_noise = simplex2d(animated_pos * 0.4) * 0.5 + 0.5;

    // ── Layer 2: medium clump patterns (~0.5-1m) ────────────────────
    let medium_noise = simplex2d(animated_pos * 1.5 + vec2<f32>(42.0, 17.0)) * 0.5 + 0.5;

    // ── Layer 3: fine grass detail (~15-30cm) ───────────────────────
    let fine_noise = fbm(animated_pos * 5.0, 3) * 0.5 + 0.5;

    // ── Color palette (unlit — must be bright enough to match PBR-lit objects) ─
    let dark_green = vec3<f32>(0.35, 0.52, 0.18);
    let mid_green = vec3<f32>(0.48, 0.68, 0.28);
    let light_green = vec3<f32>(0.62, 0.82, 0.35);
    let yellow_green = vec3<f32>(0.72, 0.78, 0.32);
    let brown = vec3<f32>(0.50, 0.38, 0.20);

    // ── Blend with strong contrast ──────────────────────────────────
    var color = mix(dark_green, light_green, large_noise);
    color = mix(color, mid_green, medium_noise * 0.5);
    color = mix(color, yellow_green, fine_noise * 0.3);

    // Dirt patches in the darkest areas
    let dirt_amount = smoothstep(0.25, 0.0, large_noise) * 0.7;
    color = mix(color, brown, dirt_amount);

    // Fake sunlight variation
    let fake_sun = 0.85 + 0.15 * simplex2d(world_pos * 0.1 + vec2<f32>(100.0, 200.0));
    color *= fake_sun;

    // Wind ripple
    let wind_ripple = sin(material.time * 2.0 + world_pos.x * 0.8 + world_pos.y * 0.5) * 0.5 + 0.5;
    color *= 1.0 + wind_ripple * 0.08 * material.wind_strength;

    // Wetness (rain darkening)
    let wet_factor = mix(1.0, 0.6, material.wetness);
    color *= wet_factor;

    return vec4<f32>(color, 1.0);
}
