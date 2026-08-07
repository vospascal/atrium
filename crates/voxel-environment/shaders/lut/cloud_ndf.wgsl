// Procedural Nubis Data Field fallback.
//
// Evolved's authored NDF is a 2D, world-sized field. Its channels describe regional coverage,
// cloud type and density scale; the vertical profile is evaluated by the cloud sampler. This
// generated field keeps that contract alive until an authored NDF importer can upload the same
// RGBA texture. It is deliberately a separate table, not another slice of the 3D erosion field:
// macro weather must not inherit the 128^3 noise tile's ~kilometre repeat.

const CLOUD_NDF_EDGE: u32 = 256u;

fn cloud_ndf_hash(cell: vec2<f32>) -> f32 {
    return fract(sin(dot(cell, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn cloud_ndf_value(position: vec2<f32>) -> f32 {
    let cell = floor(position);
    let local = fract(position);
    let smooth_local = local * local * (3.0 - 2.0 * local);
    let a = cloud_ndf_hash(cell);
    let b = cloud_ndf_hash(cell + vec2<f32>(1.0, 0.0));
    let c = cloud_ndf_hash(cell + vec2<f32>(0.0, 1.0));
    let d = cloud_ndf_hash(cell + vec2<f32>(1.0, 1.0));
    return mix(mix(a, b, smooth_local.x), mix(c, d, smooth_local.x), smooth_local.y);
}

fn cloud_ndf_fbm(position: vec2<f32>) -> f32 {
    var value = 0.0;
    var amplitude = 0.5;
    var frequency = 1.0;
    for (var octave = 0; octave < 4; octave++) {
        value += cloud_ndf_value(position * frequency) * amplitude;
        frequency *= 2.0;
        amplitude *= 0.5;
    }
    return clamp(value / 0.9375, 0.0, 1.0);
}

@group(0) @binding(1) var cloud_ndf_target: texture_storage_2d<rgba8unorm, write>;

@compute @workgroup_size(8, 8, 1)
fn cloud_ndf_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (id.x >= CLOUD_NDF_EDGE || id.y >= CLOUD_NDF_EDGE) {
        return;
    }

    let uv = (vec2<f32>(id.xy) + vec2<f32>(0.5)) / f32(CLOUD_NDF_EDGE);
    // The Evolved sky NDF covers 16 km x 16 km. Eight broad cells across the tile give the
    // procedural fallback weather-sized organisation; the imported field will use the same UV
    // contract and can replace this pass without changing the density or lighting code.
    let regional = cloud_ndf_fbm(uv * 8.0);
    let weather = cloud_ndf_fbm(uv * 19.0 + vec2<f32>(17.0, 3.0));
    let type_signal = cloud_ndf_fbm(uv * 6.0 + vec2<f32>(4.0, 29.0));
    let density_signal = cloud_ndf_fbm(uv * 11.0 + vec2<f32>(41.0, 7.0));

    // R: coverage/layout, G: local type (wispy -> billowy), B: up-res density scale,
    // A: influence/precipitation mask reserved for authored weather blending.
    let coverage = smoothstep(0.28, 0.72, regional * 0.78 + weather * 0.22);
    let cloud_type = clamp(type_signal * 0.75 + regional * 0.25, 0.0, 1.0);
    let density_scale = mix(0.55, 1.0, density_signal);
    let influence = smoothstep(0.35, 0.8, weather);
    textureStore(
        cloud_ndf_target,
        vec2<i32>(id.xy),
        vec4<f32>(coverage, cloud_type, density_scale, influence),
    );
}
