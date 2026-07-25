// Volumetric fog sea around the plateau rim.
//
// Fullscreen raymarch (proxy dome around the camera): the density field is
// zero inside the rim radius, rises to full just beyond it, and fills
// everything below a noise-billowed top surface. The march is clamped to
// the scene depth from the depth prepass, so terrain sits in the fog and
// the world edge is swallowed with no visible geometry seam.
//
// color: rgb = fog color (linear), w = master opacity
// drift: xy = wind scroll (m), z = noise scale, w = daylight
// band:  x = radius fog starts, y = radius fog is full,
//        z = mean top height, w = slab bottom

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_view_bindings::{globals, view},
    view_transformations::{frag_coord_to_ndc, position_ndc_to_world},
    prepass_utils,
}

// quality: x = raymarch step count (live lever). Rest reserved.
struct FogSeaUniform {
    color: vec4<f32>,
    drift: vec4<f32>,
    band: vec4<f32>,
    quality: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> fog: FogSeaUniform;

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
    // Perf: 2 octaves instead of 3. `value_noise` is the fog's hottest inner
    // cost (per step, per fullscreen pixel); the soft fog sea doesn't show the
    // third octave's fine detail. Normalize by 0.5 + 0.25 = 0.75.
    var total = 0.0;
    var amplitude = 0.5;
    var q = p;
    for (var octave = 0; octave < 2; octave++) {
        total += value_noise(q) * amplitude;
        q = q * 2.13 + vec3<f32>(11.7, 5.3, 7.1);
        amplitude *= 0.5;
    }
    return total / 0.75;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let camera = view.world_position;
    let direction = normalize(in.world_position.xyz - camera);

    // How far this ray travels before hitting opaque scene geometry.
    var t_scene = 100000.0;
    let raw_depth = prepass_utils::prepass_depth(in.position, 0u);
    if (raw_depth > 0.0) {
        let scene_ndc = vec3<f32>(frag_coord_to_ndc(in.position).xy, raw_depth);
        t_scene = distance(position_ndc_to_world(scene_ndc), camera);
    }

    // Restrict the march to the fog slab (bottom .. top + billow headroom).
    let top_max = fog.band.z + 3.5;
    let bottom = fog.band.w;
    var t_enter = 0.0;
    var t_exit = t_scene;
    if (abs(direction.y) > 0.0001) {
        let t_at_top = (top_max - camera.y) / direction.y;
        let t_at_bottom = (bottom - camera.y) / direction.y;
        t_enter = max(min(t_at_top, t_at_bottom), 0.0);
        t_exit = min(min(max(t_at_top, t_at_bottom), t_scene), t_enter + 420.0);
    } else if (camera.y < bottom || camera.y > top_max) {
        return vec4<f32>(0.0);
    }
    if (t_exit <= t_enter) {
        return vec4<f32>(0.0);
    }

    // Live quality lever (P-overlay): step count comes from the uniform. The
    // dithered start (below) turns coarser sampling into churn not banding, so
    // the soft fog looks the same at fewer steps.
    let step_count = max(i32(fog.quality.x), 1);
    let step_length = (t_exit - t_enter) / f32(step_count);
    // Dither the start so undersampling churns instead of banding.
    let dither = hash31(vec3<f32>(in.position.xy, fract(globals.time) * 61.7));

    var transmittance = 1.0;
    var shade_total = 0.0;
    var shade_weight = 0.0;

    var t = t_enter + step_length * (0.25 + 0.5 * dither);
    for (var i = 0; i < step_count; i++) {
        let position = camera + direction * t;
        let radius = length(position.xz);
        let radial = smoothstep(fog.band.x, fog.band.y, radius);
        if (radial > 0.003) {
            let sample = position * fog.drift.z
                + vec3<f32>(fog.drift.x, globals.time * -0.03, fog.drift.y);
            let billow = fbm(sample);
            let top = fog.band.z + (billow - 0.5) * 5.0;
            let height_mask = smoothstep(top, top - 2.6, position.y);
            let density = radial * height_mask;
            if (density > 0.004) {
                let sigma = density * step_length * 0.55;
                let absorbed = 1.0 - exp(-sigma);
                shade_total += transmittance * absorbed * (0.82 + 0.36 * billow);
                shade_weight += transmittance * absorbed;
                transmittance *= exp(-sigma);
                if (transmittance < 0.01) {
                    break;
                }
            }
        }
        t += step_length;
    }

    let alpha = (1.0 - transmittance) * fog.color.a;
    if (alpha < 0.003) {
        return vec4<f32>(0.0);
    }
    let shade = shade_total / max(shade_weight, 0.0001);
    return vec4<f32>(fog.color.rgb * shade, alpha);
}
