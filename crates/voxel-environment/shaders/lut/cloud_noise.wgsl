// Tileable Perlin-Worley density field generation.
//
// One 128^3 Rgba8Unorm volume, generated once at startup and never regenerated: it is pure
// noise, so nothing about the sun, the camera or the weather can invalidate it. Weather
// reshapes the deck by *remapping* this field, not by rebuilding it.
//
// Channel layout, following Nubis but collapsed into one texture rather than a low-frequency
// base plus a separate erosion volume:
//   R  low-frequency Perlin-Worley  -> the base shape
//   G  Worley at 1x                 -> coarse erosion
//   B  Worley at 2x                 -> medium erosion
//   A  Worley at 4x                 -> fine erosion
//
// Every function here must tile, which is the whole reason the lattice wraps with `%`
// rather than being hashed on unbounded integers.

const CLOUD_NOISE_EDGE: u32 = 128u;

// Declared first because WGSL requires declaration before use — there are no forward
// declarations, so helper order in this file is a compile requirement, not a style choice.
fn remap_range(value: f32, from_low: f32, from_high: f32, to_low: f32, to_high: f32) -> f32 {
    let span = max(from_high - from_low, 0.0001);
    return to_low + (value - from_low) / span * (to_high - to_low);
}

fn cloud_hash_3d(cell: vec3<f32>) -> vec3<f32> {
    // Integer-lattice hash. The `fract` chain keeps this stable for the modest coordinates a
    // tiling lattice produces, which is all it is ever asked for.
    var point = vec3<f32>(
        dot(cell, vec3<f32>(127.1, 311.7, 74.7)),
        dot(cell, vec3<f32>(269.5, 183.3, 246.1)),
        dot(cell, vec3<f32>(113.5, 271.9, 124.6)),
    );
    point = fract(sin(point) * 43758.5453123);
    return point;
}

fn cloud_hash_1d(cell: vec3<f32>) -> f32 {
    return fract(sin(dot(cell, vec3<f32>(127.1, 311.7, 74.7))) * 43758.5453123);
}

// Worley (cellular) noise, INVERTED so that 1 is the cell centre.
//
// Inverted because cloud wants billows — bright rounded lumps — and raw Worley returns
// distance, which is dark at the centre. Doing it here rather than at sample time means the
// erosion channels can be used directly.
fn cloud_worley_3d(position: vec3<f32>, frequency: f32) -> f32 {
    let scaled = position * frequency;
    let base_cell = floor(scaled);
    let local = scaled - base_cell;
    var nearest = 1.0;
    for (var z = -1; z <= 1; z++) {
        for (var y = -1; y <= 1; y++) {
            for (var x = -1; x <= 1; x++) {
                let offset = vec3<f32>(f32(x), f32(y), f32(z));
                // Wrap the neighbour cell into the tile so opposite faces agree.
                let neighbour = base_cell + offset;
                let wrapped = neighbour - floor(neighbour / frequency) * frequency;
                let feature = offset + cloud_hash_3d(wrapped);
                nearest = min(nearest, length(feature - local));
            }
        }
    }
    return 1.0 - clamp(nearest, 0.0, 1.0);
}

// Value-noise gradient interpolation, tiling on `frequency`.
fn cloud_value_3d(position: vec3<f32>, frequency: f32) -> f32 {
    let scaled = position * frequency;
    let base_cell = floor(scaled);
    let local = scaled - base_cell;
    // Quintic fade: continuous second derivative, so FBM sums do not show lattice creases.
    let fade = local * local * local * (local * (local * 6.0 - 15.0) + 10.0);
    var accumulated = 0.0;
    for (var z = 0; z < 2; z++) {
        for (var y = 0; y < 2; y++) {
            for (var x = 0; x < 2; x++) {
                let offset = vec3<f32>(f32(x), f32(y), f32(z));
                let neighbour = base_cell + offset;
                let wrapped = neighbour - floor(neighbour / frequency) * frequency;
                let corner = cloud_hash_1d(wrapped);
                let weight = mix(1.0 - fade, fade, offset);
                accumulated += corner * weight.x * weight.y * weight.z;
            }
        }
    }
    return accumulated;
}

// Each octave is CENTRED before summing, and the sum normalized by its own total amplitude.
//
// This is not a stylistic choice. Summing unsigned value noise (each octave mean 0.5) piles the
// result up around its mean: measured, the previous unsigned version came out sd 0.105 over a
// p05..p95 span of 0.26..0.60, and after the Perlin-Worley remap below that fell to **sd 0.077**.
// A shape field with that little contrast is uniformly half-cloudy everywhere, which renders as
// MIST — no cores, no gaps. Centring and normalizing restores the full range (measured sd 0.211).
fn cloud_perlin_fbm(position: vec3<f32>) -> f32 {
    var total = 0.0;
    var amplitude = 0.5;
    var normalizer = 0.0;
    // Octave frequencies are NOT pure doublings, for the reason Heckel drifts his lacunarity
    // (`factor = 2.02; factor += 0.21`): exact doubling leaves every octave sharing one lattice,
    // so their features align and the sum keeps a visible grid character.
    //
    // But they must stay INTEGERS. This field tiles — it is sampled through a Repeat sampler at
    // several frequencies — and the wrap in `cloud_value_3d` only closes seamlessly when the
    // frequency is the tile period. Heckel's noise samples a 2D texture and never wraps a
    // lattice, so a fractional ratio costs him nothing and would cost us seams across the sky.
    // 4/9/19/41 are ratios of ~2.1-2.25, decorrelated and each an exact period.
    let frequencies = array<f32, 4>(4.0, 9.0, 19.0, 41.0);
    for (var octave = 0; octave < 4; octave++) {
        total += (cloud_value_3d(position, frequencies[octave]) * 2.0 - 1.0) * amplitude;
        normalizer += amplitude;
        amplitude *= 0.5;
    }
    return (total / max(normalizer, 0.0001)) * 0.5 + 0.5;
}

fn cloud_worley_fbm(position: vec3<f32>, base_frequency: f32) -> f32 {
    var total = 0.0;
    var amplitude = 0.5;
    var frequency = base_frequency;
    for (var octave = 0; octave < 3; octave++) {
        total += cloud_worley_3d(position, frequency) * amplitude;
        frequency *= 2.0;
        amplitude *= 0.5;
    }
    // The raw sum occupies about 0.31..0.67 of its nominal range (measured over the tile), so it
    // is stretched to 0..1 rather than left in the middle third where it carries little signal.
    return clamp(remap_range(total / 0.875, 0.31, 0.67, 0.0, 1.0), 0.0, 1.0);
}

// Perlin-Worley: Perlin remapped by Worley so the result keeps Perlin's connected, wispy
// structure while gaining Worley's rounded billows. Neither alone reads as cloud.
fn cloud_perlin_worley(position: vec3<f32>) -> f32 {
    let perlin = cloud_perlin_fbm(position);
    let worley = cloud_worley_fbm(position, 4.0);
    // Remap perlin's low end onto worley rather than blending: a blend averages the two
    // characters away, the remap keeps both. The 0.55/-0.15 window is narrower than the
    // textbook `worley - 1.0`, which spanned up to 2.0 and so COMPRESSED perlin instead of
    // shaping it — the second half of the mist bug.
    let combined = clamp(remap_range(perlin, worley * 0.55 - 0.15, 1.0, 0.0, 1.0), 0.0, 1.0);
    // Smoothstep contrast: pushes the mid-range apart so both cores and clear gaps exist.
    let shaped = clamp(combined * combined * (3.0 - 2.0 * combined), 0.0, 1.0);

    // FLATTEN the distribution to approximately uniform on 0..1.
    //
    // This is not cosmetic — it is what makes `coverage` mean coverage. The consumer does
    // `remap(base, 1.0 - coverage, 1.0, 0.0, 1.0)`, which only yields "a `coverage` fraction of the
    // sky has cloud" if the field is uniformly distributed. Measured, `shaped` comes out
    // p05 0.001 / p50 0.330 / p95 0.688 — concentrated low. So a requested coverage of 0.45 set a
    // floor of 0.55 that most of the field could not clear, and once the weather map lowered local
    // coverage to ~0.31 the floor became 0.693 against a p95 of 0.688: **no clouds at all**.
    //
    // A linear stretch of the measured p05..p95 band onto 0.05..0.95 fits the three percentile
    // anchors to within 0.02 and costs one remap. Re-measure and re-fit these two constants if the
    // octave weights or the contrast curve above ever change — they are calibrated against this
    // exact chain, which is why they are stated as numbers rather than hidden in a magic gamma.
    return clamp(remap_range(shaped, 0.001, 0.688, 0.05, 0.95), 0.0, 1.0);
}

@group(0) @binding(1) var cloud_noise_target: texture_storage_3d<rgba8unorm, write>;
@group(0) @binding(2) var cloud_noise_source: texture_3d<f32>;

@compute @workgroup_size(4, 4, 4)
fn cloud_noise_main(@builtin(global_invocation_id) id: vec3<u32>) {
    if (any(id >= vec3<u32>(CLOUD_NOISE_EDGE))) {
        return;
    }
    // Sample at texel centres so the tile wraps exactly rather than half a texel out.
    let position = (vec3<f32>(id) + vec3<f32>(0.5)) / f32(CLOUD_NOISE_EDGE);
    let base = cloud_perlin_worley(position);
    let erosion_coarse = cloud_worley_fbm(position, 8.0);
    let erosion_medium = cloud_worley_fbm(position, 16.0);
    let erosion_fine = cloud_worley_fbm(position, 32.0);
    textureStore(
        cloud_noise_target,
        vec3<i32>(id),
        vec4<f32>(base, erosion_coarse, erosion_medium, erosion_fine),
    );
}

// Box-filter one authored/generated level into the next. The source and target views each expose
// one mip level, so level zero is always the correct textureLoad level here.
@compute @workgroup_size(4, 4, 4)
fn cloud_noise_mip_main(@builtin(global_invocation_id) id: vec3<u32>) {
    var value = vec4<f32>(0.0);
    let source_origin = vec3<i32>(id) * 2;
    for (var z = 0; z < 2; z++) {
        for (var y = 0; y < 2; y++) {
            for (var x = 0; x < 2; x++) {
                value += textureLoad(
                    cloud_noise_source,
                    source_origin + vec3<i32>(x, y, z),
                    0,
                );
            }
        }
    }
    textureStore(cloud_noise_target, vec3<i32>(id), value * 0.125);
}
