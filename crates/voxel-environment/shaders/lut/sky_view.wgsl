@group(0) @binding(1) var sky_view_out: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn atmosphere_sky_view_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(192u, 108u);
    if (any(id.xy >= size)) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) / vec2<f32>(size);
    let direction = atmosphere_view_direction(uv);
    let camera_height = atmosphere.camera_position.y / atmosphere.from_kilometers_scale
        / (atmosphere.top_radius_km - atmosphere.bottom_radius_km);
    let origin = atmosphere_origin_at_height(camera_height);
    let distance = atmosphere_ray_distance(origin, direction);
    let radiance = atmosphere_scattering(origin, direction, distance);
    textureStore(sky_view_out, vec2<i32>(id.xy), vec4<f32>(radiance, 1.0));
}
