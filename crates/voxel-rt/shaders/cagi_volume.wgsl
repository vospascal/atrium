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
    // Explicit pad where the pruned 26-neighbour rule's numerator lived: the
    // response array below must start at 32 (16-byte element alignment), and
    // the Rust `#[repr(C)]` twin has no implicit padding — so the slot is
    // spelled out in BOTH rather than left to WGSL's silent alignment.
    padding: u32,
    // S3b — row 0 is identity ("this cell does not answer events"); rows 1-7 are
    // allocated by src/cagi.rs and indexed by CAGI_CELL_EVENT_RESPONSE_MASK.
    //
    // Starts at exactly offset 32 (the explicit pad above closes the geometry
    // half there), which is the 16-byte alignment an array of 16-byte-aligned
    // elements demands. Do not add any further padding.
    event_responses: array<CagiEventResponse, 8>,
}

@group(G_WORLD) @binding(B_LIGHT_VOLUME_FRONT) var<storage, read> light_volume: array<u32>;
@group(G_WORLD) @binding(B_CAGI_CELL_DATA) var<storage, read> cagi_cell_data: array<u32>;
@group(G_WORLD) @binding(B_CAGI_VOLUME_META) var<uniform> cagi_volume_meta: CagiVolumeMeta;

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
// CAGI_LAYOUT (docs/cagi-directional-banks-plan.md) — how the light volume
// stores a cell:
//   0  isotropic — ONE light word per cell (the Quest tier's layout);
//   1  banks6 — SIX directional light words per cell (+X,-X,+Y,-Y,+Z,-Z as
//      banks 0-5, the direction the light TRAVELS), stored as SoA planes:
//      bank b of cell i lives at i + b * cell_count. The D2 transport in
//      cagi.wgsl propagates each bank directionally; omnidirectional reads
//      (fog, the pre-D4 sampler) SUM the banks. Shipped since the D5 flip,
//      paired with 8-voxel cells.
const CAGI_LAYOUT_ISOTROPIC: u32 = 0u;
const CAGI_LAYOUT_BANKS6: u32 = 1u;
const CAGI_LAYOUT: u32 = 1u;
// Fraction of the sky's radiance the four HORIZONTAL banks carry for a
// sky-seeing cell (the downward bank always carries the full value) — the
// horizon's share of the sky hemisphere. Shared because BOTH halves need it:
// the CA injects with it, and the D4 sampler's above-the-volume sky reads must
// agree with what the CA would have injected there.
const CAGI_BANKS_SKY_HORIZONTAL: f32 = 0.25;

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

// Cells in the whole volume — the SoA plane stride of the banks6 layout.
// Computed from the uniform's grid size rather than carried as another uniform
// field: the meta struct's offset table is a documented contract (src/cagi.rs)
// and two multiplies per call are cheaper than getting that table wrong.
fn cagi_cell_count() -> u32 {
    return cagi_volume_meta.grid_size.x
        * cagi_volume_meta.grid_size.y
        * cagi_volume_meta.grid_size.z;
}

// The light-buffer index of `bank` for the cell at `cell_index`. Bank 0 aliases
// the isotropic index in BOTH layouts, so callers that mean "the cell's only
// word" under isotropic can pass bank 0 unconditionally.
fn cagi_light_index(cell_index: u32, bank: u32) -> u32 {
    if (CAGI_LAYOUT == CAGI_LAYOUT_BANKS6) {
        return cell_index + bank * cagi_cell_count();
    }
    return cell_index;
}

// Bank order: +X, -X, +Y, -Y, +Z, -Z — the direction the bank's light is
// TRAVELLING (not arriving from). `bank ^ 1u` is therefore the reversed bank,
// which is what a D3 bounce reflects into.
fn cagi_bank_direction(bank: u32) -> vec3<i32> {
    let axis = bank >> 1u;
    let sign = select(1, -1, (bank & 1u) == 1u);
    if (axis == 0u) {
        return vec3<i32>(sign, 0, 0);
    }
    if (axis == 1u) {
        return vec3<i32>(0, sign, 0);
    }
    return vec3<i32>(0, 0, sign);
}

// The bank whose direction is `direction` (must be a unit face offset).
fn cagi_direction_bank(direction: vec3<i32>) -> u32 {
    let axis = select(select(2u, 1u, direction.y != 0), 0u, direction.x != 0);
    let component = direction.x + direction.y + direction.z;
    return axis * 2u + select(0u, 1u, component < 0);
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
// Takes a CELL, because the sky a cell sees depends on whether a cloud is over that cell.
//
// This used to read `environment_hillaire_sky(vec3(0, 1, 0))` — the zenith, so blue at sunset
// while the warm light sits at the horizon — and it was blind to the deck, so an overcast sky
// injected full clear-sky radiance into the volume. Both are fixed by routing through the same
// `environment_sky_ambient_at` the surface path uses: one definition of "what light comes from
// above", so the volume and the surface cannot disagree about the weather.
fn cagi_sky_radiance(cell: vec3<i32>) -> vec3<f32> {
    // The cell centre computed HERE rather than through `cagi_cell_center_voxels`: that helper
    // lives in `cagi.wgsl`, which this module is imported BY rather than imports, and it is absent
    // from the shading pass entirely. Two lines beats inverting the dependency.
    let centre_voxels = (vec3<f32>(cell) + vec3<f32>(0.5)) * cagi_volume_meta.cell_size_voxels;
    return environment_sky_ambient_at(
        centre_voxels * brickmap.voxel_size_meters,
        vec3<f32>(0.0, 1.0, 0.0),
    ) * atmosphere.ambient_scale;
}

fn cagi_sky_light(cell: vec3<i32>) -> vec3<u32> {
    return cagi_quantize(cagi_sky_radiance(cell));
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
// ABOVE the volume's clamped top, which is open sky. No attribute read: under
// isotropic a solid cell holds 0 (or its M2/E5b hand-back, which is what a
// reader SHOULD see); under banks a solid holds its outgoing bounce banks —
// samplers that must not read through surfaces drop solid taps themselves
// (`cagi_sample_trilinear`, `cagi_banks_sample_trilinear`).
//
// Under banks6 this is the SUM of the six directional banks. The D2 injection
// discipline is "sum over banks ~= the isotropic word" (an emitter splits its
// radiance across the banks it feeds), so summing here keeps every consumer —
// the surface sampler until D4, fog, the debug readouts — at the same overall
// exposure as the isotropic layout. D4 replaces the surface path with a
// normal-weighted blend; this stays the omnidirectional read.
fn cagi_cell_radiance(cell: vec3<i32>) -> vec3<f32> {
    let grid = vec3<i32>(cagi_volume_meta.grid_size);
    if (cell.y >= grid.y) {
        return cagi_sky_radiance(cell);
    }
    if (any(cell < vec3<i32>(0, 0, 0)) || any(cell >= grid)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    let cell_index = cagi_cell_index(vec3<u32>(cell));
    if (CAGI_LAYOUT == CAGI_LAYOUT_BANKS6) {
        var light = vec3<u32>(0u, 0u, 0u);
        for (var bank = 0u; bank < 6u; bank = bank + 1u) {
            light = light + cagi_unpack(light_volume[cagi_light_index(cell_index, bank)]);
        }
        return cagi_radiance(light);
    }
    return cagi_radiance(cagi_unpack(light_volume[cell_index]));
}

// Cell coordinate containing a point given in VOXEL units.
fn cagi_cell_of(position_voxels: vec3<f32>) -> vec3<i32> {
    return vec3<i32>(floor(position_voxels / cagi_volume_meta.cell_size_voxels));
}

// ---- D4: the directional surface read (banks6) ---------------------------------
// A surface with outward `normal` receives bank d in proportion to how squarely
// d's light arrives INTO the face: weight = max(0, dot(-normal, direction_d)).
// For the axis banks those weights are just the clamped components of -normal,
// so at most three banks contribute for any normal. No normalization constant,
// deliberately: ground under open sky reads the downward bank at weight 1 —
// the full sky value, exactly what the isotropic layout's flooded cell held —
// while a wall reads the horizon share. Directionality changes WALLS, not the
// overall exposure anchor.
//
// The radiance arriving at a face with `normal`, stored in one cell.
// Above-the-volume reads reconstruct what the CA injects at the boundary (full
// sky downward, the horizon fraction horizontally), so a sample against the
// clamped top agrees with the transport.
fn cagi_cell_arriving_radiance(cell: vec3<i32>, normal: vec3<f32>) -> vec3<f32> {
    let toward_face = -normal;
    let positive = max(toward_face, vec3<f32>(0.0)); // weights of banks +X, +Y, +Z
    let negative = max(-toward_face, vec3<f32>(0.0)); // weights of banks -X, -Y, -Z
    let grid = vec3<i32>(cagi_volume_meta.grid_size);
    if (cell.y >= grid.y) {
        let sky = cagi_sky_radiance(cell);
        let horizontal_weight = positive.x + negative.x + positive.z + negative.z;
        return sky * (negative.y + horizontal_weight * CAGI_BANKS_SKY_HORIZONTAL);
    }
    if (any(cell < vec3<i32>(0, 0, 0)) || any(cell >= grid)) {
        return vec3<f32>(0.0, 0.0, 0.0);
    }
    let cell_index = cagi_cell_index(vec3<u32>(cell));
    // Unrolled per axis with STATIC bank indices — a dynamically indexed local
    // array spills to scratch memory under naga/Metal, and this function runs
    // 8x per surface sample. An axis-aligned voxel face takes exactly ONE of
    // the six loads, the same count as the isotropic sampler.
    let stride = cagi_cell_count();
    var light = vec3<f32>(0.0, 0.0, 0.0);
    if (positive.x > 0.0) {
        light = light + cagi_radiance(cagi_unpack(light_volume[cell_index])) * positive.x;
    } else if (negative.x > 0.0) {
        light = light
            + cagi_radiance(cagi_unpack(light_volume[cell_index + stride])) * negative.x;
    }
    if (positive.y > 0.0) {
        light = light
            + cagi_radiance(cagi_unpack(light_volume[cell_index + 2u * stride])) * positive.y;
    } else if (negative.y > 0.0) {
        light = light
            + cagi_radiance(cagi_unpack(light_volume[cell_index + 3u * stride])) * negative.y;
    }
    if (positive.z > 0.0) {
        light = light
            + cagi_radiance(cagi_unpack(light_volume[cell_index + 4u * stride])) * positive.z;
    } else if (negative.z > 0.0) {
        light = light
            + cagi_radiance(cagi_unpack(light_volume[cell_index + 5u * stride])) * negative.z;
    }
    return light;
}

// The banks twin of `cagi_sample_trilinear`: the same 8-tap stencil, the same
// dropped-solid-tap renormalization, but each tap is the directional arriving
// read instead of the omnidirectional sum.
fn cagi_banks_sample_trilinear(position_voxels: vec3<f32>, normal: vec3<f32>) -> vec3<f32> {
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
        if (cell.y < i32(cagi_volume_meta.grid_size.y) && cagi_cell_is_solid(cell)) {
            continue;
        }
        radiance_sum = radiance_sum + cagi_cell_arriving_radiance(cell, normal) * weight;
        weight_sum = weight_sum + weight;
    }
    if (weight_sum <= 1e-4) {
        return cagi_cell_arriving_radiance(cagi_cell_of(position_voxels), normal);
    }
    return radiance_sum / weight_sum;
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
    if (CAGI_LAYOUT == CAGI_LAYOUT_BANKS6) {
        if (CAGI_SAMPLE_MODE == CAGI_SAMPLE_TRILINEAR) {
            return cagi_banks_sample_trilinear(position, normal);
        }
        return cagi_cell_arriving_radiance(cagi_cell_of(position), normal);
    }
    if (CAGI_SAMPLE_MODE == CAGI_SAMPLE_TRILINEAR) {
        return cagi_sample_trilinear(position);
    }
    return cagi_cell_radiance(cagi_cell_of(position));
}
