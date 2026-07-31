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

use glam::Vec3;

use crate::ao::patch_shader_const;
use crate::brickmap::Brickmap;
use crate::material::{
    material_is_liquid, AIR_INDEX_OF_REFRACTION, WATER_ABSORPTION_PER_METER,
    WATER_SCATTERING_PER_METER,
};
use voxel_core::world::VOXEL_SIZE;

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
    pub fn shader_value(self) -> u32 {
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
    pub fn from_shader_value(shader_value: u32) -> WaterMode {
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

    fn wgsl_literal(self) -> String {
        format!("{}u", self.shader_value())
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
    pub fn shader_value(self) -> u32 {
        match self {
            WaterTirFallback::Flat => 0,
            WaterTirFallback::CheapMirror => 1,
        }
    }

    /// Inverse of [`WaterTirFallback::shader_value`]; panics on a value the shader
    /// has no branch for.
    pub fn from_shader_value(shader_value: u32) -> WaterTirFallback {
        match shader_value {
            0 => WaterTirFallback::Flat,
            1 => WaterTirFallback::CheapMirror,
            other => panic!("no WATER_TIR_FALLBACK {other} in water.wgsl"),
        }
    }

    fn wgsl_literal(self) -> String {
        format!("{}u", self.shader_value())
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
    pub fn shader_value(self) -> u32 {
        match self {
            WaterUnderwaterInterface::Fresnel => 0,
            WaterUnderwaterInterface::Transparent => 1,
        }
    }

    /// Inverse of [`WaterUnderwaterInterface::shader_value`]; panics on a value the
    /// shader has no branch for.
    pub fn from_shader_value(shader_value: u32) -> WaterUnderwaterInterface {
        match shader_value {
            0 => WaterUnderwaterInterface::Fresnel,
            1 => WaterUnderwaterInterface::Transparent,
            other => panic!("no WATER_UNDERWATER_INTERFACE {other} in water.wgsl"),
        }
    }

    fn wgsl_literal(self) -> String {
        format!("{}u", self.shader_value())
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
}

impl Default for WaterSettings {
    /// The shipped configuration, matching the lever defaults in
    /// `shaders/water.wgsl` (pinned by `default_settings_match_shader_source`).
    fn default() -> WaterSettings {
        WaterSettings {
            mode: WaterMode::Full,
            bounces: 1,
            tir_fallback: WaterTirFallback::CheapMirror,
            underwater_interface: WaterUnderwaterInterface::Transparent,
            absorption_scale: 1.0,
            scattering_scale: 1.0,
            ray_cutoff: 0.04,
            sun_through_liquid: true,
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
    pub fn patch_shader_source(&self, shader_source: &str) -> String {
        let mut patched = patch_shader_const(
            shader_source,
            "WATER_SUN_THROUGH_LIQUID",
            boolean_literal(self.sun_through_liquid),
        );
        if patched.contains("const WATER_MODE:") {
            patched = patch_shader_const(&patched, "WATER_MODE", &self.mode.wgsl_literal());
            patched = patch_shader_const(&patched, "WATER_BOUNCES", &format!("{}u", self.bounces));
            patched = patch_shader_const(
                &patched,
                "WATER_TIR_FALLBACK",
                &self.tir_fallback.wgsl_literal(),
            );
            patched = patch_shader_const(
                &patched,
                "WATER_UNDERWATER_INTERFACE",
                &self.underwater_interface.wgsl_literal(),
            );
        }
        patched
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
    }
}

fn boolean_literal(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

// ---- The physics -------------------------------------------------------------

/// Normal-incidence reflectance of an air/medium boundary — the `F0` Schlick's
/// approximation needs, derived from the medium's own
/// [`crate::material::Material::index_of_refraction`] rather than tuned:
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
    use crate::material::WATER_INDEX_OF_REFRACTION;
    use crate::passes::cagi::CAGI_SHADER_SOURCE;
    use crate::passes::dda::SHADER_SOURCE;

    #[test]
    fn default_settings_match_shader_source() {
        assert_eq!(
            WaterSettings::default().patch_shader_source(SHADER_SOURCE),
            SHADER_SOURCE,
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
        .patch_shader_source(SHADER_SOURCE);
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
        for source in [SHADER_SOURCE, CAGI_SHADER_SOURCE] {
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
        let painted = crate::material::materials()
            [crate::material::material_id(voxel_core::world::Voxel::Water) as usize]
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
        let sun_elevation_sine = crate::lighting::SunSettings::default().sun_direction().y;
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
}
