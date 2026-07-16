// Precipitation particles: one billboarded quad per particle, wrapped
// vertically (fall) and horizontally (wind drift) inside a box volume that
// follows the camera. Rain quads stretch along their velocity; snow quads
// face the camera and sway.
//
// velocity: xyz = particle velocity (m/s, world), w = intensity 0..1
// shape:    x = quad width, y = quad length, z = sway amplitude,
//           w = snowiness (0 rain, 1 snow)
// color:    particle tint, alpha = opacity
// drift:    xy = accumulated wind drift (m), z = volume half-extent,
//           w = volume height

#import bevy_pbr::{
    mesh_functions,
    forward_io::{Vertex, VertexOutput},
    view_transformations::position_world_to_clip,
    mesh_view_bindings::{globals, view},
}

struct PrecipitationUniform {
    velocity: vec4<f32>,
    shape: vec4<f32>,
    color: vec4<f32>,
    drift: vec4<f32>,
}

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> precipitation: PrecipitationUniform;

fn wrap(value: f32, extent: f32) -> f32 {
    return fract(value / extent) * extent;
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

    let corner = vertex.uv - vec2<f32>(0.5, 0.5);
    let seeds = vertex.uv_b;
    let intensity = precipitation.velocity.w;

    // Particles above the intensity threshold collapse to nothing.
    if (seeds.y > intensity) {
        out.position = vec4<f32>(0.0, 0.0, 2.0, 1.0);
        out.world_position = vec4<f32>(0.0);
        out.uv = vertex.uv;
        return out;
    }

    let half_extent = precipitation.drift.z;
    let height = precipitation.drift.w;
    let t = globals.time;

    var local = vertex.position;
    // Fall: wrap vertically, each particle phase-offset by its seed.
    let fall_speed = -precipitation.velocity.y;
    local.y = wrap(vertex.position.y - t * fall_speed + seeds.x * height, height);
    // Wind drift: wrap horizontally around the volume center.
    local.x = wrap(vertex.position.x + precipitation.drift.x + half_extent, 2.0 * half_extent)
        - half_extent;
    local.z = wrap(vertex.position.z + precipitation.drift.y + half_extent, 2.0 * half_extent)
        - half_extent;
    // Snow sway.
    let sway = precipitation.shape.z;
    local.x += sway * sin(t * (0.8 + seeds.x * 1.5) + seeds.y * 40.0);
    local.z += sway * cos(t * (0.6 + seeds.y * 1.5) + seeds.x * 40.0);

    // Billboard: rain stretches along its velocity, snow faces the camera.
    let camera_right = view.world_from_view[0].xyz;
    let camera_up = view.world_from_view[1].xyz;
    let velocity_direction = normalize(precipitation.velocity.xyz + vec3<f32>(0.0, -0.001, 0.0));
    let axis = normalize(mix(-velocity_direction, camera_up, precipitation.shape.w));
    let offset_local = camera_right * corner.x * precipitation.shape.x
        + axis * corner.y * precipitation.shape.y;
    // Uniform entity scale (no rotation) — scale the billboard with it.
    let entity_scale = length(world_from_local[0].xyz);

    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(local, 1.0),
    ) + vec4<f32>(offset_local * entity_scale, 0.0);
    out.position = position_world_to_clip(out.world_position.xyz);
    out.uv = vertex.uv;
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
    let snowiness = precipitation.shape.w;
    // Rain streak: fade both tips. Snow flake: round falloff.
    let streak = clamp(4.0 * in.uv.y * (1.0 - in.uv.y), 0.0, 1.0);
    let radial = length(in.uv - vec2<f32>(0.5, 0.5));
    let flake = smoothstep(0.5, 0.15, radial);
    let profile = mix(streak, flake, snowiness);
    let alpha = precipitation.color.a * profile;
    return vec4<f32>(precipitation.color.rgb, alpha);
}
