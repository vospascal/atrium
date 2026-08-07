//! M1 — the material table: one row per material id, the single place a voxel
//! type's *appearance and physics* are described.
//!
//! Replaces the colour-only `palette()` this module grew out of. The change is
//! an indirection, not a data-size change: the world still stores ONE BYTE per
//! voxel ([`material_id`]), and everything below is a [`MATERIAL_COUNT`]-row
//! lookup table costing [`MATERIAL_TABLE_BYTES`] (6912 bytes) on the GPU.
//! Material richness is therefore effectively free; per-*voxel* state is the
//! expensive axis and this module deliberately does not touch it.
//!
//! Two halves, one source of truth:
//!
//! * [`MATERIALS`] — the authored rows, CPU-side, including the acoustic
//!   coefficients that no GPU pass reads. A [`Material`] is a small shared
//!   header plus a [`MaterialKind`] payload; see below.
//! * [`GpuMaterial`] — the render subset, `#[repr(C)]` and std430-clean at 80
//!   bytes, uploaded to binding 5 (`shaders/world.wgsl`).
//!
//! ## Why the authored row is a union and the uploaded row is flat
//!
//! An authored row and an uploaded row want opposite things, and conflating them
//! is what left this table full of sentinels:
//!
//! * The **uploaded** row wants every field present unconditionally, so the
//!   hottest shading path can read `materials[id].roughness` with no branch. A
//!   "does not apply" value there is correct and necessary.
//! * The **authored** row wants only the fields that mean something, because a
//!   sentinel is indistinguishable from a real value to whoever is authoring it.
//!   `index_of_refraction: 1.0` on stone did not mean "stone refracts like air",
//!   it meant "this column does not apply to stone" — and a `NOT_A_MEDIUM`
//!   absorption triple sat on 25 of 27 rows saying the same thing twice.
//!
//! So [`Material`] carries a [`MaterialKind`] — `Air`, `Solid`,
//! `Cover { transmittance }` or `Medium(..)` — with `emission` as an orthogonal
//! `Option`, because emission composes with any kind (`glow_block` is an
//! emitting `Solid`, `glow_berry` an emitting `Cover`). Every scalar the GPU row
//! needs is then *derived* by an accessor ([`Material::transmittance`],
//! [`Material::opacity`], [`Material::index_of_refraction`], …), and
//! [`MaterialFlags`] is derived rather than hand-authored alongside the data it
//! describes — which is what used to let a row's flags and its values disagree.
//!
//! ## Which fields have consumers today
//!
//! Only `albedo` is read by a shipped pass ([`crate::passes::dda`] shading and
//! [`crate::cagi`]'s bounce tint). The rest are authored now because the cost
//! of this table is *hand-writing every row*, and doing that twice is worse than
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
//! and `GlowBerry` (dim, cool, thin cover) — and E5b adds patterned `Lava`, so the
//! transport has occluding, non-occluding, and area-patterned sources to exercise.
//! None is PLACED by world generation; see the note on
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

use voxel_core::world::{Voxel, DETAIL_CELL_SIZE_METERS, WORLD_VOXEL_SIZE_METERS};

use crate::pattern::{
    apply_stack_color, GpuPatternLayer, PatternBlend, PatternFaces, PatternFrame, PatternGenerator,
    PatternLayer, PatternSample, PatternStack, PatternTarget, MAX_PATTERN_LAYERS, NO_PATTERNS,
};

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

/// The absorption/scattering value of a material a ray cannot travel INSIDE: no
/// absorption, no scattering. What [`Material::absorption_per_meter`] and
/// [`Material::scattering_per_meter`] derive for every non-[`Medium`] kind.
///
/// A sentinel in the uploaded row, which is where a sentinel belongs — it is no
/// longer something an author has to write on 24 rows.
pub const NOT_A_MEDIUM: [f32; 3] = [0.0, 0.0, 0.0];

/// Number of material ids (== number of `Voxel` variants, Air included).
pub const MATERIAL_COUNT: usize = 40;

/// Air's material id, and the DDA's miss sentinel — the shading path is only ever
/// called on an occupied voxel, so row 0 is never sampled on a hit.
///
/// Named rather than written as a bare `0` at the handful of places that must treat
/// it specially: it is the one id that is a sentinel as well as a row, and `== 0`
/// says nothing about which of the two the caller meant.
pub const AIR_MATERIAL_ID: u8 = 0;

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
        Voxel::Lava => 26,
        Voxel::SlateTile => 27,
        Voxel::HdrRed => 28,
        Voxel::HdrGreen => 29,
        Voxel::HdrBlue => 30,
        Voxel::HdrCyan => 31,
        Voxel::HdrMagenta => 32,
        Voxel::HdrYellow => 33,
        Voxel::AlbedoRed => 34,
        Voxel::AlbedoGreen => 35,
        Voxel::AlbedoBlue => 36,
        Voxel::AlbedoCyan => 37,
        Voxel::AlbedoMagenta => 38,
        Voxel::AlbedoYellow => 39,
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
        26 => Voxel::Lava,
        27 => Voxel::SlateTile,
        28 => Voxel::HdrRed,
        29 => Voxel::HdrGreen,
        30 => Voxel::HdrBlue,
        31 => Voxel::HdrCyan,
        32 => Voxel::HdrMagenta,
        33 => Voxel::HdrYellow,
        34 => Voxel::AlbedoRed,
        35 => Voxel::AlbedoGreen,
        36 => Voxel::AlbedoBlue,
        37 => Voxel::AlbedoCyan,
        38 => Voxel::AlbedoMagenta,
        39 => Voxel::AlbedoYellow,
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

/// Whether a material id is a liquid.
///
/// Reads the table's own [`MediumPhase::Liquid`] rather than testing for
/// `Voxel::Water`, which is what it used to do: a hardcoded water arm meant the
/// next transparent fluid (oil, honey — the dossier's own targets) would have
/// been a liquid to the shader, which reads [`MaterialFlags::LIQUID`], and not to
/// the character controller, which reads this. They agreed by test, not by
/// mechanism. Now there is one mechanism.
///
/// Cheap despite being table-driven, because [`MATERIALS`] is a `const` array
/// rather than an allocated `Vec` — the character samples this a few times per
/// frame and so does E6's water-medium march. Ids past the table are not liquid,
/// matching [`material_voxel`]'s air sentinel.
pub fn material_is_liquid(material: u8) -> bool {
    MATERIALS
        .get(material as usize)
        .is_some_and(Material::is_liquid)
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
    /// S1 — this row's top and bottom faces differ from its sides, so the shading
    /// path must pick per-face values instead of the row's base ones.
    ///
    /// A flag rather than a sentinel comparison in the shader: "are the top values
    /// equal to the base values" is three float compares per hit to answer a
    /// question the CPU already knows the answer to.
    pub const FACE_ROLES: MaterialFlags = MaterialFlags(1 << 4);
    /// S2 — this row has at least one pattern layer, so the shading path must run
    /// the layer stack. Same argument as [`Self::FACE_ROLES`]: the flag is what lets
    /// the 24 rows with no patterns skip the whole mechanism with one bit test
    /// instead of discovering it is pointless four slots later.
    pub const PATTERNS: MaterialFlags = MaterialFlags(1 << 5);

    /// Both flag sets combined.
    pub const fn union(self, other: MaterialFlags) -> MaterialFlags {
        MaterialFlags(self.0 | other.0)
    }

    /// S2 — this word with the active pattern-layer count written into it.
    ///
    /// The count rides in the flag word's upper bits rather than taking a field of
    /// its own, because a `u32` count would cost a whole 16-byte std430 row (three
    /// quarters of it padding) to carry a number that fits in three bits. Bits 8-10,
    /// leaving 5-7 clear so the next few flags do not have to move it.
    pub const fn with_pattern_count(self, count: u32) -> MaterialFlags {
        MaterialFlags(
            (self.0 & !(PATTERN_COUNT_MASK << PATTERN_COUNT_SHIFT))
                | ((count & PATTERN_COUNT_MASK) << PATTERN_COUNT_SHIFT),
        )
    }

    /// How many pattern layers this row's stack holds. The shader's loop bound.
    pub const fn pattern_count(self) -> u32 {
        (self.0 >> PATTERN_COUNT_SHIFT) & PATTERN_COUNT_MASK
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

/// Where the S2 pattern-layer count sits in the flag word. Mirrored in
/// `shaders/world.wgsl`.
const PATTERN_COUNT_SHIFT: u32 = 8;
const PATTERN_COUNT_MASK: u32 = 0b111;

/// The phase of a participating [`Medium`] — what the volume *is*, mechanically,
/// as opposed to how it looks.
///
/// Split out rather than carried as an `is_liquid` bool because the phase is what
/// the non-render consumers actually branch on: only a liquid is empty to the
/// editor and wadeable by the character. It also gives the dossier's whole
/// transparency target set a home — *"water, oil, clouds and honey"* — without a
/// per-material special case: clouds are a `Gas`, glass and ice are a `Solid`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MediumPhase {
    /// Flows and can be swum in: water, oil, honey. Empty to the editor ("to the
    /// editor, water IS air") and wadeable rather than blocking.
    Liquid,
    /// A volume with no surface to stand on: cloud, fog, smoke. Scattering-
    /// dominated, which is why a painted albedo cannot express one at all.
    Gas,
    /// A transparent SOLID: glass, ice. A ray travels inside it, but a body does
    /// not — it blocks movement and it is an ordinary block to the editor.
    Solid,
}

/// A participating medium: the payload of [`MaterialKind::Medium`], and the only
/// kind whose coefficients a ray reads while travelling *inside* a voxel.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Medium {
    /// What this volume is, mechanically. See [`MediumPhase`].
    pub phase: MediumPhase,
    /// Index of refraction — how hard this medium bends a ray that enters it,
    /// and (through `((n - 1) / (n + 1))^2`) how much it mirrors at normal
    /// incidence.
    ///
    /// Lives on the medium rather than being a water constant on purpose (E6,
    /// 2026-07-31): the dossier records xima's own transparency target as *"water,
    /// oil, clouds and honey"* — transparency is a per-material CLASS, and every
    /// member of it has its own index (water 1.333, oil ~1.47, honey ~1.50).
    /// Retrofitting a constant out of the Snell code later is far more work than
    /// authoring the column now, and it costs no bytes: the value took the GPU
    /// row's existing pad slot.
    pub index_of_refraction: f32,
    /// **Absorption** coefficient per metre, per channel: light this medium
    /// removes from a ray passing through it.
    ///
    /// Authored as a coefficient PAIR with [`Self::scattering_per_meter`] rather
    /// than as a volume colour, because a medium's colour is not a property you
    /// pick — it *emerges* from which wavelengths are absorbed and which are
    /// scattered (Pascal, 2026-07-31: *"water shouldn't have a colour really ..
    /// water blocks light coming in"*). The E6 model therefore derives every
    /// colour it shows from these two triples: extinction is
    /// `absorption + scattering`, and the medium's own apparent colour is the
    /// single-scattering albedo `scattering / extinction`. Nothing is painted.
    ///
    /// The pair is also what makes the whole class expressible with one model:
    /// water (moderate absorption, weak blue-favouring scattering), oil and honey
    /// (strong absorption, low scattering), clouds (near-zero absorption,
    /// scattering-dominated).
    pub absorption_per_meter: [f32; 3],
    /// **Scattering** coefficient per metre, per channel: light this medium
    /// redirects rather than destroys, and therefore the light a ray picks UP along
    /// its path.
    pub scattering_per_meter: [f32; 3],
    /// 1.0 = opaque. Below 1.0 the traversal must continue through the voxel.
    pub opacity: f32,
    /// Fraction of light passing THROUGH the voxel, for transport rather than for
    /// viewing.
    pub transmittance: f32,
}

/// What KIND of material a row is — the discriminant that decides which optical
/// payload it carries, and therefore which scalars and [`MaterialFlags`] it
/// derives.
///
/// See the module docs for why the authored row is a union while the uploaded row
/// stays flat. In short: a sentinel is correct in a wire format and misleading in
/// an authoring format, and this table had `index_of_refraction: 1.0` meaning
/// "not applicable" on 25 of 27 rows.
///
/// Deliberately open to extension rather than final — the reference engines put
/// simulation state (density, conductivity) and a behaviour rule table on the
/// material, which is where backlog B6/B10 eventually goes. Adding a variant
/// breaks every `match` site, which is the desired failure mode.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum MaterialKind {
    /// Id 0 only: the miss sentinel. The DDA never calls the shading path on an
    /// unoccupied voxel, so this row is never sampled on a hit; it is kept fully
    /// zeroed so a bug that samples it produces black rather than something
    /// plausible. Transmits everything, occludes nothing.
    Air,
    /// Ordinary opaque terrain. Nothing passes through and nothing travels
    /// inside: no transmittance, no refraction, no medium coefficients. The
    /// common case, and the reason a flat row was mostly sentinels.
    Solid,
    /// Thin vegetation: a surface that occludes for *viewing* but lets some light
    /// through for *transport*, which is what stops CAGI painting a canopy as a
    /// wall. Also the ray-direction-skew wind target.
    Cover {
        /// Fraction of light passing THROUGH the voxel. Must be above zero — a
        /// leaf that blocks 100% of the light is what makes GI paint black
        /// canopies.
        transmittance: f32,
    },
    /// A participating medium a ray travels INSIDE: water today, oil, honey,
    /// clouds and glass by the same model.
    Medium(Medium),
}

/// S1 — what one face role overrides. The row's own `albedo`/`roughness` are the
/// SIDES; a role that wants the base value simply repeats it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceOverride {
    /// Diffuse colour for this role, sRGB-encoded like [`Material::albedo`].
    pub albedo: [f32; 3],
    pub roughness: f32,
}

/// S1 — per-face-role overrides: a voxel whose top and bottom differ from its
/// sides.
///
/// The cheapest real gain in the whole arc, and the thing that makes a grass block
/// read as a grass block rather than as a green cube.
///
/// **Three roles, not six.** A voxel almost never wants its four sides to differ
/// from each other — the cases that motivate this (grass, snow-capped stone, a
/// scorched top) are all "the sky-facing face is different". Six would double the
/// uploaded row to buy a case nothing has asked for; `PerFace` stays a named
/// follow-on.
///
/// **All three roles are explicit overrides, including the sides.** The row's own
/// `albedo`/`roughness` stay what they were before S1 and are what every face reads
/// when the lever is off.
///
/// Tempting to make the sides implicit — "the base IS the side" — and it is wrong.
/// Grass is the case that proves it: a grass block's *sides* are earth and only its
/// top is green, so an implicit-side design forces the base to become dirt, and
/// then grass renders BROWN with the feature switched off. The off state has to be
/// the pre-S1 look, so the pre-S1 value has to stay where it was and the roles have
/// to be a separate set.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FaceRoles {
    /// The `+Y` face.
    pub top: FaceOverride,
    /// The four side faces.
    pub side: FaceOverride,
    /// The `-Y` face. Usually the darkest: it is the one face that never sees the
    /// sky.
    pub bottom: FaceOverride,
}

/// One authored material row: a small shared header plus a [`MaterialKind`]
/// payload.
///
/// `albedo` values are sRGB-encoded exactly as they were in the colour palette
/// this table replaced (lifted originally from the mesh renderer's `voxel_color`
/// match, one representative value per type — positional variation such as
/// dryness/season/depth blending is still not modelled here).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Material {
    /// Human-readable name; diagnostics and the material panel.
    pub name: &'static str,
    /// Diffuse colour, sRGB-encoded. Also the GI bounce tint. Shared by every
    /// kind, because everything has a surface colour — including a medium, whose
    /// `albedo` is its *surface* reflectance and emphatically not its volume
    /// colour (that is derived; see [`Material::single_scattering_albedo`]).
    pub albedo: [f32; 3],
    /// 0.0 = mirror, 1.0 = fully diffuse.
    pub roughness: f32,
    /// Specular reflectance at normal incidence (F0).
    pub specular: f32,
    /// Which kind of material this is, and its kind-specific payload.
    pub kind: MaterialKind,
    /// Emitted radiance, linear and unbounded above 1.0 — a source is allowed to
    /// be brighter than any surface can reflect. `None` on every row that does
    /// not emit, rather than a zero triple that reads as an authored value.
    ///
    /// Orthogonal to [`Self::kind`] rather than a variant of it, because emission
    /// genuinely composes with any kind: `glow_block` is an emitting `Solid` and
    /// `glow_berry` an emitting `Cover`.
    pub emission: Option<[f32; 3]>,
    /// The radiance this row injects into the LIGHT VOLUME, when it differs
    /// from what its own surface shows. `None` — the common case — means "cast
    /// exactly what you display" ([`Self::emission`]). Authoring both splits an
    /// emitter's look from its throw: a bright little crystal that casts a
    /// modest glow, or an unassuming ember block that floods a cavern
    /// (Pascal, 2026-08-07). Only read by the CAGI injection path
    /// ([`Self::mean_injected_radiance`]); the shading pass never sees it.
    pub light: Option<[f32; 3]>,
    /// S1 — per-face overrides, or `None` for a row whose faces are all alike.
    ///
    /// `None` rather than a `FaceRoles` holding three copies of the base values, so
    /// that "this row has no face roles" is a fact the CPU knows and can hand the
    /// shader as a flag, instead of something the shader has to discover by
    /// comparing floats on every hit.
    pub face_roles: Option<FaceRoles>,
    /// S2 — this row's pattern layers, or [`NO_PATTERNS`] for a flat row.
    ///
    /// Not an `Option<PatternStack>`, unlike [`Self::face_roles`]: a stack already
    /// has an unambiguous empty state (no active slots) that costs nothing to
    /// represent, so wrapping it would add a second way to spell "no patterns" and a
    /// question about whether `Some(NO_PATTERNS)` means anything.
    pub patterns: PatternStack,
    /// Acoustic absorption at [125, 250, 500, 1k, 2k, 4k] Hz; 0.0 fully
    /// reflective, 1.0 fully absorptive. See the module docs on the shared
    /// vocabulary with atrium's `WallMaterial::alpha`.
    pub acoustic_alpha: [f32; 6],
}

impl Material {
    /// This row's medium payload, or `None` for everything a ray cannot enter.
    pub const fn medium(&self) -> Option<&Medium> {
        match &self.kind {
            MaterialKind::Medium(medium) => Some(medium),
            _ => None,
        }
    }

    /// Fraction of light passing THROUGH this voxel, for transport rather than
    /// for viewing: what stops CAGI treating a leaf canopy as a wall.
    ///
    /// Derived: air transmits everything, a solid nothing, and cover and media
    /// carry their own authored value.
    pub const fn transmittance(&self) -> f32 {
        match &self.kind {
            MaterialKind::Air => 1.0,
            MaterialKind::Solid => 0.0,
            MaterialKind::Cover { transmittance } => *transmittance,
            MaterialKind::Medium(medium) => medium.transmittance,
        }
    }

    /// 1.0 = opaque; below 1.0 the traversal must continue through the voxel.
    /// Air is fully transparent, every surface is opaque, a medium authors it.
    pub const fn opacity(&self) -> f32 {
        match &self.kind {
            MaterialKind::Air => 0.0,
            MaterialKind::Solid | MaterialKind::Cover { .. } => 1.0,
            MaterialKind::Medium(medium) => medium.opacity,
        }
    }

    /// Index of refraction. Everything a ray cannot enter reads exactly
    /// [`AIR_INDEX_OF_REFRACTION`] — "refracts like the air around it" is the
    /// honest uploaded value, and the E6 water model reads this only for
    /// materials it actually enters.
    pub const fn index_of_refraction(&self) -> f32 {
        match &self.kind {
            MaterialKind::Medium(medium) => medium.index_of_refraction,
            _ => AIR_INDEX_OF_REFRACTION,
        }
    }

    /// Per-channel absorption per metre; all-zero for everything a ray cannot
    /// travel inside.
    pub const fn absorption_per_meter(&self) -> [f32; 3] {
        match &self.kind {
            MaterialKind::Medium(medium) => medium.absorption_per_meter,
            _ => NOT_A_MEDIUM,
        }
    }

    /// Per-channel scattering per metre; all-zero for everything a ray cannot
    /// travel inside.
    pub const fn scattering_per_meter(&self) -> [f32; 3] {
        match &self.kind {
            MaterialKind::Medium(medium) => medium.scattering_per_meter,
            _ => NOT_A_MEDIUM,
        }
    }

    /// Emitted radiance, or black for the rows that do not emit — the uploaded
    /// form of [`Self::emission`].
    pub const fn emitted_radiance(&self) -> [f32; 3] {
        match self.emission {
            Some(emission) => emission,
            None => [0.0, 0.0, 0.0],
        }
    }

    /// Whether this row is a liquid: flows, is empty to the editor, and is waded
    /// or swum rather than stood on.
    pub const fn is_liquid(&self) -> bool {
        matches!(
            &self.kind,
            MaterialKind::Medium(Medium {
                phase: MediumPhase::Liquid,
                ..
            })
        )
    }

    /// Whether this row is thin cover — the ray-skew wind target.
    pub const fn is_foliage(&self) -> bool {
        matches!(&self.kind, MaterialKind::Cover { .. })
    }

    /// Whether this row emits light: the CAGI injection candidate test (E5).
    /// Whether this row emits at all — including **only** through a pattern layer.
    ///
    /// S2 widened this. A stone row with red emissive specks authored as an
    /// `add`-blended emission layer has `emission: None` and still glows, so the old
    /// `self.emission.is_some()` would have left it out of CAGI's emitter set and the
    /// specks would light nothing (Pascal, 2026-07-31: *"i do think it makes sense to
    /// be able to let them emit light"*).
    ///
    /// Consumer: [`crate::cagi`]'s E5b material table. The row's mean is later
    /// weighted by exposed area into a per-cell emission value.
    pub const fn is_emissive(&self) -> bool {
        self.emission.is_some() || self.light.is_some() || self.has_emission_layers()
    }

    /// Whether any pattern layer targets emission with a non-zero amount.
    pub const fn has_emission_layers(&self) -> bool {
        let mut slot = 0;
        while slot < MAX_PATTERN_LAYERS {
            if let Some(layer) = self.patterns.layers[slot] {
                if matches!(layer.target, PatternTarget::Emission) && layer.amount > 0.0 {
                    return true;
                }
            }
            slot += 1;
        }
        false
    }

    /// The COARSEST period among this row's emission layers, which is what decides
    /// how wide a region [`Self::mean_emitted_radiance`] has to average over.
    fn coarsest_emission_period(&self) -> f32 {
        let mut coarsest = 0.0_f32;
        for slot in self.patterns.layers.iter().flatten() {
            if matches!(slot.target, PatternTarget::Emission) && slot.amount > 0.0 {
                coarsest = coarsest.max(slot.period_meters);
            }
        }
        coarsest
    }

    /// The **mean** emitted radiance over this row's surface — what the GI volume
    /// injects, as opposed to what a pixel shows.
    ///
    /// ## Why a mean is the right answer rather than a compromise
    ///
    /// CAGI is a cellular automaton on half-metre cells holding a per-cell mean.
    /// It cannot represent per-texel structure and does not need to: the light arriving
    /// somewhere else from a speckled emissive surface is the
    /// surface's **average** emission times its area. Detail matters to the eye looking
    /// at the surface; only the mean escapes it. So the two tiers are not an
    /// approximation of one model, they are the near field and the far field, and each
    /// gets the right quantity.
    ///
    /// ## How the mean is obtained
    ///
    /// By evaluating [`crate::pattern`]'s CPU reference over a grid on each of the six
    /// faces and averaging, weighted by face count (four sides to one top and one
    /// bottom) so a top-masked layer contributes its real share. Numerically rather
    /// than analytically, deliberately: an analytic mean would need a closed form per
    /// generator, a fifth generator would silently get the wrong one, and this is the
    /// evaluator's second real use after the WGSL cross-check.
    ///
    /// Costs nothing on the rows that do not use the feature — it returns
    /// [`Self::emitted_radiance`] immediately unless an emission layer exists, and
    /// there are at most a handful of those.
    /// The radiance this row injects into the light volume: the look/light
    /// split's LIGHT half. With no authored [`Self::light`] this is exactly
    /// [`Self::mean_emitted_radiance`] — cast what you display. With one, the
    /// authored light replaces the base emission and the pattern-layer stack
    /// composes on top of it, so a speckled ember block's throw still averages
    /// its speckles.
    ///
    /// Event-gated emitters (S3b) are deliberately outside the split for now:
    /// voxel-rt's CAGI attribute build re-means their authored per-state event
    /// emissions directly and ignores `light`.
    pub fn mean_injected_radiance(&self) -> [f32; 3] {
        match self.light {
            Some(light) => {
                let mut row = *self;
                row.emission = Some(light);
                row.light = None;
                row.mean_emitted_radiance()
            }
            None => self.mean_emitted_radiance(),
        }
    }

    pub fn mean_emitted_radiance(&self) -> [f32; 3] {
        let base = self.emitted_radiance();
        if !self.has_emission_layers() {
            return base;
        }
        // The sampling SPAN, in voxels, and the reason this is not simply one voxel.
        //
        // One voxel is a valid mean only while every period is SMALLER than a voxel,
        // because only then does one voxel span many pattern cells. Past that, all the
        // samples land inside a single cell and the "mean" silently becomes a point
        // sample of it. Measured on a `Speckle { density: 0.30 }` emission layer
        // (Pascal, 2026-07-31, the magenta wall): 0.077 at a 0.02 m period, and
        // **exactly 0.0** at 0.25 m and above, because the one sampled voxel fell in a
        // gap between specks. `Flat` and `Noise` were no better — they returned one
        // cell's value (7.4 and 9.1 at 2.8 m) and called it an average, which moves if
        // the hardcoded voxel below moves. So the span follows the coarsest period.
        const PERIODS_SPANNED: f32 = 8.0;
        let span_voxels = (self.coarsest_emission_period() * PERIODS_SPANNED
            / WORLD_VOXEL_SIZE_METERS)
            .ceil()
            // One snapped block face has only eight distinct positions. Sample
            // several blocks so sparse speckles cannot report a false zero.
            .max(PERIODS_SPANNED);
        // 24 samples per axis, offset by half a step so they do not land systematically
        // on texel centres or corners. Held deliberately low rather than scaled with
        // the span: this runs for every row on every EDIT (`MaterialAttributes` rides
        // in a `VoxelEdit`), and the bug above was BIAS, not variance — spreading the
        // same handful of samples over the real period makes the estimate unbiased,
        // where more samples in the wrong place would not.
        const SAMPLES: usize = 24;
        let mut total = [0.0_f64; 3];
        let mut weight_total = 0.0_f64;
        // (axis, sign, how many faces this stands for) — the four sides are one case.
        for (axis, axis_sign, faces) in [(1_u32, -1.0_f32, 1.0_f64), (1, 1.0, 1.0), (0, 1.0, 4.0)] {
            let mut face_total = [0.0_f64; 3];
            for row in 0..SAMPLES {
                for column in 0..SAMPLES {
                    let along = (row as f32 + 0.5) / SAMPLES as f32 * span_voxels;
                    let across = (column as f32 + 0.5) / SAMPLES as f32 * span_voxels;
                    // Spread over the face PLANE of this axis, so both in-plane
                    // directions vary independently — `axis == 1` is a top/bottom face
                    // (the x/z plane), otherwise it is a side (the y/z plane).
                    let offset = if axis == 1 {
                        [along, 0.0, across]
                    } else {
                        [0.0, along, across]
                    };
                    // A representative origin, off the world origin so the world frame
                    // is sampled somewhere typical rather than at its one cell whose
                    // hash is zero. The integer voxel follows the offset, so the
                    // per-voxel frame's hash varies across the span too — sampling one
                    // voxel could never average that at all.
                    let world_origin = [37.0_f32, 24.0, 50.0];
                    let world = [
                        (world_origin[0] + offset[0]) * WORLD_VOXEL_SIZE_METERS,
                        (world_origin[1] + offset[1]) * WORLD_VOXEL_SIZE_METERS,
                        (world_origin[2] + offset[2]) * WORLD_VOXEL_SIZE_METERS,
                    ];
                    let voxel = [
                        (world[0] / DETAIL_CELL_SIZE_METERS).floor() as i32,
                        (world[1] / DETAIL_CELL_SIZE_METERS).floor() as i32,
                        (world[2] / DETAIL_CELL_SIZE_METERS).floor() as i32,
                    ];
                    let sample = PatternSample {
                        world_meters: world,
                        voxel,
                        axis,
                        axis_sign,
                        distance_meters: 0.0,
                        exposure: crate::pattern::EXPOSURE_ALL,
                    };
                    let emitted = apply_stack_color(
                        &self.patterns,
                        base,
                        PatternTarget::Emission,
                        &sample,
                        // No fade: the mean is a property of the material, not of where
                        // the camera happens to be. A layer that fades out still lights
                        // the room it is in.
                        0.0,
                        0.0,
                        MAX_PATTERN_LAYERS,
                    );
                    for channel in 0..3 {
                        face_total[channel] += emitted[channel] as f64;
                    }
                }
            }
            let samples = (SAMPLES * SAMPLES) as f64;
            for channel in 0..3 {
                total[channel] += faces * face_total[channel] / samples;
            }
            weight_total += faces;
        }
        [
            (total[0] / weight_total) as f32,
            (total[1] / weight_total) as f32,
            (total[2] / weight_total) as f32,
        ]
    }

    /// This row's boolean properties, **derived** from its kind and emission
    /// rather than authored beside them.
    ///
    /// Authoring flags next to the data they describe is what let a row's flags
    /// and its values disagree; there is now no way to write a `LIQUID` row with
    /// no medium, or a foliage row that blocks all light.
    pub const fn flags(&self) -> MaterialFlags {
        let structural = match &self.kind {
            // Traversal continues through air and through any medium.
            MaterialKind::Air => MaterialFlags::TRANSPARENT,
            MaterialKind::Solid => MaterialFlags::NONE,
            MaterialKind::Cover { .. } => MaterialFlags::FOLIAGE,
            MaterialKind::Medium(medium) => match medium.phase {
                MediumPhase::Liquid => MaterialFlags::TRANSPARENT.union(MaterialFlags::LIQUID),
                MediumPhase::Gas | MediumPhase::Solid => MaterialFlags::TRANSPARENT,
            },
        };
        let with_emission = if self.emission.is_some() {
            structural.union(MaterialFlags::EMISSIVE)
        } else {
            structural
        };
        let with_roles = if self.face_roles.is_some() {
            with_emission.union(MaterialFlags::FACE_ROLES)
        } else {
            with_emission
        };
        let pattern_count = self.patterns.active_count() as u32;
        if pattern_count == 0 {
            with_roles
        } else {
            with_roles
                .union(MaterialFlags::PATTERNS)
                .with_pattern_count(pattern_count)
        }
    }

    /// This row's face roles, or the base values repeated — the uploaded form.
    ///
    /// A row without roles uploads its base values in all three slots, so the
    /// shader's per-face read is the identity and the FACE_ROLES flag is what tells
    /// it not to bother.
    pub const fn face_roles_or_base(&self) -> FaceRoles {
        match self.face_roles {
            Some(roles) => roles,
            None => {
                let base = FaceOverride {
                    albedo: self.albedo,
                    roughness: self.roughness,
                };
                FaceRoles {
                    top: base,
                    side: base,
                    bottom: base,
                }
            }
        }
    }

    /// This row's GPU subset — everything except `name` and `acoustic_alpha`,
    /// with every union payload expanded into an unconditional field.
    ///
    /// The sentinels this fills in (`AIR_INDEX_OF_REFRACTION` on a solid, a zero
    /// absorption triple on anything a ray cannot enter) are correct *here*: the
    /// shading path reads every field without branching, and that is worth more
    /// than the bytes a packed union would save on a 2 KB table.
    pub fn to_gpu(&self) -> GpuMaterial {
        let roles = self.face_roles_or_base();
        GpuMaterial {
            albedo: self.albedo,
            transmittance: self.transmittance(),
            emission: self.emitted_radiance(),
            roughness: self.roughness,
            opacity: self.opacity(),
            specular: self.specular,
            flags: self.flags().bits(),
            index_of_refraction: self.index_of_refraction(),
            absorption_per_meter: self.absorption_per_meter(),
            _pad_absorption: 0.0,
            scattering_per_meter: self.scattering_per_meter(),
            _pad_scattering: 0.0,
            top_albedo: roles.top.albedo,
            top_roughness: roles.top.roughness,
            side_albedo: roles.side.albedo,
            side_roughness: roles.side.roughness,
            bottom_albedo: roles.bottom.albedo,
            bottom_roughness: roles.bottom.roughness,
            patterns: self.patterns.to_gpu(),
        }
    }

    /// Extinction per metre, per channel: `absorption + scattering` — the total
    /// rate at which this medium removes light from a ray, and the exponent of the
    /// Beer-Lambert term.
    pub fn extinction_per_meter(&self) -> [f32; 3] {
        let absorption = self.absorption_per_meter();
        let scattering = self.scattering_per_meter();
        [
            absorption[0] + scattering[0],
            absorption[1] + scattering[1],
            absorption[2] + scattering[2],
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
    pub fn single_scattering_albedo(&self) -> [f32; 3] {
        let extinction = self.extinction_per_meter();
        let scattering = self.scattering_per_meter();
        let mut albedo = [0.0_f32; 3];
        for channel in 0..3 {
            if extinction[channel] > 0.0 {
                albedo[channel] = scattering[channel] / extinction[channel];
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
    /// S1 — the `+Y` face's values. Equal to `albedo`/`roughness` on a row with no
    /// face roles, so the shader's per-face read is the identity there.
    pub top_albedo: [f32; 3],
    pub top_roughness: f32,
    /// S1 — the four side faces' values.
    pub side_albedo: [f32; 3],
    pub side_roughness: f32,
    /// S1 — the `-Y` face's values.
    pub bottom_albedo: [f32; 3],
    pub bottom_roughness: f32,
    /// S2 — the pattern stack, always [`MAX_PATTERN_LAYERS`] slots. Slots past the
    /// row's count (carried in [`MaterialFlags::pattern_count`]) are
    /// [`GpuPatternLayer::INACTIVE`], which is the identity even if the count were
    /// ever wrong.
    pub patterns: [GpuPatternLayer; MAX_PATTERN_LAYERS],
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
/// the renderer is an aesthetic decision rather than a plumbing one. Until it is
/// made, the rows remain available to one-metre world edits and Studio previews.
///
/// Row 0 (Air) is the miss sentinel and is never sampled on a hit: the DDA only
/// calls the shading path on an occupied voxel. It is kept fully zeroed so a
/// bug that samples it produces black rather than something plausible.
///
/// A `const` array rather than a function returning a `Vec`: the CPU predicates
/// ([`material_is_liquid`] and friends) are sampled a few times per frame by the
/// character controller and by E6's medium march, and they now read the table
/// instead of hardcoding a voxel arm — which would have been an allocation per
/// call. [`crate::material_table::MaterialTable`] clones this as its `Default`
/// for live editing; this stays the compiled truth.
pub const MATERIALS: [Material; MATERIAL_COUNT] = {
    /// Shorthand for the many thin-cover rows that differ only in albedo,
    /// roughness and transmittance.
    const fn foliage(
        name: &'static str,
        albedo: [f32; 3],
        roughness: f32,
        transmittance: f32,
    ) -> Material {
        Material {
            name,
            albedo,
            roughness,
            specular: 0.03,
            kind: MaterialKind::Cover { transmittance },
            emission: None,
            light: None,
            face_roles: None,
            patterns: NO_PATTERNS,
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
            roughness,
            specular,
            kind: MaterialKind::Solid,
            emission: None,
            light: None,
            face_roles: None,
            patterns: NO_PATTERNS,
            acoustic_alpha,
        }
    }

    [
        // 0  Air — miss sentinel, never sampled.
        Material {
            name: "air",
            albedo: [0.0, 0.0, 0.0],
            roughness: 0.0,
            specular: 0.0,
            kind: MaterialKind::Air,
            emission: None,
            light: None,
            face_roles: None,
            patterns: NO_PATTERNS,
            acoustic_alpha: [0.0; 6],
        },
        // 1  Grass — S1's demonstration case, and the reason face roles exist.
        //
        // A grass block is earth with a green skin on the sky-facing face. Before
        // S1 the whole cube was the green, so a cut bank read as green rock; the
        // SIDES are now the same dirt the row below it is, and only the top is
        // green. The bottom is dirt too, marginally darker: it is the one face that
        // never sees the sky.
        //
        // The shipped tiers read these roles; Potato patches MATERIAL_FACE_ROLES off
        // as its deliberate flat-material fallback.
        Material {
            name: "grass",
            // The pre-S1 green, unchanged: this is what every face reads while the
            // lever is off, so switching the feature off is switching S1 off.
            albedo: [0.41, 0.52, 0.29],
            roughness: 0.95,
            specular: 0.02,
            kind: MaterialKind::Solid,
            emission: None,
            light: None,
            face_roles: Some(FaceRoles {
                // Green only where the sky can reach.
                top: FaceOverride {
                    albedo: [0.41, 0.52, 0.29],
                    roughness: 0.95,
                },
                // The sides are the same earth as the `dirt` row below them, which
                // is what stops a cut bank reading as green rock.
                side: FaceOverride {
                    albedo: [0.44, 0.32, 0.22],
                    roughness: 0.97,
                },
                bottom: FaceOverride {
                    albedo: [0.35, 0.26, 0.18],
                    roughness: 0.97,
                },
            }),
            patterns: NO_PATTERNS,
            acoustic_alpha: ACOUSTIC_SOFT_GROUND,
        },
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
            roughness: 0.05,
            specular: 0.02,
            kind: MaterialKind::Medium(Medium {
                phase: MediumPhase::Liquid,
                index_of_refraction: WATER_INDEX_OF_REFRACTION,
                absorption_per_meter: WATER_ABSORPTION_PER_METER,
                scattering_per_meter: WATER_SCATTERING_PER_METER,
                opacity: 0.70,
                transmittance: 0.85,
            }),
            emission: None,
            light: None,
            face_roles: None,
            patterns: NO_PATTERNS,
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
            roughness: 0.60,
            specular: 0.04,
            kind: MaterialKind::Solid,
            emission: Some([3.00, 2.80, 2.40]),
            // The look/light split's first customer (2026-08-07): the face
            // shows 3.0 (already past paper white) while the volume receives
            // 8x that, so a corridor wall 1 m away reads like a sunlit
            // surface instead of near-black. UNMEASURED — tuned at the arc's
            // in-app gate; the stride-2 ceiling is 64.
            light: Some([24.0, 22.4, 19.2]),
            face_roles: None,
            patterns: NO_PATTERNS,
            acoustic_alpha: ACOUSTIC_GLOW_BLOCK,
        },
        // 25  GlowBerry (M1b) — the dim cool emitter, and thin cover rather
        // than a block, so it emits WITHOUT occluding. The contrasting case.
        Material {
            name: "glow_berry",
            albedo: [0.55, 0.95, 0.80],
            roughness: 0.60,
            specular: 0.04,
            kind: MaterialKind::Cover {
                transmittance: 0.20,
            },
            emission: Some([0.50, 1.10, 0.80]),
            light: None,
            face_roles: None,
            patterns: NO_PATTERNS,
            acoustic_alpha: ACOUSTIC_FOLIAGE,
        },
        // 26  Lava — patterned orange/yellow emission. The surface pattern is
        // deliberately authored as material data, so it can later be refined by
        // S5 without changing the CAGI emission contract.
        Material {
            name: "lava",
            albedo: [0.42, 0.09, 0.015],
            roughness: 0.42,
            specular: 0.03,
            kind: MaterialKind::Solid,
            emission: Some([1.20, 0.16, 0.015]),
            // Molten rock throws far more than its crusted surface shows: 8x
            // the base, same hue; the pattern layers still compose on top.
            // UNMEASURED — tuned at the arc's in-app gate.
            light: Some([9.60, 1.28, 0.12]),
            face_roles: None,
            patterns: PatternStack::of(&[PatternLayer {
                generator: PatternGenerator::Noise { octaves: 2 },
                frame: PatternFrame::Face,
                period_meters: crate::pattern::DEFAULT_PERIOD_METERS,
                target: PatternTarget::Emission,
                blend: PatternBlend::Add,
                amount: 0.85,
                target_color: [1.35, 0.34, 0.02],
                faces: PatternFaces::ALL,
                relief_faces: PatternFaces::ALL,
                texels_per_voxel: 8,
                vary_per_face: true,
                domain_warp: 0.0,
                tile_aspect: 1.0,
                tile_bond: 0.5,
                tile_gap: 0.06,
                emission_intensity: 2.0,
                relief_height_meters: 0.0,
                relief_normal: true,
                relief_invert: false,
                relief_bevel_fraction: crate::pattern::DEFAULT_RELIEF_BEVEL_FRACTION,
                relief_normal_strength: 1.0,
                grid_average: false,
                relief_steps: 0,
            }]),
            acoustic_alpha: ACOUSTIC_GLOW_BLOCK,
        },
        // 27  Slate tile — the tessellation's demonstration row, and the first
        // material in the table whose look comes from WHERE the tiles are rather
        // than from a field sampled across a surface.
        //
        // Four layers, and the ORDER is the whole thing: each applies over the
        // previous one's output, so the joint has to be drawn last or the grain
        // would run straight through it and the wall would read as printed rather
        // than built.
        //
        //   1  tile tone     per-block shade, so no two blocks match
        //   2  simplex       the slate grain, in TILE frame so it restarts per block
        //   3  worley edge   fracture lines, warped so they are not lattice-straight
        //   4  tile edge     the joint, darkening last over everything above
        //
        // All four share one tessellation (2:1 running bond, 0.5 m, 6% gap) — the
        // thing the `material.tessellation` node exists to guarantee. Costed from
        // bench section 11 (median of three, 2026-08-02):
        // 0.029 + 0.383 + 1.076 + 0.052 = ~1.54 ms per layer-stack at full coverage,
        // which is why it is a demonstration row and not a terrain one. Note where
        // that goes: the worley edge alone is 70% of it, and the two tile generators
        // together are 5%.
        Material {
            name: "slate tile",
            // Lighter than the look aims for, because FOUR multiply layers stack:
            // each one only darkens, so the wall lands well below its base. Authored
            // at the value that ends up right, not the value that reads right here.
            albedo: [0.44, 0.45, 0.49],
            roughness: 0.72,
            specular: 0.04,
            kind: MaterialKind::Solid,
            emission: None,
            light: None,
            face_roles: None,
            patterns: PatternStack::of(&[
                PatternLayer {
                    generator: PatternGenerator::TileTone,
                    frame: PatternFrame::Tile,
                    period_meters: 0.5,
                    target: PatternTarget::Albedo,
                    blend: PatternBlend::Multiply,
                    // Well short of 1: the tone should read as blocks cut from one
                    // quarry, not as a chequerboard of unrelated stones.
                    amount: 0.45,
                    target_color: [1.0, 1.0, 1.0],
                    faces: PatternFaces::ALL,
                    relief_faces: PatternFaces::ALL,
                    texels_per_voxel: 0,
                    vary_per_face: false,
                    domain_warp: 0.0,
                    tile_aspect: 2.0,
                    tile_bond: 0.5,
                    tile_gap: 0.06,
                    emission_intensity: 1.0,
                    relief_height_meters: 0.0,
                    relief_normal: true,
                    relief_invert: false,
                    relief_bevel_fraction: crate::pattern::DEFAULT_RELIEF_BEVEL_FRACTION,
                    relief_normal_strength: 1.0,
                    grid_average: false,
                    relief_steps: 0,
                },
                PatternLayer {
                    // Four octaves, the ceiling, and it is still coarser than real
                    // slate grain. In TILE frame the generator sees 0..1 across a
                    // tile, so the octave count is the ONLY frequency control it
                    // has: one feature plus three harmonics, about eight across a
                    // block. Fine grain needs a per-layer detail scale that the
                    // frame does not currently offer — see `PatternFrame::Tile`.
                    generator: PatternGenerator::Simplex { octaves: 4 },
                    frame: PatternFrame::Tile,
                    period_meters: 0.5,
                    target: PatternTarget::Albedo,
                    blend: PatternBlend::Multiply,
                    amount: 0.55,
                    target_color: [1.0, 1.0, 1.0],
                    faces: PatternFaces::ALL,
                    relief_faces: PatternFaces::ALL,
                    // Continuous, not snapped: the tile frame already quantises the
                    // look at the joint, and a texel grid on top of that fights it.
                    texels_per_voxel: 0,
                    vary_per_face: false,
                    domain_warp: 0.0,
                    tile_aspect: 2.0,
                    tile_bond: 0.5,
                    tile_gap: 0.06,
                    emission_intensity: 1.0,
                    relief_height_meters: 0.0,
                    relief_normal: true,
                    relief_invert: false,
                    relief_bevel_fraction: crate::pattern::DEFAULT_RELIEF_BEVEL_FRACTION,
                    relief_normal_strength: 1.0,
                    grid_average: false,
                    relief_steps: 0,
                },
                PatternLayer {
                    generator: PatternGenerator::WorleyEdge,
                    frame: PatternFrame::Tile,
                    period_meters: 0.5,
                    target: PatternTarget::Albedo,
                    blend: PatternBlend::Multiply,
                    // An accent: past about a third the fractures stop reading as
                    // cracks in stone and become a net thrown over it.
                    amount: 0.32,
                    target_color: [1.0, 1.0, 1.0],
                    faces: PatternFaces::ALL,
                    relief_faces: PatternFaces::ALL,
                    texels_per_voxel: 0,
                    vary_per_face: false,
                    // Warped, because a cellular field on an unwarped lattice reads
                    // as bubbles; the warp is what makes the boundaries read as
                    // fractures. Costs about a second layer — see `domain_warp`.
                    domain_warp: 0.45,
                    tile_aspect: 2.0,
                    tile_bond: 0.5,
                    tile_gap: 0.06,
                    emission_intensity: 1.0,
                    relief_height_meters: 0.0,
                    relief_normal: true,
                    relief_invert: false,
                    relief_bevel_fraction: crate::pattern::DEFAULT_RELIEF_BEVEL_FRACTION,
                    relief_normal_strength: 1.0,
                    grid_average: false,
                    relief_steps: 0,
                },
                PatternLayer {
                    generator: PatternGenerator::TileEdge { sharpness: 0.72 },
                    frame: PatternFrame::Tile,
                    period_meters: 0.5,
                    target: PatternTarget::Albedo,
                    blend: PatternBlend::Multiply,
                    amount: 0.85,
                    target_color: [1.0, 1.0, 1.0],
                    faces: PatternFaces::ALL,
                    relief_faces: PatternFaces::ALL,
                    texels_per_voxel: 0,
                    vary_per_face: false,
                    domain_warp: 0.0,
                    tile_aspect: 2.0,
                    tile_bond: 0.5,
                    tile_gap: 0.06,
                    emission_intensity: 1.0,
                    relief_height_meters: 0.0,
                    relief_normal: true,
                    relief_invert: false,
                    relief_bevel_fraction: crate::pattern::DEFAULT_RELIEF_BEVEL_FRACTION,
                    relief_normal_strength: 1.0,
                    grid_average: false,
                    relief_steps: 0,
                },
            ]),
            acoustic_alpha: ACOUSTIC_STONE,
        },
        // ---- HDR test targets: six hues, base at SDR white, speckle at the PQ ceiling
        //
        // Six emissive blocks, each a pure hue at SDR reference white carrying a
        // speckle of the SAME hue at the Rec.2100 PQ encoding ceiling. They exist to
        // make the output path's dynamic range visible — see `docs/output-depth.md`.
        //
        // THE NITS CONVENTION, which is what makes any of this expressible. SDR
        // reference white is 100 cd/m^2 (Rec.709/sRGB), so:
        //
        //     linear 1.0    =    100 nits   <- SDR reference white, the BASE
        //     linear 100.0  = 10,000 nits   <- the PQ ceiling, `color(rec2100-pq 1.0)`
        //     nits          = linear * 100
        //
        // So a speckle authored at linear 100 IS `rec2100-pq 1 0 0` in extended-linear
        // terms. The value is right; only the output path cannot yet present it.
        //
        // WHAT THEY LOOK LIKE TODAY, and it is not "invisible in SDR". Through
        // `pow(reinhard(L), 1/2.2)`:
        //
        //     100 nits (base)      -> encoded 0.730 -> 8-bit code 186
        //     10,000 nits (speckle)-> encoded 0.996 -> 8-bit code 254
        //
        // A 68-code gap, so the speckle reads as a bright near-white dot. That is
        // BECAUSE REINHARD COMPRESSES RATHER THAN CLIPS: `L/(1+L)` maps every value
        // into 0..1, so nothing can ever exceed white and nothing is ever hidden. For
        // the speckle to be genuinely HDR-only, SDR would have to CLIP it — base and
        // speckle both pinned at white, indistinguishable — and then differ only where
        // real headroom exists. That is a tonemap change, not a material edit.
        //
        // Which makes their visibility useful rather than a failure: it proves the
        // authored radiance reaches the tonemap intact. When a PQ or extended-linear
        // path lands, these same six rows become the actual HDR test with no
        // re-authoring — only the numbers in the doc need recomputing against the PQ
        // curve, which allocates codes very differently from gamma 2.2.
        //
        // Pure primaries and secondaries deliberately: they are the widest-gamut
        // colours sRGB can name, so they are also the rows that will show the gamut
        // difference first once the surface carries Rec.2020 primaries.
        Material {
            name: "hdr_red",
            albedo: [0.0, 0.0, 0.0],
            roughness: 1.0,
            specular: 0.0,
            kind: MaterialKind::Solid,
            // #FF0000 at SDR reference white: linear 1.0 = 100 nits.
            emission: Some([1.0, 0.0, 0.0]),
            light: None,
            face_roles: None,
            patterns: PatternStack::of(&[PatternLayer {
                generator: PatternGenerator::Speckle { density: 0.3 },
                frame: PatternFrame::World,
                // 1.5 cm dots — finer than a voxel, so it reads as speckle rather than
                // as shapes, and a few pixels cover one dot at arm's length.
                period_meters: 0.015,
                target: PatternTarget::Emission,
                blend: PatternBlend::Add,
                amount: 1.0,
                // `rec2100-pq 1 0 0` — linear 100 = 10,000 nits, the PQ ceiling.
                // `Add` with amount 1.0 means this lands on top of the base unscaled.
                target_color: [100.0, 0.0, 0.0],
                faces: PatternFaces::ALL,
                relief_faces: PatternFaces::ALL,
                // No texel snap: the dots stay continuous rather than quantising to the
                // 1.56 cm lattice before the frame buffer sees them.
                texels_per_voxel: 0,
                vary_per_face: false,
                domain_warp: 0.0,
                tile_aspect: 2.0,
                tile_bond: 0.5,
                tile_gap: 0.06,
                emission_intensity: 1.0,
                relief_height_meters: 0.0,
                relief_normal: true,
                relief_invert: false,
                relief_bevel_fraction: crate::pattern::DEFAULT_RELIEF_BEVEL_FRACTION,
                relief_normal_strength: 1.0,
                grid_average: false,
                relief_steps: 0,
            }]),
            acoustic_alpha: ACOUSTIC_STONE,
        },
        Material {
            name: "hdr_green",
            albedo: [0.0, 0.0, 0.0],
            roughness: 1.0,
            specular: 0.0,
            kind: MaterialKind::Solid,
            // #00FF00 at SDR reference white: linear 1.0 = 100 nits.
            emission: Some([0.0, 1.0, 0.0]),
            light: None,
            face_roles: None,
            patterns: PatternStack::of(&[PatternLayer {
                generator: PatternGenerator::Speckle { density: 0.3 },
                frame: PatternFrame::World,
                // 1.5 cm dots — finer than a voxel, so it reads as speckle rather than
                // as shapes, and a few pixels cover one dot at arm's length.
                period_meters: 0.015,
                target: PatternTarget::Emission,
                blend: PatternBlend::Add,
                amount: 1.0,
                // `rec2100-pq 0 1 0` — linear 100 = 10,000 nits, the PQ ceiling.
                // `Add` with amount 1.0 means this lands on top of the base unscaled.
                target_color: [0.0, 100.0, 0.0],
                faces: PatternFaces::ALL,
                relief_faces: PatternFaces::ALL,
                // No texel snap: the dots stay continuous rather than quantising to the
                // 1.56 cm lattice before the frame buffer sees them.
                texels_per_voxel: 0,
                vary_per_face: false,
                domain_warp: 0.0,
                tile_aspect: 2.0,
                tile_bond: 0.5,
                tile_gap: 0.06,
                emission_intensity: 1.0,
                relief_height_meters: 0.0,
                relief_normal: true,
                relief_invert: false,
                relief_bevel_fraction: crate::pattern::DEFAULT_RELIEF_BEVEL_FRACTION,
                relief_normal_strength: 1.0,
                grid_average: false,
                relief_steps: 0,
            }]),
            acoustic_alpha: ACOUSTIC_STONE,
        },
        Material {
            name: "hdr_blue",
            albedo: [0.0, 0.0, 0.0],
            roughness: 1.0,
            specular: 0.0,
            kind: MaterialKind::Solid,
            // #0000FF at SDR reference white: linear 1.0 = 100 nits.
            emission: Some([0.0, 0.0, 1.0]),
            light: None,
            face_roles: None,
            patterns: PatternStack::of(&[PatternLayer {
                generator: PatternGenerator::Speckle { density: 0.3 },
                frame: PatternFrame::World,
                // 1.5 cm dots — finer than a voxel, so it reads as speckle rather than
                // as shapes, and a few pixels cover one dot at arm's length.
                period_meters: 0.015,
                target: PatternTarget::Emission,
                blend: PatternBlend::Add,
                amount: 1.0,
                // `rec2100-pq 0 0 1` — linear 100 = 10,000 nits, the PQ ceiling.
                // `Add` with amount 1.0 means this lands on top of the base unscaled.
                target_color: [0.0, 0.0, 100.0],
                faces: PatternFaces::ALL,
                relief_faces: PatternFaces::ALL,
                // No texel snap: the dots stay continuous rather than quantising to the
                // 1.56 cm lattice before the frame buffer sees them.
                texels_per_voxel: 0,
                vary_per_face: false,
                domain_warp: 0.0,
                tile_aspect: 2.0,
                tile_bond: 0.5,
                tile_gap: 0.06,
                emission_intensity: 1.0,
                relief_height_meters: 0.0,
                relief_normal: true,
                relief_invert: false,
                relief_bevel_fraction: crate::pattern::DEFAULT_RELIEF_BEVEL_FRACTION,
                relief_normal_strength: 1.0,
                grid_average: false,
                relief_steps: 0,
            }]),
            acoustic_alpha: ACOUSTIC_STONE,
        },
        Material {
            name: "hdr_cyan",
            albedo: [0.0, 0.0, 0.0],
            roughness: 1.0,
            specular: 0.0,
            kind: MaterialKind::Solid,
            // #00FFFF at SDR reference white: linear 1.0 = 100 nits.
            emission: Some([0.0, 1.0, 1.0]),
            light: None,
            face_roles: None,
            patterns: PatternStack::of(&[PatternLayer {
                generator: PatternGenerator::Speckle { density: 0.3 },
                frame: PatternFrame::World,
                // 1.5 cm dots — finer than a voxel, so it reads as speckle rather than
                // as shapes, and a few pixels cover one dot at arm's length.
                period_meters: 0.015,
                target: PatternTarget::Emission,
                blend: PatternBlend::Add,
                amount: 1.0,
                // `rec2100-pq 0 1 1` — linear 100 = 10,000 nits, the PQ ceiling.
                // `Add` with amount 1.0 means this lands on top of the base unscaled.
                target_color: [0.0, 100.0, 100.0],
                faces: PatternFaces::ALL,
                relief_faces: PatternFaces::ALL,
                // No texel snap: the dots stay continuous rather than quantising to the
                // 1.56 cm lattice before the frame buffer sees them.
                texels_per_voxel: 0,
                vary_per_face: false,
                domain_warp: 0.0,
                tile_aspect: 2.0,
                tile_bond: 0.5,
                tile_gap: 0.06,
                emission_intensity: 1.0,
                relief_height_meters: 0.0,
                relief_normal: true,
                relief_invert: false,
                relief_bevel_fraction: crate::pattern::DEFAULT_RELIEF_BEVEL_FRACTION,
                relief_normal_strength: 1.0,
                grid_average: false,
                relief_steps: 0,
            }]),
            acoustic_alpha: ACOUSTIC_STONE,
        },
        Material {
            name: "hdr_magenta",
            albedo: [0.0, 0.0, 0.0],
            roughness: 1.0,
            specular: 0.0,
            kind: MaterialKind::Solid,
            // #FF00FF at SDR reference white: linear 1.0 = 100 nits.
            emission: Some([1.0, 0.0, 1.0]),
            light: None,
            face_roles: None,
            patterns: PatternStack::of(&[PatternLayer {
                generator: PatternGenerator::Speckle { density: 0.3 },
                frame: PatternFrame::World,
                // 1.5 cm dots — finer than a voxel, so it reads as speckle rather than
                // as shapes, and a few pixels cover one dot at arm's length.
                period_meters: 0.015,
                target: PatternTarget::Emission,
                blend: PatternBlend::Add,
                amount: 1.0,
                // `rec2100-pq 1 0 1` — linear 100 = 10,000 nits, the PQ ceiling.
                // `Add` with amount 1.0 means this lands on top of the base unscaled.
                target_color: [100.0, 0.0, 100.0],
                faces: PatternFaces::ALL,
                relief_faces: PatternFaces::ALL,
                // No texel snap: the dots stay continuous rather than quantising to the
                // 1.56 cm lattice before the frame buffer sees them.
                texels_per_voxel: 0,
                vary_per_face: false,
                domain_warp: 0.0,
                tile_aspect: 2.0,
                tile_bond: 0.5,
                tile_gap: 0.06,
                emission_intensity: 1.0,
                relief_height_meters: 0.0,
                relief_normal: true,
                relief_invert: false,
                relief_bevel_fraction: crate::pattern::DEFAULT_RELIEF_BEVEL_FRACTION,
                relief_normal_strength: 1.0,
                grid_average: false,
                relief_steps: 0,
            }]),
            acoustic_alpha: ACOUSTIC_STONE,
        },
        Material {
            name: "hdr_yellow",
            albedo: [0.0, 0.0, 0.0],
            roughness: 1.0,
            specular: 0.0,
            kind: MaterialKind::Solid,
            // #FFFF00 at SDR reference white: linear 1.0 = 100 nits.
            emission: Some([1.0, 1.0, 0.0]),
            light: None,
            face_roles: None,
            patterns: PatternStack::of(&[PatternLayer {
                generator: PatternGenerator::Speckle { density: 0.3 },
                frame: PatternFrame::World,
                // 1.5 cm dots — finer than a voxel, so it reads as speckle rather than
                // as shapes, and a few pixels cover one dot at arm's length.
                period_meters: 0.015,
                target: PatternTarget::Emission,
                blend: PatternBlend::Add,
                amount: 1.0,
                // `rec2100-pq 1 1 0` — linear 100 = 10,000 nits, the PQ ceiling.
                // `Add` with amount 1.0 means this lands on top of the base unscaled.
                target_color: [100.0, 100.0, 0.0],
                faces: PatternFaces::ALL,
                relief_faces: PatternFaces::ALL,
                // No texel snap: the dots stay continuous rather than quantising to the
                // 1.56 cm lattice before the frame buffer sees them.
                texels_per_voxel: 0,
                vary_per_face: false,
                domain_warp: 0.0,
                tile_aspect: 2.0,
                tile_bond: 0.5,
                tile_gap: 0.06,
                emission_intensity: 1.0,
                relief_height_meters: 0.0,
                relief_normal: true,
                relief_invert: false,
                relief_bevel_fraction: crate::pattern::DEFAULT_RELIEF_BEVEL_FRACTION,
                relief_normal_strength: 1.0,
                grid_average: false,
                relief_steps: 0,
            }]),
            acoustic_alpha: ACOUSTIC_STONE,
        },
        // 34-39  Albedo* (L0) — saturated Lambertian REFLECTORS for the indirect
        // light fixture. Distinct from the `hdr_*` rows directly above, which are
        // pure emitters at zero albedo: a room built from those cannot bounce a
        // photon, which is how the first version of the L0 corridor came to
        // measure emission while claiming to measure albedo.
        //
        // The numbers: 0.80 in a channel the surface reflects, 0.05 in one it does
        // not. Two deliberate choices.
        //
        // * 0.80 rather than 1.0 keeps the infinite-bounce series convergent
        //   (sum 0.8^n = 5). A unit-albedo surface makes a closed room a
        //   divergent feedback loop, and CAGI resampling its own cache would grow
        //   without bound rather than settle.
        // * 0.05 rather than 0.0 leaves the off-channels a floor. At hard zero a
        //   channel that darkens has nowhere left to go, so the per-channel
        //   divergence the corpus warns about ("unequal channels diverge visibly
        //   as they darken") would clip instead of showing as the hue shift the
        //   fixture is built to make visible.
        //
        // Roughness 1.0 / specular 0.0: pure diffuse, so nothing in the read can
        // be a specular highlight.
        opaque("albedo_red", [0.80, 0.05, 0.05], 1.0, 0.0, ACOUSTIC_STONE),
        opaque("albedo_green", [0.05, 0.80, 0.05], 1.0, 0.0, ACOUSTIC_STONE),
        opaque("albedo_blue", [0.05, 0.05, 0.80], 1.0, 0.0, ACOUSTIC_STONE),
        opaque("albedo_cyan", [0.05, 0.80, 0.80], 1.0, 0.0, ACOUSTIC_STONE),
        opaque(
            "albedo_magenta",
            [0.80, 0.05, 0.80],
            1.0,
            0.0,
            ACOUSTIC_STONE,
        ),
        opaque(
            "albedo_yellow",
            [0.80, 0.80, 0.05],
            1.0,
            0.0,
            ACOUSTIC_STONE,
        ),
    ]
};

/// The compiled material table in upload form, for binding 5.
///
/// Only the *initial* upload goes through this — once
/// [`crate::material_table::MaterialTable`] owns the rows, it uploads its own.
pub fn gpu_materials() -> Vec<GpuMaterial> {
    MATERIALS.iter().map(Material::to_gpu).collect()
}

/// Which generators an authored material table can actually reach, as a bitmask
/// for `MATERIAL_PATTERN_GENERATOR_MASK`.
///
/// The table alone is authoritative, which is worth stating because a material
/// GRAPH looks like it should count too. It does not: a graph's authored layers are
/// projected into the row's [`PatternStack`] and uploaded like any other row, and
/// all a graph supplies at runtime is per-slot gain and drift — the shader reads
/// every layer from `materials[material].patterns[slot]`. So a generator the shader
/// can reach is a generator some row carries.
///
/// The flat bit is always set. It is the generator switch's FALL-THROUGH rather
/// than a masked branch, so clearing it could not remove any code, and seeding it
/// keeps the empty-table mask meaningful instead of zero.
pub fn generator_mask(rows: &[Material]) -> u32 {
    let mut mask = PatternGenerator::Flat.mask_bit();
    for row in rows {
        mask |= row.patterns.generator_mask();
    }
    mask
}

#[cfg(test)]
mod tests {
    /// Every generator's bit is its own, and inside the 14-bit field the shader
    /// masks with. A collision here would silently prune a generator that shares a
    /// bit with one the table uses.
    #[test]
    fn every_generator_owns_a_distinct_mask_bit() {
        let mut seen = 0u32;
        for generator in PatternGenerator::ALL {
            let bit = generator.mask_bit();
            assert_eq!(bit.count_ones(), 1, "{generator:?} is not a single bit");
            assert_eq!(bit, 1 << generator.code(), "{generator:?} bit != 1 << code");
            assert_eq!(seen & bit, 0, "{generator:?} collides with an earlier bit");
            assert!(
                bit <= crate::pattern::PATTERN_GENERATOR_MASK_ALL,
                "{generator:?} falls outside the all-bits mask the shader patches"
            );
            seen |= bit;
        }
        assert_eq!(
            seen,
            crate::pattern::PATTERN_GENERATOR_MASK_ALL,
            "PATTERN_GENERATOR_MASK_ALL must be exactly the union of every bit"
        );
    }

    /// THE SAFETY PROPERTY, stated as a test rather than as a comment: the derived
    /// mask contains a bit for every generator any row actually authors.
    ///
    /// This is the direction that matters. A mask with a spare bit set only leaves
    /// dead code compiled in and costs a little speed; a mask MISSING a bit makes
    /// that material render silently flat, which is the footgun a hand-set mask
    /// would have been. Run over the shipped table, so authoring a new generator
    /// into a row cannot quietly break the derivation.
    #[test]
    fn the_derived_mask_covers_every_generator_the_shipped_table_authors() {
        let mask = generator_mask(&MATERIALS);
        for row in MATERIALS {
            for layer in row.patterns.layers.iter().flatten() {
                assert_ne!(
                    mask & layer.generator.mask_bit(),
                    0,
                    "{:?} authors {:?}, which the derived mask would compile out",
                    row.kind,
                    layer.generator
                );
            }
        }
        assert_ne!(
            mask & PatternGenerator::Flat.mask_bit(),
            0,
            "flat is the generator switch's fall-through and must always be set"
        );
    }

    /// An empty table prunes everything except the fall-through, and a table using
    /// one generator compiles in exactly two bits. Pins that the mask really is
    /// derived from content rather than defaulting to all-bits.
    #[test]
    fn the_derived_mask_tracks_what_the_table_uses() {
        assert_eq!(generator_mask(&[]), PatternGenerator::Flat.mask_bit());

        let mut row = MATERIALS[1];
        row.patterns = PatternStack::of(&[PatternLayer {
            generator: PatternGenerator::Worley,
            ..PatternLayer::IDENTITY
        }]);
        let mask = generator_mask(&[row]);
        assert_eq!(
            mask,
            PatternGenerator::Flat.mask_bit() | PatternGenerator::Worley.mask_bit(),
            "one worley layer should compile in worley and the fall-through, nothing else"
        );
    }

    use super::*;

    /// The **upload pin**: the exact 26 GPU rows this table produced before the
    /// authored row became a union (`git show HEAD~:...` at the time of the
    /// refactor, dumped through the old `to_gpu`).
    ///
    /// This replaces an older test that pinned the authored *albedo column*. That
    /// pin was aimed at the right property — "a structural change must not shift a
    /// pixel" — but at the wrong layer: it froze the one column this arc exists to
    /// re-author, while leaving every other column free to drift. Pinning the
    /// UPLOADED row instead is strictly stronger (it covers all ten fields,
    /// including the derived ones and the flag word) and it constrains only what
    /// reaches the GPU, which is what "no pixel moves" actually means.
    ///
    /// When a stage deliberately re-authors the table, this constant is what gets
    /// updated, in the same commit, with the reason in the message.
    ///
    /// It pins the fields that existed BEFORE S1 and deliberately does not grow as
    /// the row does. S1 widened `GpuMaterial` with per-face-role slots; pinning
    /// those here too would mean regenerating this constant every time the row gains
    /// a field, which is how a pin quietly stops being evidence. The new slots get
    /// their own tests, which assert the property that actually matters: a row
    /// without authored roles uploads its base values in every slot.
    /// How many rows [`UPLOAD_PIN`] covers.
    ///
    /// DELIBERATELY NOT `MATERIAL_COUNT`. The pin is a recorded baseline — "these
    /// rows still upload the bytes they used to" — so its length is a property of
    /// when it was recorded, not of how long the table happens to be now. Tying the
    /// two together meant that appending a row demanded three hand-computed
    /// `CorePin` entries, which would have been fabricated evidence rather than a
    /// regression check.
    ///
    /// Appending rows past this index is therefore free and cannot weaken the pin.
    /// CHANGING any row at or below it must still fail, which is the whole point, and
    /// the assertion below is written so it does.
    const PINNED_ROW_COUNT: usize = 28;

    const UPLOAD_PIN: [CorePin; PINNED_ROW_COUNT] = [
        // 0  air
        CorePin {
            albedo: [0.0, 0.0, 0.0],
            transmittance: 1.0,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.0,
            opacity: 0.0,
            specular: 0.0,
            flags: 4,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 1  grass
        CorePin {
            albedo: [0.41, 0.52, 0.29],
            transmittance: 0.0,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.95,
            opacity: 1.0,
            specular: 0.02,
            flags: 0,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 2  tall_grass
        CorePin {
            albedo: [0.28, 0.45, 0.23],
            transmittance: 0.35,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.9,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 3  dirt
        CorePin {
            albedo: [0.44, 0.32, 0.22],
            transmittance: 0.0,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.97,
            opacity: 1.0,
            specular: 0.02,
            flags: 0,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 4  sand
        CorePin {
            albedo: [0.86, 0.77, 0.55],
            transmittance: 0.0,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.95,
            opacity: 1.0,
            specular: 0.02,
            flags: 0,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 5  sediment
        CorePin {
            albedo: [0.17, 0.16, 0.11],
            transmittance: 0.0,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.97,
            opacity: 1.0,
            specular: 0.02,
            flags: 0,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 6  stone
        CorePin {
            albedo: [0.52, 0.52, 0.55],
            transmittance: 0.0,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.85,
            opacity: 1.0,
            specular: 0.04,
            flags: 0,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 7  water
        CorePin {
            albedo: [0.19, 0.52, 0.71],
            transmittance: 0.85,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.05,
            opacity: 0.7,
            specular: 0.02,
            flags: 12,
            index_of_refraction: 1.333,
            absorption_per_meter: [0.446, 0.09, 0.015],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.004, 0.03, 0.045],
            _pad_scattering: 0.0,
        },
        // 8  trunk
        CorePin {
            albedo: [0.45, 0.31, 0.19],
            transmittance: 0.0,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.9,
            opacity: 1.0,
            specular: 0.03,
            flags: 0,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 9  trunk_birch
        CorePin {
            albedo: [0.8, 0.78, 0.72],
            transmittance: 0.0,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.85,
            opacity: 1.0,
            specular: 0.03,
            flags: 0,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 10 leaves
        CorePin {
            albedo: [0.38, 0.505, 0.235],
            transmittance: 0.25,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.88,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 11 leaves_dark
        CorePin {
            albedo: [0.281, 0.374, 0.174],
            transmittance: 0.2,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.88,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 12 leaves_birch
        CorePin {
            albedo: [0.51, 0.58, 0.28],
            transmittance: 0.28,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.88,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 13 leaves_pine
        CorePin {
            albedo: [0.21, 0.345, 0.24],
            transmittance: 0.15,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.9,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 14 flower_pink
        CorePin {
            albedo: [0.93, 0.55, 0.75],
            transmittance: 0.3,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.9,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 15 flower_white
        CorePin {
            albedo: [0.96, 0.95, 0.9],
            transmittance: 0.3,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.9,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 16 flower_yellow
        CorePin {
            albedo: [0.95, 0.83, 0.35],
            transmittance: 0.3,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.9,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 17 flower_blue
        CorePin {
            albedo: [0.45, 0.52, 0.92],
            transmittance: 0.3,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.9,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 18 water_weed
        CorePin {
            albedo: [0.15, 0.3, 0.19],
            transmittance: 0.35,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.85,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 19 lily_pad
        CorePin {
            albedo: [0.26, 0.5, 0.24],
            transmittance: 0.2,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.8,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 20 lily_bloom
        CorePin {
            albedo: [0.95, 0.92, 0.85],
            transmittance: 0.25,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.85,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 21 reed
        CorePin {
            albedo: [0.55, 0.56, 0.31],
            transmittance: 0.3,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.9,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 22 cattail_head
        CorePin {
            albedo: [0.32, 0.18, 0.08],
            transmittance: 0.1,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.92,
            opacity: 1.0,
            specular: 0.03,
            flags: 1,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 23 snow
        CorePin {
            albedo: [0.92, 0.93, 0.96],
            transmittance: 0.0,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.75,
            opacity: 1.0,
            specular: 0.03,
            flags: 0,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 24 glow_block
        CorePin {
            albedo: [0.95, 0.93, 0.88],
            transmittance: 0.0,
            emission: [3.0, 2.8, 2.4],
            roughness: 0.6,
            opacity: 1.0,
            specular: 0.04,
            flags: 2,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 25 glow_berry
        CorePin {
            albedo: [0.55, 0.95, 0.8],
            transmittance: 0.2,
            emission: [0.5, 1.1, 0.8],
            roughness: 0.6,
            opacity: 1.0,
            specular: 0.04,
            flags: 3,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 26 lava
        CorePin {
            albedo: [0.42, 0.09, 0.015],
            transmittance: 0.0,
            emission: [1.2, 0.16, 0.015],
            roughness: 0.42,
            opacity: 1.0,
            specular: 0.03,
            flags: 2,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
        // 27  slate tile — APPENDED, not re-authored. The pin's job is that a
        // structural change moves no pixel; a new row at the end of the table moves
        // none of the twenty-seven above it, and every one of those entries is
        // byte-identical to what it was. Its own pre-S1 fields are an ordinary
        // opaque solid: the tessellation lives entirely in the pattern slots, which
        // this pin deliberately does not cover.
        CorePin {
            albedo: [0.44, 0.45, 0.49],
            transmittance: 0.0,
            emission: [0.0, 0.0, 0.0],
            roughness: 0.72,
            opacity: 1.0,
            specular: 0.04,
            flags: 0,
            index_of_refraction: 1.0,
            absorption_per_meter: [0.0, 0.0, 0.0],
            _pad_absorption: 0.0,
            scattering_per_meter: [0.0, 0.0, 0.0],
            _pad_scattering: 0.0,
        },
    ];

    /// The subset of [`GpuMaterial`] that existed before S1 — what [`UPLOAD_PIN`]
    /// freezes.
    #[derive(Debug, PartialEq)]
    struct CorePin {
        albedo: [f32; 3],
        transmittance: f32,
        emission: [f32; 3],
        roughness: f32,
        opacity: f32,
        specular: f32,
        flags: u32,
        index_of_refraction: f32,
        absorption_per_meter: [f32; 3],
        _pad_absorption: f32,
        scattering_per_meter: [f32; 3],
        _pad_scattering: f32,
    }

    impl CorePin {
        /// A row's pre-S1 fields.
        ///
        /// The flag word is masked to the pre-S1 bits: S1 adds `FACE_ROLES`, and a
        /// row that legitimately sets it must not read as a pin violation. Which
        /// rows set it is pinned separately, by
        /// `only_grass_authors_face_roles_today`.
        fn of(row: &GpuMaterial) -> CorePin {
            const PRE_S1_FLAGS: u32 = 0b1111;
            CorePin {
                albedo: row.albedo,
                transmittance: row.transmittance,
                emission: row.emission,
                roughness: row.roughness,
                opacity: row.opacity,
                specular: row.specular,
                flags: row.flags & PRE_S1_FLAGS,
                index_of_refraction: row.index_of_refraction,
                absorption_per_meter: row.absorption_per_meter,
                _pad_absorption: row._pad_absorption,
                scattering_per_meter: row.scattering_per_meter,
                _pad_scattering: row._pad_scattering,
            }
        }
    }

    // ---- The S0 gate: the union must be a pure authoring refactor -----------

    /// Every uploaded byte must be what it was before the union existed. This is
    /// the whole S0 gate in one assertion: the authored row got a shape, and the
    /// GPU saw no difference at all.
    #[test]
    fn the_uploaded_table_is_unchanged_by_the_union() {
        let uploaded = gpu_materials();
        // The pin covers a PREFIX of the table. Rows appended after it was recorded
        // (the `hdr_*` output-depth test targets) are not pinned and do not need to
        // be; rows within it must not have moved.
        assert!(
            uploaded.len() >= UPLOAD_PIN.len(),
            "the table shrank below the pinned prefix — a pinned row was deleted"
        );
        for (id, (actual, pinned)) in uploaded.iter().zip(UPLOAD_PIN.iter()).enumerate() {
            assert_eq!(
                &CorePin::of(actual),
                pinned,
                "material {id} ({}) no longer uploads the values it used to",
                MATERIALS[id].name
            );
        }
    }

    /// The uploaded slice must be exactly the table, with no padding surprises —
    /// what `write_buffer` actually sends.
    #[test]
    fn the_uploaded_slice_is_the_whole_table() {
        let rows = gpu_materials();
        let bytes = bytemuck::cast_slice::<GpuMaterial, u8>(&rows);
        assert_eq!(bytes.len(), MATERIAL_TABLE_BYTES);
        assert_eq!(
            bytes.len(),
            MATERIAL_COUNT * std::mem::size_of::<GpuMaterial>()
        );
    }

    // ---- Structural invariants of the table --------------------------------

    #[test]
    fn table_covers_every_material_id() {
        assert_eq!(MATERIALS.len(), MATERIAL_COUNT);
    }

    /// Structural replacement for the old frozen-albedo pin: the properties a
    /// row must satisfy whatever colour it is authored as.
    #[test]
    fn every_row_is_structurally_sound() {
        let mut names = std::collections::HashSet::new();
        for material in &MATERIALS {
            assert!(!material.name.is_empty(), "a row has no name");
            assert!(
                names.insert(material.name),
                "duplicate material name {}",
                material.name
            );
            let finite = |value: f32| value.is_finite();
            assert!(
                material.albedo.iter().copied().all(finite)
                    && material.emitted_radiance().iter().copied().all(finite)
                    && finite(material.roughness)
                    && finite(material.specular)
                    && finite(material.transmittance())
                    && finite(material.opacity())
                    && finite(material.index_of_refraction()),
                "{} has a non-finite value",
                material.name
            );
            assert!(
                material.absorption_per_meter().iter().all(|c| *c >= 0.0)
                    && material.scattering_per_meter().iter().all(|c| *c >= 0.0),
                "{} has a negative medium coefficient",
                material.name
            );
        }
    }

    /// Air is the miss sentinel: sampling it must produce black, not a
    /// plausible-looking colour that hides the bug.
    #[test]
    fn air_row_is_zeroed() {
        let air = MATERIALS[0];
        assert!(matches!(air.kind, MaterialKind::Air));
        assert_eq!(air.albedo, [0.0, 0.0, 0.0]);
        assert_eq!(air.emitted_radiance(), [0.0, 0.0, 0.0]);
        assert_eq!(air.opacity(), 0.0);
        // Air is the ONLY row of its kind — a second one would mean two sentinels.
        assert_eq!(
            MATERIALS
                .iter()
                .filter(|m| matches!(m.kind, MaterialKind::Air))
                .count(),
            1
        );
    }

    /// The GPU row must stay a whole number of 16-byte std430 rows with no
    /// interior padding, or the WGSL `array<Material>` stride silently disagrees
    /// with the upload.
    ///
    /// 256 bytes since S2 added the four 32-byte pattern slots (E6's
    /// absorption/scattering pair took it to 80, S1's face roles to 128; the
    /// tessellation row grew the slots to 48 bytes and the relief-shaping row to
    /// 64). Twenty-four std430 rows, each a `vec3` followed by the scalar filling
    /// its `w`, or four scalars.
    #[test]
    fn gpu_row_layout_matches_wgsl() {
        assert_eq!(std::mem::size_of::<GpuMaterial>(), 384);
        assert_eq!(std::mem::size_of::<GpuMaterial>() % 16, 0);
        assert_eq!(std::mem::align_of::<GpuMaterial>(), 4);
        assert_eq!(MATERIAL_TABLE_BYTES, MATERIAL_COUNT * 384);
        // Still trivially free against a ~41 MB world: material richness is not the
        // expensive axis, per-VOXEL state is. The whole S2 layer model now costs
        // 10 KB of table (up from 3.3 when the slot was 32 bytes), which is the
        // argument for putting detail on the material rather than on the voxel
        // restated as a number.
        // 40 rows: 28 shipped, six `hdr_*` output-depth test targets, and six
        // `albedo_*` reflectors for L0's indirect-light fixture. Absolute, not
        // derived, so that a row silently changing size shows up here rather than
        // nowhere.
        //
        // The `albedo_*` six are appended past `PINNED_ROW_COUNT`, so they cannot
        // weaken `UPLOAD_PIN` and no pinned row had to be recomputed for them.
        assert_eq!(MATERIAL_TABLE_BYTES, 15360);
        // The pattern slots must account for two thirds of the row, or the WGSL's
        // fixed-size array has drifted from `MAX_PATTERN_LAYERS`.
        assert_eq!(
            std::mem::size_of::<[GpuPatternLayer; MAX_PATTERN_LAYERS]>(),
            256
        );
    }

    #[test]
    fn every_row_is_physically_in_range() {
        for material in &MATERIALS {
            let in_unit_range = |value: f32| (0.0..=1.0).contains(&value);
            assert!(
                material.albedo.iter().copied().all(in_unit_range),
                "{} albedo out of range",
                material.name
            );
            assert!(
                material
                    .emitted_radiance()
                    .iter()
                    .copied()
                    .all(|v| v >= 0.0),
                "{} emission must not be negative",
                material.name
            );
            assert!(
                in_unit_range(material.roughness)
                    && in_unit_range(material.specular)
                    && in_unit_range(material.opacity())
                    && in_unit_range(material.transmittance()),
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

    // ---- The union: flags and scalars are DERIVED, so they cannot disagree --

    /// The point of the union. Before it, `flags` was authored beside the values
    /// it described and nothing stopped the two disagreeing — a `LIQUID` row with
    /// no medium, or a `FOLIAGE` row that blocked all light, were both writable.
    /// Now each flag is a function of the kind, so this test checks a derivation
    /// rather than policing hand-written data.
    #[test]
    fn flags_are_derived_from_the_kind() {
        for material in &MATERIALS {
            let flags = material.flags();
            assert_eq!(
                flags.contains(MaterialFlags::FOLIAGE),
                matches!(material.kind, MaterialKind::Cover { .. }),
                "{} FOLIAGE disagrees with its kind",
                material.name
            );
            assert_eq!(
                flags.contains(MaterialFlags::EMISSIVE),
                material.emission.is_some(),
                "{} EMISSIVE disagrees with its emission",
                material.name
            );
            assert_eq!(
                flags.contains(MaterialFlags::TRANSPARENT),
                matches!(material.kind, MaterialKind::Air | MaterialKind::Medium(..)),
                "{} TRANSPARENT disagrees with its kind",
                material.name
            );
            assert_eq!(
                flags.contains(MaterialFlags::LIQUID),
                material.is_liquid(),
                "{} LIQUID disagrees with its phase",
                material.name
            );
        }
    }

    /// Only a medium may carry medium coefficients or bend a ray. Everything else
    /// must upload the honest "does not apply" sentinels — which is now
    /// structurally impossible to get wrong, and this pins that.
    #[test]
    fn only_media_refract_or_carry_coefficients() {
        for material in &MATERIALS {
            match material.medium() {
                Some(medium) => {
                    assert!(
                        medium.index_of_refraction > 1.0 && medium.index_of_refraction < 2.0,
                        "{} is a medium but its index of refraction is {}",
                        material.name,
                        medium.index_of_refraction
                    );
                }
                None => {
                    assert_eq!(
                        material.index_of_refraction(),
                        AIR_INDEX_OF_REFRACTION,
                        "{} cannot be entered, so its index must be exactly air's",
                        material.name
                    );
                    assert_eq!(material.absorption_per_meter(), NOT_A_MEDIUM);
                    assert_eq!(material.scattering_per_meter(), NOT_A_MEDIUM);
                }
            }
        }
    }

    /// Every cover row must transmit some light — a leaf that blocks 100% of it is
    /// what makes CAGI paint black canopies.
    #[test]
    fn cover_rows_transmit_light() {
        let mut cover_rows = 0;
        for material in &MATERIALS {
            if let MaterialKind::Cover { transmittance } = material.kind {
                cover_rows += 1;
                assert!(
                    transmittance > 0.0,
                    "{} is cover but blocks all light",
                    material.name
                );
                assert!(material.is_foliage(), "{} must be foliage", material.name);
            }
        }
        // Guard against the kind silently disappearing from the whole table.
        assert!(cover_rows > 0, "no cover rows left in the table");
    }

    /// Water is the only medium today, and its acoustic behaviour must not drift
    /// toward the visual intuition: a water surface reflects sound almost
    /// perfectly (alpha ~0.01), which transparency wrongly suggests otherwise.
    #[test]
    fn water_is_a_liquid_medium_and_acoustically_reflective() {
        let water = MATERIALS[material_id(Voxel::Water) as usize];
        let medium = water.medium().expect("water must be a medium");
        assert_eq!(medium.phase, MediumPhase::Liquid);
        assert_eq!(medium.index_of_refraction, WATER_INDEX_OF_REFRACTION);
        assert!(water.is_liquid());
        assert!(water.opacity() < 1.0);
        assert!(
            water.acoustic_alpha.iter().all(|alpha| *alpha <= 0.05),
            "water must stay acoustically reflective"
        );
        // The value must survive the upload — it rides in the row's former pad word.
        assert_eq!(
            water.to_gpu().index_of_refraction,
            WATER_INDEX_OF_REFRACTION
        );
    }

    /// The volume colour must be DERIVED from the coefficient pair, never taken
    /// from the surface albedo — the specific error the E6 rule forbids.
    #[test]
    fn a_mediums_colour_is_derived_not_painted() {
        let water = MATERIALS[material_id(Voxel::Water) as usize];
        let derived = water.single_scattering_albedo();
        assert!(
            derived[2] > derived[1] && derived[1] > derived[0],
            "water's derived colour is not blue-dominant: {derived:?}"
        );
        // Extinction is the pair's sum, per channel.
        let absorption = water.absorption_per_meter();
        let scattering = water.scattering_per_meter();
        for (channel, extinction) in water.extinction_per_meter().iter().enumerate() {
            assert!(
                (extinction - (absorption[channel] + scattering[channel])).abs() < 1e-9,
                "channel {channel} extinction is not absorption + scattering"
            );
        }
        // A row a ray cannot enter has no colour to derive, and must not divide by zero.
        assert_eq!(
            MATERIALS[material_id(Voxel::Stone) as usize].single_scattering_albedo(),
            [0.0, 0.0, 0.0]
        );
    }

    // ---- Ids, predicates and the flag word ---------------------------------

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
            Voxel::Lava,
        ] {
            assert!(
                material_blocks_movement(material_id(voxel)),
                "{voxel:?} must block movement"
            );
        }
    }

    /// The edit path's single notion of emptiness ("to the editor, water IS air").
    /// Spelled out per variant because it is a design decision, not an accident of
    /// a flag: air and every liquid are empty, everything a body can stand on or
    /// walk through is not.
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
        // The rule is the liquid PHASE, not a water special case — so the next
        // transparent fluid inherits it without a branch.
        for (id, material) in MATERIALS.iter().enumerate() {
            if material.is_liquid() {
                assert!(
                    material_is_empty_for_edits(id as u8),
                    "{} is a liquid but the editor would treat it as a block",
                    material.name
                );
            }
        }
        // A non-liquid medium (glass, ice) must NOT be empty to the editor: a ray
        // enters it, a body does not. Nothing authors one yet, so check the
        // derivation directly rather than pretend the table covers it.
        let glass = Material {
            name: "glass_probe",
            albedo: [0.9, 0.95, 1.0],
            roughness: 0.05,
            specular: 0.04,
            kind: MaterialKind::Medium(Medium {
                phase: MediumPhase::Solid,
                index_of_refraction: 1.52,
                absorption_per_meter: [0.02, 0.02, 0.03],
                scattering_per_meter: NOT_A_MEDIUM,
                opacity: 0.1,
                transmittance: 0.95,
            }),
            emission: None,
            light: None,
            face_roles: None,
            patterns: NO_PATTERNS,
            acoustic_alpha: [0.03; 6],
        };
        assert!(!glass.is_liquid());
        assert!(glass.flags().contains(MaterialFlags::TRANSPARENT));
        assert!(!glass.flags().contains(MaterialFlags::LIQUID));
    }

    /// The cheap liquid predicate and the table must not drift. They used to agree
    /// only by this test, because the predicate hardcoded `Voxel::Water`; now it
    /// reads the table, so this pins one mechanism rather than reconciling two.
    #[test]
    fn liquid_predicate_agrees_with_the_table() {
        for (id, material) in MATERIALS.iter().enumerate() {
            assert_eq!(
                material_is_liquid(id as u8),
                material.is_liquid(),
                "{} disagrees about being a liquid",
                material.name
            );
        }
        assert!(material_is_liquid(material_id(Voxel::Water)));
        // Ids past the table are not liquid, matching the air sentinel.
        assert!(!material_is_liquid(MATERIAL_COUNT as u8));
        assert!(!material_is_liquid(u8::MAX));
    }

    /// The emissive rows must actually emit, and every other row must stay dark —
    /// an accidental non-zero emission would light the world from its terrain.
    #[test]
    fn only_the_emissive_rows_emit() {
        for material in &MATERIALS {
            let emits =
                material.emitted_radiance().iter().any(|c| *c > 0.0) || material.light.is_some();
            assert_eq!(
                emits,
                material.is_emissive(),
                "{} emission disagrees with its emissive flag",
                material.name
            );
        }
        assert_eq!(
            MATERIALS.iter().filter(|m| m.is_emissive()).count(),
            9,
            "Three shipped emitters (glow_block, glow_berry, lava) plus six \
             DIAGNOSTIC ones — the `hdr_*` hue blocks, which are emissive so their \
             radiance does not depend on irradiance. There is NO emitter-palette cap \
             to worry about: `cagi.rs` states E5b carries every material's mean \
             separately rather than indexing a slot table."
        );
    }

    /// The look/light split (2026-08-07): `light` redirects what a row CASTS
    /// into the light volume without touching what its own face SHOWS. No
    /// authored light means the two halves are the same number — every
    /// pre-split row behaves bit-identically.
    #[test]
    fn light_splits_the_cast_from_the_look() {
        let glow_berry = MATERIALS[material_id(Voxel::GlowBerry) as usize];
        assert_eq!(glow_berry.light, None);
        assert_eq!(
            glow_berry.mean_injected_radiance(),
            glow_berry.mean_emitted_radiance(),
            "no authored light: cast exactly what you display"
        );

        // The shipped bright emitter is the split's first customer: it throws
        // 8x what its face shows.
        let glow_block = MATERIALS[material_id(Voxel::GlowBlock) as usize];
        assert_eq!(glow_block.emitted_radiance(), [3.00, 2.80, 2.40]);
        assert_eq!(glow_block.mean_injected_radiance(), [24.0, 22.4, 19.2]);

        let mut ember = glow_block;
        ember.emission = Some([1.5, 0.4, 0.1]);
        ember.light = Some([24.0, 6.0, 1.5]);
        assert_eq!(ember.emitted_radiance(), [1.5, 0.4, 0.1], "the look half");
        assert_eq!(
            ember.mean_injected_radiance(),
            [24.0, 6.0, 1.5],
            "the light half"
        );
        assert!(ember.is_emissive());

        let mut hidden = glow_block;
        hidden.emission = None;
        hidden.light = Some([4.0, 4.0, 4.0]);
        assert!(
            hidden.is_emissive(),
            "a light-only row still joins the emitter set"
        );
        assert_eq!(
            hidden.emitted_radiance(),
            [0.0; 3],
            "...but its face shows nothing"
        );
        assert_eq!(hidden.mean_injected_radiance(), [4.0; 3]);
    }

    /// The two emitters are deliberately a contrasting PAIR: one occludes, one
    /// does not. E5's injection rule has to handle both, so if a later edit makes
    /// them alike the test that keeps E5 honest is gone.
    ///
    /// Note what the union made explicit here: the contrast is not two tuned
    /// transmittance numbers, it is two different KINDS that happen to both emit.
    #[test]
    fn the_two_emitters_contrast_in_occlusion() {
        let glow_block = MATERIALS[material_id(Voxel::GlowBlock) as usize];
        let berry = MATERIALS[material_id(Voxel::GlowBerry) as usize];
        assert!(matches!(glow_block.kind, MaterialKind::Solid));
        assert!(matches!(berry.kind, MaterialKind::Cover { .. }));
        assert_eq!(
            glow_block.transmittance(),
            0.0,
            "the glow block must occlude"
        );
        assert!(berry.transmittance() > 0.0, "the berries must not occlude");
        assert!(
            glow_block.emitted_radiance()[0] > berry.emitted_radiance()[0],
            "the glow block is the bright one"
        );
        assert!(
            berry.emitted_radiance()[1] > berry.emitted_radiance()[0],
            "the berries are the cool one"
        );
    }

    // ---- S1: face roles ------------------------------------------------------

    /// The property the whole stage rests on: a row with no authored roles uploads
    /// its base values in EVERY role slot, so the shader's per-face read is the
    /// identity there and turning the lever on cannot change such a row.
    #[test]
    fn a_row_without_face_roles_uploads_its_base_in_every_slot() {
        for row in &MATERIALS {
            if row.face_roles.is_some() {
                continue;
            }
            let gpu = row.to_gpu();
            assert_eq!(gpu.top_albedo, row.albedo, "{} top", row.name);
            assert_eq!(gpu.side_albedo, row.albedo, "{} side", row.name);
            assert_eq!(gpu.bottom_albedo, row.albedo, "{} bottom", row.name);
            assert_eq!(gpu.top_roughness, row.roughness, "{} top", row.name);
            assert_eq!(gpu.side_roughness, row.roughness, "{} side", row.name);
            assert_eq!(gpu.bottom_roughness, row.roughness, "{} bottom", row.name);
            assert!(
                !row.flags().contains(MaterialFlags::FACE_ROLES),
                "{} claims face roles it does not have",
                row.name
            );
        }
    }

    /// The FACE_ROLES flag must be derived, never authored beside the data — the
    /// same rule every other flag follows.
    #[test]
    fn the_face_roles_flag_is_derived() {
        for row in &MATERIALS {
            assert_eq!(
                row.flags().contains(MaterialFlags::FACE_ROLES),
                row.face_roles.is_some(),
                "{} FACE_ROLES disagrees with its roles",
                row.name
            );
        }
        assert_eq!(MaterialFlags::FACE_ROLES.bits(), 16);
        // It must not collide with the pre-S1 bits, which `CorePin` masks on.
        assert_eq!(MaterialFlags::FACE_ROLES.bits() & 0b1111, 0);
    }

    /// **The bit-identity guarantee.** Grass is the only row that authors roles
    /// today, and its BASE values must still be the pre-S1 green — because the base
    /// is what every face reads while the lever is off.
    ///
    /// This is the test that would have caught the mistake made while writing S1:
    /// putting the earth colour in the base and the green in the top role reads
    /// correctly with the feature ON and turns the whole island brown with it OFF.
    #[test]
    fn only_grass_authors_face_roles_today() {
        let authored: Vec<&str> = MATERIALS
            .iter()
            .filter(|row| row.face_roles.is_some())
            .map(|row| row.name)
            .collect();
        assert_eq!(
            authored,
            vec!["grass"],
            "S1 authored roles on unexpected rows"
        );

        let grass = MATERIALS[material_id(Voxel::Grass) as usize];
        // The pre-S1 palette value, which the lever-off path must keep rendering.
        assert_eq!(grass.albedo, [0.41, 0.52, 0.29]);
        assert_eq!(grass.roughness, 0.95);

        let roles = grass.face_roles.expect("grass authors roles");
        // Green on top, earth on the sides — the point of the feature.
        assert_eq!(roles.top.albedo, grass.albedo, "the top is the grass green");
        assert_ne!(roles.side.albedo, grass.albedo, "the sides must differ");
        // The sides are the dirt row's own colour, so a cut bank matches the
        // material below it rather than approximating it.
        assert_eq!(
            roles.side.albedo,
            MATERIALS[material_id(Voxel::Dirt) as usize].albedo
        );
        // The bottom never sees the sky, so it is darker than the sides.
        let brightness = |albedo: [f32; 3]| albedo.iter().sum::<f32>();
        assert!(brightness(roles.bottom.albedo) < brightness(roles.side.albedo));
    }

    /// `face_roles_or_base` must be exactly the base for an unroled row and exactly
    /// the authored roles for a roled one — it is what `to_gpu` uploads.
    #[test]
    fn face_roles_or_base_is_the_identity_without_roles() {
        let stone = MATERIALS[material_id(Voxel::Stone) as usize];
        let roles = stone.face_roles_or_base();
        assert_eq!(roles.top.albedo, stone.albedo);
        assert_eq!(roles.side.albedo, stone.albedo);
        assert_eq!(roles.bottom.albedo, stone.albedo);
        assert_eq!(roles.top, roles.side);
        assert_eq!(roles.side, roles.bottom);

        let grass = MATERIALS[material_id(Voxel::Grass) as usize];
        assert_eq!(grass.face_roles_or_base(), grass.face_roles.unwrap());
    }

    // ---- S2: the pattern stack ---------------------------------------------

    /// Exactly which rows author pattern layers, and the tripwire for one appearing
    /// by accident.
    ///
    /// The list is spelled out rather than counted so that adding a row is a
    /// DECISION with a diff, not a number quietly going up: a layer left behind
    /// from a demo costs every hit that touches the material, and section 11 prices
    /// the cheapest generator's first layer at over a millisecond of entry.
    ///
    /// The three `hdr_*` rows were added as that kind of decision. They are diagnostic
    /// targets for the output-depth toggle (`docs/output-depth.md`), so they cost
    /// nothing unless deliberately placed — but they DO add `wave` to the derived
    /// generator mask, which is the one way a diagnostic row can charge the shipped
    /// build. `wave` was already in the mask via no other row, so that cost is real
    /// and small; a future diagnostic reaching for an unused generator would not be.
    #[test]
    fn the_expected_rows_author_pattern_layers() {
        let authored: Vec<&str> = MATERIALS
            .iter()
            .filter(|row| !row.patterns.is_empty())
            .map(|row| row.name)
            .collect();
        assert_eq!(
            authored,
            vec![
                "lava",
                "slate tile",
                "hdr_red",
                "hdr_green",
                "hdr_blue",
                "hdr_cyan",
                "hdr_magenta",
                "hdr_yellow",
            ]
        );

        let expected_counts = [
            ("lava", 1u32),
            ("slate tile", 4),
            ("hdr_red", 1),
            ("hdr_green", 1),
            ("hdr_blue", 1),
            ("hdr_cyan", 1),
            ("hdr_magenta", 1),
            ("hdr_yellow", 1),
        ];
        for row in &MATERIALS {
            match expected_counts.iter().find(|(name, _)| *name == row.name) {
                Some((_, count)) => {
                    assert!(
                        row.flags().contains(MaterialFlags::PATTERNS),
                        "{}",
                        row.name
                    );
                    assert_eq!(row.flags().pattern_count(), *count, "{}", row.name);
                    for layer in row.patterns.active() {
                        assert_eq!(layer.faces, PatternFaces::ALL, "{}", row.name);
                    }
                }
                None => {
                    assert!(!row.flags().contains(MaterialFlags::PATTERNS));
                    assert_eq!(row.flags().pattern_count(), 0);
                    for slot in row.to_gpu().patterns {
                        assert_eq!(slot, GpuPatternLayer::INACTIVE);
                    }
                }
            }
        }
    }

    /// Every layer of the slate tile shares ONE tessellation.
    ///
    /// This is the invariant the `material.tessellation` node exists to guarantee,
    /// and the table is where it can be checked without a graph: four layers that
    /// disagreed about where the tiles are would draw a tone grid, a grain grid and
    /// a grout grid that do not line up, which looks like a rendering bug rather
    /// than like an authoring mistake.
    #[test]
    fn every_slate_tile_layer_shares_one_tessellation() {
        let row = MATERIALS
            .iter()
            .find(|row| row.name == "slate tile")
            .expect("the slate tile row");
        let layers: Vec<_> = row.patterns.active().collect();
        assert_eq!(layers.len(), 4);
        for layer in &layers {
            assert_eq!(layer.frame, PatternFrame::Tile);
            assert_eq!(layer.tile_aspect, layers[0].tile_aspect);
            assert_eq!(layer.tile_bond, layers[0].tile_bond);
            assert_eq!(layer.tile_gap, layers[0].tile_gap);
            assert_eq!(layer.period_meters, layers[0].period_meters);
        }
        // And the joint is drawn LAST, or the grain would run through it.
        assert!(matches!(
            layers[3].generator,
            PatternGenerator::TileEdge { .. }
        ));
    }

    /// The PATTERNS flag and the layer count must both be derived from the stack, so
    /// a row cannot claim patterns it does not have or hide ones it does.
    #[test]
    fn the_pattern_flag_and_count_are_derived() {
        let mut row = MATERIALS[material_id(Voxel::Stone) as usize];
        assert!(!row.flags().contains(MaterialFlags::PATTERNS));

        for expected in 1..=MAX_PATTERN_LAYERS {
            row.patterns
                .push(crate::pattern::PatternLayer::IDENTITY)
                .ok_or(())
                .expect_err("the stack accepted a layer");
            assert!(row.flags().contains(MaterialFlags::PATTERNS));
            assert_eq!(row.flags().pattern_count() as usize, expected);
            let uploaded = row.to_gpu();
            // Active slots carry the layer; the tail stays inactive.
            for (slot, layer) in uploaded.patterns.iter().enumerate() {
                if slot < expected {
                    assert_ne!(*layer, GpuPatternLayer::INACTIVE, "slot {slot} is inactive");
                } else {
                    assert_eq!(*layer, GpuPatternLayer::INACTIVE, "slot {slot} is live");
                }
            }
        }
    }

    /// The count must not collide with any flag bit, in either direction — the two
    /// share a word, and an overlap would make a four-layer row claim a flag it
    /// never authored.
    #[test]
    fn the_pattern_count_does_not_collide_with_the_flags() {
        const EVERY_FLAG: u32 = MaterialFlags::FOLIAGE.bits()
            | MaterialFlags::EMISSIVE.bits()
            | MaterialFlags::TRANSPARENT.bits()
            | MaterialFlags::LIQUID.bits()
            | MaterialFlags::FACE_ROLES.bits()
            | MaterialFlags::PATTERNS.bits();
        assert_eq!(MaterialFlags::PATTERNS.bits(), 32);
        const COUNT_FIELD: u32 = PATTERN_COUNT_MASK << PATTERN_COUNT_SHIFT;
        assert_eq!(EVERY_FLAG & COUNT_FIELD, 0);
        // The field must hold every count the stack can produce.
        assert!(MAX_PATTERN_LAYERS as u32 <= PATTERN_COUNT_MASK);

        // Writing a count must leave the flags alone, and setting flags must leave a
        // written count alone.
        let flagged = MaterialFlags::LIQUID
            .union(MaterialFlags::PATTERNS)
            .with_pattern_count(3);
        assert!(flagged.contains(MaterialFlags::LIQUID));
        assert!(flagged.contains(MaterialFlags::PATTERNS));
        assert_eq!(flagged.pattern_count(), 3);
        assert_eq!(flagged.union(MaterialFlags::FOLIAGE).pattern_count(), 3);
        // Re-writing the count must replace it, not OR into it.
        assert_eq!(flagged.with_pattern_count(1).pattern_count(), 1);
        assert_eq!(flagged.with_pattern_count(0).pattern_count(), 0);
    }

    /// The pre-S1 pin masks the flag word to `0b1111`, and that mask has to keep
    /// excluding every bit added since — otherwise the pin starts failing for a row
    /// whose *uploaded* pre-S1 fields never moved.
    #[test]
    fn the_upload_pins_flag_mask_still_excludes_the_new_bits() {
        const PRE_S1_FLAGS: u32 = 0b1111;
        assert_eq!(MaterialFlags::FACE_ROLES.bits() & PRE_S1_FLAGS, 0);
        assert_eq!(MaterialFlags::PATTERNS.bits() & PRE_S1_FLAGS, 0);
        assert_eq!(
            (PATTERN_COUNT_MASK << PATTERN_COUNT_SHIFT) & PRE_S1_FLAGS,
            0
        );
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
