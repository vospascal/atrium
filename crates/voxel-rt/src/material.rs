//! M1 — the material table: one row per material id, the single place a voxel
//! type's *appearance and physics* are described.
//!
//! Replaces the colour-only `palette()` this module grew out of. The change is
//! an indirection, not a data-size change: the world still stores ONE BYTE per
//! voxel ([`material_id`]), and everything below is a 24-row lookup table
//! costing [`MATERIAL_TABLE_BYTES`] (1152 bytes) on the GPU. Material richness
//! is therefore effectively free; per-*voxel* state is the expensive axis and
//! this module deliberately does not touch it.
//!
//! Two halves, one source of truth:
//!
//! * [`Material`] — the authored row, CPU-side, including the acoustic
//!   coefficients that no GPU pass reads.
//! * [`GpuMaterial`] — the render subset, `#[repr(C)]` and std430-clean at 48
//!   bytes, uploaded to binding 5 (`shaders/world.wgsl`).
//!
//! ## Which fields have consumers today
//!
//! Only `albedo` is read by a shipped pass ([`crate::passes::dda`] shading and
//! [`crate::cagi`]'s bounce tint). The rest are authored now because the cost
//! of this table is *hand-writing 24 rows*, and doing that twice is worse than
//! carrying fields that are still dark:
//!
//! | field | consumer | status |
//! |-------|----------|--------|
//! | `albedo` | DDA shading, CAGI bounce tint | live |
//! | `transmittance` | CAGI transport (M2) | authored, unread |
//! | `emission` | CAGI emissive injection (E5) | live at M1b: 2 rows |
//! | `roughness`, `specular` | reflections (F2) | authored, unread |
//! | `opacity` | transparent traversal continuation | authored, unread |
//! | `flags` | `FOLIAGE` drives ray-skew grass | authored, unread |
//! | `acoustic_alpha` | atrium's reverb model | authored, unread |
//!
//! M1b added the first two EMISSIVE rows — `GlowBlock` (bright, warm, solid)
//! and `GlowBerry` (dim, cool, thin cover) — so E5 has something to light with and
//! two contrasting cases to light it with: an occluding source and a
//! non-occluding one. Neither is PLACED by world generation; see the note on
//! [`materials`].
//!
//! ## The acoustic column
//!
//! [`Material::acoustic_alpha`] uses the SAME six octave bands (125, 250, 500,
//! 1k, 2k, 4k Hz) and the same 0.0-reflective..1.0-absorptive convention as
//! atrium's `WallMaterial::alpha` (`src/pipeline/path.rs`), and the values are
//! taken from its constructors where one matches. It is a shared *vocabulary*,
//! not a shared type: voxel-rt does not depend on the atrium crate, and adding
//! that dependency to import a `[f32; 6]` would couple the renderer to the
//! audio engine for no benefit. The point of the column is that when a pass
//! eventually wants "what does this surface do to sound", the answer is already
//! authored and already speaks atrium's units.
//!
//! Note that **water is acoustically near-perfectly reflective** (alpha ~0.01),
//! which is the opposite of the visual intuition its transparency suggests.

use voxel_core::world::Voxel;

/// Index of refraction of air — 1.000293 in reality, and the value carried by
/// every row that does NOT refract, which is all of them except the liquids. A
/// ray entering an opaque voxel is not a thing, so "refracts like the air around
/// it" is the honest authored value rather than a sentinel.
pub const AIR_INDEX_OF_REFRACTION: f32 = 1.0;

/// Index of refraction of water at ~20 C, sodium-D line — the textbook 1.333.
/// `crate::water` derives Fresnel's `F0` and the 48.6-degree critical angle from
/// it rather than hardcoding either.
pub const WATER_INDEX_OF_REFRACTION: f32 = 1.333;

/// Water's **absorption** coefficient per metre, per channel — light that is
/// REMOVED from a ray, converted to heat and gone.
///
/// Red is absorbed roughly 30x faster than blue, which is the physical reason a
/// deep body of water is blue and the reason the bed under 5 m of it has no red
/// left. Pure water measures ~(0.35, 0.056, 0.015) /m (Pope & Fry 1997; Smith &
/// Baker 1981); these sit slightly above it for green and blue, which is what
/// dissolved organics in a lake do.
pub const WATER_ABSORPTION_PER_METER: [f32; 3] = [0.446, 0.090, 0.015];

/// Water's **scattering** coefficient per metre, per channel — light REDIRECTED
/// rather than destroyed, which is what a ray can pick up along its path and the
/// only reason deep clear ocean is blue where no bottom is visible at all.
///
/// Short wavelengths scatter hardest (molecular scattering goes as λ⁻⁴, softened
/// here by particulates), so the pair below makes water's colour **emerge**:
/// nothing in this table paints it.
///
/// The two coefficients together are what make the *material class* expressible —
/// water (moderate absorption, weak blue-favouring scattering), oil and honey
/// (strong absorption, low scattering) and **clouds** (near-zero absorption,
/// scattering-dominated) are one model with different numbers. A single painted
/// albedo cannot express a cloud at all, because a cloud IS scattering.
pub const WATER_SCATTERING_PER_METER: [f32; 3] = [0.004, 0.030, 0.045];

/// Number of material ids (== number of `Voxel` variants, Air included).
pub const MATERIAL_COUNT: usize = 26;

/// Size of the uploaded material table — the whole GPU cost of this module.
pub const MATERIAL_TABLE_BYTES: usize = MATERIAL_COUNT * std::mem::size_of::<GpuMaterial>();

/// `Voxel` -> material id, in enum declaration order with `Air = 0`
/// (crates/voxel-core/src/world.rs, the 26-variant `Voxel` enum).
pub fn material_id(voxel: Voxel) -> u8 {
    match voxel {
        Voxel::Air => 0,
        Voxel::Grass => 1,
        Voxel::TallGrass => 2,
        Voxel::Dirt => 3,
        Voxel::Sand => 4,
        Voxel::Sediment => 5,
        Voxel::Stone => 6,
        Voxel::Water => 7,
        Voxel::Trunk => 8,
        Voxel::TrunkBirch => 9,
        Voxel::Leaves => 10,
        Voxel::LeavesDark => 11,
        Voxel::LeavesBirch => 12,
        Voxel::LeavesPine => 13,
        Voxel::FlowerPink => 14,
        Voxel::FlowerWhite => 15,
        Voxel::FlowerYellow => 16,
        Voxel::FlowerBlue => 17,
        Voxel::WaterWeed => 18,
        Voxel::LilyPad => 19,
        Voxel::LilyBloom => 20,
        Voxel::Reed => 21,
        Voxel::CattailHead => 22,
        Voxel::Snow => 23,
        Voxel::GlowBlock => 24,
        Voxel::GlowBerry => 25,
    }
}

/// Material id -> `Voxel`, the exact inverse of [`material_id`]. Unknown ids
/// (nothing writes one) read as [`Voxel::Air`], the miss sentinel.
///
/// Exists because the world stores ONE BYTE per voxel and the CPU-side consumers
/// of that byte — E2b's character collision, and later the fluid CA — need the
/// voxel *semantics* `voxel-core` already defines rather than a second copy of
/// them in this crate.
pub fn material_voxel(material: u8) -> Voxel {
    match material {
        1 => Voxel::Grass,
        2 => Voxel::TallGrass,
        3 => Voxel::Dirt,
        4 => Voxel::Sand,
        5 => Voxel::Sediment,
        6 => Voxel::Stone,
        7 => Voxel::Water,
        8 => Voxel::Trunk,
        9 => Voxel::TrunkBirch,
        10 => Voxel::Leaves,
        11 => Voxel::LeavesDark,
        12 => Voxel::LeavesBirch,
        13 => Voxel::LeavesPine,
        14 => Voxel::FlowerPink,
        15 => Voxel::FlowerWhite,
        16 => Voxel::FlowerYellow,
        17 => Voxel::FlowerBlue,
        18 => Voxel::WaterWeed,
        19 => Voxel::LilyPad,
        20 => Voxel::LilyBloom,
        21 => Voxel::Reed,
        22 => Voxel::CattailHead,
        23 => Voxel::Snow,
        24 => Voxel::GlowBlock,
        25 => Voxel::GlowBerry,
        _ => Voxel::Air,
    }
}

/// Whether a material id blocks a body — the PHYSICS predicate
/// ([`crate::character`]).
///
/// It is `voxel-core`'s `Voxel::is_solid()`, which excludes air, **water** and
/// **thin cover** (tall grass, flowers, reeds, lily pads, weeds): you walk
/// through vegetation and into water. Leaves count as solid, so a canopy is
/// standable and a tree is an obstacle without a special case.
pub fn material_blocks_movement(material: u8) -> bool {
    material_voxel(material).is_solid()
}

/// Whether a material id counts as EMPTY to the world editor — **the plan's "to
/// the editor, water IS air" rule** (Pascal, 2026-07-31: *"we need to treat water
/// as air basically when adding and removing blocks"*).
///
/// This is the single notion of emptiness the whole edit path shares, and it is
/// deliberately ONE predicate rather than a liquid special case per call site: an
/// `is air` test anywhere on that path is a bug. Air is empty, and so is every
/// [`MaterialFlags::LIQUID`] row — which is what makes the next transparent fluid
/// (oil, honey; the materials the dossier records as xima's own transparency
/// targets) inherit the behaviour without a branch of its own.
///
/// What follows from it, in one place:
///
/// * an edit ray passes through water exactly as it passes through air
///   ([`crate::voxel_dda::CastTarget::EditableVoxel`]), so a click into a pond
///   lands on the bed and not on the skin;
/// * the placement cell may be a water cell, and the placed solid *displaces* the
///   water (overwritten today — B6's fluid CA owns displacement properly later);
/// * water itself is never the target of a removal, because it is never what the
///   ray stops on. It is not a block.
pub fn material_is_empty_for_edits(material: u8) -> bool {
    matches!(material_voxel(material), Voxel::Air) || material_is_liquid(material)
}

/// Whether a material id is a liquid — the per-voxel form of
/// [`MaterialFlags::LIQUID`], without building the table (the character samples
/// this a few times per frame, and so does E6's water-medium march).
/// `liquid_predicate_agrees_with_the_table` pins the two together.
pub fn material_is_liquid(material: u8) -> bool {
    matches!(material_voxel(material), Voxel::Water)
}

/// Per-material boolean properties, packed into one word for the GPU.
///
/// A newtype rather than bare `u32` so a flag word can never be passed where a
/// material id is expected, and rather than a `bitflags` dependency because
/// four flags do not justify one.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct MaterialFlags(u32);

impl MaterialFlags {
    /// No flags — the common case (plain opaque terrain).
    pub const NONE: MaterialFlags = MaterialFlags(0);
    /// Thin vegetation: the intended target of the ray-direction-skew wind
    /// animation (bend the ray entering the voxel instead of moving geometry).
    pub const FOLIAGE: MaterialFlags = MaterialFlags(1 << 0);
    /// Emits light — the CAGI injection candidate test (E5).
    pub const EMISSIVE: MaterialFlags = MaterialFlags(1 << 1);
    /// Traversal continues through this voxel instead of terminating.
    pub const TRANSPARENT: MaterialFlags = MaterialFlags(1 << 2);
    /// A fluid: relevant to the fluid CA, to swim/wade movement, and to the
    /// "listener is submerged" audio test.
    pub const LIQUID: MaterialFlags = MaterialFlags(1 << 3);

    /// Both flag sets combined.
    pub const fn union(self, other: MaterialFlags) -> MaterialFlags {
        MaterialFlags(self.0 | other.0)
    }

    /// Whether every flag in `other` is set here.
    pub const fn contains(self, other: MaterialFlags) -> bool {
        (self.0 & other.0) == other.0
    }

    /// The raw word, for upload and for WGSL bit tests.
    pub const fn bits(self) -> u32 {
        self.0
    }
}

/// One authored material row.
///
/// `albedo` values are sRGB-encoded exactly as they were in the colour palette
/// this table replaced (lifted originally from `voxel-sandbox`'s `voxel_color`
/// match, one representative value per type — positional variation such as
/// dryness/season/depth blending is still not modelled here).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    /// Human-readable name; diagnostics and the overlay only.
    pub name: &'static str,
    /// Diffuse colour, sRGB-encoded. Also the GI bounce tint.
    pub albedo: [f32; 3],
    /// Emitted radiance, linear and unbounded above 1.0 — a source is allowed
    /// to be brighter than any surface can reflect. Non-zero on the M1b
    /// emissive rows only.
    pub emission: [f32; 3],
    /// 0.0 = mirror, 1.0 = fully diffuse.
    pub roughness: f32,
    /// Specular reflectance at normal incidence (F0).
    pub specular: f32,
    /// 1.0 = opaque. Below 1.0 the traversal must continue through the voxel.
    pub opacity: f32,
    /// Fraction of light passing THROUGH the voxel, for transport rather than
    /// for viewing: this is what stops CAGI treating a leaf canopy as a wall.
    pub transmittance: f32,
    /// Index of refraction — how hard this material bends a ray that enters it,
    /// and (through `((n - 1) / (n + 1))^2`) how much it mirrors at normal
    /// incidence. **1.0 means "does not refract"**, which is the honest value for
    /// every opaque row: the E6 water model reads this column only for materials
    /// it actually enters.
    ///
    /// Authored per material rather than as a water constant on purpose (E6,
    /// 2026-07-31): the dossier records xima's own transparency target as *"water,
    /// oil, clouds and honey"* — transparency is a per-material CLASS, and every
    /// member of it has its own index (water 1.333, oil ~1.47, honey ~1.50).
    /// Retrofitting a constant out of the Snell code later is far more work than
    /// authoring the column now, and it costs no bytes: the value took the GPU
    /// row's existing pad slot.
    pub index_of_refraction: f32,
    /// **Absorption** coefficient per metre, per channel: light this medium
    /// removes from a ray passing through it. All-zero for everything a ray cannot
    /// enter.
    ///
    /// Authored as a coefficient PAIR with [`Self::scattering_per_meter`] rather
    /// than as a volume colour, because a medium's colour is not a property you
    /// pick — it *emerges* from which wavelengths are absorbed and which are
    /// scattered (Pascal, 2026-07-31: *"water shouldn't have a colour really ..
    /// water blocks light coming in"*). The E6 model therefore derives every
    /// colour it shows from these two triples: extinction is
    /// `absorption + scattering`, and the medium's own apparent colour is the
    /// single-scattering albedo `scattering / extinction`. Nothing is painted.
    pub absorption_per_meter: [f32; 3],
    /// **Scattering** coefficient per metre, per channel: light this medium
    /// redirects rather than destroys, and therefore the light a ray picks UP along
    /// its path. All-zero for everything a ray cannot enter.
    pub scattering_per_meter: [f32; 3],
    /// Boolean properties.
    pub flags: MaterialFlags,
    /// Acoustic absorption at [125, 250, 500, 1k, 2k, 4k] Hz; 0.0 fully
    /// reflective, 1.0 fully absorptive. See the module docs on the shared
    /// vocabulary with atrium's `WallMaterial::alpha`.
    pub acoustic_alpha: [f32; 6],
}

impl Material {
    /// This row's GPU subset — everything except `name` and `acoustic_alpha`.
    pub fn to_gpu(self) -> GpuMaterial {
        GpuMaterial {
            albedo: self.albedo,
            transmittance: self.transmittance,
            emission: self.emission,
            roughness: self.roughness,
            opacity: self.opacity,
            specular: self.specular,
            flags: self.flags.bits(),
            index_of_refraction: self.index_of_refraction,
            absorption_per_meter: self.absorption_per_meter,
            _pad_absorption: 0.0,
            scattering_per_meter: self.scattering_per_meter,
            _pad_scattering: 0.0,
        }
    }

    /// Extinction per metre, per channel: `absorption + scattering` — the total
    /// rate at which this medium removes light from a ray, and the exponent of the
    /// Beer-Lambert term.
    pub fn extinction_per_meter(self) -> [f32; 3] {
        [
            self.absorption_per_meter[0] + self.scattering_per_meter[0],
            self.absorption_per_meter[1] + self.scattering_per_meter[1],
            self.absorption_per_meter[2] + self.scattering_per_meter[2],
        ]
    }

    /// **Single-scattering albedo**, `scattering / extinction`, per channel — the
    /// share of the light this medium takes out of a ray that it puts back into
    /// some other direction rather than destroying.
    ///
    /// This IS the medium's apparent colour, and it is derived rather than
    /// authored: for water it comes out ~(0.009, 0.25, 0.75), i.e. deeply blue with
    /// almost no red, purely because red is absorbed ~30x faster than blue while
    /// blue scatters ~11x more than red. Zero for a medium with no scattering (an
    /// absorption-only medium darkens without colouring). Channels with no
    /// extinction at all read 0 rather than dividing by zero.
    pub fn single_scattering_albedo(self) -> [f32; 3] {
        let extinction = self.extinction_per_meter();
        let mut albedo = [0.0_f32; 3];
        for channel in 0..3 {
            if extinction[channel] > 0.0 {
                albedo[channel] = self.scattering_per_meter[channel] / extinction[channel];
            }
        }
        albedo
    }
}

/// The uploaded row: std430-clean at 80 bytes (five 16-byte rows, each a
/// `vec3<f32>` followed by the scalar that fills its `w` slot), matching
/// `struct Material` in `shaders/world.wgsl`.
///
/// E6 spent the third row's former pad word on `index_of_refraction` (free) and
/// then added two rows for the absorption/scattering pair — 32 bytes per material,
/// **2 KB for the whole table**, which is the price of a medium whose colour is
/// derived instead of painted.
#[repr(C)]
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GpuMaterial {
    pub albedo: [f32; 3],
    pub transmittance: f32,
    pub emission: [f32; 3],
    pub roughness: f32,
    pub opacity: f32,
    pub specular: f32,
    pub flags: u32,
    pub index_of_refraction: f32,
    pub absorption_per_meter: [f32; 3],
    pub _pad_absorption: f32,
    pub scattering_per_meter: [f32; 3],
    pub _pad_scattering: f32,
}

// Manual impls instead of derive so we do not depend on bytemuck's `derive`
// feature flag: the struct is `#[repr(C)]`, all fields are u32/f32, and the
// [f32; 3] + f32 pairs pack to 16 bytes exactly, so there are no padding bytes.
unsafe impl bytemuck::Zeroable for GpuMaterial {}
unsafe impl bytemuck::Pod for GpuMaterial {}

// ---- Acoustic coefficient sets ----------------------------------------------
// Named once and shared by the rows that use them, so a change to "what soft
// ground sounds like" is one edit rather than four. Values from atrium's
// `WallMaterial` constructors (Yeoward 2021 / ISO 11654) where one matches.

/// atrium `WallMaterial::grass()`: soft outdoor ground.
const ACOUSTIC_SOFT_GROUND: [f32; 6] = [0.10, 0.20, 0.40, 0.55, 0.60, 0.60];
/// atrium `WallMaterial::stone()`.
const ACOUSTIC_STONE: [f32; 6] = [0.02, 0.02, 0.03, 0.03, 0.04, 0.05];
/// atrium `WallMaterial::wood()`.
const ACOUSTIC_WOOD: [f32; 6] = [0.15, 0.11, 0.10, 0.07, 0.06, 0.07];
/// Dense vegetation: scattering-dominant, so absorption rises steeply with
/// frequency. No atrium equivalent — canopies are an outdoor case its
/// room-surface table never needed.
const ACOUSTIC_FOLIAGE: [f32; 6] = [0.03, 0.06, 0.11, 0.17, 0.27, 0.31];
/// A water surface is very nearly a perfect acoustic mirror.
const ACOUSTIC_WATER: [f32; 6] = [0.01, 0.01, 0.01, 0.01, 0.02, 0.02];
/// The glow block: a hard surface, marginally less reflective than stone.
const ACOUSTIC_GLOW_BLOCK: [f32; 6] = [0.10, 0.06, 0.04, 0.03, 0.03, 0.03];
/// Fresh snow — the strongest broadband absorber in the table by a wide margin,
/// and the reason snowfall audibly deadens a landscape.
const ACOUSTIC_SNOW: [f32; 6] = [0.45, 0.75, 0.90, 0.95, 0.95, 0.95];

/// The material table, indexed by [`material_id`].
///
/// **The M1b emissive rows are not placed by world generation.** Adding
/// lanterns or berry clusters to the generator changes how the world LOOKS in
/// both voxel-rt and voxel-sandbox, which is an aesthetic decision rather than
/// a plumbing one; until it is made, the two rows are reachable through the E2
/// edit API (the same route `crate::debug_pool` uses to carve its swim pool).
/// E5 therefore has emitters to test with and the generated world is unchanged.
///
/// Row 0 (Air) is the miss sentinel and is never sampled on a hit: the DDA only
/// calls the shading path on an occupied voxel. It is kept fully zeroed so a
/// bug that samples it produces black rather than something plausible.
pub fn materials() -> Vec<Material> {
    /// A material a ray cannot travel INSIDE: no absorption, no scattering. Every
    /// row but the liquids, including air (the miss sentinel).
    const NOT_A_MEDIUM: [f32; 3] = [0.0, 0.0, 0.0];

    /// Shorthand for the many rows that differ only in albedo and transmittance.
    const fn foliage(
        name: &'static str,
        albedo: [f32; 3],
        roughness: f32,
        transmittance: f32,
    ) -> Material {
        Material {
            name,
            albedo,
            emission: [0.0, 0.0, 0.0],
            roughness,
            specular: 0.03,
            opacity: 1.0,
            transmittance,
            index_of_refraction: AIR_INDEX_OF_REFRACTION,
            absorption_per_meter: NOT_A_MEDIUM,
            scattering_per_meter: NOT_A_MEDIUM,
            flags: MaterialFlags::FOLIAGE,
            acoustic_alpha: ACOUSTIC_FOLIAGE,
        }
    }
    /// Shorthand for opaque, non-transmitting terrain.
    const fn opaque(
        name: &'static str,
        albedo: [f32; 3],
        roughness: f32,
        specular: f32,
        acoustic_alpha: [f32; 6],
    ) -> Material {
        Material {
            name,
            albedo,
            emission: [0.0, 0.0, 0.0],
            roughness,
            specular,
            opacity: 1.0,
            transmittance: 0.0,
            index_of_refraction: AIR_INDEX_OF_REFRACTION,
            absorption_per_meter: NOT_A_MEDIUM,
            scattering_per_meter: NOT_A_MEDIUM,
            flags: MaterialFlags::NONE,
            acoustic_alpha,
        }
    }

    vec![
        // 0  Air — miss sentinel, never sampled.
        Material {
            name: "air",
            albedo: [0.0, 0.0, 0.0],
            emission: [0.0, 0.0, 0.0],
            roughness: 0.0,
            specular: 0.0,
            opacity: 0.0,
            transmittance: 1.0,
            index_of_refraction: AIR_INDEX_OF_REFRACTION,
            absorption_per_meter: NOT_A_MEDIUM,
            scattering_per_meter: NOT_A_MEDIUM,
            flags: MaterialFlags::TRANSPARENT,
            acoustic_alpha: [0.0; 6],
        },
        opaque(
            "grass",
            [0.41, 0.52, 0.29],
            0.95,
            0.02,
            ACOUSTIC_SOFT_GROUND,
        ),
        foliage("tall_grass", [0.28, 0.45, 0.23], 0.90, 0.35),
        opaque("dirt", [0.44, 0.32, 0.22], 0.97, 0.02, ACOUSTIC_SOFT_GROUND),
        opaque("sand", [0.86, 0.77, 0.55], 0.95, 0.02, ACOUSTIC_SOFT_GROUND),
        opaque(
            "sediment",
            [0.17, 0.16, 0.11],
            0.97,
            0.02,
            ACOUSTIC_SOFT_GROUND,
        ),
        opaque("stone", [0.52, 0.52, 0.55], 0.85, 0.04, ACOUSTIC_STONE),
        // 7  Water — the only row carrying real work: smooth, see-through, and a
        // participating MEDIUM (the absorption/scattering pair below). Note what
        // `albedo` is and is not here: it is the diffuse colour of the water
        // SURFACE, used by the opaque and half-mode fallbacks and as the CAGI
        // bounce tint. It is NOT the colour of the water volume — that is derived
        // from the coefficient pair as `scattering / extinction` ~ (0.009, 0.25,
        // 0.75), which is why this row's albedo no longer decides how the medium
        // looks.
        Material {
            name: "water",
            albedo: [0.19, 0.52, 0.71],
            emission: [0.0, 0.0, 0.0],
            roughness: 0.05,
            specular: 0.02,
            opacity: 0.70,
            transmittance: 0.85,
            index_of_refraction: WATER_INDEX_OF_REFRACTION,
            absorption_per_meter: WATER_ABSORPTION_PER_METER,
            scattering_per_meter: WATER_SCATTERING_PER_METER,
            flags: MaterialFlags::TRANSPARENT.union(MaterialFlags::LIQUID),
            acoustic_alpha: ACOUSTIC_WATER,
        },
        opaque("trunk", [0.45, 0.31, 0.19], 0.90, 0.03, ACOUSTIC_WOOD),
        opaque("trunk_birch", [0.80, 0.78, 0.72], 0.85, 0.03, ACOUSTIC_WOOD),
        foliage("leaves", [0.38, 0.505, 0.235], 0.88, 0.25),
        foliage("leaves_dark", [0.281, 0.374, 0.174], 0.88, 0.20),
        foliage("leaves_birch", [0.51, 0.58, 0.28], 0.88, 0.28),
        foliage("leaves_pine", [0.21, 0.345, 0.24], 0.90, 0.15),
        foliage("flower_pink", [0.93, 0.55, 0.75], 0.90, 0.30),
        foliage("flower_white", [0.96, 0.95, 0.90], 0.90, 0.30),
        foliage("flower_yellow", [0.95, 0.83, 0.35], 0.90, 0.30),
        foliage("flower_blue", [0.45, 0.52, 0.92], 0.90, 0.30),
        foliage("water_weed", [0.15, 0.30, 0.19], 0.85, 0.35),
        foliage("lily_pad", [0.26, 0.50, 0.24], 0.80, 0.20),
        foliage("lily_bloom", [0.95, 0.92, 0.85], 0.85, 0.25),
        foliage("reed", [0.55, 0.56, 0.31], 0.90, 0.30),
        foliage("cattail_head", [0.32, 0.18, 0.08], 0.92, 0.10),
        opaque("snow", [0.92, 0.93, 0.96], 0.75, 0.03, ACOUSTIC_SNOW),
        // 24  GlowBlock (M1b) — the bright emitter: a plain full voxel that
        // emits warm white. SOLID, so it occludes as well as emits, which is
        // the case that proves CAGI injects a source without letting it light
        // through its own body.
        Material {
            name: "glow_block",
            albedo: [0.95, 0.93, 0.88],
            emission: [3.00, 2.80, 2.40],
            roughness: 0.60,
            specular: 0.04,
            opacity: 1.0,
            transmittance: 0.0,
            index_of_refraction: AIR_INDEX_OF_REFRACTION,
            absorption_per_meter: NOT_A_MEDIUM,
            scattering_per_meter: NOT_A_MEDIUM,
            flags: MaterialFlags::EMISSIVE,
            acoustic_alpha: ACOUSTIC_GLOW_BLOCK,
        },
        // 25  GlowBerry (M1b) — the dim cool emitter, and thin cover rather
        // than a block, so it emits WITHOUT occluding. The contrasting case.
        Material {
            name: "glow_berry",
            albedo: [0.55, 0.95, 0.80],
            emission: [0.50, 1.10, 0.80],
            roughness: 0.60,
            specular: 0.04,
            opacity: 1.0,
            transmittance: 0.20,
            index_of_refraction: AIR_INDEX_OF_REFRACTION,
            absorption_per_meter: NOT_A_MEDIUM,
            scattering_per_meter: NOT_A_MEDIUM,
            flags: MaterialFlags::EMISSIVE.union(MaterialFlags::FOLIAGE),
            acoustic_alpha: ACOUSTIC_FOLIAGE,
        },
    ]
}

/// The material table in upload form, for binding 5.
pub fn gpu_materials() -> Vec<GpuMaterial> {
    materials().into_iter().map(Material::to_gpu).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn table_covers_every_material_id() {
        assert_eq!(materials().len(), MATERIAL_COUNT);
    }

    /// Air is the miss sentinel: sampling it must produce black, not a
    /// plausible-looking colour that hides the bug.
    #[test]
    fn air_row_is_zeroed() {
        let air = materials()[0];
        assert_eq!(air.albedo, [0.0, 0.0, 0.0]);
        assert_eq!(air.emission, [0.0, 0.0, 0.0]);
        assert_eq!(air.opacity, 0.0);
    }

    /// The GPU row must stay a whole number of 16-byte std430 rows with no
    /// interior padding, or the WGSL `array<Material>` stride silently disagrees
    /// with the upload. 80 bytes since E6 added the absorption/scattering pair.
    #[test]
    fn gpu_row_layout_matches_wgsl() {
        assert_eq!(std::mem::size_of::<GpuMaterial>(), 80);
        assert_eq!(std::mem::size_of::<GpuMaterial>() % 16, 0);
        assert_eq!(std::mem::align_of::<GpuMaterial>(), 4);
        assert_eq!(MATERIAL_TABLE_BYTES, MATERIAL_COUNT * 80);
    }

    /// Every authored row must round-trip into its GPU form unchanged — the
    /// upload path must never quietly drop a field.
    #[test]
    fn every_row_round_trips_to_gpu() {
        for material in materials() {
            let gpu = material.to_gpu();
            assert_eq!(gpu.albedo, material.albedo, "{}", material.name);
            assert_eq!(gpu.emission, material.emission, "{}", material.name);
            assert_eq!(gpu.roughness, material.roughness, "{}", material.name);
            assert_eq!(gpu.specular, material.specular, "{}", material.name);
            assert_eq!(gpu.opacity, material.opacity, "{}", material.name);
            assert_eq!(
                gpu.transmittance, material.transmittance,
                "{}",
                material.name
            );
            assert_eq!(gpu.flags, material.flags.bits(), "{}", material.name);
            assert_eq!(
                gpu.index_of_refraction, material.index_of_refraction,
                "{}",
                material.name
            );
            assert_eq!(
                gpu.absorption_per_meter, material.absorption_per_meter,
                "{}",
                material.name
            );
            assert_eq!(
                gpu.scattering_per_meter, material.scattering_per_meter,
                "{}",
                material.name
            );
        }
    }

    /// Physical sanity, applied to all 24 rows at once: the ranges every
    /// consumer will assume.
    #[test]
    fn every_row_is_physically_in_range() {
        for material in materials() {
            let in_unit_range = |value: f32| (0.0..=1.0).contains(&value);
            assert!(
                material.albedo.iter().copied().all(in_unit_range),
                "{} albedo out of range",
                material.name
            );
            assert!(
                material.emission.iter().copied().all(|v| v >= 0.0),
                "{} emission must not be negative",
                material.name
            );
            assert!(
                in_unit_range(material.roughness)
                    && in_unit_range(material.specular)
                    && in_unit_range(material.opacity)
                    && in_unit_range(material.transmittance),
                "{} has a scalar outside [0, 1]",
                material.name
            );
            assert!(
                material.acoustic_alpha.iter().copied().all(in_unit_range),
                "{} acoustic_alpha out of range",
                material.name
            );
        }
    }

    /// The albedo column is the one part of this table with live consumers, so
    /// it must be bit-identical to the colour palette M1 replaced — M1 is a
    /// structural change and must not shift a single rendered pixel.
    #[test]
    fn albedo_column_is_unchanged_from_the_colour_palette() {
        let palette: [[f32; 3]; MATERIAL_COUNT] = [
            [0.0, 0.0, 0.0],
            [0.41, 0.52, 0.29],
            [0.28, 0.45, 0.23],
            [0.44, 0.32, 0.22],
            [0.86, 0.77, 0.55],
            [0.17, 0.16, 0.11],
            [0.52, 0.52, 0.55],
            [0.19, 0.52, 0.71],
            [0.45, 0.31, 0.19],
            [0.80, 0.78, 0.72],
            [0.38, 0.505, 0.235],
            [0.281, 0.374, 0.174],
            [0.51, 0.58, 0.28],
            [0.21, 0.345, 0.24],
            [0.93, 0.55, 0.75],
            [0.96, 0.95, 0.90],
            [0.95, 0.83, 0.35],
            [0.45, 0.52, 0.92],
            [0.15, 0.30, 0.19],
            [0.26, 0.50, 0.24],
            [0.95, 0.92, 0.85],
            [0.55, 0.56, 0.31],
            [0.32, 0.18, 0.08],
            [0.92, 0.93, 0.96],
            // M1b additions — no pre-M1 palette entry to preserve.
            [0.95, 0.93, 0.88],
            [0.55, 0.95, 0.80],
        ];
        for (id, material) in materials().iter().enumerate() {
            assert_eq!(
                material.albedo, palette[id],
                "material id {id} changed colour"
            );
        }
    }

    /// Every foliage row is the ray-skew wind target, and every one of them
    /// must transmit some light — a leaf that blocks 100% of the light is what
    /// makes CA GI paint black canopies.
    #[test]
    fn foliage_rows_transmit_light() {
        for material in materials()
            .iter()
            .filter(|m| m.flags.contains(MaterialFlags::FOLIAGE))
        {
            assert!(
                material.transmittance > 0.0,
                "{} is foliage but blocks all light",
                material.name
            );
        }
        // Guard against the flag silently disappearing from the whole table.
        assert!(materials()
            .iter()
            .any(|m| m.flags.contains(MaterialFlags::FOLIAGE)));
    }

    /// Water is the only row that is both transparent and liquid, and its
    /// acoustic behaviour must not drift toward the visual intuition: a water
    /// surface reflects sound almost perfectly.
    #[test]
    fn water_is_transparent_liquid_and_acoustically_reflective() {
        let water = materials()[material_id(Voxel::Water) as usize];
        assert!(water.flags.contains(MaterialFlags::TRANSPARENT));
        assert!(water.flags.contains(MaterialFlags::LIQUID));
        assert!(water.opacity < 1.0);
        assert!(
            water.acoustic_alpha.iter().all(|alpha| *alpha <= 0.05),
            "water must stay acoustically reflective"
        );
    }

    /// [`material_voxel`] must be the exact inverse of [`material_id`], or every
    /// CPU consumer of a material byte silently reads the wrong voxel type.
    #[test]
    fn material_ids_round_trip_through_the_voxel_inverse() {
        for id in 0..MATERIAL_COUNT as u8 {
            assert_eq!(material_id(material_voxel(id)), id, "material id {id}");
        }
        // Ids past the table are the miss sentinel, not a panic.
        assert_eq!(material_voxel(MATERIAL_COUNT as u8), Voxel::Air);
        assert_eq!(material_voxel(u8::MAX), Voxel::Air);
    }

    /// The physics predicate (E2b): water and thin cover must be walk-through,
    /// terrain and wood must block. Spelled out per variant because "you can walk
    /// into water" is a design decision, not an accident of `is_solid`.
    #[test]
    fn blocking_materials_exclude_water_and_thin_cover() {
        for voxel in [
            Voxel::Air,
            Voxel::Water,
            Voxel::TallGrass,
            Voxel::FlowerPink,
            Voxel::FlowerWhite,
            Voxel::FlowerYellow,
            Voxel::FlowerBlue,
            Voxel::WaterWeed,
            Voxel::LilyPad,
            Voxel::LilyBloom,
            Voxel::Reed,
            Voxel::CattailHead,
        ] {
            assert!(
                !material_blocks_movement(material_id(voxel)),
                "{voxel:?} must not block movement"
            );
        }
        for voxel in [
            Voxel::Grass,
            Voxel::Dirt,
            Voxel::Sand,
            Voxel::Sediment,
            Voxel::Stone,
            Voxel::Trunk,
            Voxel::TrunkBirch,
            Voxel::Leaves,
            Voxel::Snow,
        ] {
            assert!(
                material_blocks_movement(material_id(voxel)),
                "{voxel:?} must block movement"
            );
        }
    }

    /// E6 — the authored index-of-refraction column: every row that a ray can
    /// actually enter must bend it, and every row it cannot must be exactly 1.0.
    /// The value is per material rather than a water constant because transparency
    /// is a material class (water 1.333, oil ~1.47, honey ~1.50).
    #[test]
    fn the_index_of_refraction_column_is_authored_per_material() {
        for material in materials() {
            let refracts = material.flags.contains(MaterialFlags::LIQUID);
            if refracts {
                assert!(
                    material.index_of_refraction > 1.0 && material.index_of_refraction < 2.0,
                    "{} is a liquid but its index of refraction is {}",
                    material.name,
                    material.index_of_refraction
                );
            } else {
                assert_eq!(
                    material.index_of_refraction, AIR_INDEX_OF_REFRACTION,
                    "{} does not refract, so its index must be exactly air's",
                    material.name
                );
            }
        }
        let water = materials()[material_id(Voxel::Water) as usize];
        assert_eq!(water.index_of_refraction, WATER_INDEX_OF_REFRACTION);
        // The value must survive the upload — it rides in the row's former pad
        // word, and the row must still be 48 bytes.
        assert_eq!(
            water.to_gpu().index_of_refraction,
            WATER_INDEX_OF_REFRACTION
        );
        assert_eq!(std::mem::size_of::<GpuMaterial>(), 80);
    }

    /// E6 — the edit path's single notion of emptiness ("to the editor, water IS
    /// air"). Spelled out per variant because it is a design decision, not an
    /// accident of a flag: air and every liquid are empty, everything a body can
    /// stand on or walk through is not.
    #[test]
    fn the_edit_predicate_treats_every_liquid_as_air() {
        assert!(material_is_empty_for_edits(material_id(Voxel::Air)));
        assert!(material_is_empty_for_edits(material_id(Voxel::Water)));
        // Thin cover is walk-through but it IS a block: you can dig a reed.
        for voxel in [
            Voxel::Grass,
            Voxel::Stone,
            Voxel::TallGrass,
            Voxel::Reed,
            Voxel::LilyPad,
            Voxel::Leaves,
            Voxel::GlowBlock,
            Voxel::GlowBerry,
        ] {
            assert!(
                !material_is_empty_for_edits(material_id(voxel)),
                "{voxel:?} must be editable, not empty"
            );
        }
        // The rule is the LIQUID flag, not a water special case — so the next
        // transparent fluid inherits it without a branch.
        for (id, material) in materials().iter().enumerate() {
            if material.flags.contains(MaterialFlags::LIQUID) {
                assert!(
                    material_is_empty_for_edits(id as u8),
                    "{} is a liquid but the editor would treat it as a block",
                    material.name
                );
            }
        }
    }

    /// The cheap liquid predicate and the table's LIQUID flag must not drift.
    #[test]
    fn liquid_predicate_agrees_with_the_table() {
        for (id, material) in materials().iter().enumerate() {
            assert_eq!(
                material_is_liquid(id as u8),
                material.flags.contains(MaterialFlags::LIQUID),
                "{} disagrees about being a liquid",
                material.name
            );
        }
        assert!(material_is_liquid(material_id(Voxel::Water)));
    }

    /// The emissive rows must actually emit, and every non-emissive row must
    /// stay dark — an accidental non-zero emission would light the world from
    /// its terrain.
    #[test]
    fn only_the_emissive_rows_emit() {
        for material in materials() {
            let emits = material.emission.iter().any(|channel| *channel > 0.0);
            assert_eq!(
                emits,
                material.flags.contains(MaterialFlags::EMISSIVE),
                "{} emission disagrees with its EMISSIVE flag",
                material.name
            );
        }
        assert_eq!(
            materials()
                .iter()
                .filter(|m| m.flags.contains(MaterialFlags::EMISSIVE))
                .count(),
            2,
            "M1b authored exactly two emitters"
        );
    }

    /// The two emitters are deliberately a contrasting PAIR: one occludes, one
    /// does not. E5's injection rule has to handle both, so if a later edit
    /// makes them alike the test that keeps E5 honest is gone.
    #[test]
    fn the_two_emitters_contrast_in_occlusion() {
        let glow_block = materials()[material_id(Voxel::GlowBlock) as usize];
        let berry = materials()[material_id(Voxel::GlowBerry) as usize];
        assert!(
            glow_block.transmittance == 0.0,
            "the glow block must occlude"
        );
        assert!(berry.transmittance > 0.0, "the berries must not occlude");
        assert!(
            glow_block.emission[0] > berry.emission[0],
            "the glow block is the bright one"
        );
        assert!(
            berry.emission[1] > berry.emission[0],
            "the berries are the cool one"
        );
    }

    /// `material_id` and `material_voxel` must stay exact inverses across the
    /// WHOLE table, M1b's additions included — the id byte is what the world
    /// stores, so a gap here silently turns a voxel into air.
    #[test]
    fn ids_and_voxels_are_exact_inverses() {
        for id in 1..MATERIAL_COUNT as u8 {
            assert_eq!(
                material_id(material_voxel(id)),
                id,
                "material id {id} does not round-trip"
            );
        }
        assert_eq!(material_voxel(0), Voxel::Air);
        assert_eq!(material_voxel(MATERIAL_COUNT as u8), Voxel::Air);
    }

    #[test]
    fn flag_set_operations_compose() {
        let both = MaterialFlags::TRANSPARENT.union(MaterialFlags::LIQUID);
        assert!(both.contains(MaterialFlags::TRANSPARENT));
        assert!(both.contains(MaterialFlags::LIQUID));
        assert!(!both.contains(MaterialFlags::FOLIAGE));
        assert!(!MaterialFlags::NONE.contains(MaterialFlags::FOLIAGE));
        assert_eq!(both.bits(), 0b1100);
    }
}
