// Shared Material Graph ABI and legacy fallback. The Rust graph compiler emits
// functions returning this struct; the DDA adapter inserts slot branches at the
// marked point. `graph_active` keeps the legacy pattern stack on the old path.
struct GraphMaterial {
    base_color: vec4<f32>,
    roughness: f32,
    emission: vec4<f32>,
    graph_active: bool,
    face_color_active: bool,
    face_roughness_active: bool,
}

// Shared procedural helpers used by material graph functions.
fn graph_hash3(point: vec3<f32>) -> f32 {
    return fract(sin(dot(point, vec3<f32>(127.1, 311.7, 74.7))) * 43758.5453);
}

fn graph_value_noise(point: vec3<f32>) -> f32 {
    let cell = floor(point);
    let local = fract(point);
    let blend = local * local * (vec3<f32>(3.0, 3.0, 3.0) - 2.0 * local);
    let n000 = graph_hash3(cell + vec3<f32>(0.0, 0.0, 0.0));
    let n100 = graph_hash3(cell + vec3<f32>(1.0, 0.0, 0.0));
    let n010 = graph_hash3(cell + vec3<f32>(0.0, 1.0, 0.0));
    let n110 = graph_hash3(cell + vec3<f32>(1.0, 1.0, 0.0));
    let n001 = graph_hash3(cell + vec3<f32>(0.0, 0.0, 1.0));
    let n101 = graph_hash3(cell + vec3<f32>(1.0, 0.0, 1.0));
    let n011 = graph_hash3(cell + vec3<f32>(0.0, 1.0, 1.0));
    let n111 = graph_hash3(cell + vec3<f32>(1.0, 1.0, 1.0));
    let x00 = mix(n000, n100, blend.x);
    let x10 = mix(n010, n110, blend.x);
    let x01 = mix(n001, n101, blend.x);
    let x11 = mix(n011, n111, blend.x);
    return mix(mix(x00, x10, blend.y), mix(x01, x11, blend.y), blend.z);
}

fn graph_fbm(point: vec3<f32>, octaves: f32, roughness: f32) -> f32 {
    var total = 0.0;
    var amplitude = 1.0;
    var frequency = 1.0;
    var normalisation = 0.0;
    for (var octave = 0u; octave < 8u; octave = octave + 1u) {
        let enabled = select(0.0, 1.0, f32(octave) < max(octaves, 1.0));
        total = total + graph_value_noise(point * frequency) * amplitude * enabled;
        normalisation = normalisation + amplitude * enabled;
        frequency = frequency * 2.0;
        amplitude = amplitude * clamp(roughness, 0.0, 1.0);
    }
    return select(0.0, total / normalisation, normalisation > 0.0);
}

fn graph_safe_normalize(vector: vec3<f32>) -> vec3<f32> {
    let magnitude = length(vector);
    return select(vec3<f32>(0.0, 1.0, 0.0), vector / magnitude, magnitude > 0.000001);
}

fn graph_face_color(normal: vec3<f32>, base: vec4<f32>, top: vec4<f32>, side: vec4<f32>, bottom: vec4<f32>) -> vec4<f32> {
    if (normal.y > 0.5) { return top; }
    if (normal.y < -0.5) { return bottom; }
    return side;
}

fn graph_face_scalar(normal: vec3<f32>, base: f32, top: f32, side: f32, bottom: f32) -> f32 {
    if (normal.y > 0.5) { return top; }
    if (normal.y < -0.5) { return bottom; }
    return side;
}

fn material_graph_surface(material: u32, position: vec3<f32>, normal: vec3<f32>) -> GraphMaterial {
    // GRAPH_DISPATCH_POINT
    let row = materials[material];
    return GraphMaterial(vec4<f32>(row.albedo, 1.0), row.roughness,
                         vec4<f32>(row.emission, 1.0), false, false, false);
}
