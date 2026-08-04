@group(0) @binding(1) var transmittance_out: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn atmosphere_transmittance_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(256u, 64u);
    if (any(id.xy >= size)) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) / vec2<f32>(size);
    let origin = atmosphere_origin_at_height(uv.y);
    let mu = uv.x * 2.0 - 1.0;
    let direction = normalize(vec3<f32>(sqrt(max(1.0 - mu * mu, 0.0)), mu, 0.0));
    let distance = atmosphere_ray_distance(origin, direction);
    let transmittance = atmosphere_transmittance_segment(origin, direction, distance);
    textureStore(transmittance_out, vec2<i32>(id.xy), vec4<f32>(transmittance, 1.0));
}
