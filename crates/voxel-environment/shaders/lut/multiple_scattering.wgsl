@group(0) @binding(1) var multiple_scattering_out: texture_storage_2d<rgba16float, write>;

@compute @workgroup_size(8, 8, 1)
fn atmosphere_multiple_scattering_main(@builtin(global_invocation_id) id: vec3<u32>) {
    let size = vec2<u32>(32u, 32u);
    if (any(id.xy >= size)) {
        return;
    }
    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) / vec2<f32>(size);
    let origin = atmosphere_origin_at_height(uv.y);
    let height = atmosphere_height(uv.y);
    let sun_mu = uv.x * 2.0 - 1.0;
    let sun_direction = normalize(vec3<f32>(sqrt(max(1.0 - sun_mu * sun_mu, 0.0)), sun_mu, 0.0));
    let sun_transmittance = atmosphere_light_transmittance(origin, sun_direction);
    let moon_direction = normalize(atmosphere.moon_direction);
    let moon_transmittance = atmosphere_light_transmittance(origin, moon_direction);
    let density = rayleigh_density(height) + mie_density(height) * 0.35;
    let angular = 0.5 + 0.5 * max(sun_mu, 0.0);
    let moon_angular = 0.5 + 0.5 * max(moon_direction.y, 0.0);
    let scattering = (
        atmosphere.sun_illuminance * (1.0 - sun_transmittance) * angular
        + atmosphere.moon_illuminance * (1.0 - moon_transmittance) * moon_angular
    ) * density * 0.18;
    textureStore(multiple_scattering_out, vec2<i32>(id.xy), vec4<f32>(scattering, 1.0));
}
