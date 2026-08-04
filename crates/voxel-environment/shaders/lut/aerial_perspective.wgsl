@group(0) @binding(1) var aerial_perspective_out: texture_storage_3d<rgba16float, write>;

@compute @workgroup_size(4, 4, 4)
fn atmosphere_aerial_perspective_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec3<u32>(32u, 32u, 32u);
    if (any(id >= size)) {
        return;
    }
    let uv = (vec3<f32>(id) + vec3<f32>(0.5)) / vec3<f32>(size);
    let distance_km = uv.z * 32.0;
    let camera_height = atmosphere.camera_position.y / atmosphere.from_kilometers_scale
        / (atmosphere.top_radius_km - atmosphere.bottom_radius_km);
    let origin = atmosphere_origin_at_height(camera_height);
    let direction = atmosphere_view_direction(uv.xy);
    let distance = min(distance_km, atmosphere_ray_distance(origin, direction));
    let inscatter = atmosphere_scattering(origin, direction, distance);
    let transmittance = atmosphere_transmittance_segment(origin, direction, distance);
    textureStore(aerial_perspective_out, vec3<i32>(id), vec4<f32>(inscatter, transmittance.r));
}
