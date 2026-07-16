// Waterfall ribbon: foam streaks racing down translucent water. Bright
// churn at the spill lip, streaks stretching as they fall, dissolving
// into the fog sea at the bottom.
//
// params: x = flow speed, y = streak density, z = per-fall seed

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_view_bindings::globals,
}

struct WaterfallUniform {
    params: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> waterfall: WaterfallUniform;

fn hash21(p: vec2<f32>) -> f32 {
    var q = fract(p * vec2<f32>(0.1031, 0.1030));
    q += dot(q, q.yx + 33.33);
    return fract((q.x + q.y) * q.x);
}

fn value_noise_2d(p: vec2<f32>) -> f32 {
    let cell = floor(p);
    let t = fract(p);
    let u = t * t * (3.0 - 2.0 * t);
    let n00 = hash21(cell);
    let n10 = hash21(cell + vec2<f32>(1.0, 0.0));
    let n01 = hash21(cell + vec2<f32>(0.0, 1.0));
    let n11 = hash21(cell + vec2<f32>(1.0, 1.0));
    return mix(mix(n00, n10, u.x), mix(n01, n11, u.x), u.y);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let speed = waterfall.params.x;
    let density = waterfall.params.y;
    let seed = waterfall.params.z;
    let across_meters = in.uv.x;
    let down = in.uv.y;

    // Streaks: noise stretched vertically, scrolling downward; a second
    // octave at higher frequency adds sparkle near the lip.
    let flow = globals.time * speed;
    let streaks = value_noise_2d(vec2<f32>(
        across_meters * density + seed,
        down * 6.0 - flow,
    ));
    let ripple = value_noise_2d(vec2<f32>(
        across_meters * density * 2.7 + seed + 40.0,
        down * 14.0 - flow * 1.6,
    ));
    let foam = smoothstep(0.42, 0.75, streaks * 0.7 + ripple * 0.3);

    // Churn at the spill lip, calmer mid-fall.
    let lip = 1.0 - smoothstep(0.0, 0.22, down);
    // Dissolve into the fog sea.
    let dissolve = 1.0 - smoothstep(0.62, 0.98, down);

    let water_body = 0.35 + 0.25 * streaks;
    let alpha = clamp(water_body + foam * 0.55 + lip * 0.30, 0.0, 1.0) * dissolve;

    // Pale blue water, white foam, slightly overbright lip for bloom.
    let water_color = vec3<f32>(0.55, 0.68, 0.75);
    let foam_color = vec3<f32>(0.94, 0.97, 1.0);
    var color = mix(water_color, foam_color, clamp(foam + lip * 0.5, 0.0, 1.0));
    color *= (0.9 + 0.35 * lip) * waterfall.params.w;

    return vec4<f32>(color, alpha * 0.9);
}
