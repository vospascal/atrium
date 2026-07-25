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
    // rgb = body-colour tint, w = reflectivity (V-panel surface controls).
    surface: vec4<f32>,
    // x = depth-darkening scale. Rest reserved.
    surface_extra: vec4<f32>,
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

    // Underwater looking UP at the surface (camera below it, top face, ray
    // heading up). The material is double-sided so this face renders from below.
    // Keep it DEAD SIMPLE: a clean tinted glass — NO shimmer, NO reflections, NO
    // bright rim (all of which read as an unwanted "white haze" looking up).
    // Just the water tint at the chosen transparency; the sky shows through.
    if camera.y < in.world_position.y - 0.02 && ray.y > 0.0 && in.world_normal.y > 0.5 {
        let light_level = max(water.zenith.w, water.horizon.w * 0.25);
        let film = water.surface.rgb * 2.0 * max(light_level, 0.12);
        // DISTANCE DARKENING: the surface straight overhead is close, so it stays
        // clear (see the sky through); toward the horizon each surface point is
        // seen through far more water, so it fogs to the opaque water tint.
        // `t_fragment` is the distance to this surface point. `surface_extra.y`
        // (underside opacity) sets the base clarity straight up.
        let opacity = clamp(water.surface_extra.y, 0.0, 1.0);
        let fade = 1.0 - exp(-t_fragment * 0.15);
        let near_alpha = 0.04 + opacity * 0.55;
        let alpha = clamp(mix(near_alpha, 0.96, fade), 0.0, 0.97);
        return vec4<f32>(film, alpha);
    }

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

    // Depth via Beer–Lambert absorption, kept SUBTLE — the water isn't really
    // "tinted": shallows read clear (the bed shows through) and deep water only
    // darkens with a faint cool cast. Depth reads as clarity + darkening, and
    // the surface colour is carried mostly by the fresnel reflection.
    let attenuation = vec3<f32>(1.4, 0.7, 0.5) * water.surface_extra.x;
    let transmit = exp(-attenuation * thickness);
    let daylight = water.zenith.w;
    let moonlight = water.horizon.w;
    let light_level = max(daylight, moonlight * 0.25);
    let shallow_tint = vec3<f32>(0.03, 0.06, 0.07) * light_level; // near-clear
    let deep_color = vec3<f32>(0.006, 0.018, 0.04) * light_level; // faint cool depth
    // Surface tint (V-panel) shifts the water's own body colour.
    let body = mix(deep_color, shallow_tint, transmit) * water.surface.rgb;

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

    // Glints: a soft specular lobe plus pin-prick sparkles for the bloom.
    let half_direction = normalize(view_direction + water.light_direction.xyz);
    let spec_base = max(dot(normal, half_direction), 0.0);
    let glint = water.light_direction.w;
    let specular = pow(spec_base, 240.0) * glint * 4.5;
    let sparkle = pow(spec_base, 1300.0) * glint * 7.0;

    // Reflectivity (V-panel) scales how much sky/mirror the surface shows.
    let reflect_amount = clamp(fresnel * water.surface.w, 0.0, 1.0);
    var color = mix(body, reflected_scene, reflect_amount);
    color += water.light_color.rgb * (specular + sparkle);
    alpha = clamp(alpha + reflect_amount * 0.18, 0.0, 1.0);

    return vec4<f32>(color, alpha);
}
