// Firefly motes: billboarded quads wandering on per-particle sine paths,
// blinking with individual rhythms. HDR-bright cores feed the bloom pass.
// The swarm's world position salts the wander phases, so two swarms with
// the shared mesh never move in lockstep.
//
// params: x = swarm width, y = mote size, z = glow gain,
//         w = night factor (0 by day — motes collapse entirely)
// tint:   rgb = mote color (linear), w = amount fraction (seed cull)
// motion: x = blink tempo, y = swarm height

#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::{globals, view},
}

struct FireflyUniform {
    params: vec4<f32>,
    tint: vec4<f32>,
    motion: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> firefly: FireflyUniform;

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

    let night = firefly.params.w;
    let seeds = vertex.uv_b;
    // Collapse by day, and collapse motes beyond the panel's amount.
    if (night < 0.02 || seeds.y > firefly.tint.w) {
        out.position = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        out.world_position = vec4<f32>(0.0);
        out.uv = vertex.uv;
        out.uv_b = vertex.uv_b;
        return out;
    }

    let width = firefly.params.x;
    let height = firefly.motion.y;
    let size = firefly.params.y;
    // Salt time by the swarm's position so shared-mesh swarms desync.
    let swarm_position = world_from_local[3].xyz;
    let t = globals.time
        + fract(dot(swarm_position, vec3<f32>(0.173, 0.291, 0.117))) * 100.0;

    // Wander: home offset plus layered sines, each mote its own tempo.
    // Horizontal motion scales with width, vertical with height.
    let tempo = 0.25 + seeds.x * 0.45;
    let phase = seeds.y * 40.0;
    var local = vertex.position * vec3<f32>(width, height, width);
    local += vec3<f32>(
        (sin(t * tempo + phase) * 0.9 + sin(t * tempo * 2.3 + phase * 1.7) * 0.25) * width,
        sin(t * tempo * 1.4 + phase * 0.6) * 0.45 * height,
        (cos(t * tempo * 0.8 + phase) * 0.9 + cos(t * tempo * 2.9 + phase * 0.3) * 0.25) * width,
    ) * 0.35;

    let corner = vertex.uv - vec2<f32>(0.5, 0.5);
    let camera_right = view.world_from_view[0].xyz;
    let camera_up = view.world_from_view[1].xyz;
    let offset = (camera_right * corner.x + camera_up * corner.y) * size;

    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(local, 1.0),
    ) + vec4<f32>(offset, 0.0);
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
    let night = firefly.params.w;
    let gain = firefly.params.z;
    let seeds = in.uv_b;

    // Individual blink rhythm: most motes glow at any moment, dipping to
    // a warm ember rather than going dark (dense light-show look).
    let rate = (0.4 + seeds.x * 0.8) * firefly.motion.x;
    let pulse = smoothstep(0.15, 0.65, sin(globals.time * rate * 6.28 + seeds.y * 40.0));
    let glow = 0.14 + 0.86 * pulse;

    // Fat orb with a soft rim, so bloom + depth of field render each mote
    // as a glowing bokeh ball.
    let radial = length(in.uv - vec2<f32>(0.5, 0.5));
    let core = smoothstep(0.5, 0.22, radial);

    // Panel-tinted, HDR so bloom halos each mote.
    let color = firefly.tint.rgb * gain;
    let strength = core * glow * night;
    return vec4<f32>(color * strength, strength);
}
