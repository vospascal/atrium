//! E6 — water optics: the settings mirror, the shader patching, and the physics
//! constants **as pure math**, so every number the shader uses can be checked on
//! the CPU against a hand computation.
//!
//! Pure data + math: no wgpu, no windowing (plan architecture rule). The
//! compile-time levers ([`WaterMode`], [`WaterSettings::bounces`]) are consts in
//! `shaders/water.wgsl` and change the pipeline; the two runtime knobs
//! (`extinction_scale`, `scatter_strength`) ride in the lighting uniform
//! ([`crate::lighting::WaterParams`]) and need no rebuild — exactly the split
//! E1c measured and [`crate::variants::REGISTRY`] records.
//!
//! ## The model, in one place
//!
//! A water voxel is not a surface with a colour, it is the boundary of a
//! *medium*. Three terms, each with its own function below:
//!
//! 1. **Fresnel** ([`fresnel_schlick`]) decides how much of the incoming ray
//!    mirrors and how much enters, from `F0` alone — and `F0` is not a taste
//!    constant, it is [`fresnel_f0`], the normal-incidence reflectance derived
//!    from the two indices of refraction. Grazing angles mirror (F -> 1), steep
//!    angles see through (F -> 2%).
//! 2. **Snell** ([`refract_direction`]) bends the transmitted ray, and *fails*
//!    past the critical angle ([`critical_angle_degrees`], 48.6 deg for water)
//!    — which is the whole of Snell's window: from below, the sky is squeezed
//!    into a 97-degree cone and everything outside it is a mirror.
//! 3. **Beer-Lambert** ([`transmittance`]) removes light along the path
//!    *travelled inside the water*, per channel, so depth reads as colour — and
//!    the medium's own colour is **derived** from the material's
//!    absorption/scattering pair ([`single_scattering_albedo`]) rather than
//!    painted. Nothing here chooses what colour water is.
//!
//! The one thing that is NOT physics here is [`WaterSettings::bounces`]: E1
//! measured a marginal full-res secondary ray at 2.25-3.55 ms, so how many water
//! interfaces a ray may cross is a budget, and the registry row carries the
//! measured verdict.

use glam::{Vec2, Vec3};

use crate::brickmap::Brickmap;
use crate::shader_consts::{ShaderConstSink, ShaderConstValue, SourcePatcher};
use voxel_core::wind::WindFrame;
use voxel_core::world::{VOXEL_SIZE, WORLD_VOXEL_SIZE_METERS};
use voxel_material::animation_clock::AnimationClockSample;
use voxel_material::material::{
    material_is_liquid, AIR_INDEX_OF_REFRACTION, WATER_ABSORPTION_PER_METER,
    WATER_SCATTERING_PER_METER,
};

/// Per-channel **extinction** of water, per metre: `absorption + scattering` from
/// the material table's authored pair. Kept as a named constant because it is the
/// number every recorded transmittance in the bench doc is computed from —
/// `(0.450, 0.120, 0.060)`, so transmittance at the debug pool's 5 m is
/// `(0.105, 0.549, 0.741)`: red nearly gone, blue mostly intact.
pub fn water_extinction_per_meter() -> [f32; 3] {
    [
        WATER_ABSORPTION_PER_METER[0] + WATER_SCATTERING_PER_METER[0],
        WATER_ABSORPTION_PER_METER[1] + WATER_SCATTERING_PER_METER[1],
        WATER_ABSORPTION_PER_METER[2] + WATER_SCATTERING_PER_METER[2],
    ]
}

/// The medium's apparent colour, **derived** from the coefficient pair rather than
/// authored: `scattering / extinction` per channel — the share of the light leaving
/// a ray that is redirected rather than destroyed.
///
/// For water this is ~`(0.009, 0.250, 0.750)`: deeply blue with almost no red,
/// purely because red is absorbed ~30x faster than blue while blue scatters ~11x
/// more than red. This is the quantity that replaced the water row's diffuse
/// ALBEDO in the in-scatter term (Pascal, 2026-07-31: *"water shouldn't have a
/// colour really"* — albedo is surface reflectance, and using it as a volume colour
/// is paint).
pub fn single_scattering_albedo() -> [f32; 3] {
    let extinction = water_extinction_per_meter();
    let mut albedo = [0.0_f32; 3];
    for channel in 0..3 {
        if extinction[channel] > 0.0 {
            albedo[channel] = WATER_SCATTERING_PER_METER[channel] / extinction[channel];
        }
    }
    albedo
}

/// The shipped share of turbidity's extinction that **scatters** rather than absorbs — the
/// default of [`WaterSettings::turbidity_scattering_fraction`], which is a runtime lever
/// (`water_params.w`) rather than a constant.
///
/// **It is a choice of WHAT is suspended, which is why it is a dial and not a derivation.**
/// Mineral silt is much larger than the wavelength, so it scatters broadband and absorbs
/// little: a silty river genuinely is milky-bright, and 0.85 renders exactly that. What
/// limits visibility in most standing water is instead dissolved organic matter and
/// phytoplankton, which ABSORB — a pond you cannot see the bottom of is dark, not white.
///
/// Measured, and the reason the first E7 build looked like milk (Pascal: *"now it looks al
/// hazy and white"*): at 0.85 ONE block of water in-scatters 0.38-0.47 of the sky's radiance,
/// so even shallow water reads as a white sheet. At 0.15 that is 0.07-0.11.
pub const TURBIDITY_SCATTERING_FRACTION: f32 = 0.15;

/// How much light must be left for a submerged surface to still be *made out*: the
/// threshold that defines what "visibility depth" means.
///
/// 0.10 — a tenth of the light. Chosen, and it has to be stated somewhere, because
/// "how deep can you see" is not a physical quantity until you say how faint counts as gone.
pub const VISIBILITY_THRESHOLD: f32 = 0.10;

/// Per-metre turbidity that puts the visibility horizon at `depth_blocks` blocks.
///
/// **Turbidity alone reaches [`VISIBILITY_THRESHOLD`] at that depth**, so
/// `exp(-turbidity * depth) = 0.10`. Defined on the suspended matter alone, independent of
/// which liquid it is suspended in, which is what keeps it meaningful for oil and honey rows
/// too; the medium's own spectral coefficients then only make the bed fade sooner than the
/// stated depth, never later.
///
/// A block is a WORLD voxel ([`voxel_core::world::WORLD_VOXEL_SIZE_METERS`] = 1 m), not a
/// detail cell — the unit Pascal asked in ("not more than 3 blocks deep"), and 8x coarser
/// than the 0.125 m cell the DDA marches.
pub fn turbidity_per_meter(depth_blocks: f32) -> f32 {
    if depth_blocks <= 0.0 {
        return 0.0;
    }
    -VISIBILITY_THRESHOLD.ln() / (depth_blocks * WORLD_VOXEL_SIZE_METERS)
}

/// The medium's per-channel **absorption and scattering** under a given configuration, per
/// metre — the CPU mirror of `water_absorption_per_meter` / `water_scattering_per_meter` in
/// `shaders/water.wgsl`, and the pair every measured transmittance in this module's tests is
/// computed from.
///
/// Both terms carry the material's own coefficients scaled by their runtime dials, PLUS
/// turbidity split by [`WaterSettings::turbidity_scattering_fraction`]. Turbidity sits outside
/// the two scales on purpose: those dials say how clear this substance is, and how much is
/// suspended in it is a different question.
pub fn coefficients_per_meter(settings: &WaterSettings) -> ([f32; 3], [f32; 3]) {
    let turbidity = turbidity_per_meter(settings.visibility_depth_blocks);
    let scattering_fraction = settings.turbidity_scattering_fraction.clamp(0.0, 1.0);
    let mut absorption = [0.0_f32; 3];
    let mut scattering = [0.0_f32; 3];
    for channel in 0..3 {
        absorption[channel] = WATER_ABSORPTION_PER_METER[channel]
            * settings.absorption_scale.max(0.0)
            + turbidity * (1.0 - scattering_fraction);
        scattering[channel] = WATER_SCATTERING_PER_METER[channel]
            * settings.scattering_scale.max(0.0)
            + turbidity * scattering_fraction;
    }
    (absorption, scattering)
}

/// Total per-channel extinction under a given configuration: `absorption + scattering`.
pub fn extinction_per_meter(settings: &WaterSettings) -> [f32; 3] {
    let (absorption, scattering) = coefficients_per_meter(settings);
    [
        absorption[0] + scattering[0],
        absorption[1] + scattering[1],
        absorption[2] + scattering[2],
    ]
}

/// The medium's apparent colour under a given configuration: `scattering / extinction` per
/// channel, the quantity the in-scatter term actually uses.
pub fn albedo_of(settings: &WaterSettings) -> [f32; 3] {
    let (_, scattering) = coefficients_per_meter(settings);
    let extinction = extinction_per_meter(settings);
    let mut albedo = [0.0_f32; 3];
    for channel in 0..3 {
        if extinction[channel] > 0.0 {
            albedo[channel] = scattering[channel] / extinction[channel];
        }
    }
    albedo
}

/// Fraction of light surviving `depth_blocks` of this medium, per channel.
pub fn transmittance_over(settings: &WaterSettings, depth_blocks: f32) -> [f32; 3] {
    let extinction = extinction_per_meter(settings);
    let meters = depth_blocks.max(0.0) * WORLD_VOXEL_SIZE_METERS;
    [
        (-extinction[0] * meters).exp(),
        (-extinction[1] * meters).exp(),
        (-extinction[2] * meters).exp(),
    ]
}

/// Ceiling on the caustic focus gain — mirrors `WATER_CAUSTIC_MAX_GAIN` in
/// `shaders/water.wgsl`.
///
/// A cap is not optional: at a true focus `det J → 0` and `1/|det J|` diverges. Real
/// caustics are bounded by the sun's 0.53° angular size smearing the focus and by
/// wavelength; modelling either is a caustic-map arc of its own, so this is the honest
/// cheap stand-in and a stated look bound.
pub const CAUSTIC_MAX_GAIN: f32 = 4.0;

/// Which water optics to compile — mirrors `WATER_MODE` in `shaders/water.wgsl`.
///
/// The variants are ordered by cost, and each isolates one half of the model so
/// the A/B can attribute milliseconds (bench doc, E6 section).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaterMode {
    /// Water is an opaque diffuse surface — the pre-E6 renderer, bit for bit.
    Opaque,
    /// **Zero secondary rays.** The reflection is the analytic sky function
    /// evaluated in the mirror direction and the transmission is the surface's
    /// own diffuse shading, mixed by Fresnel. Extinction still applies to the
    /// underwater view, whose primary ray has to march the medium regardless.
    FresnelTint,
    /// Reflection traced, transmission left as the diffuse surface — the row
    /// that prices the reflection ray alone.
    Reflection,
    /// Transmission traced (Snell + Beer-Lambert through the medium),
    /// reflection left as the analytic sky — the row that prices the refraction
    /// march alone.
    Refraction,
    /// Both: Fresnel-weighted reflection ray + refracted march. The shipped
    /// model.
    Full,
}

impl WaterMode {
    /// The `WATER_MODE` u32 this configuration compiles to — the one place the
    /// Rust<->WGSL numbering lives.
    pub(crate) fn shader_value(self) -> u32 {
        match self {
            WaterMode::Opaque => 0,
            WaterMode::FresnelTint => 1,
            WaterMode::Reflection => 2,
            WaterMode::Refraction => 3,
            WaterMode::Full => 4,
        }
    }

    /// Inverse of [`WaterMode::shader_value`]; panics on a value the shader has
    /// no branch for.
    pub(crate) fn from_shader_value(shader_value: u32) -> WaterMode {
        match shader_value {
            0 => WaterMode::Opaque,
            1 => WaterMode::FresnelTint,
            2 => WaterMode::Reflection,
            3 => WaterMode::Refraction,
            4 => WaterMode::Full,
            other => panic!("no WATER_MODE {other} in water.wgsl"),
        }
    }

    /// Whether this mode traces a mirror ray from the surface.
    pub fn traces_reflection(self) -> bool {
        matches!(self, WaterMode::Reflection | WaterMode::Full)
    }

    /// Whether this mode traces the refracted ray through the medium.
    pub fn traces_refraction(self) -> bool {
        matches!(self, WaterMode::Refraction | WaterMode::Full)
    }
}

/// What the region OUTSIDE Snell's window gets once the full-shading bounce budget
/// is spent — mirrors `WATER_TIR_FALLBACK` in `shaders/water.wgsl`.
///
/// This is the E6 look-gate failure and its fix. Past the critical angle a ray
/// leaving the water is totally internally reflected, so it needs *something*
/// mirrored back down; with one interface of budget there was nothing left to
/// trace, and the whole region became a flat constant.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaterTirFallback {
    /// The in-scatter constant. **Documented negative** — the region is one flat
    /// colour, which is most of the screen when looking up (the window is only a
    /// ~97-degree cone).
    Flat,
    /// One more medium march, shaded cheaply (albedo x downwelling, no shadow ray,
    /// no AO, no light volume). Real geometry — the bed and the pool walls,
    /// mirrored — for a fraction of a full bounce.
    CheapMirror,
}

impl WaterTirFallback {
    /// The `WATER_TIR_FALLBACK` u32 this choice compiles to.
    pub(crate) fn shader_value(self) -> u32 {
        match self {
            WaterTirFallback::Flat => 0,
            WaterTirFallback::CheapMirror => 1,
        }
    }

    /// Inverse of [`WaterTirFallback::shader_value`]; panics on a value the shader
    /// has no branch for.
    pub(crate) fn from_shader_value(shader_value: u32) -> WaterTirFallback {
        match shader_value {
            0 => WaterTirFallback::Flat,
            1 => WaterTirFallback::CheapMirror,
            other => panic!("no WATER_TIR_FALLBACK {other} in water.wgsl"),
        }
    }
}

/// What the surface interface does for a ray reaching it from BELOW — mirrors
/// `WATER_UNDERWATER_INTERFACE` in `shaders/water.wgsl`.
///
/// Pascal, 2026-07-31: *"lets disable the fresnel like camera looking up out of
/// water for now should be just transparent looking out and in .. only top should
/// have the reflection"*.
///
/// **Why "just transparent" must mean UNBENT.** Total internal reflection is not a
/// separable effect that can be switched off on its own — it *is* what Snell's law
/// yields when `sin(theta_transmitted) > 1`. Past the critical angle
/// ([`critical_angle_degrees`], 48.607 deg for water) there is no transmitted
/// direction at all, so a variant that kept the bend and dropped the mirror would
/// have nothing to draw beyond the window. Dropping the bend removes the critical
/// angle with it and the interface becomes a plain window, which is the only
/// coherent reading of the request.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WaterUnderwaterInterface {
    /// The physical interface: Snell's bend, a Fresnel-weighted split, and total
    /// internal reflection past the critical angle (whose mirrored region
    /// [`WaterTirFallback`] then fills). What E6 shipped through step 1.
    Fresnel,
    /// Fully transmissive and unbent: the ray continues straight through, with only
    /// the absorption and scattering along its path applied. Snell's window
    /// disappears from below and the surface becomes invisible from underneath —
    /// both accepted deliberately.
    Transparent,
}

impl WaterUnderwaterInterface {
    /// The `WATER_UNDERWATER_INTERFACE` u32 this choice compiles to.
    pub(crate) fn shader_value(self) -> u32 {
        match self {
            WaterUnderwaterInterface::Fresnel => 0,
            WaterUnderwaterInterface::Transparent => 1,
        }
    }

    /// Inverse of [`WaterUnderwaterInterface::shader_value`]; panics on a value the
    /// shader has no branch for.
    pub(crate) fn from_shader_value(shader_value: u32) -> WaterUnderwaterInterface {
        match shader_value {
            0 => WaterUnderwaterInterface::Fresnel,
            1 => WaterUnderwaterInterface::Transparent,
            other => panic!("no WATER_UNDERWATER_INTERFACE {other} in water.wgsl"),
        }
    }
}

/// How a ray leaves the medium at the surface, under a given interface mode — the
/// CPU mirror of the shader's underwater branch, so the behaviour is testable.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct UnderwaterExit {
    /// The direction the ray continues along after the interface.
    pub direction: Vec3,
    /// Whether it was mirrored back into the medium (total internal reflection)
    /// rather than escaping. Always false under
    /// [`WaterUnderwaterInterface::Transparent`].
    pub mirrored: bool,
}

/// Where a ray goes when it reaches the surface from inside the medium.
///
/// `normal` must point back into the medium (the convention `hit_normal` and the
/// medium march both produce). Under [`WaterUnderwaterInterface::Transparent`] this
/// is the identity — the whole point of the mode — and under
/// [`WaterUnderwaterInterface::Fresnel`] it is Snell's law, mirroring past the
/// critical angle.
pub fn underwater_exit(
    interface: WaterUnderwaterInterface,
    incident: Vec3,
    normal: Vec3,
    index_of_refraction: f32,
) -> UnderwaterExit {
    match interface {
        WaterUnderwaterInterface::Transparent => UnderwaterExit {
            direction: incident,
            mirrored: false,
        },
        WaterUnderwaterInterface::Fresnel => {
            let eta = index_of_refraction / AIR_INDEX_OF_REFRACTION;
            match refract_direction(incident, normal, eta) {
                Some(direction) => UnderwaterExit {
                    direction,
                    mirrored: false,
                },
                None => UnderwaterExit {
                    // The mirror WGSL's `reflect(I, N)` produces.
                    direction: incident - normal * (2.0 * incident.dot(normal)),
                    mirrored: true,
                },
            }
        }
    }
}

/// User-facing water configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaterSettings {
    /// Which optics to compile (`WATER_MODE`).
    pub mode: WaterMode,
    /// How many water interfaces one camera ray may cross (`WATER_BOUNCES`).
    /// 1 = the surface split plus one march through the body; 2 lets the march
    /// bounce once more (a total-internal-reflection mirror from below, or the
    /// far wall of a pool). The recursion budget, not a physical constant.
    pub bounces: u32,
    /// What the region outside Snell's window gets once that budget is spent
    /// (`WATER_TIR_FALLBACK`) — the E6 look-gate fix. **Inert** under
    /// [`WaterUnderwaterInterface::Transparent`], which has no such region.
    pub tir_fallback: WaterTirFallback,
    /// What the surface interface does from BELOW
    /// (`WATER_UNDERWATER_INTERFACE`). The shipped default is
    /// [`WaterUnderwaterInterface::Transparent`]; it also makes
    /// [`Self::bounces`] and [`Self::tir_fallback`] inert from below.
    pub underwater_interface: WaterUnderwaterInterface,
    /// Runtime multiplier on the medium's per-metre **absorption**
    /// (`lighting.water_params.x`) — light the water destroys. The
    /// clarity/darkening axis; 1.0 = the authored coefficients.
    pub absorption_scale: f32,
    /// Runtime multiplier on the medium's per-metre **scattering**
    /// (`lighting.water_params.y`) — light the water redirects, and therefore the
    /// light a ray picks up along its path. The brightness axis, and half of what
    /// the medium's colour is derived from. 0 makes the model absorption-only and
    /// the depths go black.
    pub scattering_scale: f32,
    /// Runtime smallest Fresnel weight worth a SECONDARY RAY
    /// (`lighting.water_params.z`). Below it the cheap analytic stand-in is
    /// substituted for that half of the pixel — the analytic sky for the mirror,
    /// the diffuse surface for the transmission — so a head-on water pixel does not
    /// pay a full traced reflection for 2% of its colour. 0 = always trace.
    pub ray_cutoff: f32,
    /// Whether the SUN's rays pass through liquids instead of stopping on them
    /// (`WATER_SUN_THROUGH_LIQUID`). Off, every submerged surface is in shadow and
    /// shallow water reads darker than the opaque water it replaced; on, a sunlit
    /// pool bed is sunlit — and it is the most expensive thing in E6, because a
    /// shadow ray that does not stop at the surface walks the whole body voxel by
    /// voxel. The registry row carries the measured per-tier verdict.
    pub sun_through_liquid: bool,
    /// Whether the surface carries the W1 wave field at all (`WATER_WAVES`).
    ///
    /// A COMPILE-TIME lever, so `false` folds the whole field away and the surface is
    /// the flat voxel face the pre-wave renderer shaded — bit for bit, which is the
    /// isolation rule's requirement and the bench's no-regression anchor.
    pub waves: bool,
    /// Runtime wave amplitude (`lighting.water_optics.z`), 0..1. `1.0` is the shipped
    /// look and `0.0` is exactly flat water without a pipeline rebuild, which is what
    /// makes it draggable.
    pub wave_amplitude_scale: f32,
    /// E7 — how deep you can see, in BLOCKS: the visibility horizon that
    /// [`turbidity_per_meter`] turns into `lighting.water_optics.w`.
    ///
    /// The dial in the unit the look is specified in ("not more than 3 blocks deep, and it
    /// should fade the deeper you go"). Larger = clearer water and a further horizon; 0
    /// means no turbidity at all, i.e. exactly the pure-water model, which is the isolation
    /// anchor rather than a useful setting.
    pub visibility_depth_blocks: f32,
    /// E7 — whether the sun focused by the surface's curvature reaches submerged surfaces
    /// (`WATER_CAUSTICS`).
    ///
    /// A COMPILE-TIME lever, and cheap because it rides on a march
    /// `WATER_SUN_THROUGH_LIQUID` already pays for — see [`WaveField::caustic_gain`]. Off,
    /// the sun term comes through untouched, bit for bit.
    pub caustics: bool,
    /// E7 — whether terrain catches the sun's specular reflection off nearby water
    /// (`WATER_BOUNCE_LIGHT`).
    ///
    /// A COMPILE-TIME lever, and the one E7 term that spends a RAY per shaded surface, so
    /// the registry row carries the bench point that prices it.
    pub bounce_light: bool,
    /// E7 — the MILKINESS dial (`lighting.water_params.w`): what share of turbidity's
    /// extinction scatters rather than absorbs. See [`TURBIDITY_SCATTERING_FRACTION`] for
    /// why this is a choice rather than a derived number, and for the measurement that set
    /// the default.
    ///
    /// Runtime, so it can be dragged against a real pool — which is the only way to settle
    /// how milky the water should look.
    pub turbidity_scattering_fraction: f32,
}

impl Default for WaterSettings {
    /// The shipped configuration, matching the lever defaults in
    /// `shaders/water.wgsl` (pinned by `default_settings_match_shader_source`).
    ///
    /// **The five runtime numbers here were DIALLED IN THE APP, not derived** (Pascal,
    /// 2026-08-06, against a real pool: *"this should be default"*). They are recorded as one
    /// coherent look rather than five independent choices, because they trade against each
    /// other — see `the_shipped_look_is_the_one_that_was_dialled_in` for what each does and
    /// what the combination measures.
    fn default() -> WaterSettings {
        WaterSettings {
            mode: WaterMode::Full,
            bounces: 1,
            tir_fallback: WaterTirFallback::CheapMirror,
            underwater_interface: WaterUnderwaterInterface::Transparent,
            // Water's OWN absorption off, so the medium's extinction is turbidity's almost
            // entirely. This is the deliberate part of the look: pure water's absorption is
            // what makes deep water steeply blue (it eats red 30x faster than blue), and
            // turning it off leaves a near-NEUTRAL medium that darkens without colouring.
            absorption_scale: 0.0,
            // And its scattering damped to match, so the two stay in proportion.
            scattering_scale: 0.15,
            // Always trace both secondary rays. The cheap analytic stand-ins are visible on a
            // surface being judged this closely — see the lever's verdict for what it costs.
            ray_cutoff: 0.0,
            sun_through_liquid: true,
            waves: true,
            // Calm. Cox & Munk describes open water; a courtyard pool is nearly still, and
            // the wave field at full amplitude reads as chop at this scale.
            wave_amplitude_scale: 0.21,
            // 10 blocks, not the 3 the first E7 build shipped. With the fade finally VISIBLE
            // it turned out to want to be much further out: 3 blocks hid the bed almost
            // immediately, and what was wanted was water you can see into but not through.
            visibility_depth_blocks: 10.0,
            caustics: true,
            bounce_light: true,
            turbidity_scattering_fraction: TURBIDITY_SCATTERING_FRACTION,
        }
    }
}

impl WaterSettings {
    /// `shader_source` with this configuration's compile-time consts patched in.
    /// Identity for the default settings.
    ///
    /// Handles BOTH shader sources: the water lever block itself lives in
    /// `water.wgsl` and therefore only in the shading pass, while
    /// `LIQUIDS_CAST_NO_SHADOW` lives in the shared `world.wgsl` and must move in
    /// the CA pass too — a liquid that stops the shading pass's sun ray but not the
    /// light volume's (or the reverse) would light the bed under water in one and
    /// not the other.
    pub(crate) fn declare_consts(&self, sink: &mut dyn ShaderConstSink) {
        // Lives in the shared `world.wgsl`, so it must move in BOTH passes.
        sink.boolean("WATER_SUN_THROUGH_LIQUID", self.sun_through_liquid);
        // The rest live in `water.wgsl`, which the CA pass does not include — hence
        // `set_if_present` rather than `set`. Absence there is correct, and keeping it a
        // distinct call is what lets a genuinely renamed const still panic.
        sink.set_if_present(
            "WATER_MODE",
            ShaderConstValue::Unsigned(self.mode.shader_value()),
        );
        sink.set_if_present("WATER_BOUNCES", ShaderConstValue::Unsigned(self.bounces));
        sink.set_if_present(
            "WATER_TIR_FALLBACK",
            ShaderConstValue::Unsigned(self.tir_fallback.shader_value()),
        );
        sink.set_if_present(
            "WATER_UNDERWATER_INTERFACE",
            ShaderConstValue::Unsigned(self.underwater_interface.shader_value()),
        );
        sink.set_if_present("WATER_WAVES", ShaderConstValue::Boolean(self.waves));
        // E7. Caustics live in `water.wgsl` (absent from the CA pass, like the rest of the
        // optics); the bounce light lives in `dda.wgsl`, which the CA pass also does not
        // include — it shades nothing.
        sink.set_if_present("WATER_CAUSTICS", ShaderConstValue::Boolean(self.caustics));
        sink.set_if_present(
            "WATER_BOUNCE_LIGHT",
            ShaderConstValue::Boolean(self.bounce_light),
        );
    }

    pub fn patch_shader_source(&self, shader_source: &str) -> String {
        let mut patcher = SourcePatcher::new(shader_source);
        self.declare_consts(&mut patcher);
        patcher.finish()
    }

    /// Whether `bounces` and `tir_fallback` do anything at all. They describe what
    /// happens after a ray MIRRORS off the underside of the surface, and the shipped
    /// transparent interface never mirrors — so from below both are inert, and the
    /// overlay greys them out rather than offering dead dials.
    pub fn bounce_levers_have_an_effect(&self) -> bool {
        self.underwater_interface == WaterUnderwaterInterface::Fresnel
    }

    /// Whether switching from `applied` to `self` changes a compile-time const —
    /// i.e. everything except the two runtime uniform fields.
    pub fn requires_pipeline_rebuild(&self, applied: &WaterSettings) -> bool {
        self.mode != applied.mode
            || self.bounces != applied.bounces
            || self.tir_fallback != applied.tir_fallback
            || self.underwater_interface != applied.underwater_interface
            || self.sun_through_liquid != applied.sun_through_liquid
            || self.waves != applied.waves
    }
}

// ---- The physics -------------------------------------------------------------

/// Normal-incidence reflectance of an air/medium boundary — the `F0` Schlick's
/// approximation needs, derived from the medium's own
/// [`voxel_material::material::Material::index_of_refraction`] rather than tuned:
/// `((n1 - n2) / (n1 + n2))^2`. For water's 1.333 that is **0.0204**, i.e. water
/// seen straight down reflects 2% and transmits 98%.
pub fn fresnel_f0(index_of_refraction: f32) -> f32 {
    let ratio = (index_of_refraction - AIR_INDEX_OF_REFRACTION)
        / (index_of_refraction + AIR_INDEX_OF_REFRACTION);
    ratio * ratio
}

/// Schlick's approximation of the Fresnel reflectance at an air/medium boundary,
/// from the cosine of the angle between the ray and the surface normal.
///
/// `cos_incidence = 1` (straight on) gives [`fresnel_f0`]; `cos_incidence = 0`
/// (grazing) gives 1.0 — the mirror-at-grazing-angles behaviour the E6 gate asks
/// for. Clamped, so a caller passing a slightly negative cosine from float error
/// cannot produce a reflectance above 1.
pub fn fresnel_schlick(cos_incidence: f32, index_of_refraction: f32) -> f32 {
    let cosine = cos_incidence.clamp(0.0, 1.0);
    let f0 = fresnel_f0(index_of_refraction);
    f0 + (1.0 - f0) * (1.0 - cosine).powi(5)
}

/// The refracted direction, or `None` for total internal reflection.
///
/// `normal` must point toward the side the incident ray comes FROM (the same
/// convention the DDA's `hit_normal` produces: it opposes the ray). `eta` is the
/// ratio `index_from / index_to` — 0.75 entering water, 1.333 leaving it.
///
/// This is the vector form of Snell's law; `None` is the case
/// `sin(theta_transmitted) > 1`, which can only happen going from the denser
/// medium to the thinner one, i.e. looking up from underwater past the critical
/// angle.
pub fn refract_direction(incident: Vec3, normal: Vec3, eta: f32) -> Option<Vec3> {
    let cos_incidence = -incident.dot(normal);
    let sin_squared_transmitted = eta * eta * (1.0 - cos_incidence * cos_incidence);
    if sin_squared_transmitted > 1.0 {
        return None;
    }
    let cos_transmitted = (1.0 - sin_squared_transmitted).sqrt();
    Some(incident * eta + normal * (eta * cos_incidence - cos_transmitted))
}

/// Critical angle of a medium/air boundary, degrees from the surface normal:
/// `asin(n_air / n_medium)`. Past this, a ray leaving the medium is totally
/// internally reflected — the edge of Snell's window, 48.6 deg for water's 1.333.
pub fn critical_angle_degrees(index_of_refraction: f32) -> f32 {
    (AIR_INDEX_OF_REFRACTION / index_of_refraction)
        .asin()
        .to_degrees()
}

/// Beer-Lambert transmittance of `distance_meters` of water, per channel:
/// `exp(-(absorption * absorption_scale + scattering * scattering_scale) * distance)`.
///
/// Strictly decreasing in distance and never negative, which is the property the
/// shading composition relies on (the share taken out of the ray is
/// `1 - transmittance` and must stay a valid weight).
pub fn transmittance(
    distance_meters: f32,
    absorption_scale: f32,
    scattering_scale: f32,
) -> [f32; 3] {
    let distance = distance_meters.max(0.0);
    let mut result = [0.0_f32; 3];
    for channel in 0..3 {
        let extinction = WATER_ABSORPTION_PER_METER[channel] * absorption_scale.max(0.0)
            + WATER_SCATTERING_PER_METER[channel] * scattering_scale.max(0.0);
        result[channel] = (-extinction * distance).exp();
    }
    result
}

// ---- W1: the wave field ------------------------------------------------------
//
// The water surface used to be a perfectly flat axis-aligned voxel face, which made
// its Fresnel mirror a PERFECT mirror: no sun glitter, and a reflected shoreline that
// does not move. This section is the height field whose gradient replaces that flat
// normal, and `docs/water-waves-plan.md` carries the argument. Two properties matter
// for everything downstream:
//
//   * The normal is the ANALYTIC gradient of the height field, not a finite
//     difference of it — one evaluation instead of three, and exact. (The
//     finite difference is still computed, in a test, as the check.)
//   * With no wind it is EXACTLY `Vec3::Y` and no trigonometry runs, so the
//     lever-off path is bit-identical to the pre-wave renderer. That is the
//     isolation rule, and `flat_water_is_exactly_flat` pins it.

/// Standard gravity, m/s², for the deep-water dispersion relation `ω = sqrt(g·k)`.
///
/// **Deliberately not [`crate::character::GRAVITY_METERS_PER_SECOND_SQUARED`]**, which
/// is 22.0: that one is tuned game feel (a platformer's fall rate, chosen so a jump
/// arc reads well), and sharing it because both are spelled "gravity" would make every
/// wave travel 1.5x too fast. Dispersion is physics and takes the physical value.
pub const WAVE_GRAVITY_METERS_PER_SECOND_SQUARED: f32 = 9.806_65;

/// How many directional wave components the sum carries.
///
/// Four is where the interference stops reading as a repeating pattern; the cost is
/// one `sin` and one `cos` each, so this is a look decision rather than a budget.
pub const WAVE_COMPONENTS: usize = 4;

/// Longest and shortest wavelength in the band, metres. The components are spaced
/// **geometrically** between them (equal steps in `log k`).
///
/// These are CHOSEN, and the honest reason is in the plan doc: the fully-developed
/// Pierson-Moskowitz peak is `λ ≈ 0.88·U²` metres — 22 m at 5 m/s of wind — which is
/// right for open ocean and absurd for every body of water we have. What actually
/// limits a pond is **fetch**, not wind speed, and we do not model fetch. A band sized
/// to the voxel scale with this paragraph attached beats a spectrum we would be
/// pretending to evaluate.
pub const WAVE_LONGEST_METERS: f32 = 6.0;
/// See [`WAVE_LONGEST_METERS`].
pub const WAVE_SHORTEST_METERS: f32 = 0.6;

/// Cox & Munk's mean-square surface slope: `sigma^2 = 0.003 + 5.12e-3 * U`, with the wind
/// speed `U` in m/s. **This is what sets how rough the water is, and it is a measurement
/// rather than a choice.**
///
/// Cox & Munk (1954) obtained it by photographing **sun glitter** from an aircraft and
/// inverting the width of the glitter pattern — so it is calibrated against exactly the
/// phenomenon a wave normal exists to produce. At 5 m/s it gives `sigma = 0.169`, a 9.6°
/// RMS slope.
///
/// It replaced an arbitrary mapping (a fixed fraction of the breaking limit times the
/// wind's `activity`) that measured out at **2.4° RMS — four times too flat** — which is
/// why the first build had no visible shimmer. It also ignored `WindFrame::speed`, the one
/// physical quantity this relation needs.
pub const WAVE_SLOPE_VARIANCE_INTERCEPT: f32 = 0.003;
/// See [`WAVE_SLOPE_VARIANCE_INTERCEPT`]. Per m/s of wind speed.
pub const WAVE_SLOPE_VARIANCE_PER_METER_PER_SECOND: f32 = 5.12e-3;

/// Per-component ceiling on steepness `A·k` — the **Stokes breaking limit**, with margin.
///
/// A deep-water wave breaks at `A·k ≈ 0.443`, so 0.35 sits just below whitecapping. Note
/// this bounds ONE component, which is what the physical limit is about; the sum is
/// bounded separately by [`WAVE_MAX_TOTAL_STEEPNESS`].
///
/// With the Cox–Munk calibration this cap never binds at any weather the wind model can
/// produce — it would take about 47 m/s — which is the right place for a safety limit to
/// sit: reachable in principle, never in practice.
pub const WAVE_MAX_STEEPNESS: f32 = 0.35;

/// Ceiling on the SUM's steepness, and therefore on how far the normal may tilt:
/// `atan(0.75) = 36.9°`.
///
/// **Derived from the refraction invariant, not chosen for looks.** Refraction bends toward
/// the normal, so with `eta = 0.75` the transmitted ray sits within 48.6° of `-optics`; for
/// it to stay below the face — which is what lets `water_surface_radiance` skip a guard on
/// the refracted ray entirely — the normal's own tilt must satisfy
/// `tilt + 48.6° < 90°`, i.e. `tilt < 41.4°`. This leaves 4.5° of margin.
///
/// Cox–Munk at the wind model's maximum (12 m/s) asks for 0.72, just inside — so this cap
/// binds only in a full gale, and `the_refracted_ray_always_stays_under_the_face` checks
/// the invariant numerically over the whole input range rather than trusting the algebra.
pub const WAVE_MAX_TOTAL_STEEPNESS: f32 = 0.75;

/// Half-angle of the directional fan around the wind bearing, radians (35°).
///
/// A sum of parallel waves is corrugated iron. Spreading the components produces a
/// **short-crested** sea, which is what makes glitter break up into moving highlights
/// instead of banding. The fan widens toward the short components
/// ([`WaveField::component`]), matching real angular spreading, which is narrow at the
/// spectral peak and broad in the tail.
pub const WAVE_SPREAD_RADIANS: f32 = 0.610_865_24;

/// How far a full gust shifts the slope variance toward the SHORT components, as a
/// fraction of each component's equal share.
///
/// Short only, because wave response time scales with period: a gust ruffles a surface
/// within seconds (cat's paws) while the long components would need minutes, and this
/// model has no memory to give them.
///
/// It **redistributes** variance rather than adding any, and the weights are renormalised
/// so the total stays exactly Cox–Munk whatever the gust is doing. Adding energy here
/// would double-count, because `WindFrame::speed` already carries the gust through
/// `activity` — so a gust already roughens the water, and this only changes *which
/// wavelengths* carry it.
pub const WAVE_GUST_SHORT_BIAS: f32 = 0.6;

/// Radians of phase jitter the wind's eddy channel applies to the shortest component.
///
/// The eddy channel exists for exactly this: `voxel_core::wind::WindFrame` documents it
/// as what "foam wants", i.e. the chop. It moves phase rather than amplitude so it
/// cannot disturb the steepness cap.
pub const WAVE_EDDY_PHASE_RADIANS: f32 = 0.8;

/// Golden ratio, for the per-component phase offset `fract(i · φ) · 2π`.
///
/// The offsets exist so the components do not all cross zero together at `t = 0`, which
/// would start every session with one conspicuous flat instant. Golden-ratio spacing is
/// the same trick the FDN's delay rates use: the sequence that stays maximally unaligned
/// at every prefix length.
///
/// **Computed rather than tabulated**, so the WGSL mirror evaluates the identical
/// expression and two literal tables cannot drift apart.
const WAVE_GOLDEN_RATIO: f32 = 0.618_034;

/// One directional wave in the sum, fully resolved — every quantity the height field
/// and its gradient need, and nothing derived twice.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveComponent {
    /// Crest-to-crest distance, metres.
    pub wavelength_meters: f32,
    /// `k = 2π / λ`, radians per metre.
    pub wavenumber: f32,
    /// `ω = sqrt(g·k)`, radians per second — deep-water gravity-wave dispersion.
    /// This is the term that keeps the sum from reading as a scrolling texture: long
    /// components genuinely outrun short ones, so the interference never repeats on a
    /// beat.
    pub angular_frequency: f32,
    /// Unit propagation direction in the world's XZ plane.
    pub direction: Vec2,
    /// `A`, metres — the crest height above the mean surface.
    pub amplitude_meters: f32,
    /// Fixed phase plus, for the shortest component, the wind's eddy jitter.
    pub phase_radians: f32,
}

impl WaveComponent {
    /// `A·k`, the dimensionless slope scale. This — not amplitude — is what the
    /// gradient is linear in, which is why the steepness cap is the natural place to
    /// bound the model.
    pub fn steepness(self) -> f32 {
        self.amplitude_meters * self.wavenumber
    }

    /// Phase speed `c = ω / k = sqrt(g / k)`, m/s. A 6 m wave runs at 3.06 m/s, a
    /// 0.6 m wave at 0.97 m/s.
    pub fn phase_speed_meters_per_second(self) -> f32 {
        self.angular_frequency / self.wavenumber
    }

    /// Temporal frequency in Hz, which is what the epoch-split clock's oscillator
    /// primitive takes.
    pub fn frequency_hz(self) -> f32 {
        self.angular_frequency / std::f32::consts::TAU
    }
}

/// The wave field, resolved from one frame of wind.
///
/// **The wind is not a new noise source.** `voxel_core::wind::WindDriver` already drives
/// the cloud deck and the weather, and [`crate::sky_weather::SkyWeather`] states the rule
/// this obeys: the deck's drift and the weather's severity must come from ONE wind
/// history, or the sky moves at a speed the weather does not agree with. Waves are the
/// third consumer of that history, and each takes the channel that suits it — the deck
/// takes the slow weather, grass takes gusts, and here the mean drives amplitude, the
/// gust roughens the short end and the eddy is the chop.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct WaveField {
    /// Mean wind bearing, radians. The same angle the cloud deck drifts along —
    /// `voxel_core::weather` turns it into a vector as `[cos, sin]`, and
    /// [`WaveField::component`] uses that convention exactly so the two cannot diverge.
    pub bearing_radians: f32,
    /// `WindFrame::speed`, m/s — the physical wind speed, which is what Cox & Munk's
    /// slope relation takes.
    ///
    /// **`speed` and not `activity`.** The two are not interchangeable: `activity` is a
    /// normalised 0..1 shape, and feeding it to a relation whose argument is metres per
    /// second is what made the first build four times too flat. `speed` also already
    /// carries the gust (it is `min + (max - min) * activity`), which is why
    /// [`WAVE_GUST_SHORT_BIAS`] only redistributes.
    pub speed_meters_per_second: f32,
    /// `WindFrame::gust`, 0..1 — positive gust pressure. Roughens the short end.
    pub gust: f32,
    /// `WindFrame::eddy`, -1..1 — small turbulence. Jitters the shortest component's
    /// phase.
    pub eddy: f32,
    /// The look lever, 0..1. `1.0` is the shipped look and `0.0` is exactly flat water.
    ///
    /// There is no value above 1: [`WAVE_MAX_STEEPNESS`] is a physical ceiling (the
    /// Stokes breaking limit), so "more waves than the wind justifies" is a change to
    /// that constant and its argument, not a slider that quietly walks past it.
    pub amplitude_scale: f32,
}

impl WaveField {
    /// Flat water: no wind, no waves. The identity the lever-off path returns to.
    pub const FLAT: Self = Self {
        bearing_radians: 0.0,
        speed_meters_per_second: 0.0,
        gust: 0.0,
        eddy: 0.0,
        amplitude_scale: 0.0,
    };

    /// Resolve a field from the wind history's current frame.
    ///
    /// Taking `WindFrame` by value rather than three loose floats is the point: it makes
    /// the one-wind-history rule visible in the signature, so a second driver cannot be
    /// introduced by accident.
    pub fn from_wind(wind: WindFrame, bearing_radians: f32, amplitude_scale: f32) -> Self {
        Self {
            bearing_radians,
            speed_meters_per_second: wind.speed,
            gust: wind.gust,
            eddy: wind.eddy,
            amplitude_scale,
        }
    }

    /// RMS surface slope this frame's wind produces — **Cox & Munk, evaluated**, scaled
    /// by the look lever.
    ///
    /// `sigma = sqrt(0.003 + 5.12e-3 * U)`. This is the ONE number that says how rough the
    /// water is; everything below only decides which wavelengths carry it.
    pub fn rms_slope(&self) -> f32 {
        let speed = self.speed_meters_per_second.max(0.0);
        // The 0.003 INTERCEPT applies at every speed, including zero, and that is what
        // makes "there is always a small ripple" a property of the model rather than a
        // tuned floor: dead calm still measures sigma = 0.055, a 3.1° RMS slope.
        //
        // An earlier version returned 0 below any wind at all, on the argument that the
        // intercept describes residual sea state in the PRESENCE of wind. Two things were
        // wrong with it. It diverged from `wave_rms_slope` in `shaders/water.wgsl`, which
        // has no such branch — so the CPU mirror and the rendered pixel disagreed at
        // exactly the state a weather preset can ask for. And it put a second, implicit
        // flat switch beside the explicit one: flatness is the AMPLITUDE LEVER's job
        // (`WaveField::amplitude_scale`, `WATER_WAVES`), which is the only thing
        // `wave_field_is_flat` tests on the GPU side.
        let variance =
            WAVE_SLOPE_VARIANCE_INTERCEPT + WAVE_SLOPE_VARIANCE_PER_METER_PER_SECOND * speed;
        variance.sqrt() * self.amplitude_scale.clamp(0.0, 1.0)
    }

    /// Component `index`'s share of the slope variance, normalised so the shares sum to 1.
    ///
    /// Equal shares are the Phillips result — a `k^-3` equilibrium spectrum spreads slope
    /// variance evenly across logarithmic bands, and the components are spaced evenly in
    /// `log k`. The gust then tilts the shares toward the short end and they are
    /// renormalised, so the TOTAL is untouched (see [`WAVE_GUST_SHORT_BIAS`]).
    fn component_variance_share(&self, index: usize) -> f32 {
        let mut shares = [0.0_f32; WAVE_COMPONENTS];
        let mut total = 0.0;
        let gust = self.gust.clamp(0.0, 1.0);
        for (slot, share) in shares.iter_mut().enumerate() {
            let short_weight = slot as f32 / (WAVE_COMPONENTS - 1) as f32;
            // (short_weight - 0.5) * 2 spans -1 (longest) to +1 (shortest).
            *share = (1.0 + WAVE_GUST_SHORT_BIAS * gust * (short_weight - 0.5) * 2.0).max(0.0);
            total += *share;
        }
        if total <= 0.0 {
            return 1.0 / WAVE_COMPONENTS as f32;
        }
        shares[index] / total
    }

    /// The steepness `A·k` of component `index`.
    ///
    /// **Derived from the RMS slope, not from a fraction of a cap.** For a sum of waves with
    /// independent phases the slope variance is `Σ sᵢ² / 2` (the cross terms vanish), so a
    /// component carrying variance share `wᵢ` of a total `sigma²` has
    /// `sᵢ = sigma * sqrt(2 wᵢ)`. With equal shares that is `sigma * sqrt(2/N)`, and the
    /// sum comes to `sigma * sqrt(2N)` — the relation
    /// `the_model_reproduces_the_cox_munk_slope` checks numerically against a measured
    /// patch rather than against this algebra.
    ///
    /// Two caps then apply, and both are documented bounds rather than tuning: the Stokes
    /// breaking limit per component, and [`WAVE_MAX_TOTAL_STEEPNESS`] on the sum, which is
    /// the refraction invariant.
    fn component_steepness(&self, index: usize) -> f32 {
        let uncapped = self.rms_slope() * (2.0 * self.component_variance_share(index)).sqrt();
        let per_component = uncapped.min(WAVE_MAX_STEEPNESS);
        // The total cap is applied by scaling every component equally, so capping cannot
        // change the SHAPE of the sea — only its overall roughness.
        per_component * self.total_cap_scale()
    }

    /// How much every component must shrink for the sum to respect
    /// [`WAVE_MAX_TOTAL_STEEPNESS`]. `1.0` whenever the cap is not binding, which is
    /// everything short of a gale.
    fn total_cap_scale(&self) -> f32 {
        let uncapped: f32 = (0..WAVE_COMPONENTS)
            .map(|index| {
                (self.rms_slope() * (2.0 * self.component_variance_share(index)).sqrt())
                    .min(WAVE_MAX_STEEPNESS)
            })
            .sum();
        if uncapped <= WAVE_MAX_TOTAL_STEEPNESS {
            return 1.0;
        }
        WAVE_MAX_TOTAL_STEEPNESS / uncapped
    }

    /// Resolve component `index` (`0` = longest, `WAVE_COMPONENTS - 1` = shortest).
    pub fn component(&self, index: usize) -> WaveComponent {
        let last = (WAVE_COMPONENTS - 1) as f32;
        let position = index as f32 / last;

        // Geometric spacing: equal steps in log wavelength, so equal steps in log k.
        let wavelength_meters =
            WAVE_LONGEST_METERS * (WAVE_SHORTEST_METERS / WAVE_LONGEST_METERS).powf(position);
        let wavenumber = std::f32::consts::TAU / wavelength_meters;
        let angular_frequency = (WAVE_GRAVITY_METERS_PER_SECOND_SQUARED * wavenumber).sqrt();

        // The fan widens toward the short components and alternates side, so the sum is
        // short-crested rather than a single corrugation. Component 0 carries no fan at
        // all, which is what makes it the bearing itself.
        let fan_side = if index.is_multiple_of(2) { -1.0 } else { 1.0 };
        let angle = self.bearing_radians + WAVE_SPREAD_RADIANS * position * fan_side;
        let (sin_angle, cos_angle) = angle.sin_cos();

        let steepness = self.component_steepness(index);
        let is_shortest = index == WAVE_COMPONENTS - 1;
        let eddy_phase = if is_shortest {
            WAVE_EDDY_PHASE_RADIANS * self.eddy.clamp(-1.0, 1.0)
        } else {
            0.0
        };

        WaveComponent {
            wavelength_meters,
            wavenumber,
            angular_frequency,
            // The cloud deck's convention, from `voxel_core::weather`: bearing -> [cos, sin].
            direction: Vec2::new(cos_angle, sin_angle),
            amplitude_meters: steepness / wavenumber,
            phase_radians: (index as f32 * WAVE_GOLDEN_RATIO).fract() * std::f32::consts::TAU
                + eddy_phase,
        }
    }

    /// How much of component `index` survives at a pixel footprint of
    /// `footprint_meters` — W4's anti-aliasing fade. `0.0` means "infinitely sharp",
    /// i.e. no fade, which is what a caller with no camera passes.
    ///
    /// Deliberately NOT folded into [`WaveField::component_steepness`]: that is the
    /// WIND's budget, a property of the weather, while this is a property of where the
    /// pixel is. Keeping them apart is what lets [`WaveField::total_steepness`] stay the
    /// quantity [`WAVE_MAX_STEEPNESS`] bounds — the fade can only ever reduce it, so the
    /// cap holds at every distance for free.
    fn component_lod_fade(&self, index: usize, footprint_meters: f32) -> f32 {
        if footprint_meters <= 0.0 {
            return 1.0;
        }
        let cycles_per_pixel = footprint_meters / self.component(index).wavelength_meters;
        1.0 - smoothstep(
            WAVE_LOD_FADE_START_CYCLES_PER_PIXEL,
            WAVE_LOD_FADE_END_CYCLES_PER_PIXEL,
            cycles_per_pixel,
        )
    }

    /// Total steepness of the sum — the quantity [`WAVE_MAX_STEEPNESS`] bounds.
    pub fn total_steepness(&self) -> f32 {
        self.total_steepness_at(0.0)
    }

    /// Total steepness surviving at a pixel footprint of `footprint_meters` — how rough
    /// the surface still is at that distance, and the honest bound on the gradient.
    ///
    /// **This, not the gradient's magnitude, is the quantity that decreases with
    /// distance.** Fading a component always shrinks its own term, but the gradient is a
    /// VECTOR SUM: dropping a component that was partly cancelling the others can make
    /// `|Σ vᵢ|` larger even as every `|vᵢ|` shrinks. What always holds is the triangle
    /// inequality, `|Σ vᵢ| ≤ Σ |vᵢ|` — so this sum bounds the slope at every point, and it
    /// is monotone in the footprint. `the_fade_can_only_ever_reduce_the_roughness` pins
    /// both halves.
    pub fn total_steepness_at(&self, footprint_meters: f32) -> f32 {
        (0..WAVE_COMPONENTS)
            .map(|index| {
                self.component_steepness(index) * self.component_lod_fade(index, footprint_meters)
            })
            .sum()
    }

    /// Whether this field has no waves at all, in which case the surface is exactly the
    /// voxel face and no trigonometry needs to run.
    pub fn is_flat(&self) -> bool {
        self.total_steepness() <= 0.0
    }

    /// Phase of component `index` at a point and an instant:
    /// `k·(d·p) - ωt + φ`.
    ///
    /// The temporal term goes through
    /// [`AnimationClockSample::oscillator_phase`], which is the codebase's existing
    /// epoch-split recombination rather than a second one invented here: `ω·t` in a
    /// plain f32 loses the fraction an oscillator needs within hours of uptime, which
    /// is the whole reason the clock ships as epochs plus a remainder. The spatial term
    /// needs no such care — world positions are bounded.
    fn component_phase(
        &self,
        component: WaveComponent,
        position_meters: Vec2,
        clock: AnimationClockSample,
    ) -> f32 {
        let spatial = component.wavenumber * component.direction.dot(position_meters);
        let temporal = std::f32::consts::TAU * clock.oscillator_phase(component.frequency_hz());
        spatial - temporal + component.phase_radians
    }

    /// Surface height above the mean water plane, metres:
    /// `h = Σ Aᵢ sin(kᵢ(dᵢ·p) - ωᵢt + φᵢ)`.
    ///
    /// The shading path never calls this — it needs the gradient, which
    /// [`WaveField::height_gradient`] computes analytically. It exists because it is the
    /// function the gradient is the gradient *of*, so a finite difference of it is an
    /// independent check on that analytic derivative
    /// (`analytic_gradient_matches_finite_difference`), and because a future
    /// heightfield fluid will want the height itself.
    pub fn height_meters(
        &self,
        position_meters: Vec2,
        clock: AnimationClockSample,
        footprint_meters: f32,
    ) -> f32 {
        if self.is_flat() {
            return 0.0;
        }
        let mut height = 0.0;
        for index in 0..WAVE_COMPONENTS {
            let component = self.component(index);
            let phase = self.component_phase(component, position_meters, clock);
            height += component.amplitude_meters
                * self.component_lod_fade(index, footprint_meters)
                * phase.sin();
        }
        height
    }

    /// `(∂h/∂x, ∂h/∂z)` — the analytic gradient of [`WaveField::height_meters`]:
    /// `Σ Aᵢkᵢ dᵢ cos(…)`.
    ///
    /// Note what the amplitude does here: `Aᵢkᵢ` is exactly the component's steepness,
    /// so the gradient is linear in the quantity [`WAVE_MAX_STEEPNESS`] bounds and the
    /// slope ceiling is enforced without a clamp anywhere in this loop.
    pub fn height_gradient(
        &self,
        position_meters: Vec2,
        clock: AnimationClockSample,
        footprint_meters: f32,
    ) -> Vec2 {
        if self.is_flat() {
            return Vec2::ZERO;
        }
        let mut gradient = Vec2::ZERO;
        for index in 0..WAVE_COMPONENTS {
            let component = self.component(index);
            let phase = self.component_phase(component, position_meters, clock);
            let steepness =
                component.steepness() * self.component_lod_fade(index, footprint_meters);
            gradient += component.direction * (steepness * phase.cos());
        }
        gradient
    }

    /// The **Hessian** `(∂²h/∂x², ∂²h/∂z², ∂²h/∂x∂z)` — the analytic second derivative of
    /// [`WaveField::height_meters`]: `-Σ Aᵢkᵢ² (dᵢ ⊗ dᵢ) sin(…)`.
    ///
    /// This is what caustics are made of. `Aᵢkᵢ` is the component's steepness, so each term
    /// is `steepness · wavenumber` — one more factor of `k` than the gradient carries, which
    /// is why the SHORT components dominate curvature and therefore dominate the caustic
    /// filaments. Physically right: fine ripples make fine filaments.
    pub fn height_hessian(
        &self,
        position_meters: Vec2,
        clock: AnimationClockSample,
        footprint_meters: f32,
    ) -> Vec3 {
        if self.is_flat() {
            return Vec3::ZERO;
        }
        let mut hessian = Vec3::ZERO;
        for index in 0..WAVE_COMPONENTS {
            let component = self.component(index);
            let phase = self.component_phase(component, position_meters, clock);
            let magnitude = -component.steepness()
                * component.wavenumber
                * self.component_lod_fade(index, footprint_meters)
                * phase.sin();
            let direction = component.direction;
            hessian += Vec3::new(
                direction.x * direction.x,
                direction.y * direction.y,
                direction.x * direction.y,
            ) * magnitude;
        }
        hessian
    }

    /// The sun's **focus gain** on a bed `depth_meters` below the surface point
    /// `position_meters` — the CPU mirror of `water_caustic_gain` in `shaders/water.wgsl`.
    ///
    /// `1 / |det J|` with `J = I + d(1 - 1/n)·H`: a near-vertical sun ray meeting slope `s`
    /// refracts to `s/n`, so it leaves the vertical by `s(1 - 1/n)` and lands displaced by
    /// `d(1 - 1/n)·∇h`. Light is conserved along a tube, so the irradiance density at the bed
    /// is the reciprocal of that map's Jacobian determinant. `det J < 0` is past the focus,
    /// where the map has folded, and `|det|` is still the right density there.
    ///
    /// Clamped to [`CAUSTIC_MAX_GAIN`], because at a true focus `det J → 0` and the gain
    /// diverges. Returns exactly 1.0 for a flat field, with no trigonometry evaluated.
    pub fn caustic_gain(
        &self,
        position_meters: Vec2,
        clock: AnimationClockSample,
        depth_meters: f32,
        index_of_refraction: f32,
    ) -> f32 {
        if self.is_flat() || index_of_refraction <= AIR_INDEX_OF_REFRACTION {
            return 1.0;
        }
        let hessian = self.height_hessian(position_meters, clock, 0.0);
        let bend = depth_meters * (1.0 - AIR_INDEX_OF_REFRACTION / index_of_refraction);
        let determinant = (1.0 + bend * hessian.x) * (1.0 + bend * hessian.y)
            - (bend * hessian.z) * (bend * hessian.z);
        (1.0 / determinant.abs().max(1.0 / CAUSTIC_MAX_GAIN)).min(CAUSTIC_MAX_GAIN)
    }

    /// The surface normal: `normalize(-∂h/∂x, 1, -∂h/∂z)`, y up.
    ///
    /// Exactly `Vec3::Y` for a flat field, with no trigonometry evaluated — the
    /// bit-identity the isolation rule asks for. And always in the upper hemisphere,
    /// because the `y` component is 1 before normalising and the steepness cap keeps
    /// the other two below 0.35.
    ///
    /// **This is the OPTICS normal, not the bias normal.** The shading path must keep
    /// using the geometric face normal to offset secondary-ray origins; offsetting
    /// along a perturbed normal moves a ray in a direction unrelated to the face it is
    /// escaping and it self-intersects. See `docs/water-waves-plan.md`, trap 1.
    pub fn surface_normal(
        &self,
        position_meters: Vec2,
        clock: AnimationClockSample,
        footprint_meters: f32,
    ) -> Vec3 {
        if self.is_flat() {
            return Vec3::Y;
        }
        let gradient =
            clamp_surface_gradient(self.height_gradient(position_meters, clock, footprint_meters));
        Vec3::new(-gradient.x, 1.0, -gradient.y).normalize()
    }
}

/// The one cap on the **summed** surface gradient — the wind field plus every live splash
/// ring — mirroring `water_clamp_surface_gradient` in `shaders/water.wgsl`.
///
/// A no-op for the wind field alone, because [`WaveField::total_cap_scale`] already bounds
/// the sum of the components' magnitudes and the triangle inequality carries that to the
/// vector sum. It exists for what rides ON TOP: a splash ring adds up to
/// [`WAVE_MAX_STEEPNESS`] of slope that the wind field's own cap never saw, and the plan's
/// W6 note is explicit that the cap has to absorb the splash term "or a jump could fold the
/// surface past breaking".
///
/// **[`WAVE_MAX_TOTAL_STEEPNESS`] is the refraction invariant, not a taste knob.** A tilt
/// past `atan(0.75)` = 36.9° can throw the refracted ray back above the face, and
/// `water_surface_radiance` omits a guard on that ray precisely because this bound holds.
/// Scaled rather than clipped per-axis, so capping flattens the normal without rotating it.
pub fn clamp_surface_gradient(gradient: Vec2) -> Vec2 {
    let steepness = gradient.length();
    if steepness <= WAVE_MAX_TOTAL_STEEPNESS {
        return gradient;
    }
    gradient * (WAVE_MAX_TOTAL_STEEPNESS / steepness)
}

/// Where the per-component distance fade starts and ends, in **wave cycles per pixel**.
///
/// W4, and it is not optional: a 0.6 m component seen at 40 m is sub-pixel, and a
/// sub-pixel sinusoid does not read as a small wave — it reads as aliasing sparkle, which
/// looks *worse* than the flat water it replaced. Each component therefore fades out on
/// its own, as its wavelength approaches the pixel footprint.
///
/// **The criterion is Nyquist, not taste.** A pixel footprint of `f` metres samples a
/// wavelength `λ` at `f / λ` cycles per pixel, and above 0.5 — two pixels per wave — the
/// sinusoid is past the sampling limit and cannot be represented at all. The fade
/// therefore reaches zero exactly there, and begins at 0.125 (eight pixels per wave) so it
/// is a ramp rather than a pop. `material_params.z` supplies the metres-per-pixel at one
/// metre that turns a distance into `f` — the same term `pattern.wgsl` already uses to
/// budget fractal octaves, for the same reason.
///
/// **Why the ramp starts well before Nyquist.** Nyquist bounds when the NORMAL FIELD
/// itself becomes unrepresentable, and that is the hard end. But the shading is not
/// linear in the normal — the mirror term is a near-perfect reflection carrying a sun
/// disc, so a normal that wobbles by a fraction of a pixel can swing a pixel from sky to
/// sun to shoreline. Visible sparkle therefore appears well before the sampling limit,
/// which is why the ramp begins at 0.125 (eight pixels per wave) rather than at 0.25.
/// Doing this properly means pre-filtering the normal distribution — roughening the
/// surface with distance instead of flattening it, LEAN/Toksvig style — and that is a
/// separate arc; fading to flat is the cheap, correct-in-the-limit version.
pub const WAVE_LOD_FADE_START_CYCLES_PER_PIXEL: f32 = 0.125;
/// See [`WAVE_LOD_FADE_START_CYCLES_PER_PIXEL`]. This is the Nyquist limit itself.
pub const WAVE_LOD_FADE_END_CYCLES_PER_PIXEL: f32 = 0.5;

/// WGSL's `smoothstep`, so the LOD fade ramps identically on both sides.
fn smoothstep(edge0: f32, edge1: f32, value: f32) -> f32 {
    if edge1 <= edge0 {
        return f32::from(value >= edge1);
    }
    let t = ((value - edge0) / (edge1 - edge0)).clamp(0.0, 1.0);
    t * t * (3.0 - 2.0 * t)
}

/// The lowest cosine to the GEOMETRIC face a mirror ray may leave at — about 1.1° above
/// the plane. Mirrors `WATER_REFLECTION_MIN_COSINE` in `shaders/water.wgsl`.
///
/// Not zero, because a ray exactly in the plane of the face is the case the DDA is least
/// happy with.
pub const WATER_REFLECTION_MIN_COSINE: f32 = 0.02;

/// Lift a mirror direction back above the face it leaves, if the wave normal threw it
/// below — the CPU mirror of the guard in `water_surface_radiance` (the plan doc's
/// trap 2).
///
/// **Why the guard is needed at all:** perturbing the normal can point `reflect()` INTO
/// the water body, and a mirror ray that starts inside water has `trace` hit liquid at
/// t ≈ 0, so the pixel goes black. That is the classic wave-normal
/// sparkle-of-black-dots. [`WAVE_MAX_STEEPNESS`] bounds how bad it gets — the normal
/// tilts at most `atan(0.35) = 19.3°`, so a grazing ray is thrown at most 38.6° under
/// the plane — and this closes the remainder.
///
/// **Why one step is enough, with no iteration.** Adding `g·(m − d)` (where `d` is the
/// current cosine and `m` the minimum) makes the un-normalised dot product exactly `m`,
/// because `g` is a unit vector. Normalising divides by
/// `|v| = sqrt(1 + m² − d²)`, so the result is `m / sqrt(1 + m² − d²)` — which is `≥ m`
/// everywhere except the razor-thin band `|d| < m`, where it is at worst
/// `m / sqrt(1 + m²) = 0.019996`. The requirement is *strictly above the plane*, and
/// that holds unconditionally; landing a rounding error under the nominal cosine does
/// not matter.
///
/// It only ever fires at grazing incidence, where the ray runs nearly parallel to the
/// surface and the redirection is imperceptible.
///
/// **The precondition is not decoration.** A ray reflected off the geometric face itself
/// always leaves at `|cos(incidence)|` above that face — strictly positive, never inside
/// the water — so a flat surface cannot reflect into itself and needs no guard at all.
/// Applying the lift anyway would nudge extremely grazing rays (past ~88.9°, where
/// `cos < WATER_REFLECTION_MIN_COSINE`) on flat water too, and that would change the
/// no-waves image. `a_flat_surface_needs_no_lift_at_all` is the test that caught exactly
/// that, and it is why the comparison is here rather than at the call site.
pub fn lift_reflection_above_face(reflected: Vec3, geometric: Vec3, optics: Vec3) -> Vec3 {
    if optics == geometric {
        return reflected;
    }
    let above_face = reflected.dot(geometric);
    if above_face < WATER_REFLECTION_MIN_COSINE {
        return (reflected + geometric * (WATER_REFLECTION_MIN_COSINE - above_face)).normalize();
    }
    reflected
}

/// Whether an eye at `eye_position_meters` sits inside a liquid voxel — the
/// CPU mirror of the shader's underwater test, and the reason the underwater view
/// works in fly mode as well as in walk mode.
///
/// E2b's [`crate::character::CharacterController::head_submerged`] answers the
/// same question for the walking body's head;
/// `the_two_underwater_predicates_agree` pins the two together. The shader is the
/// authority (it tests the primary ray's own origin), so this exists for the
/// overlay readout, for the tests, and for E8's submerged listener.
pub fn eye_is_submerged(brickmap: &Brickmap, eye_position_meters: Vec3) -> bool {
    let voxel = (eye_position_meters / VOXEL_SIZE).floor();
    material_is_liquid(brickmap.get(voxel.x as i32, voxel.y as i32, voxel.z as i32))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passes::cagi::CAGI_SHADER_SOURCE;
    use crate::passes::dda::SHADER_SOURCE;
    use voxel_material::material::WATER_INDEX_OF_REFRACTION;

    #[test]
    fn default_settings_match_shader_source() {
        assert_eq!(
            WaterSettings::default().patch_shader_source(&SHADER_SOURCE),
            SHADER_SOURCE.as_str(),
            "WaterSettings::default() drifted from the water lever defaults in water.wgsl"
        );
    }

    #[test]
    fn patched_source_carries_both_compile_time_knobs() {
        let shader_source = WaterSettings {
            mode: WaterMode::FresnelTint,
            bounces: 2,
            ..WaterSettings::default()
        }
        .patch_shader_source(&SHADER_SOURCE);
        assert!(shader_source.contains("const WATER_MODE: u32 = 1u;"));
        assert!(shader_source.contains("const WATER_BOUNCES: u32 = 2u;"));
        // ...without touching the mode NAME constants declared above it.
        assert!(shader_source.contains("const WATER_MODE_OPAQUE: u32 = 0u;"));
        assert!(shader_source.contains("const WATER_MODE_FULL: u32 = 4u;"));
    }

    /// `WATER_SUN_THROUGH_LIQUID` lives in the SHARED `world.wgsl`, so it must be
    /// patched into whichever source it is handed — including the CA pass's, which
    /// has no water lever block at all. A liquid that stops the shading pass's sun
    /// ray but not the light volume's would light the bed under water in one and
    /// shadow it in the other.
    #[test]
    fn the_sun_shadow_lever_reaches_both_pass_sources() {
        let no_sun = WaterSettings {
            sun_through_liquid: false,
            ..WaterSettings::default()
        };
        for source in [SHADER_SOURCE.as_str(), CAGI_SHADER_SOURCE.as_str()] {
            assert!(source.contains("const WATER_SUN_THROUGH_LIQUID: bool = true;"));
            assert!(no_sun
                .patch_shader_source(source)
                .contains("const WATER_SUN_THROUGH_LIQUID: bool = false;"));
        }
        // Only the shading pass has the optics block, and patching the CA source
        // must not panic on the consts that are missing from it.
        assert!(SHADER_SOURCE.contains("const WATER_MODE:"));
        assert!(!CAGI_SHADER_SOURCE.contains("const WATER_MODE:"));
        assert!(no_sun.requires_pipeline_rebuild(&WaterSettings::default()));
    }

    #[test]
    fn runtime_knobs_alone_never_force_a_rebuild() {
        let applied = WaterSettings::default();
        let runtime_only = WaterSettings {
            absorption_scale: 2.5,
            scattering_scale: 0.1,
            ..applied
        };
        assert!(!runtime_only.requires_pipeline_rebuild(&applied));
        assert!(WaterSettings {
            bounces: 2,
            ..applied
        }
        .requires_pipeline_rebuild(&applied));
        assert!(WaterSettings {
            mode: WaterMode::Opaque,
            ..applied
        }
        .requires_pipeline_rebuild(&applied));
    }

    #[test]
    fn modes_round_trip_through_their_shader_values() {
        for mode in [
            WaterMode::Opaque,
            WaterMode::FresnelTint,
            WaterMode::Reflection,
            WaterMode::Refraction,
            WaterMode::Full,
        ] {
            assert_eq!(WaterMode::from_shader_value(mode.shader_value()), mode);
        }
        assert!(WaterMode::Full.traces_reflection() && WaterMode::Full.traces_refraction());
        assert!(!WaterMode::FresnelTint.traces_reflection());
        assert!(!WaterMode::FresnelTint.traces_refraction());
        assert!(
            WaterMode::Reflection.traces_reflection() && !WaterMode::Reflection.traces_refraction()
        );
        assert!(
            !WaterMode::Refraction.traces_reflection() && WaterMode::Refraction.traces_refraction()
        );
    }

    /// `F0` must be the value the two indices imply, not a tuned constant: 2.04%.
    #[test]
    fn fresnel_f0_is_derived_from_the_indices_of_refraction() {
        assert!(
            (fresnel_f0(WATER_INDEX_OF_REFRACTION) - 0.020373).abs() < 1e-5,
            "F0 = {} (expected ((1.333-1)/(1.333+1))^2 = 0.020373)",
            fresnel_f0(WATER_INDEX_OF_REFRACTION)
        );
    }

    /// The gate's requirement, as arithmetic: grazing mirrors, steep sees
    /// through, and the curve is monotone in between.
    #[test]
    fn fresnel_mirrors_at_grazing_and_transmits_head_on() {
        assert!(
            (fresnel_schlick(1.0, WATER_INDEX_OF_REFRACTION)
                - fresnel_f0(WATER_INDEX_OF_REFRACTION))
            .abs()
                < 1e-6
        );
        assert!((fresnel_schlick(0.0, WATER_INDEX_OF_REFRACTION) - 1.0).abs() < 1e-6);
        // Hand-computed rungs: F = F0 + (1 - F0) * (1 - cos)^5.
        for (cosine, expected) in [
            (0.5_f32, 0.020373 + 0.979627 * 0.5_f32.powi(5)),
            (0.25, 0.020373 + 0.979627 * 0.75_f32.powi(5)),
            (0.75, 0.020373 + 0.979627 * 0.25_f32.powi(5)),
        ] {
            assert!(
                (fresnel_schlick(cosine, WATER_INDEX_OF_REFRACTION) - expected).abs() < 1e-5,
                "F({cosine}) = {}, expected {expected}",
                fresnel_schlick(cosine, WATER_INDEX_OF_REFRACTION)
            );
        }
        let mut previous = fresnel_schlick(0.0, WATER_INDEX_OF_REFRACTION);
        for step in 1..=64 {
            let value = fresnel_schlick(step as f32 / 64.0, WATER_INDEX_OF_REFRACTION);
            assert!(
                value <= previous + 1e-7,
                "Fresnel is not monotone at {step}"
            );
            assert!((0.0..=1.0).contains(&value));
            previous = value;
        }
        // Out-of-range cosines from float error must not escape [F0, 1].
        assert_eq!(fresnel_schlick(-0.2, WATER_INDEX_OF_REFRACTION), 1.0);
        assert!(
            (fresnel_schlick(1.4, WATER_INDEX_OF_REFRACTION)
                - fresnel_f0(WATER_INDEX_OF_REFRACTION))
            .abs()
                < 1e-6
        );
    }

    /// Snell's law against hand-computed angles, entering water: 45 deg in air
    /// refracts to `asin(sin(45)/1.333) = 32.03 deg` in water.
    #[test]
    fn refraction_into_water_matches_snells_law() {
        let normal = Vec3::Y;
        let incidence_degrees = 45.0_f32;
        let incidence = incidence_degrees.to_radians();
        // A downward ray at 45 deg from the surface normal.
        let incident = Vec3::new(incidence.sin(), -incidence.cos(), 0.0);
        let refracted = refract_direction(
            incident,
            normal,
            AIR_INDEX_OF_REFRACTION / WATER_INDEX_OF_REFRACTION,
        )
        .expect("entering the denser medium can never totally reflect");
        assert!(
            (refracted.length() - 1.0).abs() < 1e-5,
            "the refracted direction must stay unit length, got {}",
            refracted.length()
        );
        let transmitted_degrees = (-refracted.dot(normal)).acos().to_degrees();
        let expected_degrees = (incidence.sin() / WATER_INDEX_OF_REFRACTION)
            .asin()
            .to_degrees();
        assert!(
            (expected_degrees - 32.037).abs() < 0.01,
            "hand check: 45 deg into water is 32.04 deg, not {expected_degrees}"
        );
        assert!(
            (transmitted_degrees - expected_degrees).abs() < 0.01,
            "refracted at {transmitted_degrees} deg, Snell says {expected_degrees}"
        );
        // Straight down must not bend at all.
        let straight = refract_direction(
            Vec3::NEG_Y,
            normal,
            AIR_INDEX_OF_REFRACTION / WATER_INDEX_OF_REFRACTION,
        )
        .expect("straight down transmits");
        assert!((straight - Vec3::NEG_Y).length() < 1e-5);
    }

    /// The critical angle, and the total-internal-reflection behaviour on both
    /// sides of it — Snell's window, as a predicate.
    #[test]
    fn leaving_water_totally_reflects_past_the_critical_angle() {
        assert!(
            (critical_angle_degrees(WATER_INDEX_OF_REFRACTION) - 48.607).abs() < 0.01,
            "critical angle {} deg, expected 48.61",
            critical_angle_degrees(WATER_INDEX_OF_REFRACTION)
        );
        let eta = WATER_INDEX_OF_REFRACTION / AIR_INDEX_OF_REFRACTION;
        // The interface normal opposing an upward ray points back down into the
        // water, exactly as the DDA's face normal does.
        let normal = Vec3::NEG_Y;
        let upward_at = |degrees: f32| {
            let angle = degrees.to_radians();
            Vec3::new(angle.sin(), angle.cos(), 0.0)
        };
        assert!(
            refract_direction(upward_at(0.0), normal, eta).is_some(),
            "straight up must leave the water"
        );
        assert!(
            refract_direction(upward_at(48.0), normal, eta).is_some(),
            "48 deg is inside the window"
        );
        assert!(
            refract_direction(upward_at(49.0), normal, eta).is_none(),
            "49 deg is past the critical angle and must totally reflect"
        );
        assert!(
            refract_direction(upward_at(80.0), normal, eta).is_none(),
            "a near-horizontal upward ray must mirror, not escape"
        );
        // The window compresses the whole sky hemisphere into the cone: a ray at
        // the critical angle leaves along the horizon.
        let at_edge = refract_direction(upward_at(48.6), normal, eta).expect("just inside");
        let escape_degrees = at_edge.dot(Vec3::Y).clamp(-1.0, 1.0).acos().to_degrees();
        assert!(
            escape_degrees > 88.0,
            "at the critical angle the escaping ray should graze the horizon, got {escape_degrees} \
             deg from vertical"
        );
    }

    /// E6 step 3, the shipped default: from below the interface transmits UNBENT at
    /// every angle, including well past the critical angle where the physical
    /// interface would mirror. This is the whole content of "just transparent".
    #[test]
    fn the_underwater_interface_transmits_unbent_by_default() {
        assert_eq!(
            WaterSettings::default().underwater_interface,
            WaterUnderwaterInterface::Transparent
        );
        // The interface normal opposing an upward ray points back down into the water.
        let normal = Vec3::NEG_Y;
        for degrees in [0.0_f32, 20.0, 45.0, 48.0, 49.0, 70.0, 85.0, 89.0] {
            let angle = degrees.to_radians();
            let incident = Vec3::new(angle.sin(), angle.cos(), 0.0);
            let exit = underwater_exit(
                WaterUnderwaterInterface::Transparent,
                incident,
                normal,
                WATER_INDEX_OF_REFRACTION,
            );
            assert!(
                !exit.mirrored,
                "{degrees} deg mirrored under the transparent interface"
            );
            assert!(
                (exit.direction - incident).length() < 1e-6,
                "{degrees} deg was bent: {:?} vs the incident {:?}",
                exit.direction,
                incident
            );
        }
        // The one the request is really about: 70 deg is 21 deg past critical, so the
        // physical interface mirrors it and the shipped one lets it straight out.
        let angle = 70.0_f32.to_radians();
        let steep = Vec3::new(angle.sin(), angle.cos(), 0.0);
        let physical = underwater_exit(
            WaterUnderwaterInterface::Fresnel,
            steep,
            normal,
            WATER_INDEX_OF_REFRACTION,
        );
        assert!(
            physical.mirrored,
            "70 deg is past the {:.3} deg critical angle and must mirror under fresnel",
            critical_angle_degrees(WATER_INDEX_OF_REFRACTION)
        );
        assert!(
            physical.direction.y < 0.0,
            "the mirrored ray must head back down into the water, got {:?}",
            physical.direction
        );
        let shipped = underwater_exit(
            WaterUnderwaterInterface::Transparent,
            steep,
            normal,
            WATER_INDEX_OF_REFRACTION,
        );
        assert!(shipped.direction.y > 0.0 && !shipped.mirrored);
    }

    /// The off-lever must still be the real physics: bent inside the window,
    /// mirrored outside it.
    #[test]
    fn the_fresnel_underwater_interface_still_obeys_snells_law() {
        let normal = Vec3::NEG_Y;
        let at = |degrees: f32| {
            let angle = degrees.to_radians();
            underwater_exit(
                WaterUnderwaterInterface::Fresnel,
                Vec3::new(angle.sin(), angle.cos(), 0.0),
                normal,
                WATER_INDEX_OF_REFRACTION,
            )
        };
        // Inside the window: escapes, and BENT away from the incident direction.
        let inside = at(45.0);
        assert!(!inside.mirrored);
        let incident_45 = {
            let angle = 45.0_f32.to_radians();
            Vec3::new(angle.sin(), angle.cos(), 0.0)
        };
        assert!(
            (inside.direction - incident_45).length() > 0.1,
            "the fresnel interface must actually bend the ray, got {:?}",
            inside.direction
        );
        // Outside it: mirrored.
        assert!(at(49.0).mirrored);
        assert!(at(80.0).mirrored);
    }

    /// **The above-water side is untouched.** Entering the medium still bends by
    /// Snell's law and still carries the full Fresnel weight, whichever underwater
    /// interface is selected — the lever must not reach the "only top has the
    /// reflection" half.
    #[test]
    fn the_above_water_interface_is_untouched_by_the_underwater_lever() {
        // The above-water entry path takes eta = air / water and cannot ever totally
        // reflect, independent of the lever.
        let incidence = 45.0_f32.to_radians();
        let incident = Vec3::new(incidence.sin(), -incidence.cos(), 0.0);
        let refracted = refract_direction(
            incident,
            Vec3::Y,
            AIR_INDEX_OF_REFRACTION / WATER_INDEX_OF_REFRACTION,
        )
        .expect("entering the denser medium can never totally reflect");
        let transmitted_degrees = (-refracted.dot(Vec3::Y)).acos().to_degrees();
        assert!(
            (transmitted_degrees - 32.037).abs() < 0.01,
            "entering water at 45 deg must still bend to 32.04 deg, got {transmitted_degrees}"
        );
        // ...and the surface's own Fresnel weight is a function of the angle and the
        // authored index alone — the lever is not one of its inputs.
        assert!(
            (fresnel_schlick(1.0, WATER_INDEX_OF_REFRACTION)
                - fresnel_f0(WATER_INDEX_OF_REFRACTION))
            .abs()
                < 1e-6
        );
        assert!((fresnel_schlick(0.0, WATER_INDEX_OF_REFRACTION) - 1.0).abs() < 1e-6);
        // The shipped shader still mixes the traced mirror into the above-water
        // surface, and the underwater branch is the only consumer of the new lever.
        assert!(SHADER_SOURCE.contains("fn water_surface_radiance("));
        assert!(SHADER_SOURCE.contains("return mix(transmitted, mirrored, fresnel);"));
        let occurrences = SHADER_SOURCE
            .matches("WATER_UNDERWATER_INTERFACE == WATER_INTERFACE_TRANSPARENT")
            .count();
        assert_eq!(
            occurrences, 1,
            "the underwater interface lever must be consulted in exactly one place \
             (the medium loop), not on the above-water path"
        );
    }

    /// Both gated levers must be reported as inert under the shipped interface, so
    /// the overlay can grey them out instead of offering dead dials.
    #[test]
    fn the_transparent_interface_makes_the_bounce_levers_inert() {
        let shipped = WaterSettings::default();
        assert!(!shipped.bounce_levers_have_an_effect());
        assert!(WaterSettings {
            underwater_interface: WaterUnderwaterInterface::Fresnel,
            ..shipped
        }
        .bounce_levers_have_an_effect());
        // Changing an inert lever still forces a rebuild — it is a compile-time const
        // and the pipeline key must follow it, inert or not.
        assert!(WaterSettings {
            bounces: 2,
            ..shipped
        }
        .requires_pipeline_rebuild(&shipped));
    }

    /// The medium's colour must be DERIVED from the coefficient pair, not chosen.
    /// Hand-checked: extinction = absorption + scattering channel by channel, and
    /// the single-scattering albedo = scattering / extinction.
    #[test]
    fn the_medium_colour_is_derived_from_the_coefficient_pair() {
        let extinction = water_extinction_per_meter();
        for (channel, expected) in [(0, 0.450), (1, 0.120), (2, 0.060)] {
            assert!(
                (extinction[channel] - expected).abs() < 1e-6,
                "extinction channel {channel} = {}, expected {expected} \
                 (absorption {} + scattering {})",
                extinction[channel],
                WATER_ABSORPTION_PER_METER[channel],
                WATER_SCATTERING_PER_METER[channel]
            );
        }
        // scattering / extinction, computed by hand: 0.004/0.450, 0.030/0.120,
        // 0.045/0.060.
        let albedo = single_scattering_albedo();
        for (channel, expected) in [(0, 0.008889_f32), (1, 0.250_f32), (2, 0.750_f32)] {
            assert!(
                (albedo[channel] - expected).abs() < 1e-4,
                "single-scattering albedo channel {channel} = {}, expected {expected}",
                albedo[channel]
            );
        }
        // The property that matters: the medium reads BLUE because red is absorbed
        // fastest and blue scatters most — not because anything is painted blue.
        assert!(
            albedo[2] > albedo[1] && albedo[1] > albedo[0],
            "the derived colour is not blue-dominant: {albedo:?}"
        );
        assert!(
            albedo[2] > 30.0 * albedo[0],
            "blue should out-scatter red by more than an order of magnitude, got {albedo:?}"
        );
        // ...and it is NOT the water row's diffuse albedo, which is what it replaced.
        let painted = voxel_material::material::MATERIALS
            [voxel_material::material::material_id(voxel_core::world::Voxel::Water) as usize]
            .albedo;
        assert!(
            (albedo[1] - painted[1]).abs() > 0.2,
            "the derived colour {albedo:?} is suspiciously close to the painted albedo \
             {painted:?} — the volume colour must not come from surface reflectance"
        );
    }

    /// Extinction must fall strictly with distance, stay in [0, 1], and go
    /// blue-green rather than merely grey — the property that makes depth
    /// readable — and the two scales must act on their own coefficient only.
    #[test]
    fn extinction_decays_monotonically_and_shifts_blue() {
        let mut previous = transmittance(0.0, 1.0, 1.0);
        assert_eq!(previous, [1.0, 1.0, 1.0]);
        for step in 1..=40 {
            let meters = step as f32 * 0.25;
            let current = transmittance(meters, 1.0, 1.0);
            for channel in 0..3 {
                assert!(
                    current[channel] < previous[channel],
                    "channel {channel} did not decay at {meters} m"
                );
                assert!((0.0..=1.0).contains(&current[channel]));
            }
            assert!(
                current[0] < current[1] && current[1] < current[2],
                "at {meters} m transmittance is not red < green < blue: {current:?}"
            );
            previous = current;
        }
        // Hand-checked against exp(-sigma_t * d) at the debug pool's own depth.
        let at_pool_depth = transmittance(5.0, 1.0, 1.0);
        for (channel, expected) in [(0, 0.10540), (1, 0.54881), (2, 0.74082)] {
            assert!(
                (at_pool_depth[channel] - expected).abs() < 1e-4,
                "5 m transmittance channel {channel} = {}, expected {expected}",
                at_pool_depth[channel]
            );
        }
        // Each scale touches only its own coefficient: zeroing scattering leaves
        // absorption's exponent exactly, and vice versa.
        let absorption_only = transmittance(5.0, 1.0, 0.0);
        for channel in 0..3 {
            let expected = (-WATER_ABSORPTION_PER_METER[channel] * 5.0).exp();
            assert!((absorption_only[channel] - expected).abs() < 1e-6);
        }
        let scattering_only = transmittance(5.0, 0.0, 1.0);
        for channel in 0..3 {
            let expected = (-WATER_SCATTERING_PER_METER[channel] * 5.0).exp();
            assert!((scattering_only[channel] - expected).abs() < 1e-6);
        }
        assert_eq!(transmittance(5.0, 0.0, 0.0), [1.0, 1.0, 1.0]);
        assert_eq!(transmittance(-3.0, 1.0, 1.0), [1.0, 1.0, 1.0]);
    }

    /// **The bed darkens with depth, and red goes first** — the behaviour Pascal
    /// described (*"the distance it travels the less light comes down so ... the
    /// block at the bottom become darker"*). Pinned as NUMBERS rather than
    /// asserted: this is the transmittance of the SUN's own path down to a bed at
    /// 1 / 3 / 5 m, which is exactly what `water_sun_transmission` multiplies the
    /// sun term by in the shader.
    #[test]
    fn the_sun_reaching_a_submerged_surface_dims_and_reddens_with_depth() {
        let sun_elevation_sine = voxel_environment::SunSettings::default().sun_direction().y;
        assert!(
            (sun_elevation_sine - 0.7752).abs() < 0.001,
            "default sun elevation sine is {sun_elevation_sine}; the table assumes 0.7752"
        );

        // Hand-computed as exp(-sigma_t * depth / sin(elevation)), sigma_t =
        // (0.450, 0.120, 0.060): a low sun travels further through the same depth,
        // which is why the slant path and not the depth is the argument.
        let recorded: [(f32, [f32; 3]); 3] = [
            (1.0, [0.5597, 0.8566, 0.9255]),
            (3.0, [0.1753, 0.6285, 0.7928]),
            (5.0, [0.0549, 0.4612, 0.6791]),
        ];
        let mut previous = [1.0_f32; 3];
        for (depth_meters, expected) in recorded {
            let slant_meters = depth_meters / sun_elevation_sine;
            let sun_transmission = transmittance(slant_meters, 1.0, 1.0);
            for channel in 0..3 {
                assert!(
                    (sun_transmission[channel] - expected[channel]).abs() < 2e-3,
                    "sun transmission at {depth_meters} m, channel {channel} = {}, \
                     recorded {}",
                    sun_transmission[channel],
                    expected[channel]
                );
                assert!(
                    sun_transmission[channel] < previous[channel],
                    "channel {channel} did not dim further at {depth_meters} m"
                );
            }
            assert!(
                sun_transmission[0] < sun_transmission[2],
                "red must be the first channel to go at {depth_meters} m: \
                 {sun_transmission:?}"
            );
            previous = sun_transmission;
        }

        // The consequence worth stating: at 5 m the sun arrives with 5% of its red
        // and 68% of its blue, and the CAMERA's own 5 m path then attenuates it
        // again — so a red voxel on a 5 m bed retains under 1% of its red.
        let sun_at_five = transmittance(5.0 / sun_elevation_sine, 1.0, 1.0);
        let eye_at_five = transmittance(5.0, 1.0, 1.0);
        let round_trip_red = sun_at_five[0] * eye_at_five[0];
        assert!(
            round_trip_red < 0.01,
            "red should be all but gone over a 5 m down-and-back path, got {round_trip_red}"
        );
        assert!(
            sun_at_five[2] * eye_at_five[2] > 20.0 * round_trip_red,
            "blue must survive that path far better than red"
        );
    }

    // ---- W1: the wave field --------------------------------------------------
    //
    // Per the project rule, these test the VALUES rather than that the code compiles:
    // the dispersion relation against hand computations, the analytic gradient against
    // a numerical one, and the flat case against exact bit equality.

    /// A wind frame at the top of a strong gust — the worst case for the steepness cap.
    fn strong_wind() -> WaveField {
        WaveField {
            bearing_radians: 0.6,
            // The wind model's maximum (`WindShape::default().max_speed`), which is also
            // where the total-steepness cap comes closest to binding.
            speed_meters_per_second: 12.0,
            gust: 1.0,
            eddy: 0.5,
            amplitude_scale: 1.0,
        }
    }

    /// Cox & Munk, recomputed independently of the implementation.
    fn cox_munk_rms_slope(speed_meters_per_second: f32) -> f32 {
        (0.003 + 5.12e-3 * speed_meters_per_second).sqrt()
    }

    fn clock_at(seconds: f32) -> AnimationClockSample {
        AnimationClockSample {
            epoch: (seconds / 64.0).floor(),
            remainder_seconds: seconds - (seconds / 64.0).floor() * 64.0,
        }
    }

    #[test]
    fn wave_dispersion_matches_hand_computation() {
        let field = strong_wind();

        // The 6 m component, by hand: k = 2pi / 6, which is exactly pi / 3 =
        // 1.047198 rad/m; omega = sqrt(9.80665 * 1.047198) = sqrt(10.27329) =
        // 3.205197 rad/s; c = omega / k = 3.0607 m/s.
        let longest = field.component(0);
        assert!(
            (longest.wavelength_meters - 6.0).abs() < 1e-5,
            "component 0 must be the longest wavelength, got {}",
            longest.wavelength_meters
        );
        assert!(
            (longest.wavenumber - std::f32::consts::FRAC_PI_3).abs() < 1e-4,
            "k of a 6 m wave is 2pi/6 = pi/3 = 1.047198, got {}",
            longest.wavenumber
        );
        assert!(
            (longest.angular_frequency - 3.205_197).abs() < 1e-3,
            "omega of a 6 m wave is 3.205197 rad/s, got {}",
            longest.angular_frequency
        );
        assert!(
            (longest.phase_speed_meters_per_second() - 3.0607).abs() < 1e-3,
            "a 6 m wave travels at 3.0607 m/s, got {}",
            longest.phase_speed_meters_per_second()
        );

        // Every component must satisfy omega^2 = g*k, recomputed independently.
        for index in 0..WAVE_COMPONENTS {
            let component = field.component(index);
            let expected = (WAVE_GRAVITY_METERS_PER_SECOND_SQUARED * component.wavenumber).sqrt();
            assert!(
                (component.angular_frequency - expected).abs() < 1e-4,
                "component {index} breaks omega = sqrt(g k)"
            );
        }

        // The whole point of dispersion: long outruns short, and by sqrt of the
        // wavelength ratio. 6 m / 0.6 m = 10, so the speeds differ by sqrt(10) = 3.1623.
        let shortest = field.component(WAVE_COMPONENTS - 1);
        let ratio =
            longest.phase_speed_meters_per_second() / shortest.phase_speed_meters_per_second();
        assert!(
            (ratio - 10.0_f32.sqrt()).abs() < 1e-3,
            "the 6 m wave must outrun the 0.6 m wave by sqrt(10), got {ratio}"
        );
    }

    #[test]
    fn wavelength_band_is_geometric() {
        let field = strong_wind();
        assert!((field.component(0).wavelength_meters - WAVE_LONGEST_METERS).abs() < 1e-5);
        assert!(
            (field.component(WAVE_COMPONENTS - 1).wavelength_meters - WAVE_SHORTEST_METERS).abs()
                < 1e-5
        );
        // Equal steps in log wavelength means a constant ratio between neighbours.
        let first_ratio =
            field.component(1).wavelength_meters / field.component(0).wavelength_meters;
        for index in 1..WAVE_COMPONENTS {
            let ratio = field.component(index).wavelength_meters
                / field.component(index - 1).wavelength_meters;
            assert!(
                (ratio - first_ratio).abs() < 1e-4,
                "component {index} breaks the geometric spacing"
            );
        }
    }

    #[test]
    fn wave_steepness_never_breaks() {
        // Sweep the whole input range, including inputs outside the documented domain
        // (negative speed, a lever above 1), since nothing stops a caller passing them.
        for speed_step in -2..=30 {
            for gust_step in -2..=12 {
                for scale_step in 0..=12 {
                    let field = WaveField {
                        bearing_radians: 0.6,
                        speed_meters_per_second: speed_step as f32,
                        gust: gust_step as f32 / 10.0,
                        eddy: 1.0,
                        amplitude_scale: scale_step as f32 / 10.0,
                    };
                    // Per component: the Stokes breaking limit.
                    for index in 0..WAVE_COMPONENTS {
                        let steepness = field.component(index).steepness();
                        assert!(
                            steepness <= WAVE_MAX_STEEPNESS + 1e-6,
                            "component {index} broke at {speed_step} m/s: {steepness}"
                        );
                        assert!(
                            steepness < 0.443,
                            "component {index} reached the Stokes limit"
                        );
                    }
                    // The sum: the refraction invariant's bound.
                    let total = field.total_steepness();
                    assert!(
                        total <= WAVE_MAX_TOTAL_STEEPNESS + 1e-6,
                        "total steepness {total} exceeded the cap at {speed_step} m/s \
                         gust {} scale {}",
                        field.gust,
                        field.amplitude_scale
                    );
                }
            }
        }
    }

    #[test]
    fn the_model_reproduces_the_cox_munk_slope() {
        // THE calibration test, and the one that would have caught the 4x-too-flat build.
        // It measures the RMS slope of the actual surface over a patch and compares it to
        // Cox & Munk recomputed independently — not to the implementation's own
        // `rms_slope()`, which would only be checking the algebra against itself.
        let clock = clock_at(3.5);
        for speed in [1.0_f32, 3.0, 4.8, 8.0, 12.0] {
            let field = WaveField {
                bearing_radians: 0.6,
                speed_meters_per_second: speed,
                // No gust: it redistributes variance across components and the total is
                // what is being checked, so leaving it in would test two things at once.
                gust: 0.0,
                eddy: 0.0,
                amplitude_scale: 1.0,
            };

            // Sample on a lattice whose spacing shares no factor with any wavelength, so
            // the phases are effectively decorrelated.
            let mut sum_squared = 0.0;
            let mut count = 0.0;
            for x_step in 0..96 {
                for z_step in 0..96 {
                    let position = Vec2::new(x_step as f32 * 0.173, z_step as f32 * 0.229);
                    let slope = field.height_gradient(position, clock, 0.0).length();
                    sum_squared += slope * slope;
                    count += 1.0;
                }
            }
            let measured = (sum_squared / count).sqrt();
            let expected = cox_munk_rms_slope(speed);

            // 12% — the residual is the finite sample and the four-component
            // discretisation of a continuous spectrum, not a modelling disagreement.
            assert!(
                (measured - expected).abs() < expected * 0.12,
                "at {speed} m/s the surface measured {measured:.4} RMS slope \
                 ({:.2} deg) but Cox & Munk says {expected:.4} ({:.2} deg)",
                measured.atan().to_degrees(),
                expected.atan().to_degrees()
            );
        }
    }

    // ---- E7: turbidity, the depth fade -----------------------------------------

    #[test]
    fn pure_water_is_far_too_clear_to_hide_a_bed() {
        // The measurement that motivated turbidity, kept as a test so the premise cannot
        // quietly change: with turbidity off, the model is PURE water and a bed 3 blocks
        // down keeps 0.835 of its blue. The look asks for ~0.05 — 16.8x too see-through,
        // which is why a shallow bed read as one flat cyan sheet with no fade.
        //
        // This is not a bug in the coefficients. Clear water at 3 m genuinely does not hide
        // a sand bed; that is what a tropical lagoon looks like. The missing term is what
        // is IN the water.
        let pure_water = WaterSettings {
            absorption_scale: 1.0,
            scattering_scale: 1.0,
            visibility_depth_blocks: 0.0, // no turbidity at all
            ..WaterSettings::default()
        };
        let pure = extinction_per_meter(&pure_water);
        assert_eq!(pure, water_extinction_per_meter());

        let blue_at_three_blocks = (-pure[2] * 3.0 * WORLD_VOXEL_SIZE_METERS).exp();
        assert!(
            (blue_at_three_blocks - 0.835).abs() < 0.001,
            "pure water passes {blue_at_three_blocks:.3} of blue at 3 blocks"
        );
        assert!(
            blue_at_three_blocks / (-3.0_f32).exp() > 16.0,
            "the gap against a grey 1/m reference should be ~16.8x"
        );
    }

    #[test]
    fn turbidity_puts_the_visibility_horizon_where_the_lever_says() {
        // The lever is a depth in BLOCKS and the shader wants a per-metre coefficient, so
        // the relation `exp(-turbidity * depth) = VISIBILITY_THRESHOLD` is what the two
        // agree on. Checked by inverting it, not by restating the algebra.
        for depth_blocks in [1.0_f32, 3.0, 8.0, 24.0] {
            let turbidity = turbidity_per_meter(depth_blocks);
            let left = (-turbidity * depth_blocks * WORLD_VOXEL_SIZE_METERS).exp();
            assert!(
                (left - VISIBILITY_THRESHOLD).abs() < 1e-6,
                "at a {depth_blocks}-block horizon turbidity alone left {left:.4}, \
                 not the stated {VISIBILITY_THRESHOLD}"
            );
        }
        // Clearer water = further horizon, monotonically, and zero disables it entirely.
        assert!(turbidity_per_meter(3.0) > turbidity_per_meter(8.0));
        assert_eq!(turbidity_per_meter(0.0), 0.0);
        assert_eq!(turbidity_per_meter(-5.0), 0.0);
    }

    #[test]
    fn a_stated_visibility_depth_fades_the_bed_out_over_that_depth() {
        // The mechanism, checked at an arbitrary depth rather than at whatever the shipped
        // look happens to be — so retuning the look cannot silently break the relation the
        // lever advertises. A FADE means each block passes visibly less than the one above.
        let settings = WaterSettings {
            absorption_scale: 1.0,
            scattering_scale: 1.0,
            visibility_depth_blocks: 3.0,
            ..WaterSettings::default()
        };
        let extinction = extinction_per_meter(&settings);
        let expected = [1.2175_f32, 0.8875, 0.8275];
        for channel in 0..3 {
            assert!(
                (extinction[channel] - expected[channel]).abs() < 0.001,
                "channel {channel} extinction {} drifted from {}",
                extinction[channel],
                expected[channel]
            );
        }

        let one = transmittance_over(&settings, 1.0);
        let three = transmittance_over(&settings, 3.0);
        let six = transmittance_over(&settings, 6.0);
        assert!(
            (one[2] - 0.437).abs() < 0.005 && (three[2] - 0.083).abs() < 0.005,
            "blue read {:.3} at one block and {:.3} at three",
            one[2],
            three[2]
        );
        assert!(
            three.iter().all(|&value| value <= VISIBILITY_THRESHOLD),
            "nothing may exceed the visibility threshold at the stated depth: {three:?}"
        );
        assert!(
            six.iter().all(|&value| value < 0.01),
            "twice the stated depth must be effectively opaque: {six:?}"
        );
        for channel in 0..3 {
            let mut previous = 1.0;
            for blocks in 1..=6 {
                let current = transmittance_over(&settings, blocks as f32)[channel];
                assert!(
                    current < previous * 0.95,
                    "channel {channel} barely changed between blocks: {previous} -> {current}"
                );
                previous = current;
            }
        }
    }

    #[test]
    fn the_shipped_look_is_the_one_that_was_dialled_in() {
        // The five runtime numbers in `WaterSettings::default` were dialled in the app against
        // a real pool (Pascal, 2026-08-06: "this should be default"), so what they MEASURE is
        // worth recording — a look arrived at by dragging sliders is exactly the kind that
        // drifts silently when something upstream changes.
        let shipped = WaterSettings::default();
        assert_eq!(shipped.absorption_scale, 0.0);
        assert_eq!(shipped.scattering_scale, 0.15);
        assert_eq!(shipped.ray_cutoff, 0.0);
        assert_eq!(shipped.wave_amplitude_scale, 0.21);
        assert_eq!(shipped.visibility_depth_blocks, 10.0);

        // With water's OWN absorption dialled to zero, the medium is turbidity's almost
        // entirely — and turbidity is grey, so the water is very nearly NEUTRAL: the spread
        // across channels is under 3%, against a factor of 30 for pure water.
        let extinction = extinction_per_meter(&shipped);
        let expected = [0.23086_f32, 0.23476, 0.23701];
        for channel in 0..3 {
            assert!(
                (extinction[channel] - expected[channel]).abs() < 0.0005,
                "channel {channel} extinction {} drifted from {}",
                extinction[channel],
                expected[channel]
            );
        }
        assert!(
            extinction[2] / extinction[0] < 1.03,
            "the shipped medium should be near-neutral, got {extinction:?}"
        );
        let pure = water_extinction_per_meter();
        assert!(
            pure[0] / pure[2] > 7.0,
            "pure water is steeply spectral: {pure:?}"
        );

        // **This is Pascal's own instinct carried to its conclusion** (2026-07-31: "water
        // shouldn't have a colour really .. water blocks light coming in"). The albedo is dark
        // and near-grey, so what you see in the water is what it reflects and what shows
        // through it, not a tint the medium painted on.
        let albedo = albedo_of(&shipped);
        let expected_albedo = [0.1522_f32, 0.1663, 0.1742];
        for channel in 0..3 {
            assert!(
                (albedo[channel] - expected_albedo[channel]).abs() < 0.0005,
                "channel {channel} albedo {} drifted from {}",
                albedo[channel],
                expected_albedo[channel]
            );
        }

        // And the depth behaviour the dialling was after: see clearly into the first block,
        // half-way down at three, at the threshold by ten. "Water you can see into but not
        // through" — much further out than the 3 blocks the first E7 build shipped.
        let one = transmittance_over(&shipped, 1.0);
        let three = transmittance_over(&shipped, 3.0);
        let ten = transmittance_over(&shipped, 10.0);
        assert!((one[2] - 0.789).abs() < 0.005, "one block: {one:?}");
        assert!((three[2] - 0.491).abs() < 0.005, "three blocks: {three:?}");
        assert!(
            ten.iter().all(|&value| value <= VISIBILITY_THRESHOLD),
            "ten blocks must reach the stated horizon: {ten:?}"
        );
    }

    /// The deep-water colour at a visibility depth and a milkiness split, with the material's
    /// own dials at 1.0 so the split is the only thing varying.
    fn turbid_albedo(depth_blocks: f32, scattering_fraction: f32) -> [f32; 3] {
        albedo_of(&WaterSettings {
            absorption_scale: 1.0,
            scattering_scale: 1.0,
            visibility_depth_blocks: depth_blocks,
            turbidity_scattering_fraction: scattering_fraction,
            ..WaterSettings::default()
        })
    }

    #[test]
    fn the_milkiness_split_is_what_made_the_first_build_white() {
        // Pascal on the first E7 build: "now it looks al hazy and white". This test is that
        // report turned into numbers, and the reason the split became a dial.
        //
        // The albedo IS the deep water's colour (derived, not painted), and the in-scattered
        // radiance is albedo x downwelling x (1 - transmittance). So a high albedo does not
        // merely tint the depths — it makes even ONE BLOCK of water return a large fraction
        // of the sky, which is a white sheet over everything.
        let in_scatter_one_block = |fraction: f32| {
            let extinction = extinction_per_meter(&WaterSettings {
                absorption_scale: 1.0,
                scattering_scale: 1.0,
                visibility_depth_blocks: 3.0,
                turbidity_scattering_fraction: fraction,
                ..WaterSettings::default()
            });
            let albedo = turbid_albedo(3.0, fraction);
            let mut result = [0.0_f32; 3];
            for channel in 0..3 {
                let transmittance = (-extinction[channel] * WORLD_VOXEL_SIZE_METERS).exp();
                result[channel] = albedo[channel] * (1.0 - transmittance);
            }
            result
        };

        // Mineral silt (0.85): a milky river, and far too much of the sky comes back.
        let silty = in_scatter_one_block(0.85);
        assert!(
            silty.iter().all(|&value| value > 0.35),
            "at 0.85 one block should return more than a third of the sky: {silty:?}"
        );
        // Organics (the shipped 0.15): water again.
        let shipped = in_scatter_one_block(TURBIDITY_SCATTERING_FRACTION);
        assert!(
            shipped.iter().all(|&value| value < 0.12),
            "at {TURBIDITY_SCATTERING_FRACTION} one block should return under 12% of the \
             sky: {shipped:?}"
        );
        // A factor of four between the two settings — the whole of the fix.
        assert!(silty[2] / shipped[2] > 4.0);
    }

    #[test]
    fn turbid_water_darkens_without_losing_its_tint() {
        // What the shipped split has to preserve. Two properties, and they pull opposite
        // ways: dark enough that the depths are not a haze, and still ORDERED
        // blue-over-green-over-red, or the water has stopped being water.
        let pure = single_scattering_albedo();
        assert!(pure[0] < 0.02 && pure[2] > 0.7, "pure water: {pure:?}");

        let albedo = turbid_albedo(3.0, TURBIDITY_SCATTERING_FRACTION);
        let expected = [0.098_f32, 0.164, 0.194];
        for channel in 0..3 {
            assert!(
                (albedo[channel] - expected[channel]).abs() < 0.005,
                "channel {channel} albedo {} drifted from {}",
                albedo[channel],
                expected[channel]
            );
        }
        assert!(
            albedo[2] > albedo[1] && albedo[1] > albedo[0],
            "the tint order must survive: {albedo:?}"
        );

        // The dial spans a real range, monotonically: pure absorption is nearly black and
        // full scattering is milk. That span is what makes it worth dragging.
        let absorbing = turbid_albedo(3.0, 0.0);
        let milk = turbid_albedo(3.0, 1.0);
        assert!(
            absorbing[2] < 0.06,
            "0 should go nearly black: {absorbing:?}"
        );
        assert!(milk[2] > 0.9, "1 should be milk: {milk:?}");
        for fraction in [0.0_f32, 0.15, 0.3, 0.5, 0.85, 1.0].windows(2) {
            assert!(
                turbid_albedo(3.0, fraction[1])[2] > turbid_albedo(3.0, fraction[0])[2],
                "the dial must be monotone across {fraction:?}"
            );
        }
    }

    #[test]
    fn the_turbidity_terms_reach_the_shader_from_the_right_slots() {
        // Turbidity is a RUNTIME pair — magnitude in `water_optics.w`, split in
        // `water_params.w` — so both are draggable without a pipeline rebuild. Nothing else
        // checks that the shader reads the slots the CPU writes.
        let source = SHADER_SOURCE.as_str();
        assert!(
            source.contains("max(lighting.water_optics.w, 0.0)"),
            "turbidity's magnitude must come from water_optics.w and never go negative"
        );
        assert!(
            source.contains("clamp(lighting.water_params.w, 0.0, 1.0)"),
            "the milkiness split must come from water_params.w, clamped to a fraction"
        );
        // Added OUTSIDE the two material scales, so "how clear is this water" cannot also
        // scale how much is suspended in it — the visibility horizon has to mean what the
        // lever says.
        assert!(source.contains("fn water_turbidity_per_meter()"));
        assert!(source.contains("fn water_turbidity_scattering_fraction()"));
        // And the default the settings ship must be the one the registry row advertises.
        assert_eq!(
            WaterSettings::default().turbidity_scattering_fraction,
            TURBIDITY_SCATTERING_FRACTION
        );
    }

    // ---- E7: caustics ------------------------------------------------------------

    #[test]
    fn analytic_hessian_matches_a_finite_difference_of_the_gradient() {
        // The same rule the gradient is held to, one derivative up: the analytic second
        // derivative must agree with a numerical differentiation of the analytic FIRST one.
        // If the two agree, the caustic Jacobian is differentiating the surface that is
        // actually being rendered.
        let field = strong_wind();
        let clock = clock_at(5.75);
        let epsilon = 0.002;

        for step in 0..24 {
            let position = Vec2::new(step as f32 * 0.37 - 4.0, step as f32 * -0.53 + 2.0);
            let analytic = field.height_hessian(position, clock, 0.0);

            let gradient_at = |offset: Vec2| field.height_gradient(position + offset, clock, 0.0);
            let dx = Vec2::new(epsilon, 0.0);
            let dz = Vec2::new(0.0, epsilon);
            let numerical_xx = (gradient_at(dx).x - gradient_at(-dx).x) / (2.0 * epsilon);
            let numerical_zz = (gradient_at(dz).y - gradient_at(-dz).y) / (2.0 * epsilon);
            // The mixed partial, from either direction — they must also agree with each
            // other, which is Schwarz's theorem and a check that the analytic form is a
            // genuine Hessian rather than two unrelated numbers.
            let numerical_xz = (gradient_at(dz).x - gradient_at(-dz).x) / (2.0 * epsilon);
            let numerical_zx = (gradient_at(dx).y - gradient_at(-dx).y) / (2.0 * epsilon);

            assert!(
                (analytic.x - numerical_xx).abs() < 0.05,
                "d2h/dx2: analytic {} vs numerical {numerical_xx}",
                analytic.x
            );
            assert!(
                (analytic.y - numerical_zz).abs() < 0.05,
                "d2h/dz2: analytic {} vs numerical {numerical_zz}",
                analytic.y
            );
            assert!(
                (analytic.z - numerical_xz).abs() < 0.05
                    && (numerical_xz - numerical_zx).abs() < 0.05,
                "mixed partial disagreed: analytic {} vs {numerical_xz} / {numerical_zx}",
                analytic.z
            );
        }
    }

    #[test]
    fn flat_water_casts_no_caustics_at_all() {
        // The isolation rule again: a flat surface has zero curvature, so it focuses
        // nothing and the sun term must come through completely untouched. Exact equality —
        // a gain of 1.0000001 would still be a change to every submerged pixel.
        let clock = clock_at(3.0);
        let flat = WaveField {
            amplitude_scale: 0.0,
            ..strong_wind()
        };
        assert_eq!(
            flat.height_hessian(Vec2::new(3.0, 4.0), clock, 0.0),
            Vec3::ZERO
        );
        for depth in [0.0_f32, 1.0, 3.0, 10.0] {
            assert_eq!(
                flat.caustic_gain(Vec2::new(3.0, 4.0), clock, depth, 1.333),
                1.0
            );
        }
        // And a surface at zero depth cannot have focused anything yet, however rough it is:
        // the rays have had no distance over which to converge.
        assert_eq!(
            strong_wind().caustic_gain(Vec2::new(3.0, 4.0), clock, 0.0, 1.333),
            1.0
        );
        // Nor can a medium that does not bend light (the refraction dial fully open).
        assert_eq!(
            strong_wind().caustic_gain(Vec2::new(3.0, 4.0), clock, 2.0, 1.0),
            1.0
        );
    }

    #[test]
    fn caustics_brighten_and_darken_around_unity_rather_than_only_adding() {
        // What separates a caustic from a brightness offset: focusing REDISTRIBUTES light,
        // so a bed must have dim patches as well as bright filaments, and the average must
        // stay near 1. Measured over a patch, because that is the only way to know.
        //
        // Measured at the wind model's floor and at a gale, one block down:
        //   1 m/s  -> mean 1.01, range 0.86..1.19   (a gentle shimmer)
        //   12 m/s -> mean 1.19, range 0.36..4.00   (filaments, and dark between them)
        let clock = clock_at(9.25);
        for (speed, expected_mean) in [(1.0_f32, 1.01_f32), (12.0, 1.19)] {
            let field = WaveField {
                bearing_radians: 0.6,
                speed_meters_per_second: speed,
                gust: 0.0,
                eddy: 0.0,
                amplitude_scale: 1.0,
            };
            let mut total = 0.0;
            let mut count = 0.0;
            let mut brightest: f32 = 0.0;
            let mut dimmest = f32::MAX;
            for x_step in 0..128 {
                for z_step in 0..128 {
                    let position = Vec2::new(x_step as f32 * 0.173, z_step as f32 * 0.229);
                    let gain = field.caustic_gain(position, clock, 1.0, 1.333);
                    total += gain;
                    brightest = brightest.max(gain);
                    dimmest = dimmest.min(gain);
                    count += 1.0;
                }
            }
            let mean = total / count;
            assert!(
                (mean - expected_mean).abs() < 0.05,
                "at {speed} m/s the mean gain was {mean:.3}, expected {expected_mean:.2}"
            );
            assert!(
                brightest > 1.05 && dimmest < 0.95,
                "at {speed} m/s the gain never went both ways: {dimmest:.3}..{brightest:.3}"
            );
            // Not a free brightness boost: the mean stays within 25% of unity, so the
            // sunlight is being MOVED rather than manufactured.
            assert!(
                (mean - 1.0).abs() < 0.25,
                "at {speed} m/s caustics changed total sun energy by {:.0}%",
                (mean - 1.0) * 100.0
            );
        }
    }

    #[test]
    fn caustic_gain_stays_within_its_stated_bounds() {
        // The divergence guard, over the whole range the model can reach: rough water, deep
        // beds, every phase. Without the cap `1/|det J|` is unbounded at a focus, and one
        // pixel of infinity is a white hole in the bed.
        let field = strong_wind();
        for depth_step in 0..40 {
            let depth = depth_step as f32 * 0.5;
            for time_step in 0..8 {
                let clock = clock_at(time_step as f32 * 1.37);
                for step in 0..64 {
                    let position = Vec2::new(step as f32 * 0.31, step as f32 * -0.19);
                    let gain = field.caustic_gain(position, clock, depth, 1.333);
                    assert!(
                        gain.is_finite() && (0.0..=CAUSTIC_MAX_GAIN).contains(&gain),
                        "gain {gain} escaped [0, {CAUSTIC_MAX_GAIN}] at depth {depth}"
                    );
                }
            }
        }
    }

    #[test]
    fn the_wgsl_caustics_mirror_the_rust_ones() {
        assert_eq!(shader_float("WATER_CAUSTIC_MAX_GAIN"), CAUSTIC_MAX_GAIN);
        let source = SHADER_SOURCE.as_str();
        assert!(source.contains("fn wave_height_hessian("));
        // Caustics must scale the SUN term only. Ambient light is not focused by anything,
        // and scaling it would just be a contrast knob on every submerged pixel.
        let start = source
            .find("fn water_sun_transmission(")
            .expect("the sun path must exist");
        let end = source[start..]
            .find("\n}\n")
            .expect("unterminated function")
            + start;
        assert!(
            source[start..end].contains("water_caustic_gain(medium, water_material)"),
            "the caustic gain must ride on the sun's own transmittance"
        );
        // The CA pass shades nothing, so caustics must be absent there rather than unused.
        assert!(!CAGI_SHADER_SOURCE.contains("water_caustic_gain"));
    }

    #[test]
    fn the_bounce_light_is_a_second_sun_and_not_a_sky_sample() {
        // The trap this test exists for, because it is a units error rather than a visible
        // one: `sky_color` carries the sun DISC, whose radiance is enormous (the disc
        // subtends 6.8e-5 sr). A diffuse surface integrates radiance over solid angle, so
        // feeding disc radiance through a made-up strength factor is thousands of times too
        // bright — and it would also put a CLOUD MARCH on every shaded pixel in the frame.
        //
        // The fix is structural: the mirrored sun's magnitude must be the sun's own term.
        let source = SHADER_SOURCE.as_str();
        let start = source
            .find("fn water_bounce_light(")
            .expect("the bounce light must exist");
        let end = source[start..]
            .find("\n}\n")
            .expect("unterminated function")
            + start;
        let body = &source[start..end];

        assert!(
            body.contains("lighting.sun_color_intensity.rgb * lighting.sun_color_intensity.w"),
            "the bounce must be built from the SUN's radiance, in the same units as the \
             direct sun term"
        );
        assert!(
            !body.contains("sky_color("),
            "sky_color carries the sun disc and runs a cloud march; neither belongs on \
             every shaded surface"
        );
        // The lobe width is DERIVED from the wave field's roughness (a normal tilted by s
        // deflects a reflection by 2s), not authored as a gloss parameter.
        assert!(
            body.contains("2.0 * wave_rms_slope()"),
            "the lobe width must come from the measured surface roughness"
        );
        // And the double-count guards: water lighting water, and a submerged surface that
        // already receives the sun through the surface with caustics on it.
        assert!(body.contains("material_is_liquid(hit.material)"));
        assert!(body.contains("point_is_submerged(surface_point)"));
        // Bounded reach, because this is a ray per shaded surface.
        assert!(body.contains("WATER_BOUNCE_MAX_DISTANCE_METERS / brickmap.voxel_size_meters"));

        // Flat water aims its reflection exactly at the mirrored sun, so the lobe must be
        // 1.0 there — the band exists on a still pool, it just does not wobble. Recomputed
        // here from the shader's own constants rather than restated.
        let minimum_lobe = shader_float("WATER_BOUNCE_MIN_LOBE_RADIANS");
        let flat_alignment = 1.0_f32;
        let lobe = (-2.0 * (1.0 - flat_alignment) / (minimum_lobe * minimum_lobe)).exp();
        assert_eq!(lobe, 1.0);

        // A gale's lobe is far wider than a calm one's: 2 * sigma, so 28.6 deg against
        // 10.3 deg at the wind floor. That ratio IS the "broad soft band vs tight bright
        // spot" difference, and it comes out of Cox & Munk rather than a taste knob.
        // (Slope is a tangent, not an angle; at these magnitudes the two agree to well
        // under a degree — 2*atan(0.2496) = 28.1 against 2*0.2496 = 28.6.)
        let calm = 2.0
            * WaveField {
                bearing_radians: 0.0,
                speed_meters_per_second: 1.0,
                gust: 0.0,
                eddy: 0.0,
                amplitude_scale: 1.0,
            }
            .rms_slope();
        let gale = 2.0 * strong_wind().rms_slope();
        assert!(
            (calm.to_degrees() - 10.3).abs() < 0.2,
            "calm lobe half-width {:.2} deg",
            calm.to_degrees()
        );
        assert!(
            gale > calm * 2.0,
            "a gale must spread the band much wider: {:.2} vs {:.2} deg",
            gale.to_degrees(),
            calm.to_degrees()
        );
    }

    #[test]
    fn dead_calm_still_carries_a_small_ripple() {
        // "There should always be a small ripple" (Pascal, 2026-08-06), as a number.
        //
        // The claim under test is that the ripple needs no floor of its own — Cox & Munk's
        // 0.003 intercept IS the floor, so the surface is never glass and the only thing
        // that flattens it is the amplitude lever. Measured on the surface, exactly as
        // `the_model_reproduces_the_cox_munk_slope` does, so it cannot pass by checking
        // `rms_slope()` against itself.
        //
        // The whole range in degrees of RMS slope:
        //   0 m/s (dead calm) -> sigma 0.0548 ->  3.135 deg  <- the always-on ripple
        //   1 m/s (wind floor)-> sigma 0.0901 ->  5.150 deg
        //  12 m/s (gale)      -> sigma 0.2496 -> 14.000 deg
        let clock = clock_at(3.5);
        for (speed, expected_degrees) in [(0.0_f32, 3.135_f32), (1.0, 5.15), (12.0, 14.00)] {
            let field = WaveField {
                bearing_radians: 0.6,
                speed_meters_per_second: speed,
                gust: 0.0,
                eddy: 0.0,
                amplitude_scale: 1.0,
            };
            assert!(
                !field.is_flat(),
                "at {speed} m/s the field reported itself flat, so no ripple would render"
            );

            let mut sum_squared = 0.0;
            let mut count = 0.0;
            let mut steepest: f32 = 0.0;
            for x_step in 0..96 {
                for z_step in 0..96 {
                    let position = Vec2::new(x_step as f32 * 0.173, z_step as f32 * 0.229);
                    let slope = field.height_gradient(position, clock, 0.0).length();
                    sum_squared += slope * slope;
                    steepest = steepest.max(slope);
                    count += 1.0;
                }
            }
            let measured_degrees = (sum_squared / count).sqrt().atan().to_degrees();
            assert!(
                (measured_degrees - expected_degrees).abs() < 0.6,
                "at {speed} m/s the surface measured {measured_degrees:.2} deg RMS slope, \
                 expected {expected_degrees:.2}"
            );
            // Small, and stated as such: even the gale's steepest sampled point stays well
            // inside the refraction invariant, and dead calm is a ripple rather than chop.
            assert!(
                steepest.atan().to_degrees() < 41.4,
                "at {speed} m/s the steepest point tilted {:.2} deg, past the refraction \
                 invariant",
                steepest.atan().to_degrees()
            );
            assert_ne!(
                field.surface_normal(Vec2::new(7.5, -3.25), clock, 0.0),
                Vec3::Y,
                "at {speed} m/s the surface came back as the flat voxel face"
            );
        }
    }

    #[test]
    fn a_splash_on_top_of_wind_cannot_fold_the_surface_past_breaking() {
        // W6's requirement, now that the two sources are SUMMED rather than exclusive: the
        // splash term rides on a gradient the wind field already capped, so the cap has to
        // be re-applied to the sum or a jump into a rough sea folds the surface.
        //
        // Worst case, by construction: the gale's own gradient (bounded by
        // WAVE_MAX_TOTAL_STEEPNESS = 0.75) plus one ring at full envelope and full strength
        // (RIPPLE_STEEPNESS = 0.35, from `shaders/water.wgsl`), aligned. 1.10 uncapped.
        let ripple_steepness = shader_float("RIPPLE_STEEPNESS");
        assert_eq!(
            ripple_steepness, WAVE_MAX_STEEPNESS,
            "a splash ring is allowed exactly one breaking-limit component's worth of slope"
        );

        let wind = Vec2::new(WAVE_MAX_TOTAL_STEEPNESS, 0.0);
        let uncapped = wind + Vec2::new(ripple_steepness, 0.0);
        assert!(
            (uncapped.length() - 1.10).abs() < 1e-6,
            "the worst case should be 1.10 of steepness, got {}",
            uncapped.length()
        );

        // Uncapped, that is 47.7 deg of tilt — past the 41.4 deg the refracted ray's
        // missing guard depends on. Capped, it is exactly the invariant's 36.9 deg.
        assert!(uncapped.length().atan().to_degrees() > 41.4);
        let capped = clamp_surface_gradient(uncapped);
        assert!(
            (capped.length() - WAVE_MAX_TOTAL_STEEPNESS).abs() < 1e-6,
            "the cap must bind at exactly WAVE_MAX_TOTAL_STEEPNESS, got {}",
            capped.length()
        );
        assert!((capped.length().atan().to_degrees() - 36.87).abs() < 0.01);

        // Scaling, not clipping: capping flattens the normal without rotating it.
        let skew = clamp_surface_gradient(Vec2::new(0.9, 1.2));
        assert!(
            (skew.normalize() - Vec2::new(0.9, 1.2).normalize()).length() < 1e-6,
            "the cap rotated the gradient"
        );
        // And it is inert below the bound, so the wind field alone is untouched.
        let calm = Vec2::new(0.05, -0.02);
        assert_eq!(clamp_surface_gradient(calm), calm);
    }

    #[test]
    fn the_two_ripple_sources_are_summed_rather_than_exclusive() {
        // The bug this pair of stages actually had: `RIPPLE_USE_WIND_FIELD = false` returned
        // the splash normal before the wave field was ever evaluated, so W1-W5 was dead code
        // and the weather's wind speed moved nothing. Structural, so it is checked
        // structurally — there is no CPU mirror of the splash term to measure.
        let source = SHADER_SOURCE.as_str();
        // The declaration, not the name — the section header keeps the name in prose so the
        // next reader learns why the two sources are structured this way.
        assert!(
            !source.contains("const RIPPLE_USE_WIND_FIELD"),
            "the exclusive switch is gone; the two sources add"
        );

        let start = source
            .find("fn water_surface_normal(")
            .expect("the surface normal must exist");
        let end = source[start..]
            .find("\n}\n")
            .expect("unterminated function")
            + start;
        let body = &source[start..end];

        assert!(
            body.contains("gradient = wave_height_gradient("),
            "the wind field must reach the surface normal"
        );
        assert!(
            body.contains("gradient = gradient + water_splash_gradient("),
            "splash rings must ADD to the wind gradient, not replace it"
        );
        assert!(
            body.contains("water_clamp_surface_gradient(gradient)"),
            "the summed gradient must go through the shared cap"
        );
    }

    #[test]
    fn the_refracted_ray_always_stays_under_the_face() {
        // Why `water_surface_radiance` needs no guard on the refracted ray, checked
        // numerically over the whole input range rather than trusted from the algebra.
        // This is what WAVE_MAX_TOTAL_STEEPNESS is FOR, so if the cap is ever raised this
        // test is the thing that objects.
        let critical = (1.0_f32 / 1.333).asin();
        for speed_step in 0..=30 {
            for gust_step in 0..=10 {
                let field = WaveField {
                    bearing_radians: 0.6,
                    speed_meters_per_second: speed_step as f32,
                    gust: gust_step as f32 / 10.0,
                    eddy: 1.0,
                    amplitude_scale: 1.0,
                };
                let tilt = field.total_steepness().atan();
                let worst_from_vertical = tilt + critical;
                assert!(
                    worst_from_vertical < std::f32::consts::FRAC_PI_2,
                    "at {speed_step} m/s a refracted ray could reach {:.1} deg from \
                     vertical, i.e. above the face",
                    worst_from_vertical.to_degrees()
                );
            }
        }
    }

    #[test]
    fn analytic_gradient_matches_finite_difference() {
        // The strong claim of W1: the normal comes from an ANALYTIC derivative, one
        // evaluation instead of three. This checks that derivative against a numerical
        // one of the height field it claims to differentiate.
        let field = strong_wind();
        let step = 1e-3;
        let mut worst = 0.0_f32;
        for x_step in 0..7 {
            for z_step in 0..7 {
                for time_step in 0..5 {
                    let position = Vec2::new(x_step as f32 * 0.83, z_step as f32 * 1.31);
                    let clock = clock_at(time_step as f32 * 0.37);

                    let analytic = field.height_gradient(position, clock, 0.0);
                    let numeric = Vec2::new(
                        (field.height_meters(position + Vec2::new(step, 0.0), clock, 0.0)
                            - field.height_meters(position - Vec2::new(step, 0.0), clock, 0.0))
                            / (2.0 * step),
                        (field.height_meters(position + Vec2::new(0.0, step), clock, 0.0)
                            - field.height_meters(position - Vec2::new(0.0, step), clock, 0.0))
                            / (2.0 * step),
                    );
                    worst = worst.max((analytic - numeric).abs().max_element());
                }
            }
        }
        assert!(
            worst < 2e-3,
            "analytic gradient disagreed with the finite difference by {worst}"
        );
    }

    #[test]
    fn surface_normals_are_unit_and_point_up() {
        let field = strong_wind();
        for x_step in 0..12 {
            for z_step in 0..12 {
                for time_step in 0..6 {
                    let position = Vec2::new(x_step as f32 * 0.71, z_step as f32 * 0.53);
                    let normal =
                        field.surface_normal(position, clock_at(time_step as f32 * 0.41), 0.0);
                    assert!(
                        (normal.length() - 1.0).abs() < 1e-5,
                        "normal {normal} is not unit"
                    );
                    // The total-steepness cap guarantees this, and the bound is DERIVED
                    // from it rather than picked: y is 1 before normalising and the
                    // horizontal terms sum to at most WAVE_MAX_TOTAL_STEEPNESS, so
                    // y >= cos(atan(cap)) = 0.8 at the very worst.
                    let steepest = WAVE_MAX_TOTAL_STEEPNESS.atan().cos();
                    assert!(
                        normal.y >= steepest - 1e-6,
                        "normal {normal} tipped past the cap's {steepest} bound"
                    );
                }
            }
        }
    }

    #[test]
    fn the_surface_has_no_net_tilt() {
        // Averaged over whole spatial periods the gradient must vanish, or the water
        // would have a permanent slope and its mean reflection would be aimed wrong.
        // 60 m is ten periods of the longest component and a whole number of periods of
        // none of the others, which is the point: the residual has to come out small
        // anyway.
        let field = strong_wind();
        let clock = clock_at(3.7);
        let samples = 600;
        let mut sum = Vec2::ZERO;
        for step in 0..samples {
            let along = step as f32 / samples as f32 * 60.0;
            // Sample along the bearing so every component is traversed, not just the
            // ones with an x or z component.
            let (sin_bearing, cos_bearing) = field.bearing_radians.sin_cos();
            sum += field.height_gradient(Vec2::new(cos_bearing, sin_bearing) * along, clock, 0.0);
        }
        let mean = sum / samples as f32;
        assert!(
            mean.length() < 5e-3,
            "the surface has a net tilt of {mean} (length {})",
            mean.length()
        );
    }

    #[test]
    fn flat_water_is_exactly_flat() {
        // The isolation rule: with the AMPLITUDE LEVER off, the wave field must return the
        // voxel face EXACTLY, so the rendered image is bit-identical to the pre-wave
        // renderer. Exact equality, not a tolerance.
        //
        // The lever is the only thing that flattens it. Dead calm does NOT — see
        // `dead_calm_still_carries_a_small_ripple`, which is the other half of this pair.
        let clock = clock_at(12.34);
        let position = Vec2::new(7.5, -3.25);

        let lever_off = WaveField {
            amplitude_scale: 0.0,
            ..strong_wind()
        };

        for field in [WaveField::FLAT, lever_off] {
            assert!(field.is_flat());
            assert_eq!(field.total_steepness(), 0.0);
            assert_eq!(field.height_meters(position, clock, 0.0), 0.0);
            assert_eq!(field.height_gradient(position, clock, 0.0), Vec2::ZERO);
            assert_eq!(
                field.surface_normal(position, clock, 0.0),
                Vec3::Y,
                "flat water must return exactly the geometric face normal"
            );
        }
    }

    #[test]
    fn a_gust_moves_variance_to_the_short_end_without_adding_any() {
        // Wave response time scales with period: a gust ruffles the surface within seconds
        // while the long components cannot answer that fast.
        //
        // It REDISTRIBUTES rather than adds, and that is the load-bearing half — the wind
        // SPEED already carries the gust (`speed = min + (max - min) * activity`), so
        // adding energy here as well would double-count it and overshoot Cox & Munk.
        let calm = WaveField {
            speed_meters_per_second: 6.0,
            gust: 0.0,
            ..strong_wind()
        };
        let gusting = WaveField { gust: 1.0, ..calm };

        let shortest = WAVE_COMPONENTS - 1;
        assert!(
            gusting.component(shortest).steepness() > calm.component(shortest).steepness() * 1.2,
            "a gust must visibly roughen the shortest component"
        );
        assert!(
            gusting.component(0).steepness() < calm.component(0).steepness(),
            "the variance the short end gained has to come from the long end"
        );

        // The total slope VARIANCE — Cox & Munk's quantity — is untouched. Note this sums
        // squares, because variance is what is conserved, not steepness.
        let variance = |field: &WaveField| -> f32 {
            (0..WAVE_COMPONENTS)
                .map(|index| {
                    let steepness = field.component(index).steepness();
                    steepness * steepness / 2.0
                })
                .sum()
        };
        let calm_variance = variance(&calm);
        assert!(
            (variance(&gusting) - calm_variance).abs() < calm_variance * 1e-3,
            "a gust changed the total slope variance: {} vs {calm_variance}",
            variance(&gusting)
        );
    }

    #[test]
    fn the_eddy_channel_moves_only_the_chop() {
        let still = WaveField {
            eddy: 0.0,
            ..strong_wind()
        };
        let churning = WaveField {
            eddy: 1.0,
            ..strong_wind()
        };
        for index in 0..WAVE_COMPONENTS - 1 {
            assert_eq!(
                still.component(index).phase_radians,
                churning.component(index).phase_radians,
                "the eddy channel must not reach component {index}"
            );
        }
        let last = WAVE_COMPONENTS - 1;
        assert!(
            (churning.component(last).phase_radians
                - still.component(last).phase_radians
                - WAVE_EDDY_PHASE_RADIANS)
                .abs()
                < 1e-6,
            "the shortest component must carry the full eddy jitter"
        );
        // Phase, not amplitude — so the steepness cap cannot be disturbed by chop.
        assert_eq!(
            still.component(last).steepness(),
            churning.component(last).steepness()
        );
    }

    #[test]
    fn the_bearing_is_the_cloud_decks_bearing() {
        // The one-wind-history rule at the field level: `voxel_core::weather` turns the
        // bearing into a drift vector as [cos, sin], and component 0 carries no
        // directional fan, so it must BE that vector. W3 asserts the same equality
        // across the uniform.
        for degrees in [0.0_f32, 34.4, 90.0, 217.5, 359.0] {
            let bearing = degrees.to_radians();
            let field = WaveField {
                bearing_radians: bearing,
                ..strong_wind()
            };
            let (sin_bearing, cos_bearing) = bearing.sin_cos();
            let direction = field.component(0).direction;
            assert!(
                (direction - Vec2::new(cos_bearing, sin_bearing))
                    .abs()
                    .max_element()
                    < 1e-6,
                "component 0 at {degrees} deg drifts along {direction}, not the wind bearing"
            );
        }
    }

    #[test]
    fn the_fan_widens_toward_the_short_components() {
        // Angular spreading is narrow at the spectral peak and broad in the tail; a sum
        // of parallel waves would be corrugated iron.
        let field = WaveField {
            bearing_radians: 0.0,
            ..strong_wind()
        };
        let mut previous = 0.0;
        for index in 0..WAVE_COMPONENTS {
            let direction = field.component(index).direction;
            let offset = direction.y.atan2(direction.x).abs();
            assert!(
                offset >= previous - 1e-6,
                "component {index} is less spread than the one before it"
            );
            assert!(
                offset <= WAVE_SPREAD_RADIANS + 1e-6,
                "component {index} escaped the fan"
            );
            previous = offset;
        }
        // And the sides alternate, or the fan would be one-sided.
        assert!(field.component(1).direction.y > 0.0);
        assert!(field.component(2).direction.y < 0.0);
    }

    #[test]
    fn from_wind_reads_the_shared_wind_history() {
        let mut driver =
            voxel_core::wind::WindDriver::new(17, voxel_core::wind::WindShape::default());
        let mut frame = WindFrame::default();
        for _ in 0..200 {
            frame = driver.advance(1.0 / 60.0);
        }
        let field = WaveField::from_wind(frame, 0.6, 1.0);
        assert_eq!(field.speed_meters_per_second, frame.speed);
        assert_eq!(field.gust, frame.gust);
        assert_eq!(field.eddy, frame.eddy);
        assert!(
            field.total_steepness() > 0.0,
            "a live wind history must produce waves"
        );
    }

    #[test]
    fn waves_actually_move() {
        // Dispersion is only worth having if the surface is not static: the same point
        // must have a different slope a moment later.
        let field = strong_wind();
        let position = Vec2::new(3.0, 4.0);
        let now = field.surface_normal(position, clock_at(0.0), 0.0);
        let later = field.surface_normal(position, clock_at(0.25), 0.0);
        assert!(
            (now - later).length() > 1e-3,
            "the surface did not move between frames"
        );
    }

    // ---- W4: the distance fade -----------------------------------------------

    /// A footprint that puts component `index` at exactly `cycles_per_pixel`.
    ///
    /// Expressed this way rather than in metres of camera distance because the criterion
    /// IS cycles per pixel: it holds at every resolution and every render scale, and a
    /// test written in metres would silently encode one screen.
    fn footprint_for(field: &WaveField, index: usize, cycles_per_pixel: f32) -> f32 {
        field.component(index).wavelength_meters * cycles_per_pixel
    }

    #[test]
    fn the_fade_spans_exactly_the_documented_band() {
        let field = strong_wind();
        for index in 0..WAVE_COMPONENTS {
            // Sharper than the ramp's start: fully present.
            let sharp = footprint_for(&field, index, WAVE_LOD_FADE_START_CYCLES_PER_PIXEL * 0.5);
            assert_eq!(field.component_lod_fade(index, sharp), 1.0);
            // Exactly at the start: still fully present (smoothstep's lower edge).
            let start = footprint_for(&field, index, WAVE_LOD_FADE_START_CYCLES_PER_PIXEL);
            assert!((field.component_lod_fade(index, start) - 1.0).abs() < 1e-6);
            // At Nyquist and beyond: gone, because the sinusoid is unrepresentable there.
            let nyquist = footprint_for(&field, index, WAVE_LOD_FADE_END_CYCLES_PER_PIXEL);
            assert_eq!(field.component_lod_fade(index, nyquist), 0.0);
            assert_eq!(field.component_lod_fade(index, nyquist * 4.0), 0.0);
            // And monotone in between, or the ramp would pop.
            let mut previous = 1.0;
            for step in 0..=20 {
                let cycles = WAVE_LOD_FADE_START_CYCLES_PER_PIXEL
                    + (WAVE_LOD_FADE_END_CYCLES_PER_PIXEL - WAVE_LOD_FADE_START_CYCLES_PER_PIXEL)
                        * step as f32
                        / 20.0;
                let fade = field.component_lod_fade(index, footprint_for(&field, index, cycles));
                assert!(
                    fade <= previous + 1e-6,
                    "the fade rose at {cycles} cycles/pixel"
                );
                previous = fade;
            }
        }
        // No footprint at all means "infinitely sharp" — the identity every CPU caller
        // without a camera relies on.
        assert_eq!(field.component_lod_fade(0, 0.0), 1.0);
    }

    #[test]
    fn each_component_fades_at_its_own_distance() {
        // The whole point of fading PER COMPONENT: the chop should disappear into
        // distance while the swell is still visible, which is what real water does. A
        // single whole-field fade would flatten both together.
        let field = strong_wind();
        let shortest = WAVE_COMPONENTS - 1;
        // A footprint that has taken the shortest component past Nyquist.
        let footprint = footprint_for(&field, shortest, WAVE_LOD_FADE_END_CYCLES_PER_PIXEL);
        assert_eq!(field.component_lod_fade(shortest, footprint), 0.0);
        assert!(
            field.component_lod_fade(0, footprint) > 0.99,
            "the 6 m swell must still be there when the 0.6 m chop has gone"
        );
    }

    #[test]
    fn distant_water_goes_smooth_rather_than_sparkly() {
        // The W4 gate as a measurement rather than a look: roughness must fall
        // monotonically with the pixel footprint and reach exactly zero, because a slope
        // that survived into the distance IS the sparkle this fade exists to remove.
        let field = strong_wind();
        let clock = clock_at(2.5);
        let nyquist = footprint_for(&field, 0, WAVE_LOD_FADE_END_CYCLES_PER_PIXEL);

        let mut previous = f32::MAX;
        for step in 0..=10 {
            let footprint = nyquist * step as f32 / 10.0;
            let roughness = field.total_steepness_at(footprint);
            assert!(
                roughness <= previous + 1e-6,
                "roughness grew with distance at footprint {footprint}: \
                 {roughness} > {previous}"
            );
            previous = roughness;

            // And the triangle inequality's promise: this bounds the actual slope
            // everywhere, so no patch of water can be steeper than the distance allows.
            for x_step in 0..12 {
                for z_step in 0..12 {
                    let position = Vec2::new(x_step as f32 * 0.61, z_step as f32 * 0.47);
                    let slope = field.height_gradient(position, clock, footprint).length();
                    assert!(
                        slope <= roughness + 1e-5,
                        "slope {slope} exceeded the roughness bound {roughness}"
                    );
                }
            }
        }

        // The far limit is the flat face exactly — every component past Nyquist.
        assert_eq!(field.total_steepness_at(nyquist), 0.0);
        let position = Vec2::new(3.0, 7.0);
        assert_eq!(field.height_gradient(position, clock, nyquist), Vec2::ZERO);
        assert_eq!(field.surface_normal(position, clock, nyquist), Vec3::Y);
    }

    #[test]
    fn the_fade_can_only_ever_reduce_the_roughness() {
        // Why the steepness cap survives W4 without a second clamp: the fade is a factor
        // in [0, 1] on each component, so the surviving roughness can never exceed the
        // unfaded total, which WAVE_MAX_STEEPNESS already bounds.
        //
        // Note this is asserted on the ROUGHNESS and not on the gradient's magnitude —
        // see `total_steepness_at` for why the latter is not monotone.
        let field = strong_wind();
        let unfaded = field.total_steepness();
        for step in 0..24 {
            let footprint = step as f32 * 0.05;
            let faded = field.total_steepness_at(footprint);
            assert!(
                faded <= unfaded + 1e-6,
                "the fade raised the roughness at footprint {footprint}"
            );
            assert!(faded <= WAVE_MAX_TOTAL_STEEPNESS + 1e-6);
        }
    }

    // ---- W2: the WGSL mirror and the two safety rules ------------------------

    /// Pull a `const NAME: f32 = <literal>;` out of the built shader source.
    fn shader_float(name: &str) -> f32 {
        let source = SHADER_SOURCE.as_str();
        let needle = format!("const {name}: f32 = ");
        let start = source
            .find(&needle)
            .unwrap_or_else(|| panic!("`{name}` is missing from the built shader source"))
            + needle.len();
        let rest = &source[start..];
        let end = rest.find(';').expect("unterminated const");
        rest[..end]
            .trim()
            .parse()
            .unwrap_or_else(|error| panic!("`{name}` is not a float literal: {error}"))
    }

    fn shader_unsigned(name: &str) -> u32 {
        let source = SHADER_SOURCE.as_str();
        let needle = format!("const {name}: u32 = ");
        let start = source
            .find(&needle)
            .unwrap_or_else(|| panic!("`{name}` is missing from the built shader source"))
            + needle.len();
        let rest = &source[start..];
        let end = rest.find(';').expect("unterminated const");
        rest[..end]
            .trim()
            .trim_end_matches('u')
            .parse()
            .unwrap_or_else(|error| panic!("`{name}` is not an unsigned literal: {error}"))
    }

    #[test]
    fn the_wgsl_wave_field_mirrors_the_rust_one() {
        // The load-bearing test of W2. Every number the shader's wave field uses is
        // duplicated in this module by necessity — WGSL cannot call Rust — so the ONLY
        // thing keeping the tested CPU physics and the rendered pixel in agreement is
        // that the two sets of constants are equal. Nothing else in the build checks it.
        assert_eq!(shader_unsigned("WAVE_COMPONENTS"), WAVE_COMPONENTS as u32);
        assert_eq!(
            shader_float("WAVE_GRAVITY"),
            WAVE_GRAVITY_METERS_PER_SECOND_SQUARED
        );
        assert_eq!(shader_float("WAVE_LONGEST_METERS"), WAVE_LONGEST_METERS);
        assert_eq!(shader_float("WAVE_SHORTEST_METERS"), WAVE_SHORTEST_METERS);
        assert_eq!(shader_float("WAVE_MAX_STEEPNESS"), WAVE_MAX_STEEPNESS);
        assert_eq!(shader_float("WAVE_SPREAD_RADIANS"), WAVE_SPREAD_RADIANS);
        assert_eq!(shader_float("WAVE_GUST_SHORT_BIAS"), WAVE_GUST_SHORT_BIAS);
        assert_eq!(
            shader_float("WAVE_MAX_TOTAL_STEEPNESS"),
            WAVE_MAX_TOTAL_STEEPNESS
        );
        assert_eq!(
            shader_float("WAVE_SLOPE_VARIANCE_INTERCEPT"),
            WAVE_SLOPE_VARIANCE_INTERCEPT
        );
        assert_eq!(
            shader_float("WAVE_SLOPE_VARIANCE_PER_METER_PER_SECOND"),
            WAVE_SLOPE_VARIANCE_PER_METER_PER_SECOND
        );
        assert_eq!(
            shader_float("WAVE_EDDY_PHASE_RADIANS"),
            WAVE_EDDY_PHASE_RADIANS
        );
        assert_eq!(shader_float("WAVE_GOLDEN_RATIO"), WAVE_GOLDEN_RATIO);
        assert_eq!(
            shader_float("WAVE_LOD_FADE_START_CYCLES_PER_PIXEL"),
            WAVE_LOD_FADE_START_CYCLES_PER_PIXEL
        );
        assert_eq!(
            shader_float("WAVE_LOD_FADE_END_CYCLES_PER_PIXEL"),
            WAVE_LOD_FADE_END_CYCLES_PER_PIXEL
        );
        assert_eq!(
            shader_float("WATER_REFLECTION_MIN_COSINE"),
            WATER_REFLECTION_MIN_COSINE
        );
        // And the steepness cap must still sit below the Stokes breaking limit on both
        // sides, which is the property the whole safety argument rests on.
        assert!(shader_float("WAVE_MAX_STEEPNESS") < 0.443);
    }

    #[test]
    fn the_wave_lever_folds_the_field_away() {
        let flat = WaterSettings {
            waves: false,
            ..WaterSettings::default()
        };
        let source = flat.patch_shader_source(&SHADER_SOURCE);
        assert!(
            source.contains("const WATER_WAVES: bool = false;"),
            "the wave lever must reach the built source"
        );
        // Default ships waves ON, so the identity check in
        // `default_settings_match_shader_source` already pins the other direction.
        assert!(SHADER_SOURCE.contains("const WATER_WAVES: bool = true;"));
        // The CA pass has no camera and no surface to look at, so the const must be
        // absent there rather than merely unused.
        assert!(!CAGI_SHADER_SOURCE.contains("const WATER_WAVES:"));
    }

    #[test]
    fn the_shading_path_keeps_the_two_normals_apart() {
        // Trap 1: `geometric` does the GEOMETRY jobs (offsetting a secondary ray off the
        // face it escapes) and `optics` does the LIGHT jobs (Fresnel, the mirror
        // direction, Snell's bend). Offsetting along a perturbed normal moves a ray in a
        // direction unrelated to the face it is leaving, and it self-intersects.
        let source = SHADER_SOURCE.as_str();
        let start = source
            .find("fn water_surface_radiance(")
            .expect("the water composition must exist");
        let end = source[start..]
            .find("\n}\n")
            .expect("unterminated function")
            + start;
        let body = &source[start..end];

        assert!(
            body.contains("shadow_ray_origin(hit, ray_origin, ray_direction, geometric)"),
            "the mirror ray's origin must be biased along the GEOMETRIC normal"
        );
        assert!(
            body.contains("water_interior_origin(hit, ray_origin, ray_direction, geometric)"),
            "the refracted ray's origin must be biased along the GEOMETRIC normal"
        );
        assert!(
            body.contains("reflect(ray_direction, optics)"),
            "the mirror DIRECTION must come from the wave normal, or there is no glitter"
        );
        assert!(
            body.contains("refract_at(ray_direction, optics,"),
            "Snell's bend must use the wave normal"
        );
        assert!(
            body.contains("-dot(ray_direction, optics)"),
            "Fresnel's weight must use the wave normal"
        );
        assert!(
            !body.contains("shadow_ray_origin(hit, ray_origin, ray_direction, optics)"),
            "a bias along the wave normal would self-intersect the face"
        );
    }

    #[test]
    fn a_lifted_mirror_ray_never_re_enters_the_water() {
        // Trap 2, over the worst case the steepness cap allows: a grazing view and a
        // normal tilted the full atan(WAVE_MAX_STEEPNESS) toward the ray.
        let geometric = Vec3::Y;
        let max_tilt = WAVE_MAX_STEEPNESS.atan();
        let mut lifts = 0;
        for tilt_step in 0..=12 {
            for incidence_step in 0..=40 {
                for azimuth_step in 0..8 {
                    let tilt = max_tilt * tilt_step as f32 / 12.0;
                    let azimuth = azimuth_step as f32 / 8.0 * std::f32::consts::TAU;
                    let optics = Vec3::new(
                        tilt.sin() * azimuth.cos(),
                        tilt.cos(),
                        tilt.sin() * azimuth.sin(),
                    );
                    // Incidence from straight down to 89.5 degrees off the normal.
                    let angle = incidence_step as f32 / 40.0 * 89.5_f32.to_radians();
                    let ray = Vec3::new(angle.sin(), -angle.cos(), 0.0).normalize();

                    let reflected = reflect(ray, optics);
                    let lifted = lift_reflection_above_face(reflected, geometric, optics);
                    let cosine = lifted.dot(geometric);

                    // The property that actually matters, and it holds in both regimes:
                    // the mirror ray never points into the water.
                    assert!(
                        cosine > 0.0,
                        "a mirror ray at tilt {tilt} incidence {angle} still points into \
                         the water (cosine {cosine})"
                    );
                    assert!((lifted.length() - 1.0).abs() < 1e-5);

                    if optics == geometric {
                        // A flat face cannot reflect into itself, so the guard is inert
                        // — including past 88.9 degrees, where the reflected cosine is
                        // below the minimum but was never a problem.
                        assert_eq!(lifted, reflected);
                        continue;
                    }
                    if reflected.dot(geometric) < WATER_REFLECTION_MIN_COSINE {
                        lifts += 1;
                        // The one-step lift lands on the cone to within float error; see
                        // `lift_reflection_above_face` for why the tiny shortfall is
                        // exact and harmless.
                        assert!(
                            cosine >= WATER_REFLECTION_MIN_COSINE - 1e-4,
                            "the lift undershot: {cosine}"
                        );
                    }
                }
            }
        }
        assert!(
            lifts > 0,
            "the sweep never exercised the guard, so it proves nothing"
        );
    }

    #[test]
    fn a_flat_surface_needs_no_lift_at_all() {
        // The isolation rule reaching the guard: with no wave normal, every reflection
        // off a top face is already above it, so the guard is inert and the shipped
        // no-waves image is untouched.
        let geometric = Vec3::Y;
        for incidence_step in 0..=40 {
            let angle = incidence_step as f32 / 40.0 * 89.5_f32.to_radians();
            let ray = Vec3::new(angle.sin(), -angle.cos(), 0.0).normalize();
            let reflected = reflect(ray, geometric);
            assert_eq!(
                lift_reflection_above_face(reflected, geometric, geometric),
                reflected,
                "the guard must not touch a flat surface's mirror ray, even at 89.5 \
                 degrees where the reflected cosine is below the minimum"
            );
        }
    }

    /// WGSL's `reflect`, so the guard test exercises the same geometry the shader does.
    fn reflect(incident: Vec3, normal: Vec3) -> Vec3 {
        incident - 2.0 * incident.dot(normal) * normal
    }

    #[test]
    fn the_epoch_split_keeps_phase_after_hours_of_uptime() {
        // The reason the clock ships as epochs plus a remainder: omega * t in a plain
        // f32 loses the fraction an oscillator needs. Two readings one epoch apart must
        // still differ, rather than collapsing onto the same value.
        let field = strong_wind();
        let position = Vec2::new(1.5, 2.5);
        let early = field.surface_normal(position, clock_at(3.0), 0.0);
        // Six hours in, offset by a quarter second.
        let late_a = AnimationClockSample {
            epoch: 337.0,
            remainder_seconds: 3.0,
        };
        let late_b = AnimationClockSample {
            epoch: 337.0,
            remainder_seconds: 3.25,
        };
        assert!(
            (field.surface_normal(position, late_a, 0.0)
                - field.surface_normal(position, late_b, 0.0))
            .length()
                > 1e-3,
            "phase collapsed after six hours of uptime"
        );
        assert!(early.y > 0.9);
    }
}
