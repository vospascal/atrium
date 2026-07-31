//! E4 — CAGI v0 (Cellular-Automata Global Illumination): the light volume's
//! pure-data half. Settings + shader-source patching + the CPU-built static cell
//! attributes + the integer transport reference implementation. No wgpu, no
//! windowing (plan architecture rule); the GPU resources and the dispatch live in
//! [`crate::passes::cagi`].
//!
//! WHY a cellular automaton at all (dossier `xima-engine-dossier.md`, GI
//! generations): E1 measured a marginal full-res secondary ray at 2.25-3.55 ms,
//! so gathering indirect light per PIXEL is unaffordable at any useful ray count.
//! A CA pays for transport once per CELL per iteration instead, and the result is
//! sampled with zero extra rays. It is also noiseless by construction (pure
//! integers, no Monte Carlo), which is this renderer's stated identity.
//!
//! THE VOLUME. One `u32` per cell, ping-ponged between two buffers (the dossier
//! records xima's explicit preference for double buffering over in-place
//! updates). Packing, the cell attribute word and the transport rules are
//! documented in `shaders/cagi_volume.wgsl` and `shaders/cagi.wgsl`; this module
//! mirrors them for the CPU-side build, the memory accounting and the tests.
//!
//! RESOLUTION. The cell edge is 2, 4 or 8 voxels (always a divisor of the
//! 8-voxel brick, so a cell never straddles two bricks — the whole attribute
//! build and the sky test depend on that). The vertical extent is CLAMPED to the
//! world's occupied height plus [`SKY_MARGIN_CELLS`]: everything above is open
//! sky by definition, so allocating it would be paying to store a constant.
//! Measured footprints are in the bench doc's E4 section.

use crate::ao::patch_shader_const;
use crate::brickmap::{
    brick_is_uniform, brick_slot, brick_uniform_material, Brickmap, BRICK_GRID_X, BRICK_GRID_Y,
    BRICK_GRID_Z, BRICK_SIZE, EMPTY_BRICK, EMPTY_COLUMN, MATERIAL_WORDS_PER_BRICK,
    OCCUPANCY_WORDS_PER_BRICK,
};
use crate::material::{Material, MATERIALS, MATERIAL_COUNT};
use voxel_core::world::{VOXEL_SIZE, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z};

/// Cell attribute bit 24: the cell absorbs light (see [`SOLID_FILL_DIVISOR`]).
pub const CELL_SOLID: u32 = 0x0100_0000;

/// Cell attribute bits 25-28 (M2): the cell's TRANSMITTANCE, quantized to 4
/// bits, `0 = opaque .. 15 = fully transparent`.
///
/// Only meaningful on a solid cell — that is the whole point: before M2 a solid
/// cell absorbed everything ([`CELL_SOLID`] alone), so a leaf canopy was a wall
/// and CA GI painted the ground under a tree black. The transmitting fraction is
/// what lets light seep through foliage without making the canopy non-solid
/// (which would also stop it casting any shadow at all).
///
/// Four bits because the value is a coarse material property multiplying an
/// already-quantized 10-bit channel: the finest step it can express (1/15) is
/// well under the visible difference between two adjacent light levels, and it
/// leaves bits 29-31 free.
pub const CELL_TRANSMITTANCE_SHIFT: u32 = 25;
pub const CELL_TRANSMITTANCE_LEVELS: u32 = 15;
pub const CELL_TRANSMITTANCE_MASK: u32 = 0xf << CELL_TRANSMITTANCE_SHIFT;

/// Cell attribute bits 29-31 (E5): which EMITTER the cell is, or 0 for "not an
/// emitter".
///
/// An index rather than a colour, because emission is a MATERIAL constant, not a
/// per-cell value — the cell only has to say which of the few emissive materials
/// it holds, and the CA looks the radiance up in [`emitter_palette`]. That fits
/// the three bits left over after albedo, solidity and transmittance, so E5 costs
/// the attribute volume nothing at all: no second array, no wider word.
///
/// Three bits means [`MAX_EMITTERS`] distinct emissive materials. The table has
/// two; [`emitter_palette`] panics rather than silently truncating if that is
/// ever exceeded.
pub const CELL_EMITTER_SHIFT: u32 = 29;
pub const CELL_EMITTER_MASK: u32 = 0x7 << CELL_EMITTER_SHIFT;

/// Distinct emissive materials the 3-bit field can name (index 0 means "none").
pub const MAX_EMITTERS: usize = 7;

/// Palette slots uploaded to the GPU: [`MAX_EMITTERS`] plus the black slot 0.
pub const EMITTER_PALETTE_SLOTS: usize = MAX_EMITTERS + 1;

/// Emitter index per material id: 0 for everything that does not emit, and
/// 1.. for each [`MaterialFlags::EMISSIVE`] row in table order.
fn emitter_indices(rows: &[Material]) -> Vec<u32> {
    let mut next_index = 0_u32;
    rows.iter()
        .map(|material| {
            if !material.is_emissive() {
                return 0;
            }
            next_index += 1;
            next_index
        })
        .collect()
}

/// The emitter radiances the CA looks up, indexed by the attribute field.
///
/// Slot 0 is black and is what every non-emissive cell reads, so the CA needs no
/// branch: it can always look up, and a non-emitter simply injects nothing.
pub fn emitter_palette(rows: &[Material]) -> Vec<[f32; 4]> {
    let mut palette = vec![[0.0_f32; 4]; EMITTER_PALETTE_SLOTS];
    for (material, index) in rows.iter().zip(emitter_indices(rows)) {
        if index == 0 {
            continue;
        }
        assert!(
            (index as usize) <= MAX_EMITTERS,
            "the material table has more than {MAX_EMITTERS} emissive rows, which \
             no longer fit the 3-bit cell emitter field (widen it or share slots)"
        );
        // The MEAN over the surface, not the row's base value: a patterned emitter's
        // specks are what a pixel shows, and their average is what escapes to light
        // anything else. See `Material::mean_emitted_radiance`.
        let emission = material.mean_emitted_radiance();
        palette[index as usize] = [emission[0], emission[1], emission[2], 0.0];
    }
    palette
}

/// A material's transmittance in the 4-bit attribute form, rounded to nearest.
pub fn quantize_transmittance(transmittance: f32) -> u32 {
    let levels = CELL_TRANSMITTANCE_LEVELS as f32;
    let quantized = (transmittance.clamp(0.0, 1.0) * levels + 0.5) as u32;
    quantized.min(CELL_TRANSMITTANCE_LEVELS) << CELL_TRANSMITTANCE_SHIFT
}
/// Light word bit 30: this cell's value was injected from a sunlit surface and is
/// pinned (the `CAGI_SUN_CACHE` amortization).
pub const SUN_SOURCE_FLAG: u32 = 0x4000_0000;
/// Largest value one 10-bit channel can hold; 1023 = linear radiance 1.0.
pub const CHANNEL_MAX: u32 = 1023;
/// Fixed-point shift of the diffusion numerators (mirrors `CAGI_DIFFUSION_SHIFT`).
pub const DIFFUSION_SHIFT: u32 = 12;
/// Weight sum of the 26-neighbour stencil: 6 faces x 4 + 12 edges x 2 + 8
/// corners x 1.
pub const NEIGHBOUR_26_WEIGHT_SUM: u32 = 56;

/// A cell absorbs once a quarter of its voxels are occupied. Binary absorption is
/// the documented v0 simplification, but the THRESHOLD matters more than it
/// sounds: with "any occupied voxel" a single grass tuft or leaf would seal a
/// whole cell, and the cell touching any surface would read as an absorber, so
/// the flood would never reach the ground it is supposed to light. A quarter fill
/// is exactly one voxel layer of a cell (16 of 64 at 4 voxels per cell), i.e. a
/// one-voxel wall counts as solid while scattered cover does not.
pub const SOLID_FILL_DIVISOR: u32 = 4;

/// Cells of headroom kept above the world's occupied height. Two is enough for
/// the trilinear sampler's upper tap over the tallest tree.
pub const SKY_MARGIN_CELLS: u32 = 2;

/// Max-decrement attenuation per METER, in 1/1023 light steps: the flood's reach
/// is `CHANNEL_MAX / ATTENUATION_PER_METER` ~ 12.8 m regardless of the resolution
/// lever, which is what makes the two rules comparable across cell sizes.
pub const ATTENUATION_PER_METER: f32 = 80.0;
/// Diffusion transmission per METER (0.884/m = 0.94 per 0.5 m cell). Same
/// motivation: the physics must not change when the resolution changes.
pub const TRANSMISSION_PER_METER: f32 = 0.884;

/// Propagation rule — mirrors `CAGI_RULE` in `shaders/cagi.wgsl`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CagiRule {
    /// `L = max(neighbours) - attenuation` over the 6 face neighbours: the
    /// Minecraft-style flood. Sharp and cheapest.
    MaxDecrement,
    /// `L = sum(6 face neighbours) * transmission / 6`: the dossier's
    /// reconstructed diffusion equation.
    Diffusion6,
    /// The same diffusion over all 26 neighbours (face 4 / edge 2 / corner 1) —
    /// the isotropy contender.
    Diffusion26,
}

impl CagiRule {
    pub fn shader_value(self) -> u32 {
        match self {
            CagiRule::MaxDecrement => 0,
            CagiRule::Diffusion6 => 1,
            CagiRule::Diffusion26 => 2,
        }
    }

    pub fn from_shader_value(shader_value: u32) -> CagiRule {
        match shader_value {
            0 => CagiRule::MaxDecrement,
            1 => CagiRule::Diffusion6,
            2 => CagiRule::Diffusion26,
            other => panic!("no CAGI_RULE {other} in cagi.wgsl"),
        }
    }

    fn wgsl_literal(self) -> String {
        format!("{}u", self.shader_value())
    }
}

/// How the shading pass reads the volume — mirrors `CAGI_SAMPLE_MODE` in
/// `shaders/cagi_volume.wgsl`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CagiSampleMode {
    /// One load from the cell in front of the hit face.
    Nearest,
    /// Eight loads, weights renormalized over the non-solid taps.
    Trilinear,
}

impl CagiSampleMode {
    pub fn shader_value(self) -> u32 {
        match self {
            CagiSampleMode::Nearest => 0,
            CagiSampleMode::Trilinear => 1,
        }
    }

    pub fn from_shader_value(shader_value: u32) -> CagiSampleMode {
        match shader_value {
            0 => CagiSampleMode::Nearest,
            1 => CagiSampleMode::Trilinear,
            other => panic!("no CAGI_SAMPLE_MODE {other} in cagi_volume.wgsl"),
        }
    }

    fn wgsl_literal(self) -> String {
        format!("{}u", self.shader_value())
    }
}

/// How a cell decides it sees the sky — mirrors `CAGI_SKY_TEST` in
/// `shaders/cagi.wgsl`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CagiSkyTest {
    /// One load of the traversal's per-XZ-brick-column max occupied brick Y
    /// (binding 8). O(1), exact vertically, quantized to the 1 m brick column.
    ColumnMax,
    /// A real vertical shadow ray per candidate cell: exact per voxel.
    UpwardTrace,
}

impl CagiSkyTest {
    pub fn shader_value(self) -> u32 {
        match self {
            CagiSkyTest::ColumnMax => 0,
            CagiSkyTest::UpwardTrace => 1,
        }
    }

    pub fn from_shader_value(shader_value: u32) -> CagiSkyTest {
        match shader_value {
            0 => CagiSkyTest::ColumnMax,
            1 => CagiSkyTest::UpwardTrace,
            other => panic!("no CAGI_SKY_TEST {other} in cagi.wgsl"),
        }
    }

    fn wgsl_literal(self) -> String {
        format!("{}u", self.shader_value())
    }
}

/// User-facing CAGI configuration. `enabled`, `sample_mode`, `rule`, `sky_test`
/// and `sun_cache` are compile-time shader consts (pipeline rebuild on change);
/// `cell_voxels` needs a VOLUME rebuild but no new pipeline (the grid dimensions
/// ride in the volume uniform); the three float knobs and the iteration count are
/// free to change per frame.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CagiSettings {
    /// `CAGI_ENABLED`: with it off the shading pass is bit-identical to E1c and
    /// the volume shrinks to a placeholder buffer.
    pub enabled: bool,
    /// Voxels per cell edge: 2, 4 or 8. Must divide [`BRICK_SIZE`].
    pub cell_voxels: u32,
    /// `CAGI_RULE`.
    pub rule: CagiRule,
    /// `CAGI_SAMPLE_MODE`.
    pub sample_mode: CagiSampleMode,
    /// `CAGI_SKY_TEST`.
    pub sky_test: CagiSkyTest,
    /// `CAGI_SUN_CACHE`: pin sun-injected cells so their shadow ray is traced
    /// once per re-flood instead of once per iteration.
    pub sun_cache: bool,
    /// `CAGI_EMISSIVE` (E5): let emissive materials inject their radiance.
    pub emissive: bool,
    /// `CAGI_TRANSMISSION` (M2): let a solid cell pass its material's
    /// transmitted fraction on instead of absorbing everything. Off reproduces
    /// E4's binary absorption bit for bit.
    pub transmission: bool,
    /// CA iterations per frame (CPU-side dispatch count — no shader const).
    pub iterations_per_frame: u32,
    /// Multiplier on the sampled volume (`gi_params.x`).
    pub strength: f32,
    /// Share of the E1c hemisphere ambient kept under CAGI (`gi_params.y`).
    pub ambient_floor: f32,
    /// Share of the sun's radiance a sunlit surface injects (`gi_params.z`).
    pub sun_bounce: f32,
    /// Multiplier on every emitter's authored radiance (`gi_params.w`, E5).
    pub emissive_scale: f32,
}

impl Default for CagiSettings {
    /// The shipped configuration. MUST match the CAGI lever defaults in
    /// `shaders/cagi_volume.wgsl` + `shaders/cagi.wgsl` (guarded by
    /// `default_settings_match_shader_sources`).
    fn default() -> CagiSettings {
        CagiSettings {
            enabled: true,
            cell_voxels: 4,
            rule: CagiRule::Diffusion6,
            sample_mode: CagiSampleMode::Trilinear,
            sky_test: CagiSkyTest::ColumnMax,
            sun_cache: true,
            emissive: true,
            transmission: false,
            iterations_per_frame: 2,
            strength: 1.0,
            ambient_floor: 0.25,
            sun_bounce: 0.35,
            emissive_scale: 1.0,
        }
    }
}

impl CagiSettings {
    /// Patch the consts that live in `cagi_volume.wgsl` — the file BOTH pass
    /// shaders include, so this applies to both sources.
    pub fn patch_volume_consts(&self, shader_source: &str) -> String {
        let patched =
            patch_shader_const(shader_source, "CAGI_ENABLED", boolean_literal(self.enabled));
        patch_shader_const(
            &patched,
            "CAGI_SAMPLE_MODE",
            &self.sample_mode.wgsl_literal(),
        )
    }

    /// Patch the consts that live in `cagi.wgsl` — the CA pass only.
    pub fn patch_propagation_consts(&self, shader_source: &str) -> String {
        let mut patched = patch_shader_const(shader_source, "CAGI_RULE", &self.rule.wgsl_literal());
        patched = patch_shader_const(&patched, "CAGI_SKY_TEST", &self.sky_test.wgsl_literal());
        patched = patch_shader_const(&patched, "CAGI_SUN_CACHE", boolean_literal(self.sun_cache));
        patched = patch_shader_const(
            &patched,
            "CAGI_TRANSMISSION",
            boolean_literal(self.transmission),
        );
        patch_shader_const(&patched, "CAGI_EMISSIVE", boolean_literal(self.emissive))
    }

    /// Whether switching from `applied` to `self` changes a compile-time const.
    pub fn requires_pipeline_rebuild(&self, applied: &CagiSettings) -> bool {
        self.enabled != applied.enabled
            || self.sample_mode != applied.sample_mode
            || self.rule != applied.rule
            || self.sky_test != applied.sky_test
            || self.sun_cache != applied.sun_cache
            || self.transmission != applied.transmission
            || self.emissive != applied.emissive
    }

    /// Whether switching from `applied` to `self` needs the GPU volume rebuilt
    /// (its size or its static attributes change).
    pub fn requires_volume_rebuild(&self, applied: &CagiSettings) -> bool {
        self.enabled != applied.enabled || self.cell_voxels != applied.cell_voxels
    }

    /// The grid this configuration wants for `brickmap`'s world.
    pub fn grid(&self, brickmap: &Brickmap) -> CagiGrid {
        if !self.enabled {
            return CagiGrid::placeholder();
        }
        CagiGrid::for_world(self.cell_voxels, brickmap.metadata().max_occupied_brick_y)
    }
}

fn boolean_literal(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

/// The light volume's geometry: cell size and grid dimensions, plus the index
/// math the shaders mirror.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CagiGrid {
    /// Voxels per cell edge.
    pub cell_voxels: u32,
    /// Cells along x, y, z. Y is clamped to the occupied height (see the module
    /// docs).
    pub size: [u32; 3],
}

impl CagiGrid {
    /// The grid for the island world at `cell_voxels`, vertically clamped to
    /// `max_occupied_brick_y` (the brickmap's own world-height metadata;
    /// [`EMPTY_COLUMN`] for an empty world) plus [`SKY_MARGIN_CELLS`].
    pub fn for_world(cell_voxels: u32, max_occupied_brick_y: u32) -> CagiGrid {
        assert!(
            cell_voxels > 0 && (BRICK_SIZE as u32).is_multiple_of(cell_voxels),
            "cell_voxels {cell_voxels} must divide the {BRICK_SIZE}-voxel brick"
        );
        let occupied_voxel_height = if max_occupied_brick_y == EMPTY_COLUMN {
            0
        } else {
            (max_occupied_brick_y + 1) * BRICK_SIZE as u32
        };
        let full_height_cells = (WORLD_SIZE_Y as u32).div_ceil(cell_voxels);
        let clamped_height_cells = occupied_voxel_height.div_ceil(cell_voxels) + SKY_MARGIN_CELLS;
        CagiGrid {
            cell_voxels,
            size: [
                (WORLD_SIZE_X as u32).div_ceil(cell_voxels),
                clamped_height_cells.min(full_height_cells).max(1),
                (WORLD_SIZE_Z as u32).div_ceil(cell_voxels),
            ],
        }
    }

    /// The one-cell grid used while CAGI is switched off: the shading pass still
    /// declares the volume bindings, so *something* valid must be bound, but it
    /// must not cost 13 MB of VRAM to keep a folded-away lever addressable.
    pub fn placeholder() -> CagiGrid {
        CagiGrid {
            cell_voxels: BRICK_SIZE as u32,
            size: [1, 1, 1],
        }
    }

    pub fn cell_count(&self) -> usize {
        self.size[0] as usize * self.size[1] as usize * self.size[2] as usize
    }

    /// Flat cell index — x-major, then y, then z, exactly like the brick grid
    /// (mirrors `cagi_cell_index` in `cagi_volume.wgsl`).
    pub fn cell_index(&self, cell: [u32; 3]) -> usize {
        cell[0] as usize
            + cell[1] as usize * self.size[0] as usize
            + cell[2] as usize * self.size[0] as usize * self.size[1] as usize
    }

    /// Cell edge length in meters.
    pub fn cell_meters(&self) -> f32 {
        self.cell_voxels as f32 * VOXEL_SIZE
    }

    /// Bytes of ONE ping-pong buffer.
    pub fn volume_bytes(&self) -> usize {
        self.cell_count() * 4
    }

    /// Total GPU bytes: both ping-pong buffers plus the static attributes.
    pub fn total_bytes(&self) -> usize {
        self.volume_bytes() * 3
    }

    /// Max-decrement attenuation per cell step, derived from the per-meter
    /// constant (at least 1, or the flood would never end).
    pub fn attenuation(&self) -> u32 {
        ((ATTENUATION_PER_METER * self.cell_meters()).round() as u32).max(1)
    }

    /// Transmission per cell step for the diffusion rules.
    pub fn transmission(&self) -> f32 {
        TRANSMISSION_PER_METER.powf(self.cell_meters())
    }

    /// `(sum_of_6_neighbours * numerator) >> DIFFUSION_SHIFT` — the 6-neighbour
    /// diffusion coefficient in fixed point.
    pub fn diffusion_numerator(&self) -> u32 {
        ((self.transmission() / 6.0) * (1u32 << DIFFUSION_SHIFT) as f32).round() as u32
    }

    /// The same for the 26-neighbour weighted stencil.
    pub fn diffusion_26_numerator(&self) -> u32 {
        ((self.transmission() / NEIGHBOUR_26_WEIGHT_SUM as f32) * (1u32 << DIFFUSION_SHIFT) as f32)
            .round() as u32
    }

    /// The GPU uniform describing this volume.
    ///
    /// Takes the material rows because the emitter palette is built from them, and
    /// since S2 those rows can be LIVE-EDITED — a patterned emitter authored in the
    /// panel has to reach this palette or it lights nothing.
    pub fn uniform(&self, rows: &[Material]) -> CagiVolumeUniform {
        CagiVolumeUniform {
            grid_size: self.size,
            cell_voxels: self.cell_voxels,
            cell_size_voxels: self.cell_voxels as f32,
            attenuation: self.attenuation(),
            diffusion_numerator: self.diffusion_numerator(),
            diffusion_26_numerator: self.diffusion_26_numerator(),
            emitters: emitter_palette(rows)
                .try_into()
                .expect("emitter_palette always returns EMITTER_PALETTE_SLOTS slots"),
        }
    }
}

/// Volume geometry + transport coefficients for the GPU, bindable as a uniform.
///
/// `#[repr(C)]` layout (32 bytes, 16-byte aligned — matches the WGSL
/// `CagiVolumeMeta` struct in `shaders/cagi_volume.wgsl`):
///
/// | offset | field                    | WGSL type   |
/// |--------|--------------------------|-------------|
/// | 0      | `grid_size`              | `vec3<u32>` |
/// | 12     | `cell_voxels`            | `u32`       |
/// | 16     | `cell_size_voxels`       | `f32`       |
/// | 20     | `attenuation`            | `u32`       |
/// | 24     | `diffusion_numerator`    | `u32`       |
/// | 28     | `diffusion_26_numerator` | `u32`       |
/// | 32     | `emitters`               | `array<vec4<f32>, 8>` (E5) |
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct CagiVolumeUniform {
    pub grid_size: [u32; 3],
    pub cell_voxels: u32,
    pub cell_size_voxels: f32,
    pub attenuation: u32,
    pub diffusion_numerator: u32,
    pub diffusion_26_numerator: u32,
    /// E5 emitter radiances, indexed by a cell's 3-bit emitter field. Slot 0 is
    /// black — the value every non-emissive cell reads. Rides in this uniform
    /// rather than a new binding: 128 bytes does not deserve one, and it keeps
    /// the CAGI pass's binding list unchanged.
    pub emitters: [[f32; 4]; EMITTER_PALETTE_SLOTS],
}

// Manual impls instead of derive so we do not depend on bytemuck's `derive`
// feature: `#[repr(C)]`, all fields u32/f32, no implicit padding.
unsafe impl bytemuck::Zeroable for CagiVolumeUniform {}
unsafe impl bytemuck::Pod for CagiVolumeUniform {}

// ---- Packing (the CPU mirror of cagi_volume.wgsl) ----------------------------

/// Pack three 10-bit channels into the light word (saturating).
pub fn pack_light(light: [u32; 3]) -> u32 {
    light[0].min(CHANNEL_MAX)
        | (light[1].min(CHANNEL_MAX) << 10)
        | (light[2].min(CHANNEL_MAX) << 20)
}

/// Unpack the three channels, dropping the flag bits.
pub fn unpack_light(word: u32) -> [u32; 3] {
    [
        word & CHANNEL_MAX,
        (word >> 10) & CHANNEL_MAX,
        (word >> 20) & CHANNEL_MAX,
    ]
}

/// Linear radiance in [0, 1] -> integer level (round to nearest, saturating).
pub fn quantize_radiance(radiance: [f32; 3]) -> [u32; 3] {
    let quantize = |value: f32| (value.clamp(0.0, 1.0) * CHANNEL_MAX as f32 + 0.5) as u32;
    [
        quantize(radiance[0]),
        quantize(radiance[1]),
        quantize(radiance[2]),
    ]
}

// ---- Static cell attributes (built once from the brickmap) --------------------

/// An sRGB albedo packed 8:8:8 into the low 24 bits of a cell attribute word,
/// the form both attribute builders below store and `cagi_cell_albedo` in
/// `shaders/cagi_volume.wgsl` decodes. Bits 24+ stay clear for the flags the
/// CA adds ([`CELL_SOLID`] at bit 24 and the M2 transmittance at bits 25-28).
fn packed_albedo(albedo: [f32; 3]) -> u32 {
    let channel = |value: f32| ((value.clamp(0.0, 1.0) * 255.0) as u32) & 0xff;
    channel(albedo[0]) | (channel(albedo[1]) << 8) | (channel(albedo[2]) << 16)
}

/// The static half of the attribute word for EVERY material, indexed by material
/// id: albedo in the low 24 bits, transmittance in 25-28, emitter index in
/// 29-31. [`CELL_SOLID`] is added later, by the fill-count sweep.
///
/// Built once per sweep and indexed per voxel — the E2 single-cell path walks up
/// to 512 voxels, so recomputing this per voxel would rebuild the whole material
/// table half a thousand times for one edit.
///
/// Every field is taken from the SAME voxel — the cell's highest occupied one —
/// so a cell's transmittance and emitter always describe the same surface its
/// bounce colour does. Coarse (one voxel stands for up to 512), and deliberately
/// the same coarseness the albedo has had since E4.
pub fn material_attribute_table(rows: &[Material]) -> MaterialAttributes {
    let emitters = emitter_indices(rows);
    let mut table = MaterialAttributes([0; MATERIAL_COUNT]);
    for (slot, (material, emitter)) in rows.iter().zip(emitters).enumerate().take(MATERIAL_COUNT) {
        table.0[slot] = packed_albedo(material.albedo)
            | quantize_transmittance(material.transmittance())
            | (emitter << CELL_EMITTER_SHIFT);
    }
    table
}

/// The material table reduced to exactly what the attribute sweep needs: one packed
/// word per material id.
///
/// This is the seam that lets a **live-edited** material reach the light volume. The
/// builders used to read [`MATERIALS`] — the *compiled* table — so an edited albedo or
/// emission could never reach the GI bounce no matter how many times the attributes
/// were re-packed. Passing the whole `&[Material]` down instead would not work either:
/// the incremental per-edit path runs on the world thread and would need an 8 KB copy
/// per 2.8 us edit.
///
/// 104 bytes and `Copy`, so it rides along in a [`crate::world_edit::VoxelEdit`] and
/// crosses the thread boundary for free. The emitter *palette* still needs the rows
/// themselves (it carries f32 radiances), but that is built once per uniform upload on
/// the render thread, where the live table is at hand.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MaterialAttributes([u32; MATERIAL_COUNT]);

impl MaterialAttributes {
    /// The compiled table's attributes — what the world starts with and what every
    /// test that does not care about live edits should use.
    pub fn compiled() -> MaterialAttributes {
        material_attribute_table(&MATERIALS)
    }

    /// The packed word for one material id. Ids past the table read zero, matching the
    /// air sentinel.
    pub fn word(&self, material: u8) -> u32 {
        self.0.get(material as usize).copied().unwrap_or(0)
    }
}

/// One `u32` per cell: the cell's bounce albedo (sRGB 8:8:8 in the low 24 bits),
/// [`CELL_SOLID`] at bit 24, and its 4-bit transmittance at bits 25-28 (M2).
///
/// The albedo is the table color of the cell's HIGHEST occupied voxel — the
/// surface the sun is most likely to be hitting, which is what the bounce tint
/// wants. Solidity is a quarter-fill threshold ([`SOLID_FILL_DIVISOR`]).
///
/// Cost is bounded by the OCCUPIED bricks, not by the cell count: empty bricks
/// are skipped with one pointer read, so the sweep touches
/// `occupied_bricks * 512` voxels (~37 M on the island) rather than
/// `cell_count * cell_voxels^3`.
pub fn build_cell_attributes(
    brickmap: &Brickmap,
    grid: &CagiGrid,
    attribute_table: &MaterialAttributes,
) -> Vec<u32> {
    let mut attributes = vec![0_u32; grid.cell_count()];
    let mut fill_counts = vec![0_u16; grid.cell_count()];

    for brick_z in 0..BRICK_GRID_Z {
        for brick_y in 0..BRICK_GRID_Y {
            for brick_x in 0..BRICK_GRID_X {
                let brick_cell =
                    brick_x + brick_y * BRICK_GRID_X + brick_z * BRICK_GRID_X * BRICK_GRID_Y;
                let pointer = brickmap.brick_indices[brick_cell];
                if pointer == EMPTY_BRICK {
                    continue;
                }
                // A UNIFORM brick keeps no level-1 words, so expand its tag into
                // the shape the voxel loop below expects — all 512 bits set, the
                // material splatted across every byte. One code path beats a
                // second copy of the accumulation loop.
                let uniform_occupancy = [u32::MAX; OCCUPANCY_WORDS_PER_BRICK];
                let uniform_materials = [u32::from_le_bytes([brick_uniform_material(pointer); 4]);
                    MATERIAL_WORDS_PER_BRICK];
                let (occupancy, materials): (&[u32], &[u32]) = if brick_is_uniform(pointer) {
                    (&uniform_occupancy, &uniform_materials)
                } else {
                    let slot = brick_slot(pointer) as usize;
                    (
                        &brickmap.occupancy_words[slot * OCCUPANCY_WORDS_PER_BRICK
                            ..(slot + 1) * OCCUPANCY_WORDS_PER_BRICK],
                        &brickmap.material_words[slot * MATERIAL_WORDS_PER_BRICK
                            ..(slot + 1) * MATERIAL_WORDS_PER_BRICK],
                    )
                };
                // Ascending local Y on the OUTSIDE so the last albedo written
                // into a cell comes from its highest occupied voxel.
                for local_y in 0..BRICK_SIZE {
                    for local_z in 0..BRICK_SIZE {
                        for local_x in 0..BRICK_SIZE {
                            let bit = local_x + local_y * 8 + local_z * 64;
                            if (occupancy[bit >> 5] >> (bit & 31)) & 1 == 0 {
                                continue;
                            }
                            let cell = [
                                ((brick_x * BRICK_SIZE + local_x) as u32) / grid.cell_voxels,
                                ((brick_y * BRICK_SIZE + local_y) as u32) / grid.cell_voxels,
                                ((brick_z * BRICK_SIZE + local_z) as u32) / grid.cell_voxels,
                            ];
                            if cell[0] >= grid.size[0]
                                || cell[1] >= grid.size[1]
                                || cell[2] >= grid.size[2]
                            {
                                continue;
                            }
                            let index = grid.cell_index(cell);
                            fill_counts[index] = fill_counts[index].saturating_add(1);
                            let material = (materials[bit >> 2] >> ((bit & 3) * 8)) & 0xff;
                            attributes[index] = attribute_table.word(material as u8);
                        }
                    }
                }
            }
        }
    }

    let solid_threshold = ((grid.cell_voxels.pow(3)) / SOLID_FILL_DIVISOR).max(1) as u16;
    for (index, fill) in fill_counts.iter().enumerate() {
        if *fill >= solid_threshold {
            attributes[index] |= CELL_SOLID;
        }
    }
    attributes
}

/// ONE cell's attribute word, recomputed from the brickmap — E2's incremental
/// counterpart to [`build_cell_attributes`].
///
/// A cell never straddles a brick (the cell size divides [`BRICK_SIZE`]) and the
/// attribute is a function of the cell's OWN voxels only, so an edit invalidates
/// exactly the cell containing the edited voxel: the 48 ms full rebuild collapses
/// to `cell_voxels^3` ≤ 512 occupancy-bit reads. Iterates in the same
/// y-then-z-then-x order as the full build, so "the albedo of the highest
/// occupied voxel" resolves ties identically (pinned by a test).
pub fn cell_attribute(
    brickmap: &Brickmap,
    grid: &CagiGrid,
    cell: [u32; 3],
    attribute_table: &MaterialAttributes,
) -> u32 {
    let cell_voxels = grid.cell_voxels as i32;
    let base = [
        cell[0] as i32 * cell_voxels,
        cell[1] as i32 * cell_voxels,
        cell[2] as i32 * cell_voxels,
    ];
    let mut fill_count = 0_u32;
    let mut albedo = 0_u32;
    for local_y in 0..cell_voxels {
        for local_z in 0..cell_voxels {
            for local_x in 0..cell_voxels {
                let material =
                    brickmap.get(base[0] + local_x, base[1] + local_y, base[2] + local_z);
                if material == 0 {
                    continue;
                }
                fill_count += 1;
                albedo = attribute_table.word(material);
            }
        }
    }
    let solid_threshold = ((grid.cell_voxels.pow(3)) / SOLID_FILL_DIVISOR).max(1);
    if fill_count >= solid_threshold {
        albedo |= CELL_SOLID;
    }
    albedo
}

/// World size in voxels, for the memory table and the grid tests.
pub const WORLD_SIZE_VOXELS: [u32; 3] = [
    WORLD_SIZE_X as u32,
    WORLD_SIZE_Y as u32,
    WORLD_SIZE_Z as u32,
];

// ---- CPU reference transport --------------------------------------------------

/// The propagation half of one CA step for ONE cell, in exactly the integer
/// arithmetic `shaders/cagi.wgsl` performs — the reference the bench cross-checks
/// the GPU volume against (E4 section, "CPU cross-check"). Injection is NOT
/// included: the caller identifies source cells (sky by the column test, sun by
/// the pinned flag) and skips them.
///
/// `neighbour_light` is called with a cell coordinate that may be out of grid; it
/// must return the same values the shader's `cagi_neighbour_light` does (sky
/// above the volume top, black elsewhere, and 0 inside solid cells, which hold 0
/// by construction).
pub fn propagate_reference(
    rule: CagiRule,
    grid: &CagiGrid,
    cell: [i32; 3],
    neighbour_light: &mut impl FnMut([i32; 3]) -> [u32; 3],
) -> [u32; 3] {
    match rule {
        CagiRule::MaxDecrement => {
            let attenuation = grid.attenuation();
            let mut brightest = [0_u32; 3];
            for offset in FACE_OFFSETS {
                let neighbour = neighbour_light([
                    cell[0] + offset[0],
                    cell[1] + offset[1],
                    cell[2] + offset[2],
                ]);
                for channel in 0..3 {
                    brightest[channel] = brightest[channel].max(neighbour[channel]);
                }
            }
            [
                brightest[0].saturating_sub(attenuation),
                brightest[1].saturating_sub(attenuation),
                brightest[2].saturating_sub(attenuation),
            ]
        }
        CagiRule::Diffusion6 => {
            let numerator = grid.diffusion_numerator();
            let mut sum = [0_u32; 3];
            for offset in FACE_OFFSETS {
                let neighbour = neighbour_light([
                    cell[0] + offset[0],
                    cell[1] + offset[1],
                    cell[2] + offset[2],
                ]);
                for channel in 0..3 {
                    sum[channel] += neighbour[channel];
                }
            }
            [
                (sum[0] * numerator) >> DIFFUSION_SHIFT,
                (sum[1] * numerator) >> DIFFUSION_SHIFT,
                (sum[2] * numerator) >> DIFFUSION_SHIFT,
            ]
        }
        CagiRule::Diffusion26 => {
            let numerator = grid.diffusion_26_numerator();
            let mut sum = [0_u32; 3];
            for offset_z in -1..=1_i32 {
                for offset_y in -1..=1_i32 {
                    for offset_x in -1..=1_i32 {
                        let axis_count = offset_x.abs() + offset_y.abs() + offset_z.abs();
                        if axis_count == 0 {
                            continue;
                        }
                        let weight = match axis_count {
                            1 => 4,
                            2 => 2,
                            _ => 1,
                        };
                        let neighbour = neighbour_light([
                            cell[0] + offset_x,
                            cell[1] + offset_y,
                            cell[2] + offset_z,
                        ]);
                        for channel in 0..3 {
                            sum[channel] += neighbour[channel] * weight;
                        }
                    }
                }
            }
            [
                (sum[0] * numerator) >> DIFFUSION_SHIFT,
                (sum[1] * numerator) >> DIFFUSION_SHIFT,
                (sum[2] * numerator) >> DIFFUSION_SHIFT,
            ]
        }
    }
}

/// The 6 face-neighbour offsets, in the shader's order.
pub const FACE_OFFSETS: [[i32; 3]; 6] = [
    [1, 0, 0],
    [-1, 0, 0],
    [0, 1, 0],
    [0, -1, 0],
    [0, 0, 1],
    [0, 0, -1],
];

/// Whether the cell at `cell` sees the sky by the column-max test — the CPU
/// mirror of `cagi_cell_sees_sky` in [`CagiSkyTest::ColumnMax`] mode, used by the
/// bench's cross-check to identify source cells.
pub fn cell_sees_sky_by_column(
    grid: &CagiGrid,
    column_max_brick_y: &[u32],
    cell: [u32; 3],
) -> bool {
    let brick_x = (cell[0] * grid.cell_voxels) as usize / BRICK_SIZE;
    let brick_y = (cell[1] * grid.cell_voxels) as usize / BRICK_SIZE;
    let brick_z = (cell[2] * grid.cell_voxels) as usize / BRICK_SIZE;
    let column = brick_x + brick_z * BRICK_GRID_X;
    brick_y as i32 > column_max_brick_y[column] as i32
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passes::cagi::CAGI_SHADER_SOURCE;
    use crate::passes::dda::SHADER_SOURCE;
    use voxel_core::world::Voxel;

    /// The Rust defaults and the two shader files' lever blocks must be the same
    /// configuration: the app's default pipelines are built from the UNPATCHED
    /// shipped sources.
    #[test]
    fn default_settings_match_shader_sources() {
        let settings = CagiSettings::default();
        assert_eq!(
            settings.patch_volume_consts(SHADER_SOURCE),
            SHADER_SOURCE,
            "CagiSettings::default() drifted from the CAGI levers in cagi_volume.wgsl"
        );
        let patched =
            settings.patch_propagation_consts(&settings.patch_volume_consts(CAGI_SHADER_SOURCE));
        assert_eq!(
            patched, CAGI_SHADER_SOURCE,
            "CagiSettings::default() drifted from the CAGI levers in cagi.wgsl"
        );
    }

    #[test]
    fn patched_sources_carry_every_lever() {
        let settings = CagiSettings {
            enabled: false,
            cell_voxels: 8,
            rule: CagiRule::Diffusion26,
            sample_mode: CagiSampleMode::Nearest,
            sky_test: CagiSkyTest::UpwardTrace,
            sun_cache: false,
            ..CagiSettings::default()
        };
        let volume = settings.patch_volume_consts(SHADER_SOURCE);
        assert!(volume.contains("const CAGI_ENABLED: bool = false;"));
        assert!(volume.contains("const CAGI_SAMPLE_MODE: u32 = 0u;"));
        let propagation = settings.patch_propagation_consts(CAGI_SHADER_SOURCE);
        assert!(propagation.contains("const CAGI_RULE: u32 = 2u;"));
        assert!(propagation.contains("const CAGI_SKY_TEST: u32 = 1u;"));
        assert!(propagation.contains("const CAGI_SUN_CACHE: bool = false;"));
        // The mode NAME constants must survive their own patching.
        assert!(propagation.contains("const CAGI_RULE_DIFFUSION_6: u32 = 1u;"));
        assert!(volume.contains("const CAGI_SAMPLE_TRILINEAR: u32 = 1u;"));
    }

    #[test]
    fn runtime_knobs_never_force_a_rebuild() {
        let applied = CagiSettings::default();
        for runtime_only in [
            CagiSettings {
                strength: 0.5,
                ..applied
            },
            CagiSettings {
                ambient_floor: 0.9,
                ..applied
            },
            CagiSettings {
                sun_bounce: 0.1,
                ..applied
            },
            CagiSettings {
                iterations_per_frame: 8,
                ..applied
            },
        ] {
            assert!(!runtime_only.requires_pipeline_rebuild(&applied));
            assert!(!runtime_only.requires_volume_rebuild(&applied));
        }
        // The resolution needs new buffers but no new pipeline: the grid
        // dimensions ride in the volume uniform.
        let coarser = CagiSettings {
            cell_voxels: 8,
            ..applied
        };
        assert!(!coarser.requires_pipeline_rebuild(&applied));
        assert!(coarser.requires_volume_rebuild(&applied));
        // Switching the experiment off changes both.
        let disabled = CagiSettings {
            enabled: false,
            ..applied
        };
        assert!(disabled.requires_pipeline_rebuild(&applied));
        assert!(disabled.requires_volume_rebuild(&applied));
    }

    /// Packing round-trip over the channel extremes and the flag bits: the
    /// integer volume's whole correctness rests on this.
    #[test]
    fn light_packing_round_trips() {
        for light in [
            [0, 0, 0],
            [1023, 1023, 1023],
            [1, 512, 1023],
            [409, 591, 1023],
        ] {
            assert_eq!(unpack_light(pack_light(light)), light);
        }
        // Saturation, not wraparound — a channel must never bleed into the next.
        assert_eq!(unpack_light(pack_light([2000, 0, 0])), [1023, 0, 0]);
        // The flag bits survive packing and are invisible to unpacking.
        let word = pack_light([1023, 1023, 1023]) | SUN_SOURCE_FLAG;
        assert_eq!(unpack_light(word), [1023, 1023, 1023]);
        assert_ne!(word & SUN_SOURCE_FLAG, 0);
    }

    #[test]
    fn radiance_quantization_matches_the_shader_curve() {
        assert_eq!(quantize_radiance([0.0, 0.0, 0.0]), [0, 0, 0]);
        assert_eq!(quantize_radiance([1.0, 1.0, 1.0]), [1023, 1023, 1023]);
        assert_eq!(quantize_radiance([2.0, -1.0, 0.5]), [1023, 0, 512]);
    }

    /// Grid indexing must round-trip, and the flat index must be x-major like
    /// every other grid in this renderer.
    #[test]
    fn grid_indexing_round_trips() {
        let grid = CagiGrid::for_world(4, 24);
        assert_eq!(grid.size[0], 250);
        assert_eq!(grid.size[2], 250);
        assert_eq!(grid.cell_index([0, 0, 0]), 0);
        assert_eq!(grid.cell_index([1, 0, 0]), 1);
        assert_eq!(grid.cell_index([0, 1, 0]), 250);
        assert_eq!(grid.cell_index([0, 0, 1]), 250 * grid.size[1] as usize);
        let mut seen = std::collections::HashSet::new();
        for z in 0..grid.size[2] {
            for y in 0..grid.size[1] {
                for x in 0..grid.size[0] {
                    let index = grid.cell_index([x, y, z]);
                    assert!(index < grid.cell_count());
                    assert!(seen.insert(index), "cell index {index} used twice");
                }
            }
        }
        assert_eq!(seen.len(), grid.cell_count());
    }

    /// The vertical clamp is the LOW-MEMORY lever of this experiment: the volume
    /// must cover every occupied voxel plus the sampler's margin, and nothing
    /// more.
    #[test]
    fn vertical_extent_is_clamped_to_the_occupied_height() {
        let grid = CagiGrid::for_world(4, 24);
        // 25 brick rows = 200 voxels = 50 cells, plus 2 margin cells.
        assert_eq!(grid.size[1], 52);
        // Never taller than the world.
        let tall = CagiGrid::for_world(4, 31);
        assert_eq!(tall.size[1], (WORLD_SIZE_Y as u32) / 4);
        // An empty world still gets a usable grid.
        let empty = CagiGrid::for_world(8, EMPTY_COLUMN);
        assert_eq!(empty.size[1], SKY_MARGIN_CELLS);
    }

    #[test]
    fn memory_scales_as_the_cube_of_the_resolution() {
        let fine = CagiGrid::for_world(2, 24);
        let medium = CagiGrid::for_world(4, 24);
        let coarse = CagiGrid::for_world(8, 24);
        assert_eq!(medium.volume_bytes() * 3, medium.total_bytes());
        // Each step doubles the cell edge, so cells fall by ~8x.
        assert!(fine.cell_count() > medium.cell_count() * 7);
        assert!(medium.cell_count() > coarse.cell_count() * 7);
        // Every configuration must be addressable as one wgpu buffer binding.
        assert!(fine.volume_bytes() < 256 * 1024 * 1024);
    }

    /// The physics must not change when the resolution lever moves: the reach of
    /// the max-decrement flood and the per-meter transmission of the diffusion
    /// rule are defined per METER and quantized per cell.
    #[test]
    fn transport_coefficients_are_resolution_independent() {
        for cell_voxels in [2, 4, 8] {
            let grid = CagiGrid::for_world(cell_voxels, 24);
            let reach_meters = CHANNEL_MAX as f32 / grid.attenuation() as f32 * grid.cell_meters();
            assert!(
                (reach_meters - CHANNEL_MAX as f32 / ATTENUATION_PER_METER).abs() < 1.0,
                "{cell_voxels}-voxel cells reach {reach_meters} m"
            );
            let transmission_per_meter = grid.transmission().powf(1.0 / grid.cell_meters());
            assert!(
                (transmission_per_meter - TRANSMISSION_PER_METER).abs() < 1e-3,
                "{cell_voxels}-voxel cells transmit {transmission_per_meter} per meter"
            );
            // The fixed-point numerators must round to something usable.
            assert!(grid.diffusion_numerator() > 0);
            assert!(grid.diffusion_26_numerator() > 0);
        }
    }

    /// The diffusion rule's fixed-point sum can never overflow u32: six (or the
    /// 26-neighbour weighted sum of) saturated channels times the numerator.
    #[test]
    fn diffusion_arithmetic_cannot_overflow() {
        for cell_voxels in [2, 4, 8] {
            let grid = CagiGrid::for_world(cell_voxels, 24);
            let worst_6 = (CHANNEL_MAX * 6) as u64 * grid.diffusion_numerator() as u64;
            let worst_26 = (CHANNEL_MAX * NEIGHBOUR_26_WEIGHT_SUM) as u64
                * grid.diffusion_26_numerator() as u64;
            assert!(worst_6 < u32::MAX as u64, "6-neighbour sum overflows");
            assert!(worst_26 < u32::MAX as u64, "26-neighbour sum overflows");
        }
    }

    /// Hand-computed reference values for each rule — the CPU reference the bench
    /// cross-checks the GPU against must itself be pinned.
    #[test]
    fn reference_rules_match_hand_computed_values() {
        let grid = CagiGrid::for_world(4, 24);
        let attenuation = grid.attenuation();
        assert_eq!(attenuation, 40); // 80/m at 0.5 m cells

        // One bright neighbour, five dark: max-decrement keeps the brightest
        // minus the attenuation; diffusion averages.
        let mut single_bright = |cell: [i32; 3]| {
            if cell == [1, 0, 0] {
                [1000, 500, 100]
            } else {
                [0, 0, 0]
            }
        };
        assert_eq!(
            propagate_reference(CagiRule::MaxDecrement, &grid, [0, 0, 0], &mut single_bright),
            [960, 460, 60]
        );
        let numerator = grid.diffusion_numerator();
        assert_eq!(numerator, 642);
        assert_eq!(
            propagate_reference(CagiRule::Diffusion6, &grid, [0, 0, 0], &mut single_bright),
            [
                (1000 * numerator) >> DIFFUSION_SHIFT,
                (500 * numerator) >> DIFFUSION_SHIFT,
                (100 * numerator) >> DIFFUSION_SHIFT
            ]
        );
        // Saturating subtraction: a dim neighbourhood floors at zero.
        let mut all_dim = |_cell: [i32; 3]| [10, 0, 0];
        assert_eq!(
            propagate_reference(CagiRule::MaxDecrement, &grid, [0, 0, 0], &mut all_dim),
            [0, 0, 0]
        );
        // Uniform surroundings under diffusion decay by exactly the
        // transmission, which is the property that makes the flood converge.
        let mut uniform = |_cell: [i32; 3]| [600, 600, 600];
        let diffused = propagate_reference(CagiRule::Diffusion6, &grid, [0, 0, 0], &mut uniform);
        assert_eq!(diffused[0], (3600 * numerator) >> DIFFUSION_SHIFT);
        assert!(diffused[0] < 600 && diffused[0] > 550);
        // The 26-neighbour rule reaches the same equilibrium from a uniform
        // neighbourhood — it differs in SHAPE, not in energy.
        let diffused_26 =
            propagate_reference(CagiRule::Diffusion26, &grid, [0, 0, 0], &mut uniform);
        assert!(
            diffused_26[0].abs_diff(diffused[0]) <= 2,
            "26-neighbour equilibrium {} vs 6-neighbour {}",
            diffused_26[0],
            diffused[0]
        );
    }

    /// E2: the single-cell attribute recompute must agree with the full build on
    /// the real world, cell for cell — including the tie-breaking order that
    /// decides which occupied voxel's albedo a cell shows.
    /// NOTE: generates the full world — run with `--release`.
    #[test]
    fn one_cell_attribute_matches_the_full_build() {
        let world = voxel_core::world::VoxelWorld::generate(1234, 0.0);
        let brickmap = Brickmap::build(&world);
        for cell_voxels in [4, 8] {
            let grid = CagiGrid::for_world(cell_voxels, brickmap.metadata().max_occupied_brick_y);
            let built = build_cell_attributes(&brickmap, &grid, &MaterialAttributes::compiled());
            let mut non_empty_checked = 0_usize;
            // Deterministic stride over the whole volume, prime-ish so it walks
            // every axis instead of one slab.
            for index in (0..grid.cell_count()).step_by(1013) {
                let cell = [
                    (index % grid.size[0] as usize) as u32,
                    ((index / grid.size[0] as usize) % grid.size[1] as usize) as u32,
                    (index / (grid.size[0] as usize * grid.size[1] as usize)) as u32,
                ];
                let expected = built[grid.cell_index(cell)];
                assert_eq!(
                    cell_attribute(&brickmap, &grid, cell, &MaterialAttributes::compiled()),
                    expected,
                    "cell {cell:?} at {cell_voxels}-voxel resolution"
                );
                if expected != 0 {
                    non_empty_checked += 1;
                }
            }
            assert!(
                non_empty_checked > 50,
                "only {non_empty_checked} non-empty cells sampled at {cell_voxels} voxels"
            );
        }
    }

    /// The 4-bit transmittance quantizer must hit both endpoints exactly:
    /// stone at 0 is what keeps opaque geometry bit-identical with the lever on,
    /// and a hypothetical 1.0 must not wrap into a neighbouring bit field.
    #[test]
    fn transmittance_quantizes_within_its_four_bits() {
        assert_eq!(quantize_transmittance(0.0), 0);
        assert_eq!(
            quantize_transmittance(1.0) >> CELL_TRANSMITTANCE_SHIFT,
            CELL_TRANSMITTANCE_LEVELS
        );
        // Out-of-range input must clamp, never overflow into bit 29+.
        for value in [-1.0, 1.5, f32::INFINITY] {
            assert_eq!(
                quantize_transmittance(value) & !CELL_TRANSMITTANCE_MASK,
                0,
                "{value} escaped the transmittance field"
            );
        }
        // Monotonic, and round-to-nearest rather than truncating.
        let level = |value: f32| quantize_transmittance(value) >> CELL_TRANSMITTANCE_SHIFT;
        assert!(level(0.25) < level(0.5) && level(0.5) < level(0.85));
        assert_eq!(level(1.0 / CELL_TRANSMITTANCE_LEVELS as f32), 1);
    }

    /// The attribute word's three fields must not collide: albedo owns 0-23,
    /// solidity bit 24, transmittance 25-28. A shift error here would tint the
    /// world or silently mark cells solid.
    #[test]
    fn attribute_fields_do_not_overlap() {
        assert_eq!(CELL_TRANSMITTANCE_MASK & 0x00ff_ffff, 0, "albedo collision");
        assert_eq!(
            CELL_TRANSMITTANCE_MASK & CELL_SOLID,
            0,
            "solid-flag collision"
        );
        assert_eq!(CELL_TRANSMITTANCE_MASK, 0x1e00_0000);
    }

    /// Opaque materials must carry transmittance 0 through the whole pipeline,
    /// because that is what makes `CAGI_TRANSMISSION` a no-op on stone — the
    /// claim the lever's verdict rests on ("any delta outside vegetation is a
    /// bug"). Foliage must carry a non-zero one, or M2 fixes nothing.
    #[test]
    fn opaque_materials_transmit_nothing_and_foliage_transmits_something() {
        let table = MaterialAttributes::compiled();
        let field =
            |voxel| table.word(crate::material::material_id(voxel)) & CELL_TRANSMITTANCE_MASK;
        for opaque in [Voxel::Stone, Voxel::Dirt, Voxel::Sand, Voxel::Trunk] {
            assert_eq!(field(opaque), 0, "{opaque:?} must not transmit");
        }
        for foliage in [Voxel::Leaves, Voxel::LeavesPine, Voxel::TallGrass] {
            assert!(field(foliage) > 0, "{foliage:?} must transmit");
        }
    }

    /// The attribute builders must agree on transmittance exactly as they
    /// already do on albedo — the E2 incremental path recomputing one cell must
    /// never disagree with a full rebuild.
    #[test]
    fn both_attribute_builders_pack_transmittance_identically() {
        let materials = MATERIALS;
        let table = MaterialAttributes::compiled();
        for (id, material) in materials.iter().enumerate() {
            let packed = table.word(id as u8);
            assert_eq!(
                packed & CELL_TRANSMITTANCE_MASK,
                quantize_transmittance(material.transmittance()),
                "material {id} transmittance drifted"
            );
            assert_eq!(
                packed & !(CELL_TRANSMITTANCE_MASK | CELL_EMITTER_MASK),
                packed_albedo(material.albedo),
                "material {id} albedo drifted"
            );
        }
    }

    /// The emitter field must name exactly the EMISSIVE rows, and must not
    /// collide with albedo, solidity or transmittance. A shift error here would
    /// turn ordinary terrain into light sources.
    #[test]
    fn the_emitter_field_names_exactly_the_emissive_rows() {
        assert_eq!(CELL_EMITTER_MASK, 0xe000_0000);
        assert_eq!(CELL_EMITTER_MASK & CELL_TRANSMITTANCE_MASK, 0);
        assert_eq!(CELL_EMITTER_MASK & CELL_SOLID, 0);
        assert_eq!(CELL_EMITTER_MASK & 0x00ff_ffff, 0);

        let table = MaterialAttributes::compiled();
        for (id, material) in MATERIALS.iter().enumerate() {
            let slot = (table.word(id as u8) & CELL_EMITTER_MASK) >> CELL_EMITTER_SHIFT;
            assert_eq!(
                slot != 0,
                material.is_emissive(),
                "{} emitter slot disagrees with its EMISSIVE flag",
                material.name
            );
        }
    }

    /// Every emissive material must reach the CA with its authored radiance
    /// intact: the index the cell stores has to select that material's row in
    /// the palette the shader reads.
    #[test]
    fn every_emitter_round_trips_through_the_palette() {
        let palette = emitter_palette(&MATERIALS);
        assert_eq!(palette.len(), EMITTER_PALETTE_SLOTS);
        assert_eq!(palette[0], [0.0; 4], "slot 0 is what non-emitters read");

        let table = MaterialAttributes::compiled();
        let mut emitters_seen = 0;
        for (id, material) in MATERIALS.iter().enumerate() {
            let slot = ((table.word(id as u8) & CELL_EMITTER_MASK) >> CELL_EMITTER_SHIFT) as usize;
            if slot == 0 {
                continue;
            }
            emitters_seen += 1;
            assert_eq!(
                &palette[slot][..3],
                &material.emitted_radiance()[..],
                "{} radiance did not survive the palette",
                material.name
            );
        }
        assert_eq!(emitters_seen, 2, "M1b authored two emitters");
    }

    /// Distinct emitters must not share a slot — two lights that differ in the
    /// table have to differ in the volume.
    #[test]
    fn emitters_get_distinct_slots() {
        let table = MaterialAttributes::compiled();
        let slots: Vec<u32> = MATERIALS
            .iter()
            .enumerate()
            .filter(|(_, material)| material.is_emissive())
            .map(|(id, _)| (table.word(id as u8) & CELL_EMITTER_MASK) >> CELL_EMITTER_SHIFT)
            .collect();
        let mut unique = slots.clone();
        unique.sort_unstable();
        unique.dedup();
        assert_eq!(
            unique.len(),
            slots.len(),
            "two emitters share a palette slot"
        );
        assert!(
            slots.iter().all(|slot| (*slot as usize) <= MAX_EMITTERS),
            "an emitter slot escaped the 3-bit field"
        );
    }

    /// A placed glow block must actually become an emissive cell — the E2
    /// single-cell recompute is what the L key ultimately drives, so if it
    /// dropped the emitter bits the light would never appear.
    #[test]
    fn an_edited_cell_carries_its_emitter() {
        let table = MaterialAttributes::compiled();
        let glow_block = table.word(crate::material::material_id(Voxel::GlowBlock));
        assert_ne!(
            glow_block & CELL_EMITTER_MASK,
            0,
            "the glow block must emit"
        );
        let stone = table.word(crate::material::material_id(Voxel::Stone));
        assert_eq!(stone & CELL_EMITTER_MASK, 0, "stone must not emit");
    }

    #[test]
    fn placeholder_grid_is_one_cell() {
        let disabled = CagiSettings {
            enabled: false,
            ..CagiSettings::default()
        };
        let grid = CagiGrid::placeholder();
        assert_eq!(grid.cell_count(), 1);
        assert_eq!(grid.total_bytes(), 12);
        assert_eq!(disabled.cell_voxels, 4); // the knob keeps its value
    }

    #[test]
    #[should_panic(expected = "must divide")]
    fn a_cell_size_that_straddles_bricks_panics() {
        CagiGrid::for_world(3, 24);
    }
    // ---- S2: a live-edited material must reach the light volume ----------------

    /// **The bug this seam exists to fix.** The attribute builders used to read the
    /// COMPILED table, so a live material edit could never reach the GI bounce no matter
    /// how often the attributes were re-packed — the re-pack recomputed the values it
    /// already had. The panel documented a two-tier model in which the second tier did
    /// not work.
    #[test]
    fn a_live_edited_albedo_reaches_the_cell_attributes() {
        let stone = crate::material::material_id(Voxel::Stone);
        let compiled = MaterialAttributes::compiled();

        let mut rows = MATERIALS.to_vec();
        rows[stone as usize].albedo = [1.0, 0.0, 0.0];
        let edited = material_attribute_table(&rows);

        assert_ne!(
            compiled.word(stone),
            edited.word(stone),
            "an edited albedo did not change the attribute word"
        );
        // Only that row moved.
        for id in 0..MATERIAL_COUNT as u8 {
            if id == stone {
                continue;
            }
            assert_eq!(compiled.word(id), edited.word(id), "row {id} moved");
        }
    }

    /// A patterned emitter must claim a palette slot and inject its MEAN.
    ///
    /// This is the whole "let them emit light" feature: a stone row with red emissive
    /// specks has `emission: None`, so before S2 it was not in the emitter set at all
    /// and the specks lit nothing.
    #[test]
    fn a_patterned_emitter_claims_a_slot_and_injects_its_mean() {
        use crate::pattern::{PatternBlend, PatternLayer, PatternStack, PatternTarget};

        let stone = crate::material::material_id(Voxel::Stone);
        let mut rows = MATERIALS.to_vec();
        assert!(
            !rows[stone as usize].is_emissive(),
            "stone must not start emissive"
        );
        assert_eq!(
            MaterialAttributes::compiled().word(stone) & CELL_EMITTER_MASK,
            0,
            "stone must not start with an emitter slot"
        );

        let specks = PatternLayer {
            target: PatternTarget::Emission,
            blend: PatternBlend::Add,
            amount: 1.0,
            target_color: [4.0, 0.0, 0.0],
            ..PatternLayer::IDENTITY
        };
        rows[stone as usize].patterns = PatternStack::of(&[specks]);

        // It is now an emitter, so it takes a cell-attribute slot...
        assert!(rows[stone as usize].is_emissive());
        let slot =
            (material_attribute_table(&rows).word(stone) & CELL_EMITTER_MASK) >> CELL_EMITTER_SHIFT;
        assert_ne!(slot, 0, "a patterned emitter got no emitter slot");

        // ...and the palette carries the MEAN, which must sit strictly between "no
        // light at all" and "the speck colour everywhere" — that IS the point of a mean.
        let palette = emitter_palette(&rows);
        let injected = palette[slot as usize];
        assert!(
            injected[0] > 0.0 && injected[0] < 4.0,
            "the injected red {} is not a mean of 0 and 4",
            injected[0]
        );
        // Green and blue were never emitted, so they must stay dark.
        assert_eq!(injected[1], 0.0);
        assert_eq!(injected[2], 0.0);
    }

    /// The mean must scale with the layer's amount, or the brightness slider does not
    /// control how much light the surface casts.
    #[test]
    fn the_injected_mean_follows_the_layers_amount() {
        use crate::pattern::{PatternBlend, PatternLayer, PatternStack, PatternTarget};

        let mean_at = |amount: f32| {
            let mut row = MATERIALS[crate::material::material_id(Voxel::Stone) as usize];
            row.patterns = PatternStack::of(&[PatternLayer {
                target: PatternTarget::Emission,
                blend: PatternBlend::Add,
                amount,
                target_color: [4.0, 0.0, 0.0],
                ..PatternLayer::IDENTITY
            }]);
            row.mean_emitted_radiance()[0]
        };
        let dim = mean_at(0.25);
        let bright = mean_at(1.0);
        assert!(dim > 0.0);
        assert!(bright > dim * 2.0, "{dim} -> {bright} is not proportional");

        // Amount zero is not an emitter at all: no slot spent, nothing injected.
        let mut row = MATERIALS[crate::material::material_id(Voxel::Stone) as usize];
        row.patterns = PatternStack::of(&[PatternLayer {
            target: PatternTarget::Emission,
            blend: PatternBlend::Add,
            amount: 0.0,
            ..PatternLayer::IDENTITY
        }]);
        assert!(
            !row.is_emissive(),
            "a zero-amount layer must not claim a slot"
        );
        assert_eq!(row.mean_emitted_radiance(), [0.0; 3]);
    }

    /// A row that emits WITHOUT patterns must be untouched by the mean machinery — the
    /// two shipped emissive rows are the regression risk here.
    #[test]
    fn an_unpatterned_emitter_injects_exactly_its_authored_radiance() {
        for row in &MATERIALS {
            if !row.is_emissive() {
                continue;
            }
            assert!(
                !row.has_emission_layers(),
                "{} authors an emission layer, which no shipped row should yet",
                row.name
            );
            assert_eq!(
                row.mean_emitted_radiance(),
                row.emitted_radiance(),
                "{} drifted from its authored emission",
                row.name
            );
        }
    }
}
