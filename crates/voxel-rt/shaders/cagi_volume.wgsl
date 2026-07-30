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
//  13  storage  cagi_cell_attributes — one u32 per cell: the cell's bounce
//                                    albedo (sRGB 8:8:8) plus the solid flag
//                                    (bit 24). Static, built on the CPU from
//                                    the brickmap (src/cagi.rs).
//  14  uniform  CagiVolumeMeta      — grid dimensions + the integer transport
//                                    coefficients (cagi.rs CagiVolumeUniform)
//
// PACKING (the "pure integer" property the dossier's CA GI depends on — no
// float accumulation anywhere in the transport, so the volume is deterministic
// and noiseless):
//
//   bits  0..9   red    (0..1023)
//   bits 10..19  green
//   bits 20..29  blue
//   bit  30      sun-source flag: this cell's value was injected from a sunlit
//                surface and is PINNED (see CAGI_SUN_CACHE in cagi.wgsl)
//   bit  31      unused
//
// 10:10:10 rather than 8:8:8 because the choice is free: both fit one u32, so
// they cost exactly the same bytes, and the extra two bits per channel give the
// diffusion rule's integer division four times the headroom before its rounding
// error shows up as banding in a long flood. A channel value of 1023 means
// linear radiance 1.0; injected values (sky ambient ~0.4, sun bounce ~0.5) sit
// well under that, so saturation is not a practical concern.

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
    // Max-decrement rule: light lost per cell step, in 1/1023 units. Derived on
    // the CPU from a per-METER attenuation so the flood's reach in meters does
    // not change when the resolution lever moves.
    attenuation: u32,
    // Diffusion rule: L = (sum_of_6_neighbours * numerator) >> 12.
    diffusion_numerator: u32,
    // Diffusion rule, 26-neighbour variant: the weighted sum (face 4, edge 2,
    // corner 1) times this, >> 12.
    diffusion_26_numerator: u32,
}

@group(0) @binding(11) var<storage, read> light_volume: array<u32>;
@group(0) @binding(13) var<storage, read> cagi_cell_attributes: array<u32>;
@group(0) @binding(14) var<uniform> cagi_volume_meta: CagiVolumeMeta;

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

// Cell attribute bits (see the header): albedo in the low 24, solid flag at 24.
const CAGI_CELL_SOLID: u32 = 0x01000000u;
// Light word bits.
const CAGI_CHANNEL_MASK: u32 = 0x3ffu;
const CAGI_CHANNEL_MAX: u32 = 1023u;
const CAGI_SUN_SOURCE_FLAG: u32 = 0x40000000u;
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
    let clamped = min(light, vec3<u32>(CAGI_CHANNEL_MAX, CAGI_CHANNEL_MAX, CAGI_CHANNEL_MAX));
    return clamped.x | (clamped.y << 10u) | (clamped.z << 20u);
}

fn cagi_unpack(word: u32) -> vec3<u32> {
    return vec3<u32>(word & CAGI_CHANNEL_MASK,
                     (word >> 10u) & CAGI_CHANNEL_MASK,
                     (word >> 20u) & CAGI_CHANNEL_MASK);
}

// Integer light level -> linear radiance.
fn cagi_radiance(light: vec3<u32>) -> vec3<f32> {
    return vec3<f32>(light) * CAGI_RADIANCE_PER_STEP;
}

// Linear radiance -> integer light level (round to nearest, saturating).
fn cagi_quantize(radiance: vec3<f32>) -> vec3<u32> {
    let scaled = clamp(radiance, vec3<f32>(0.0), vec3<f32>(1.0)) * f32(CAGI_CHANNEL_MAX);
    return vec3<u32>(scaled + vec3<f32>(0.5));
}

// The sky's own radiance — the value injected into every cell that sees the sky,
// and what a sample above the volume's clamped top reads. Same hemisphere
// constants the E1c ambient used, so switching CAGI on cannot change the overall
// exposure of open ground, only its distribution.
fn cagi_sky_radiance() -> vec3<f32> {
    return lighting.sky_ambient.rgb * lighting.sky_ambient.w;
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
    return cagi_cell_attributes[cagi_cell_index(vec3<u32>(cell))];
}

fn cagi_cell_is_solid(cell: vec3<i32>) -> bool {
    return (cagi_attributes_of(cell) & CAGI_CELL_SOLID) != 0u;
}

// The cell's bounce albedo, decoded to linear. Zero for cells with no occupied
// voxel (nothing to bounce off).
fn cagi_cell_albedo(attributes: u32) -> vec3<f32> {
    let bytes = vec3<u32>(attributes & 0xffu, (attributes >> 8u) & 0xffu,
                          (attributes >> 16u) & 0xffu);
    return srgb_decode(vec3<f32>(bytes) * (1.0 / 255.0));
}

// Light stored in one cell, in [0, 1] radiance. Out of grid: black, except
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
