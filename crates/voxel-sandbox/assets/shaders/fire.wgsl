// Procedural campfire flame + rising embers.
//
// One mesh: a big flame quad (uv_b.x = -1 sentinel) billboarded around
// the y axis, and small ember quads that rise, sway, and fade. The flame
// fragment shapes scrolling fbm noise into a teardrop with a fire color
// ramp — HDR core so the bloom pass gives it a glow.
//
// params: x = flame width (m), y = flame height (m), z = HDR gain,
//         w = flame base height above the prop origin (m)

#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::{globals, view},
}

struct FireUniform {
    params: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> fire: FireUniform;

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

fn fbm2(p: vec2<f32>) -> f32 {
    return (value_noise_2d(p) * 0.55
        + value_noise_2d(p * 2.3 + 11.7) * 0.3
        + value_noise_2d(p * 4.9 + 27.3) * 0.15);
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);
    let fire_position = world_from_local[3].xyz;
    let corner = vertex.uv;
    let seeds = vertex.uv_b;
    let t = globals.time
        + fract(dot(fire_position, vec3<f32>(0.173, 0.291, 0.117))) * 100.0;

    var local: vec3<f32>;
    if (seeds.x < -0.5) {
        // Flame: cylindrical billboard (rotates around y only, so it stays
        // upright), anchored at its base.
        let to_camera = view.world_position - fire_position;
        let right = normalize(vec3<f32>(-to_camera.z, 0.0, to_camera.x));
        local = right * (corner.x - 0.5) * fire.params.x
            + vec3<f32>(0.0, fire.params.w + corner.y * fire.params.y, 0.0);
    } else {
        // Ember: rises from the flame, swaying, shrinking as it climbs.
        let cycle = fract(t * (0.16 + seeds.x * 0.22) + seeds.y);
        let sway_phase = seeds.y * 40.0 + t * (1.2 + seeds.x);
        let rise = cycle * (0.9 + seeds.y * 0.5);
        let size = 0.020 * (1.0 - cycle * 0.6);
        let camera_right = view.world_from_view[0].xyz;
        let camera_up = view.world_from_view[1].xyz;
        local = vec3<f32>(
            sin(sway_phase) * (0.05 + rise * 0.10),
            fire.params.w + 0.25 + rise,
            cos(sway_phase * 0.7) * (0.05 + rise * 0.10),
        ) + (camera_right * (corner.x - 0.5) + camera_up * (corner.y - 0.5)) * size;
    }

    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(local, 1.0),
    );
    out.position = position_world_to_clip(out.world_position.xyz);
    out.uv = vertex.uv;
    out.uv_b = vertex.uv_b;
#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
    return out;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let gain = fire.params.z;
    let seeds = in.uv_b;
    let t = globals.time;

    if (seeds.x >= -0.5) {
        // Ember: glowing dot, cooling from yellow-orange toward red.
        let cycle = fract(t * (0.16 + seeds.x * 0.22) + seeds.y);
        let radial = length(in.uv - vec2<f32>(0.5, 0.5));
        let core = smoothstep(0.5, 0.15, radial);
        let cooling = mix(vec3<f32>(1.0, 0.62, 0.18), vec3<f32>(0.9, 0.18, 0.04), cycle);
        let strength = core * (1.0 - cycle) * (0.7 + 0.3 * sin(t * 9.0 + seeds.y * 30.0));
        return vec4<f32>(cooling * gain * strength, strength);
    }

    // Flame body: p.x in -1..1, p.y 0 at the base, 1 at the tip.
    let p = vec2<f32>(in.uv.x * 2.0 - 1.0, in.uv.y);
    let churn = fbm2(vec2<f32>(p.x * 2.6, p.y * 3.2 - t * 2.4));
    let lick = fbm2(vec2<f32>(p.x * 5.5 + 7.0, p.y * 6.5 - t * 3.9));
    let noise = churn * 0.7 + lick * 0.3;

    // Teardrop envelope, gnawed by the noise; licks stretch the tip.
    let envelope = (1.0 - p.y) * (0.62 + 0.5 * noise) + 0.06;
    let across = 1.0 - clamp(abs(p.x) / max(envelope, 0.001), 0.0, 1.0);
    let vertical = clamp(1.25 - p.y - (noise - 0.5) * 0.9, 0.0, 1.0);
    var heat = across * vertical;
    heat *= 0.82 + 0.18 * sin(t * 11.0 + p.x * 5.0);

    // Fire ramp: red edges → orange → yellow → near-white core.
    var color = mix(vec3<f32>(0.85, 0.16, 0.02), vec3<f32>(1.0, 0.48, 0.04),
        smoothstep(0.20, 0.50, heat));
    color = mix(color, vec3<f32>(1.0, 0.85, 0.30), smoothstep(0.50, 0.78, heat));
    color = mix(color, vec3<f32>(1.0, 0.98, 0.80), smoothstep(0.82, 0.96, heat));

    let alpha = smoothstep(0.12, 0.38, heat);
    return vec4<f32>(color * gain * (0.4 + 0.6 * heat), alpha);
}
