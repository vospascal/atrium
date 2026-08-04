// cagi_volume.wgsl — E4 CAGI, the SHARED half of the light volume: its
// bindings, its integer packing, and the sampler the shading pass reads it
// with. Concatenated after `world.wgsl` into BOTH pass shaders:
//
//   dda source  = world.wgsl + cagi_volume.wgsl + dda.wgsl
//   cagi source = world.wgsl + cagi_volume.wgsl + cagi.wgsl
//
// so the packing and the grid indexing exist exactly once — the cellular
// automaton that writes the volume and the shading pass that reads it can never
// disagree about the layout. The propagation rule itself lives in `cagi.wgsl`
// (only that pass needs it) together with the write binding.
//
// Own bindings (group 0), shared by both consumers:
//  11  storage  light_volume        — the FRONT ping-pong buffer: one u32 per
//                                    cell, layout below (read-only here; the
//                                    CA writes the BACK buffer at binding 12)
//  13  storage  cagi_cell_data — two u32 words per cell: the bounce attribute
//                                    word followed by E5b's packed HDR emission.
//                                    exposed-area-weighted emission.
//  14  uniform  CagiVolumeMeta      — grid dimensions, the integer transport
//                                    coefficients and the S3b event-response
//                                    table (cagi.rs CagiVolumeUniform)
//
// PACKING (the "pure integer" property the dossier's CA GI depends on — no
// float accumulation anywhere in the transport, so the volume is deterministic
// and noiseless):
//
//   bits  0..9   red mantissa
//   bits 10..19  green mantissa
//   bits 20..29  blue mantissa
//   bits 30..31  shared exponent (0..3), restoring a 1/2/4/8 scale
//
// The shared exponent keeps the storage at one u32 while preserving HDR source
// values and avoiding the old silent clamp at linear radiance 1.0.

// S3b — one row of the event-response table: how a cell whose attribute word
// carries this row's index modulates its stored emission as an event comes and
// goes. Mirrors `GpuEventResponse` in src/cagi.rs (48 bytes, three 16-byte rows).
//
// The stored emission is the channel-wise MAX of the material's resting and
// triggered ends, and both scales are fractions of it — so a surface that is
// black until something arrives has resting_scale 0, and one that goes dark on
// approach has triggered_scale 0. There is no `invert` flag: the two scales
// already say which way round it is.
struct CagiEventResponse {
    radius_meters: f32,
    attack_seconds: f32,
    hold_seconds: f32,
    release_seconds: f32,
    resting_scale: vec3<f32>,
    channel: f32,
    triggered_scale: vec3<f32>,
    falloff: f32,
}

struct CagiVolumeMeta {
    // Cells along each axis. Y is CLAMPED to the world's occupied height plus a
    // margin (src/cagi.rs) — everything above that is open sky by definition,
    // so allocating it would be paying for a constant.
    grid_size: vec3<u32>,
    // Voxels per cell edge (2, 4 or 8 — always a divisor of the 8-voxel brick,
    // so a cell never straddles two bricks).
    cell_voxels: u32,
    // f32(cell_voxels), the sample math's scale factor.
    cell_size_voxels: f32,
    // Max-decrement rule: light lost per cell step in packed HDR channel units. Derived on
    // the CPU from a per-METER attenuation so the flood's reach in meters does
    // not change when the resolution lever moves.
    attenuation: u32,
    // Diffusion rule: L = (sum_of_6_neighbours * numerator) >> 12.
    diffusion_numerator: u32,
    // Diffusion rule, 26-neighbour variant: the weighted sum (face 4, edge 2,
    // corner 1) times this, >> 12.
    diffusion_26_numerator: u32,
    // S3b — row 0 is identity ("this cell does not answer events"); rows 1-7 are
    // allocated by src/cagi.rs and indexed by CAGI_CELL_EVENT_RESPONSE_MASK.
    //
    // Starts at offset 32 with no padding before it: the geometry half above
    // ends there, and 32 already satisfies the 16-byte alignment an array of
    // 16-byte-aligned elements demands. Do not "helpfully" insert a pad.
    event_responses: array<CagiEventResponse, 8>,
}

@group(0) @binding(11) var<storage, read> light_volume: array<u32>;
@group(0) @binding(13) var<storage, read> cagi_cell_data: array<u32>;
@group(0) @binding(14) var<uniform> cagi_volume_meta: CagiVolumeMeta;

const CAGI_CELL_DATA_WORDS: u32 = 2u;

fn cagi_cell_attribute(cell_index: u32) -> u32 {
    return cagi_cell_data[cell_index * CAGI_CELL_DATA_WORDS];
}

fn cagi_cell_emission(cell_index: u32) -> vec3<f32> {
    let base = cell_index * CAGI_CELL_DATA_WORDS + 1u;
    return cagi_radiance(cagi_unpack(cagi_cell_data[base])) * lighting.gi_params.w;
}

// ---- E4: CAGI levers, the half both passes need -------------------------------
// The rest (propagation rule, sky test, sun-source caching) are levers of the CA
// pass alone and live in `cagi.wgsl`. Registry rows with the measured verdicts:
// `src/variants.rs::REGISTRY`, subsystem `Gi`.
//
// CAGI_ENABLED folds the WHOLE experiment away: with it false the shading pass
// is bit-identical to the E1c renderer (the light volume shrinks to a
// placeholder buffer on the Rust side too), which is the isolation rule's
// requirement and the bench's no-regression anchor.
const CAGI_ENABLED: bool = true;
// How the shading pass reads the volume:
//   0  nearest — one load from the cell in front of the hit face;
//   1  trilinear — 8 loads, weights renormalized over the NON-solid taps so a
//      wall's interior (which always holds 0) can never bleed darkness across
//      the surface in front of it.
const CAGI_SAMPLE_NEAREST: u32 = 0u;
const CAGI_SAMPLE_TRILINEAR: u32 = 1u;
const CAGI_SAMPLE_MODE: u32 = 1u;

// Cell attribute bits (see the header): albedo in the low 24, solid flag at 24,
// the M2 transmittance in bits 25-28, and the S3b event-response index in 29-31.
const CAGI_CELL_SOLID: u32 = 0x01000000u;
const CAGI_TRANSMITTANCE_SHIFT: u32 = 25u;
const CAGI_TRANSMITTANCE_LEVELS: f32 = 15.0;
const CAGI_EVENT_RESPONSE_SHIFT: u32 = 29u;
// Light word bits. Three 10-bit mantissas plus a shared two-bit exponent keep
// the word compact while representing radiance well above SDR white.
const CAGI_CHANNEL_MASK: u32 = 0x3ffu;
const CAGI_CHANNEL_MAX: u32 = 1023u;
const CAGI_RADIANCE_MAX: f32 = 8.0;
const CAGI_RADIANCE_PER_STEP: f32 = 1.0 / 1023.0;
// Cells searched outward along the hit normal for a non-solid cell to sample.
// A cell counts as solid at a quarter fill (src/cagi.rs), so the cell touching a
// surface often IS solid — sampling it would print black patches onto lit
// ground. Two steps is enough for every surface measured on the island.
const CAGI_SAMPLE_SEARCH_STEPS: u32 = 3u;

// Flat cell index — x-major, then y, then z, exactly like the brick grid.
fn cagi_cell_index(cell: vec3<u32>) -> u32 {
    return cell.x
        + cell.y * cagi_volume_meta.grid_size.x
        + cell.z * cagi_volume_meta.grid_size.x * cagi_volume_meta.grid_size.y;
}

fn cagi_pack(light: vec3<u32>) -> u32 {
    let largest = max(max(light.x, light.y), light.z);
    var exponent = 0u;
    var scale = 1u;
    loop {
        if (exponent >= 3u || largest <= CAGI_CHANNEL_MAX * scale) {
            break;
        }
        exponent = exponent + 1u;
        scale = scale << 1u;
    }
    let quantized = min((light + vec3<u32>(scale / 2u)) / scale,
        vec3<u32>(CAGI_CHANNEL_MAX));
    return quantized.x | (quantized.y << 10u) | (quantized.z << 20u)
        | (exponent << 30u);
}

fn cagi_unpack(word: u32) -> vec3<u32> {
    let exponent = word >> 30u;
    let scale = 1u << exponent;
    return vec3<u32>(word & CAGI_CHANNEL_MASK,
                     (word >> 10u) & CAGI_CHANNEL_MASK,
                     (word >> 20u) & CAGI_CHANNEL_MASK) * scale;
}

// Integer light level -> linear radiance.
fn cagi_radiance(light: vec3<u32>) -> vec3<f32> {
    return vec3<f32>(light) * CAGI_RADIANCE_PER_STEP;
}

// Linear radiance -> integer light level (round to nearest, saturating).
fn cagi_quantize(radiance: vec3<f32>) -> vec3<u32> {
    let scaled = clamp(radiance, vec3<f32>(0.0), vec3<f32>(CAGI_RADIANCE_MAX))
        / CAGI_RADIANCE_PER_STEP;
    return vec3<u32>(scaled + vec3<f32>(0.5));
}

// The sky's own radiance — the value injected into every cell that sees the sky,
// and what a sample above the volume's clamped top reads. Same hemisphere
// constants the E1c ambient used, so switching CAGI on cannot change the overall
// exposure of open ground, only its distribution.
fn cagi_sky_radiance() -> vec3<f32> {
    // Sky-visible cells receive the same LUT-derived zenith irradiance as the
    // surface path. CAGI then transports this value only through neighbours.
    return environment_hillaire_sky(vec3<f32>(0.0, 1.0, 0.0)) * lighting.sky_ambient.w;
}

fn cagi_sky_light() -> vec3<u32> {
    return cagi_quantize(cagi_sky_radiance());
}

// A cell's static attributes (out of grid = solid, so nothing leaks in from
// outside the volume — except above the top, handled by the callers).
fn cagi_attributes_of(cell: vec3<i32>) -> u32 {
    let grid = vec3<i32>(cagi_volume_meta.grid_size);
    if (any(cell < vec3<i32>(0, 0, 0)) || any(cell >= grid)) {
        return CAGI_CELL_SOLID;
    }
    return cagi_cell_attribute(cagi_cell_index(vec3<u32>(cell)));
}

fn cagi_cell_is_solid(cell: vec3<i32>) -> bool {
    return (cagi_attributes_of(cell) & CAGI_CELL_SOLID) != 0u;
}

// The fraction of light a SOLID cell passes on instead of absorbing (M2), from
// the 4 bits above the solid flag. 0 for stone, ~0.25 for a leaf canopy. Read
// only when CAGI_TRANSMISSION is compiled in; without it a solid cell absorbs
// everything, which is E4's original behaviour.
fn cagi_cell_transmittance(attributes: u32) -> f32 {
    let quantized = (attributes >> CAGI_TRANSMITTANCE_SHIFT) & 0xfu;
    return f32(quantized) * (1.0 / CAGI_TRANSMITTANCE_LEVELS);
}

// S3b: which row of `event_responses` this cell's emission follows. 0 = none,
// which is every cell of a world nobody has authored an event sensor into.
fn cagi_cell_event_response(attributes: u32) -> u32 {
    return (attributes >> CAGI_EVENT_RESPONSE_SHIFT) & 0x7u;
}

// The cell's bounce albedo, decoded to linear. Zero for cells with no occupied
// voxel (nothing to bounce off).
fn cagi_cell_albedo(attributes: u32) -> vec3<f32> {
    let bytes = vec3<u32>(attributes & 0xffu, (attributes >> 8u) & 0xffu,
                          (attributes >> 16u) & 0xffu);
    return srgb_decode(vec3<f32>(bytes) * (1.0 / 255.0));
}

// Light stored in one cell, in [0, CAGI_RADIANCE_MAX] radiance. Out of grid: black, except
// ABOVE the volume's clamped top, which is open sky. Solid cells always hold 0
// (the CA writes it every iteration), so no attribute read is needed here.
fn cagi_cell_radiance(cell: vec3<i32>) -> vec3<f32> {
    let grid = vec3<i32>(cagi_volume_meta.grid_size);
    if (cell.y >= grid.y) {
        return cagi_sky_radiance();
    }
    if (any(cell < vec3<i32>(0, 0, 0)) || any(cell >= grid)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    return cagi_radiance(cagi_unpack(light_volume[cagi_cell_index(vec3<u32>(cell))]));
}

// Cell coordinate containing a point given in VOXEL units.
fn cagi_cell_of(position_voxels: vec3<f32>) -> vec3<i32> {
    return vec3<i32>(floor(position_voxels / cagi_volume_meta.cell_size_voxels));
}

// Trilinear blend of the eight cells around `position_voxels`, with SOLID taps
// dropped and the remaining weights renormalized. Falls back to the containing
// cell when every tap is solid (a deeply enclosed nook — legitimately dark).
fn cagi_sample_trilinear(position_voxels: vec3<f32>) -> vec3<f32> {
    let cell_space = position_voxels / cagi_volume_meta.cell_size_voxels - vec3<f32>(0.5);
    let base = vec3<i32>(floor(cell_space));
    let fraction = cell_space - floor(cell_space);
    var radiance_sum = vec3<f32>(0.0, 0.0, 0.0);
    var weight_sum = 0.0;
    for (var corner = 0u; corner < 8u; corner = corner + 1u) {
        let offset = vec3<i32>(i32(corner & 1u), i32((corner >> 1u) & 1u),
                               i32((corner >> 2u) & 1u));
        let cell = base + offset;
        let blend = select(vec3<f32>(1.0) - fraction, fraction, offset == vec3<i32>(1, 1, 1));
        let weight = blend.x * blend.y * blend.z;
        if (weight <= 0.0) {
            continue;
        }
        // Above the volume top is sky, not solid — let it contribute.
        if (cell.y < i32(cagi_volume_meta.grid_size.y) && cagi_cell_is_solid(cell)) {
            continue;
        }
        radiance_sum = radiance_sum + cagi_cell_radiance(cell) * weight;
        weight_sum = weight_sum + weight;
    }
    if (weight_sum <= 1e-4) {
        return cagi_cell_radiance(cagi_cell_of(position_voxels));
    }
    return radiance_sum / weight_sum;
}

// The indirect radiance arriving at a surface point (VOXEL units, on the hit
// face) with outward `normal`: step out along the normal to the first non-solid
// cell, then sample. Never samples inside a solid cell, which is what keeps the
// volume's absorbing cells from printing themselves onto the surfaces in front
// of them.
fn cagi_sample_surface(surface_point: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
    let step = normal * cagi_volume_meta.cell_size_voxels;
    var position = surface_point + step * 0.5;
    for (var search = 0u; search < CAGI_SAMPLE_SEARCH_STEPS; search = search + 1u) {
        if (!cagi_cell_is_solid(cagi_cell_of(position))) {
            break;
        }
        position = position + step;
    }
    if (CAGI_SAMPLE_MODE == CAGI_SAMPLE_TRILINEAR) {
        return cagi_sample_trilinear(position);
    }
    return cagi_cell_radiance(cagi_cell_of(position));
}
