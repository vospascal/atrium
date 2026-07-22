// Stylized voxel water.
//
// Technique notes (approach informed by the SoftVoxels shaderpack, code
// original): Beer–Lambert absorption with per-channel coefficients in a
// roughly 8:4:3 red:green:blue ratio — red dies first so shallows read
// warm over the sand and depth fades through teal into dark blue — and
// wave normals built from a few noise octaves whose direction rotates per
// octave, with the amplitude fading out at distance so far water lies calm
// instead of shimmering.
//
// Optical depth comes from the depth prepass: this material is
// alpha-blended (excluded from the prepass), so the prepass holds the
// riverbed/terrain behind each fragment.
//
// zenith:  rgb = zenith sky (linear), w = daylight 0..1
// horizon: rgb = horizon sky (linear), w = moonlight 0..1
// light_direction: xyz = to the sun/moon, w = glint strength
// light_color: rgb = light color (linear), w = wave choppiness 0..1

#import bevy_pbr::{
    forward_io::VertexOutput,
    mesh_view_bindings::{globals, view},
    view_transformations::{frag_coord_to_ndc, position_ndc_to_world},
    prepass_utils,
}

struct WaterUniform {
    zenith: vec4<f32>,
    horizon: vec4<f32>,
    light_direction: vec4<f32>,
    light_color: vec4<f32>,
    // xy = fraction of the reflection texture in use (dynamic-resolution
    // viewport), z = live-mirror strength (0 = procedural fallback only).
    reflection: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> water: WaterUniform;
@group(#{MATERIAL_BIND_GROUP}) @binding(1) var reflection_texture: texture_2d<f32>;
@group(#{MATERIAL_BIND_GROUP}) @binding(2) var reflection_sampler: sampler;
@group(#{MATERIAL_BIND_GROUP}) @binding(3) var<uniform> reflection_clip_from_world: mat4x4<f32>;

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

// Wave heightfield: three octaves, direction rotated ~113° each octave so
// crests never line up, with a light domain warp from the running total.
fn wave_height(p_in: vec2<f32>, t_in: f32) -> f32 {
    var p = p_in;
    var direction = vec2<f32>(0.35, 0.94);
    let rotation = mat2x2<f32>(vec2<f32>(-0.39, 0.92), vec2<f32>(-0.92, -0.39));
    var amplitude = 1.0;
    var frequency = 1.4;
    var time = t_in;
    var total = 0.0;
    var norm = 0.0;
    for (var octave = 0; octave < 3; octave++) {
        total += value_noise_2d(p * frequency + direction * time) * amplitude;
        norm += amplitude;
        p += direction * total * 0.3;
        direction = rotation * direction;
        frequency *= 1.9;
        amplitude *= 0.55;
        time *= 1.35;
    }
    return total / norm;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let camera = view.world_position;
    let to_fragment = in.world_position.xyz - camera;
    let t_fragment = length(to_fragment);
    let ray = to_fragment / t_fragment;

    // Optical depth: how much water the ray crosses before the terrain
    // behind this surface. No prepass hit (rim spill against sky) = deep.
    var thickness = 4.0;
    let raw_depth = prepass_utils::prepass_depth(in.position, 0u);
    if (raw_depth > 0.0) {
        let scene_ndc = vec3<f32>(frag_coord_to_ndc(in.position).xy, raw_depth);
        let t_scene = distance(position_ndc_to_world(scene_ndc), camera);
        thickness = max(t_scene - t_fragment, 0.0);
    }

    // Waves: wind-driven choppiness, flattening with distance.
    let choppiness = water.light_color.w;
    let t = globals.time * (0.5 + 1.2 * choppiness);
    let calm = 1.0 - smoothstep(20.0, 90.0, t_fragment) * 0.85;
    let amplitude = (0.035 + 0.18 * choppiness) * calm;
    let p = in.world_position.xz * 0.8;
    let sample_step = 0.3;
    let height_center = wave_height(p, t);
    let height_x = wave_height(p + vec2<f32>(sample_step, 0.0), t);
    let height_z = wave_height(p + vec2<f32>(0.0, sample_step), t);
    var normal = normalize(vec3<f32>(
        (height_center - height_x) * amplitude / sample_step,
        1.0,
        (height_center - height_z) * amplitude / sample_step,
    ));
    // Side faces (waterfall lips at the rim) keep their own flat normal.
    if (in.world_normal.y < 0.5) {
        normal = normalize(in.world_normal);
    }

    let view_direction = -ray;
    let facing = max(dot(view_direction, normal), 0.0);
    let fresnel = 0.03 + 0.97 * pow(1.0 - facing, 5.0);

    // Beer–Lambert absorption. Real pond water has almost NO color of its
    // own (reference check): what you see is the dimmed bed plus the
    // reflection. The body term only darkens with depth — near-black with
    // the faintest green, never teal.
    let attenuation = vec3<f32>(1.8, 0.9, 0.65);
    let transmit = exp(-attenuation * thickness);
    let daylight = water.zenith.w;
    let moonlight = water.horizon.w;
    let light_level = max(daylight, moonlight * 0.25);
    let shallow_tint = vec3<f32>(0.035, 0.075, 0.065) * light_level;
    let deep_color = vec3<f32>(0.004, 0.011, 0.010) * light_level;
    let body = mix(deep_color, shallow_tint, transmit);

    // REAL planar reflections: a mirrored camera has already rendered the
    // above-water world this frame. Project this surface point through its
    // clip matrix and sample — points on the mirrored eye's ray project to
    // the same texel, so this is exactly what the reflected ray sees. The
    // sample point is nudged by the wave normal for a wavy mirror.
    let reflected = reflect(ray, normal);
    let sky_mix = pow(clamp(reflected.y, 0.0, 1.0), 0.6);
    let sky_color = mix(water.horizon.rgb, water.zenith.rgb, sky_mix);
    let wobble = vec3<f32>(normal.x, 0.0, normal.z) * 1.4;
    let reflection_clip = reflection_clip_from_world
        * vec4<f32>(in.world_position.xyz + wobble, 1.0);
    // Fallback when the mirror is off (far from water) or out of frame.
    var reflected_scene = sky_color * 0.55;
    let mirror_strength = water.reflection.z;
    if (mirror_strength > 0.001 && reflection_clip.w > 0.0) {
        let reflection_ndc = reflection_clip.xy / reflection_clip.w;
        let reflection_uv = vec2<f32>(
            reflection_ndc.x * 0.5 + 0.5,
            -reflection_ndc.y * 0.5 + 0.5,
        );
        if (all(reflection_uv >= vec2<f32>(0.0)) && all(reflection_uv <= vec2<f32>(1.0))) {
            // The live frame occupies only the dynamic-resolution viewport
            // corner of the texture; clamp away from its edge so filtering
            // never bleeds in stale texels from a previous, larger tier.
            let scaled_uv = clamp(reflection_uv, vec2<f32>(0.002), vec2<f32>(0.998))
                * water.reflection.xy;
            let mirror = textureSampleLevel(
                reflection_texture,
                reflection_sampler,
                scaled_uv,
                0.0,
            ).rgb;
            reflected_scene = mix(reflected_scene, mirror, mirror_strength);
        }
    }

    // Shallow water shows the bed through; deep water goes near-opaque.
    var alpha = mix(0.88, 0.07, transmit.g);
    // Waterline melts into the shore instead of a hard voxel seam.
    alpha *= smoothstep(0.0, 0.05, thickness);

    // Foam: a noisy band hugging the shore (and anything poking through).
    let shore = 1.0 - smoothstep(0.02, 0.32, thickness);
    let foam_noise = value_noise_2d(in.world_position.xz * 9.0 + vec2<f32>(t * 0.7, -t * 0.5));
    let foam = shore * smoothstep(0.42, 0.72, foam_noise + shore * 0.30);

    // Glints: a soft specular lobe plus pin-prick sparkles for the bloom.
    let half_direction = normalize(view_direction + water.light_direction.xyz);
    let spec_base = max(dot(normal, half_direction), 0.0);
    let glint = water.light_direction.w;
    let specular = pow(spec_base, 240.0) * glint * 4.5;
    let sparkle = pow(spec_base, 1300.0) * glint * 7.0;

    var color = mix(body, reflected_scene, fresnel);
    color += water.light_color.rgb * (specular + sparkle);
    color = mix(color, vec3<f32>(0.92, 0.96, 0.97) * max(light_level, 0.06), foam);
    alpha = max(alpha, foam * 0.85);
    alpha = clamp(alpha + fresnel * 0.18, 0.0, 1.0);

    return vec4<f32>(color, alpha);
}
