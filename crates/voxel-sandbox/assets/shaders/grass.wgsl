// Grass main-pass vertex shader: Bevy's default mesh vertex transform (no
// skin/morph — grass is static geometry) plus a wind sway on the world
// position. The fragment is StandardMaterial's default (vertex-color tone), so
// this only overrides the vertex stage. Its positions MUST match
// grass_prepass.wgsl bit-for-bit (same shared wind fn) or the depth prepass
// z-fights.

#import bevy_pbr::{
    mesh_functions,
    view_transformations::position_world_to_clip,
    forward_io::{Vertex, VertexOutput},
}

// Wind time on the material bind group (x = now, y = previous frame). NOT
// `globals`, which isn't bound in the depth prepass — grass_prepass.wgsl reads
// this same uniform so both passes agree on depth.
@group(#{MATERIAL_BIND_GROUP}) @binding(100)
var<uniform> grass_wind_time: vec4<f32>;
// The live weather wind: xy = unit ground direction (x, z), z = strength 0..1
// (see `FULL_SWAY_WIND_SPEED`). Same uniform in both passes, for the same reason.
@group(#{MATERIAL_BIND_GROUP}) @binding(101)
var<uniform> grass_wind: vec4<f32>;

// Wind offset, INLINED (kept byte-identical to the other pass so the main and
// prepass passes compute the same depth). `phase` and `height` both arrive as
// VERTEX DATA now, not from the instance transform: grass clumps are baked
// into one mesh per chunk, so there is no per-clump transform left to read a
// phase from, and `position.y` is world space rather than blade-local. See
// `grass::build_chunk_grass_mesh` — UV.x is the clump's wind phase, UV.y is the
// unscaled blade height.
const WIND_STRENGTH: f32 = 0.35;
fn grass_wind_offset(phase: f32, height: f32, time: f32) -> vec3<f32> {
    // `grass_wind.z` is the weather's wind speed normalised to 0..1. It drives
    // three things at once, which is what makes the blades actually read as
    // responding to the wind rather than idling at a fixed wobble:
    //   1. flutter RATE  — blades whip faster as it picks up,
    //   2. wobble DEPTH  — a calm day keeps a small idle breeze,
    //   3. a steady LEAN downwind, which is the cue that says "wind direction".
    let strength = clamp(grass_wind.z, 0.0, 1.0);
    let rate = 0.55 + 1.45 * strength;
    let sway_x = sin(time * 1.4 * rate + phase) * 0.6 + sin(time * 2.7 * rate + phase * 1.9) * 0.4;
    let sway_z = cos(time * 1.1 * rate + phase * 1.3) * 0.6 + cos(time * 2.3 * rate + phase * 0.7) * 0.4;
    let wobble = 0.22 + 0.78 * strength;
    let lean = grass_wind.xy * (0.95 * strength);
    let bend = max(height, 0.0) * 0.66 * WIND_STRENGTH;
    return vec3<f32>((sway_x * wobble + lean.x) * bend, 0.0, (sway_z * wobble + lean.y) * bend);
}

@vertex
fn vertex(vertex: Vertex) -> VertexOutput {
    var out: VertexOutput;

    let world_from_local = mesh_functions::get_world_from_local(vertex.instance_index);

#ifdef VERTEX_NORMALS
    out.world_normal = mesh_functions::mesh_normal_local_to_world(
        vertex.normal,
        vertex.instance_index,
    );
#endif

#ifdef VERTEX_POSITIONS
    out.world_position = mesh_functions::mesh_position_local_to_world(
        world_from_local,
        vec4<f32>(vertex.position, 1.0),
    );
    let wind = grass_wind_offset(vertex.uv.x, vertex.uv.y, grass_wind_time.x);
    out.world_position = vec4<f32>(out.world_position.xyz + wind, out.world_position.w);
    out.position = position_world_to_clip(out.world_position.xyz);
#endif

#ifdef VERTEX_UVS_A
    out.uv = vertex.uv;
#endif
#ifdef VERTEX_UVS_B
    out.uv_b = vertex.uv_b;
#endif
#ifdef VERTEX_TANGENTS
    out.world_tangent = mesh_functions::mesh_tangent_local_to_world(
        world_from_local,
        vertex.tangent,
        vertex.instance_index,
    );
#endif
#ifdef VERTEX_COLORS
    out.color = vertex.color;
#endif
#ifdef VERTEX_OUTPUT_INSTANCE_INDEX
    out.instance_index = vertex.instance_index;
#endif
#ifdef VISIBILITY_RANGE_DITHER
    out.visibility_range_dither = mesh_functions::get_visibility_range_dither_level(
        vertex.instance_index, world_from_local[3]);
#endif

    return out;
}
