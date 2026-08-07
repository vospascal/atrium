//! E1c — the LEVER REGISTRY and the quality presets: one table that every
//! consumer reads, so a lever cannot exist in the shader, the settings structs,
//! the benchmark or the overlay without existing in all of them.
//!
//! Why a registry at all (Pascal, 2026-07-30): the measured losers of S2/E1/E1b
//! must stay *runnable* — an M3 Max loss can be a Quest win — without
//! cluttering the hot loop or rotting into dead code. So every lever keeps its
//! WGSL implementation and gets exactly one [`Lever`] row here carrying its
//! kind, its default, its measured verdict WITH the numbers, and the benchmark
//! points that sweep it.
//!
//! Who reads what:
//!
//! - **The benchmark** (`examples/bench_dda.rs`) derives its variant tables
//!   from [`REGISTRY`]'s [`BenchPoint`]s and the preset table — adding a lever
//!   row adds a bench column forever after, with no parallel list to update.
//! - **The overlay** (`crate::overlay`) draws the Quality panel from the rows:
//!   grouping by [`LeverSubsystem`], widget shape from [`LeverRange`], and the
//!   verdict as the hover text that answers "why is this off?" in-app.
//! - **The tests** pin the three copies of every default against each other:
//!   registry ↔ `dda.wgsl` consts ↔ the typed `Default` impls, in both
//!   directions (a shader lever with no row fails too).
//!
//! Seams (plan modularity rule): this module is pure data — no wgpu, no
//! windowing, no shader source of its own. It composes
//! [`crate::traversal::TraversalSettings`] and [`crate::ao::AoSettings`],
//! each of which still owns its own WGSL patching, and
//! `crate::passes::dda::build_shader_source` remains the single place a
//! shader source is assembled.

use crate::ao::{AoDirectionMode, AoMode, AoSettings};
use crate::cagi::{CagiLayout, CagiRule, CagiSampleMode, CagiSettings, CagiSkyTest};
use crate::lighting::{
    AnimationParams, EventParams, GiParams, MaterialParams, ShadingParams, WaterParams,
};
use crate::shader_consts::{float_literal, ShaderConstSink, ShaderDefs, SourcePatcher};
use crate::traversal::TraversalSettings;
use crate::water::{
    WaterMode, WaterSettings, WaterTirFallback, WaterUnderwaterInterface, WaveField,
};
use crate::world_edit::{ClearanceUpdateMode, WorldEditSettings};
use voxel_material::animation_clock::AnimationClockSample;
use voxel_material::pattern::{
    MAX_PATTERN_LAYERS, PATTERN_FADE_END_METERS, PATTERN_FADE_START_METERS,
};

/// Render-scale bounds (the resolution lever's range, also enforced by
/// `crate::render::Renderer::set_render_scale`).
pub(crate) const MIN_RENDER_SCALE: f32 = 0.5;
pub(crate) const MAX_RENDER_SCALE: f32 = 1.0;

/// Voxels per meter — the fade levers are voxel counts but are judged (and
/// sliders labelled) in meters.
pub const VOXELS_PER_METER: f32 = 8.0;

/// The highest rung of the pattern entry-cost probe, mirroring
/// `PATTERN_ENTRY_NO_LAYERS` in `shaders/pattern.wgsl`. Clamped on the way to the
/// shader for the same reason `pattern_max_layers` is: a value past the top would
/// still compile, silently pick the top rung's behaviour, and label a column with
/// a rung that does not exist.
pub(crate) const PATTERN_ENTRY_PROBE_TOP: u32 = 11;

/// The three generators bench section 9's saturated table actually authors: flat,
/// noise and speckle. Pruning to this must leave the frame BIT-IDENTICAL, which is
/// what makes the pixel gate a self-check on the measurement rather than a
/// formality.
pub(crate) const PATTERN_GENERATOR_MASK_SECTION_NINE: u32 = 0b111;

// ---- Lever identity ----------------------------------------------------------

/// Every lever the renderer has, as one identifier. [`LeverId::read`] and
/// [`LeverId::apply`] match exhaustively over this enum, so a new variant
/// cannot compile until it is wired to a [`RenderQuality`] field — and
/// `registry_has_a_row_for_every_lever_id` fails until it also has a
/// [`REGISTRY`] row.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum LeverId {
    // Traversal (S2) — all compile-time, all inside the coarse DDA loops.
    ColumnFastForward,
    GlobalMaxTerminate,
    BrickBitGrid,
    DistanceSkip,
    DirectionalSkip,
    // Ambient occlusion (E1 / E1b).
    AoMode,
    AoStrength,
    AoRayCount,
    AoMaxDistance,
    AoDirectionMode,
    AoDistanceFalloff,
    AoBrickEarlyOut,
    AoDistanceFade,
    AoFadeStart,
    AoFadeEnd,
    AoSunAwareRayBudget,
    AoMissRadiance,
    // CAGI global illumination (E4).
    GiEnabled,
    GiResolution,
    GiLayout,
    GiBanksLossPerMeter,
    GiBanksSideLossMultiplier,
    GiBanksSkyHorizontal,
    GiBanksBounce,
    GiBanksTransmission,
    GiBanksDirectionMix,
    GiBanksSealPartial,
    GiRule,
    GiSkyTest,
    GiSunCache,
    GiTransmission,
    GiReflectance,
    GiEmissive,
    GiEmitterBounce,
    GiEventLight,
    GiEmissiveScale,
    GiSampleMode,
    GiIterationsPerFrame,
    GiStrength,
    GiAmbientFloor,
    GiSunBounce,
    // Water optics (E6).
    WaterMode,
    WaterBounces,
    WaterTirFallback,
    WaterUnderwaterInterface,
    WaterAbsorption,
    WaterScattering,
    WaterRayCutoff,
    WaterSunThroughLiquid,
    WaterWaves,
    WaterWaveAmplitude,
    WaterVisibilityDepth,
    WaterCaustics,
    WaterBounceLight,
    WaterTurbidityScattering,
    // World edits (E2) — all runtime; an edit changes buffer contents, never a
    // shader.
    EditWorldThread,
    EditClearanceUpdate,
    EditClearanceRadius,
    EditGiReflood,
    // Materials (S1, S2).
    MaterialFaceRoles,
    MaterialPatterns,
    MaterialPatternCache,
    MaterialPatternTexelLod,
    MaterialPatternEntryProbe,
    MaterialPatternAnimation,
    MaterialPatternGeneratorMask,
    MaterialPatternStrength,
    MaterialParallax,
    MaterialParallaxSamples,
    MaterialParallaxShadowSamples,
    MaterialParallaxEnd,
    MaterialPatternMaxLayers,
    MaterialPatternOctaveLod,
    MaterialPatternFadeStart,
    MaterialPatternFadeEnd,
    MaterialAnimationSpeed,
    MaterialAnimationDeterministic,
    // Direct lighting.
    SunShadow,
    // Resolution (S0).
    RenderScale,
}

/// Overlay grouping — and the answer to "which subsystem pays for this".
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeverSubsystem {
    Traversal,
    AmbientOcclusion,
    /// E4 — the CAGI light volume.
    GlobalIllumination,
    /// Direct sun in the shading pass — the third light system next to AO and
    /// CAGI (one lever today: the traced sun shadow).
    Lighting,
    /// E6 — water reflection, refraction and extinction.
    Water,
    /// E2 — world authority, threading and the edit pipeline.
    WorldEdit,
    /// S1 — the material model: face roles now, pattern layers and animation later.
    Materials,
    Resolution,
}

impl LeverSubsystem {
    pub fn label(self) -> &'static str {
        match self {
            LeverSubsystem::Traversal => "Traversal",
            LeverSubsystem::AmbientOcclusion => "AO",
            LeverSubsystem::GlobalIllumination => "GI (CAGI)",
            LeverSubsystem::Lighting => "Direct light",
            LeverSubsystem::Water => "Water",
            LeverSubsystem::WorldEdit => "World edits",
            LeverSubsystem::Materials => "Materials",
            LeverSubsystem::Resolution => "Resolution",
        }
    }

    /// Panel order.
    pub const ALL: [LeverSubsystem; 8] = [
        LeverSubsystem::Traversal,
        LeverSubsystem::AmbientOcclusion,
        LeverSubsystem::GlobalIllumination,
        LeverSubsystem::Lighting,
        LeverSubsystem::Water,
        LeverSubsystem::WorldEdit,
        LeverSubsystem::Materials,
        LeverSubsystem::Resolution,
    ];
}

/// Where a lever's value lives — the compile-time/runtime split E1c measured.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LeverKind {
    /// A compile-time WGSL const: naga folds the disabled branch away, which is
    /// what the S2 optimization round bought, so anything inside the traversal
    /// loops or the AO estimator selection stays here. Changing it compiles a
    /// new pipeline (precompiled per preset at startup, so switching does not
    /// stutter).
    ShaderConst,
    /// A field of the lighting uniform ([`ShadingParams`]) or a CPU-side render
    /// setting: changing it needs NO pipeline rebuild.
    Runtime,
}

/// A lever's value, in the shape the shader wants it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LeverValue {
    /// WGSL `bool`.
    Flag(bool),
    /// WGSL `u32` mode selector (see [`Lever::mode_options`]).
    Mode(u32),
    /// WGSL `u32` count.
    Count(u32),
    /// A voxel distance: `u32` on the Rust side, WGSL `f32` (or a uniform
    /// component) on the GPU side.
    VoxelDistance(u32),
    /// A runtime-only float.
    Scalar(f32),
}

impl LeverValue {
    /// The WGSL literal this value patches into a const declaration, or `None`
    /// for runtime-only values (which have no const to patch).
    pub fn wgsl_literal(self) -> Option<String> {
        match self {
            LeverValue::Flag(true) => Some("true".to_string()),
            LeverValue::Flag(false) => Some("false".to_string()),
            LeverValue::Mode(value) | LeverValue::Count(value) => Some(format!("{value}u")),
            LeverValue::VoxelDistance(voxels) => Some(format!("{voxels}.0")),
            // S2's pattern strength is the first SCALAR lever that reaches the shader
            // as a const rather than through a uniform, so this stopped being `None`.
            // Anything with a `shader_const` must have a literal, or
            // `registry_defaults_match_shader_source` cannot check it against the
            // shipped source.
            LeverValue::Scalar(value) => Some(float_literal(value)),
        }
    }

    fn expect_flag(self, lever_id: LeverId) -> bool {
        match self {
            LeverValue::Flag(value) => value,
            other => panic!("lever {lever_id:?} takes a Flag, got {other:?}"),
        }
    }

    fn expect_mode(self, lever_id: LeverId) -> u32 {
        match self {
            LeverValue::Mode(value) => value,
            other => panic!("lever {lever_id:?} takes a Mode, got {other:?}"),
        }
    }

    fn expect_count(self, lever_id: LeverId) -> u32 {
        match self {
            LeverValue::Count(value) => value,
            other => panic!("lever {lever_id:?} takes a Count, got {other:?}"),
        }
    }

    fn expect_voxel_distance(self, lever_id: LeverId) -> u32 {
        match self {
            LeverValue::VoxelDistance(value) => value,
            other => panic!("lever {lever_id:?} takes a VoxelDistance, got {other:?}"),
        }
    }

    fn expect_scalar(self, lever_id: LeverId) -> f32 {
        match self {
            LeverValue::Scalar(value) => value,
            other => panic!("lever {lever_id:?} takes a Scalar, got {other:?}"),
        }
    }
}

/// Widget shape / bounds for the overlay.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum LeverRange {
    /// Checkbox or radio row — bounds come from the value type itself.
    Discrete,
    /// Continuous slider.
    Continuous {
        minimum: f32,
        maximum: f32,
        logarithmic: bool,
    },
    /// Fixed integer rungs, drawn as a radio row.
    Rungs(&'static [u32]),
    /// A voxel-distance slider whose bounds are expressed in METERS.
    Meters { minimum: f32, maximum: f32 },
}

/// One selectable value of a mode lever, with its own verdict — "why is 3x3x3
/// off" is a per-option answer, not a per-lever one.
pub struct ModeOption {
    pub value: u32,
    pub label: &'static str,
    pub verdict: &'static str,
}

/// One bench column: a label plus the lever overrides that build it, applied on
/// top of the section's baseline quality. Most points override only their own
/// lever; a point may list companions when the recorded row IS a combination
/// (e.g. the fade ramp needs the fade flag on).
pub struct BenchPoint {
    pub section: BenchSection,
    pub label: &'static str,
    pub overrides: &'static [(LeverId, LeverValue)],
}

/// The benchmark's independent sections (isolation rule — an experiment's
/// numbers never contaminate the gate below it).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BenchSection {
    /// Section 1: traversal levers with AO forced off (the S2 regression gate).
    Traversal,
    /// Section 2: E1's ray-traced-AO ladder around the grid center.
    RayTracedAo,
    /// Section 3: E1b's cheap-occlusion / soft-shadow shootout.
    CheapOcclusion,
    /// Section 5: E4's CAGI contenders (propagation rule, resolution, sky test,
    /// sampling, the sun-source cache). Measured on TWO axes — the CA pass's own
    /// per-iteration cost and the shading pass's sampling cost — plus the
    /// convergence and memory tables.
    Cagi,
    /// Section 6: E2's edit storm — the authority/threading variants, the
    /// clearance-update strategies and the CAGI re-flood, judged on per-frame cost
    /// DISTRIBUTIONS (median / p99 / max) rather than medians, because the whole
    /// question is hitches.
    EditStorm,
    /// Section 8: E6's water optics — the four cost tiers (no secondary rays /
    /// reflection only / refraction only / both) and the bounce budget, measured
    /// on scenes that actually contain water, including an UNDERWATER camera.
    /// Runs over its own brickmap (the island plus a carved debug pool), so its
    /// numbers deliberately do not compare with sections 1-5.
    Water,
    /// Section 9: S1+ material-model levers — face roles now, pattern layers and
    /// animation as they land. Measured on the shading pass, since everything in
    /// this arc is ALU on a hit the traversal already found.
    Materials,
}

/// One registry row.
pub struct Lever {
    pub id: LeverId,
    pub subsystem: LeverSubsystem,
    pub kind: LeverKind,
    /// The WGSL const this lever patches, or `None` when the value rides in a
    /// uniform / lives on the CPU.
    pub shader_const: Option<&'static str>,
    /// Overlay label.
    pub label: &'static str,
    /// The shipped value. Pinned against `dda.wgsl` AND against the typed
    /// `Default` impls by the tests below.
    pub default_value: LeverValue,
    pub range: LeverRange,
    /// The measured verdict, with numbers. Shown as the overlay's hover text.
    pub verdict: &'static str,
    /// Selectable values, for mode levers only.
    pub mode_options: &'static [ModeOption],
    /// Bench columns this lever contributes.
    pub bench: &'static [BenchPoint],
}

impl LeverId {
    /// This lever's current value in `quality`.
    pub fn read(self, quality: &RenderQuality) -> LeverValue {
        let traversal = &quality.traversal;
        let ambient_occlusion = &quality.ambient_occlusion;
        let global_illumination = &quality.global_illumination;
        match self {
            LeverId::ColumnFastForward => LeverValue::Flag(traversal.column_fast_forward),
            LeverId::GlobalMaxTerminate => LeverValue::Flag(traversal.global_max_terminate),
            LeverId::BrickBitGrid => LeverValue::Flag(traversal.brick_bit_grid),
            LeverId::DistanceSkip => LeverValue::Flag(traversal.distance_skip),
            LeverId::DirectionalSkip => LeverValue::Flag(traversal.directional_skip),
            LeverId::AoMode => LeverValue::Mode(ambient_occlusion.mode.shader_value()),
            LeverId::AoStrength => LeverValue::Scalar(ambient_occlusion.strength),
            LeverId::AoRayCount => LeverValue::Count(ambient_occlusion.ray_count),
            LeverId::AoMaxDistance => {
                LeverValue::VoxelDistance(ambient_occlusion.max_distance_voxels)
            }
            LeverId::AoDirectionMode => {
                LeverValue::Mode(ambient_occlusion.direction_mode.shader_value())
            }
            LeverId::AoDistanceFalloff => LeverValue::Flag(ambient_occlusion.distance_falloff),
            LeverId::AoBrickEarlyOut => LeverValue::Flag(ambient_occlusion.brick_early_out),
            LeverId::AoDistanceFade => LeverValue::Flag(ambient_occlusion.distance_fade),
            LeverId::AoFadeStart => LeverValue::VoxelDistance(ambient_occlusion.fade_start_voxels),
            LeverId::AoFadeEnd => LeverValue::VoxelDistance(ambient_occlusion.fade_end_voxels),
            LeverId::AoSunAwareRayBudget => {
                LeverValue::Flag(ambient_occlusion.sun_aware_ray_budget)
            }
            LeverId::AoMissRadiance => LeverValue::Flag(ambient_occlusion.miss_radiance),
            LeverId::GiEnabled => LeverValue::Flag(global_illumination.enabled),
            LeverId::GiResolution => LeverValue::Count(global_illumination.cell_voxels),
            LeverId::GiLayout => LeverValue::Mode(global_illumination.layout.shader_value()),
            LeverId::GiBanksLossPerMeter => {
                LeverValue::Scalar(global_illumination.banks_loss_per_meter)
            }
            LeverId::GiBanksSideLossMultiplier => {
                LeverValue::Scalar(global_illumination.banks_side_loss_multiplier)
            }
            LeverId::GiBanksSkyHorizontal => {
                LeverValue::Scalar(global_illumination.banks_sky_horizontal)
            }
            LeverId::GiBanksBounce => LeverValue::Scalar(global_illumination.banks_bounce),
            LeverId::GiBanksTransmission => {
                LeverValue::Scalar(global_illumination.banks_transmission_per_meter)
            }
            LeverId::GiBanksDirectionMix => {
                LeverValue::Scalar(global_illumination.banks_direction_mix)
            }
            LeverId::GiBanksSealPartial => {
                LeverValue::Scalar(global_illumination.banks_seal_partial)
            }
            LeverId::GiRule => LeverValue::Mode(global_illumination.rule.shader_value()),
            LeverId::GiSkyTest => LeverValue::Mode(global_illumination.sky_test.shader_value()),
            LeverId::GiSunCache => LeverValue::Flag(global_illumination.sun_cache),
            LeverId::GiTransmission => LeverValue::Flag(global_illumination.transmission),
            LeverId::GiReflectance => LeverValue::Flag(global_illumination.reflectance),
            LeverId::GiEmissive => LeverValue::Flag(global_illumination.emissive),
            LeverId::GiEmitterBounce => LeverValue::Flag(global_illumination.emitter_bounce),
            LeverId::GiEventLight => LeverValue::Flag(global_illumination.event_light),
            LeverId::GiEmissiveScale => LeverValue::Scalar(global_illumination.emissive_scale),
            LeverId::GiSampleMode => {
                LeverValue::Mode(global_illumination.sample_mode.shader_value())
            }
            LeverId::GiIterationsPerFrame => {
                LeverValue::Count(global_illumination.iterations_per_frame)
            }
            LeverId::GiStrength => LeverValue::Scalar(global_illumination.strength),
            LeverId::GiAmbientFloor => LeverValue::Scalar(global_illumination.ambient_floor),
            LeverId::GiSunBounce => LeverValue::Scalar(global_illumination.sun_bounce),
            LeverId::WaterMode => LeverValue::Mode(quality.water.mode.shader_value()),
            LeverId::WaterBounces => LeverValue::Count(quality.water.bounces),
            LeverId::WaterTirFallback => {
                LeverValue::Mode(quality.water.tir_fallback.shader_value())
            }
            LeverId::WaterUnderwaterInterface => {
                LeverValue::Mode(quality.water.underwater_interface.shader_value())
            }
            LeverId::WaterAbsorption => LeverValue::Scalar(quality.water.absorption_scale),
            LeverId::WaterScattering => LeverValue::Scalar(quality.water.scattering_scale),
            LeverId::WaterRayCutoff => LeverValue::Scalar(quality.water.ray_cutoff),
            LeverId::WaterSunThroughLiquid => LeverValue::Flag(quality.water.sun_through_liquid),
            LeverId::WaterWaves => LeverValue::Flag(quality.water.waves),
            LeverId::WaterWaveAmplitude => LeverValue::Scalar(quality.water.wave_amplitude_scale),
            LeverId::WaterVisibilityDepth => {
                LeverValue::Scalar(quality.water.visibility_depth_blocks)
            }
            LeverId::WaterCaustics => LeverValue::Flag(quality.water.caustics),
            LeverId::WaterBounceLight => LeverValue::Flag(quality.water.bounce_light),
            LeverId::WaterTurbidityScattering => {
                LeverValue::Scalar(quality.water.turbidity_scattering_fraction)
            }
            LeverId::EditWorldThread => LeverValue::Flag(quality.world_edit.world_thread),
            LeverId::EditClearanceUpdate => {
                LeverValue::Mode(quality.world_edit.clearance_update.shader_value())
            }
            LeverId::EditClearanceRadius => {
                LeverValue::Count(quality.world_edit.clearance_radius_cells)
            }
            LeverId::EditGiReflood => LeverValue::Flag(quality.world_edit.gi_reflood),
            LeverId::MaterialFaceRoles => LeverValue::Flag(quality.materials.face_roles),
            LeverId::MaterialPatterns => LeverValue::Flag(quality.materials.patterns),
            LeverId::MaterialPatternCache => LeverValue::Flag(quality.materials.pattern_cache),
            LeverId::MaterialPatternTexelLod => {
                LeverValue::Flag(quality.materials.pattern_texel_lod)
            }
            LeverId::MaterialPatternEntryProbe => {
                LeverValue::Mode(quality.materials.pattern_entry_probe)
            }
            LeverId::MaterialPatternAnimation => {
                LeverValue::Flag(quality.materials.pattern_animation)
            }
            LeverId::MaterialPatternGeneratorMask => {
                LeverValue::Count(quality.materials.pattern_generator_mask)
            }
            LeverId::MaterialPatternStrength => {
                LeverValue::Scalar(quality.materials.pattern_strength)
            }
            LeverId::MaterialParallax => LeverValue::Flag(quality.materials.parallax),
            LeverId::MaterialParallaxSamples => {
                LeverValue::Count(quality.materials.parallax_samples)
            }
            LeverId::MaterialParallaxShadowSamples => {
                LeverValue::Count(quality.materials.parallax_shadow_samples)
            }
            LeverId::MaterialParallaxEnd => {
                LeverValue::Scalar(quality.materials.parallax_end_meters)
            }
            LeverId::MaterialPatternMaxLayers => {
                LeverValue::Count(quality.materials.pattern_max_layers)
            }
            LeverId::MaterialPatternOctaveLod => {
                LeverValue::Flag(quality.materials.pattern_octave_lod)
            }
            LeverId::MaterialPatternFadeStart => {
                LeverValue::Scalar(quality.materials.pattern_fade_start_meters)
            }
            LeverId::MaterialPatternFadeEnd => {
                LeverValue::Scalar(quality.materials.pattern_fade_end_meters)
            }
            LeverId::MaterialAnimationSpeed => {
                LeverValue::Scalar(quality.materials.animation_speed)
            }
            LeverId::MaterialAnimationDeterministic => {
                LeverValue::Flag(quality.materials.animation_deterministic)
            }
            LeverId::SunShadow => LeverValue::Flag(quality.sun_shadow),
            LeverId::RenderScale => LeverValue::Scalar(quality.render_scale),
        }
    }

    /// Write `value` into `quality`. Panics when the value's shape does not
    /// match the lever — the registry, the presets and the bench all go through
    /// here, so a mismatch is a programming error, not user input.
    pub fn apply(self, quality: &mut RenderQuality, value: LeverValue) {
        let traversal = &mut quality.traversal;
        let ambient_occlusion = &mut quality.ambient_occlusion;
        let global_illumination = &mut quality.global_illumination;
        match self {
            LeverId::ColumnFastForward => traversal.column_fast_forward = value.expect_flag(self),
            LeverId::GlobalMaxTerminate => traversal.global_max_terminate = value.expect_flag(self),
            LeverId::BrickBitGrid => traversal.brick_bit_grid = value.expect_flag(self),
            LeverId::DistanceSkip => traversal.distance_skip = value.expect_flag(self),
            LeverId::DirectionalSkip => traversal.directional_skip = value.expect_flag(self),
            LeverId::AoMode => {
                ambient_occlusion.mode = AoMode::from_shader_value(value.expect_mode(self));
            }
            LeverId::AoStrength => ambient_occlusion.strength = value.expect_scalar(self),
            LeverId::AoRayCount => ambient_occlusion.ray_count = value.expect_count(self),
            LeverId::AoMaxDistance => {
                ambient_occlusion.max_distance_voxels = value.expect_voxel_distance(self);
            }
            LeverId::AoDirectionMode => {
                ambient_occlusion.direction_mode =
                    AoDirectionMode::from_shader_value(value.expect_mode(self));
            }
            LeverId::AoDistanceFalloff => {
                ambient_occlusion.distance_falloff = value.expect_flag(self);
            }
            LeverId::AoBrickEarlyOut => ambient_occlusion.brick_early_out = value.expect_flag(self),
            LeverId::AoDistanceFade => ambient_occlusion.distance_fade = value.expect_flag(self),
            LeverId::AoFadeStart => {
                ambient_occlusion.fade_start_voxels = value.expect_voxel_distance(self);
            }
            LeverId::AoFadeEnd => {
                ambient_occlusion.fade_end_voxels = value.expect_voxel_distance(self);
            }
            LeverId::AoSunAwareRayBudget => {
                ambient_occlusion.sun_aware_ray_budget = value.expect_flag(self);
            }
            LeverId::AoMissRadiance => {
                ambient_occlusion.miss_radiance = value.expect_flag(self);
            }
            LeverId::GiEnabled => global_illumination.enabled = value.expect_flag(self),
            LeverId::GiResolution => {
                global_illumination.cell_voxels = value.expect_count(self);
            }
            LeverId::GiLayout => {
                global_illumination.layout = CagiLayout::from_shader_value(value.expect_mode(self));
            }
            LeverId::GiBanksLossPerMeter => {
                global_illumination.banks_loss_per_meter =
                    value.expect_scalar(self).clamp(0.1, 64.0);
            }
            LeverId::GiBanksSideLossMultiplier => {
                global_illumination.banks_side_loss_multiplier =
                    value.expect_scalar(self).clamp(1.0, 16.0);
            }
            LeverId::GiBanksSkyHorizontal => {
                global_illumination.banks_sky_horizontal =
                    value.expect_scalar(self).clamp(0.0, 1.0);
            }
            LeverId::GiBanksBounce => {
                global_illumination.banks_bounce = value.expect_scalar(self).clamp(0.0, 1.0);
            }
            LeverId::GiBanksTransmission => {
                global_illumination.banks_transmission_per_meter =
                    value.expect_scalar(self).clamp(0.25, 1.0);
            }
            LeverId::GiBanksDirectionMix => {
                global_illumination.banks_direction_mix = value.expect_scalar(self).clamp(0.0, 0.5);
            }
            LeverId::GiBanksSealPartial => {
                global_illumination.banks_seal_partial = value.expect_scalar(self).clamp(0.0, 1.0);
            }
            LeverId::GiRule => {
                global_illumination.rule = CagiRule::from_shader_value(value.expect_mode(self));
            }
            LeverId::GiSkyTest => {
                global_illumination.sky_test =
                    CagiSkyTest::from_shader_value(value.expect_mode(self));
            }
            LeverId::GiSunCache => global_illumination.sun_cache = value.expect_flag(self),
            LeverId::GiTransmission => {
                global_illumination.transmission = value.expect_flag(self);
            }
            LeverId::GiReflectance => {
                global_illumination.reflectance = value.expect_flag(self);
            }
            LeverId::GiEmissive => global_illumination.emissive = value.expect_flag(self),
            LeverId::GiEmitterBounce => {
                global_illumination.emitter_bounce = value.expect_flag(self)
            }
            LeverId::GiEventLight => global_illumination.event_light = value.expect_flag(self),
            LeverId::GiEmissiveScale => {
                global_illumination.emissive_scale = value.expect_scalar(self);
            }
            LeverId::GiSampleMode => {
                global_illumination.sample_mode =
                    CagiSampleMode::from_shader_value(value.expect_mode(self));
            }
            LeverId::GiIterationsPerFrame => {
                global_illumination.iterations_per_frame = value.expect_count(self);
            }
            LeverId::GiStrength => global_illumination.strength = value.expect_scalar(self),
            LeverId::GiAmbientFloor => {
                global_illumination.ambient_floor = value.expect_scalar(self);
            }
            LeverId::GiSunBounce => global_illumination.sun_bounce = value.expect_scalar(self),
            LeverId::WaterMode => {
                quality.water.mode = WaterMode::from_shader_value(value.expect_mode(self));
            }
            LeverId::WaterBounces => quality.water.bounces = value.expect_count(self),
            LeverId::WaterTirFallback => {
                quality.water.tir_fallback =
                    WaterTirFallback::from_shader_value(value.expect_mode(self));
            }
            LeverId::WaterUnderwaterInterface => {
                quality.water.underwater_interface =
                    WaterUnderwaterInterface::from_shader_value(value.expect_mode(self));
            }
            LeverId::WaterAbsorption => {
                quality.water.absorption_scale = value.expect_scalar(self);
            }
            LeverId::WaterScattering => {
                quality.water.scattering_scale = value.expect_scalar(self);
            }
            LeverId::WaterRayCutoff => {
                quality.water.ray_cutoff = value.expect_scalar(self);
            }
            LeverId::WaterSunThroughLiquid => {
                quality.water.sun_through_liquid = value.expect_flag(self);
            }
            LeverId::WaterWaves => quality.water.waves = value.expect_flag(self),
            LeverId::WaterWaveAmplitude => {
                quality.water.wave_amplitude_scale = value.expect_scalar(self);
            }
            LeverId::WaterVisibilityDepth => {
                quality.water.visibility_depth_blocks = value.expect_scalar(self);
            }
            LeverId::WaterCaustics => quality.water.caustics = value.expect_flag(self),
            LeverId::WaterBounceLight => quality.water.bounce_light = value.expect_flag(self),
            LeverId::WaterTurbidityScattering => {
                quality.water.turbidity_scattering_fraction = value.expect_scalar(self);
            }
            LeverId::EditWorldThread => {
                quality.world_edit.world_thread = value.expect_flag(self);
            }
            LeverId::EditClearanceUpdate => {
                quality.world_edit.clearance_update =
                    ClearanceUpdateMode::from_shader_value(value.expect_mode(self));
            }
            LeverId::EditClearanceRadius => {
                quality.world_edit.clearance_radius_cells = value.expect_count(self);
            }
            LeverId::EditGiReflood => quality.world_edit.gi_reflood = value.expect_flag(self),
            LeverId::MaterialFaceRoles => {
                quality.materials.face_roles = value.expect_flag(self);
            }
            LeverId::MaterialPatterns => {
                quality.materials.patterns = value.expect_flag(self);
            }
            LeverId::MaterialPatternCache => {
                quality.materials.pattern_cache = value.expect_flag(self);
            }
            LeverId::MaterialPatternTexelLod => {
                quality.materials.pattern_texel_lod = value.expect_flag(self);
            }
            LeverId::MaterialPatternEntryProbe => {
                quality.materials.pattern_entry_probe = value.expect_mode(self);
            }
            LeverId::MaterialPatternAnimation => {
                quality.materials.pattern_animation = value.expect_flag(self);
            }
            LeverId::MaterialPatternGeneratorMask => {
                quality.materials.pattern_generator_mask = value.expect_count(self);
            }
            LeverId::MaterialPatternStrength => {
                quality.materials.pattern_strength = value.expect_scalar(self).clamp(0.0, 1.0);
            }
            LeverId::MaterialParallax => {
                quality.materials.parallax = value.expect_flag(self);
            }
            LeverId::MaterialParallaxSamples => {
                quality.materials.parallax_samples = value.expect_count(self).min(128);
            }
            LeverId::MaterialParallaxShadowSamples => {
                quality.materials.parallax_shadow_samples = value.expect_count(self).min(128);
            }
            LeverId::MaterialParallaxEnd => {
                quality.materials.parallax_end_meters = value.expect_scalar(self).clamp(0.0, 512.0);
            }
            LeverId::MaterialPatternMaxLayers => {
                // Not clamped here: `apply` and `read` must round-trip, and
                // `patch_shader_source` clamps on the way to the shader — which is the
                // only place the bound actually matters, since it is the array length.
                quality.materials.pattern_max_layers = value.expect_count(self);
            }
            LeverId::MaterialPatternOctaveLod => {
                quality.materials.pattern_octave_lod = value.expect_flag(self);
            }
            LeverId::MaterialPatternFadeStart => {
                quality.materials.pattern_fade_start_meters = value.expect_scalar(self).max(0.0);
            }
            LeverId::MaterialPatternFadeEnd => {
                quality.materials.pattern_fade_end_meters = value.expect_scalar(self).max(0.0);
            }
            LeverId::MaterialAnimationSpeed => {
                quality.materials.animation_speed = value.expect_scalar(self).max(0.0);
            }
            LeverId::MaterialAnimationDeterministic => {
                quality.materials.animation_deterministic = value.expect_flag(self);
            }
            LeverId::SunShadow => quality.sun_shadow = value.expect_flag(self),
            LeverId::RenderScale => {
                quality.render_scale = value
                    .expect_scalar(self)
                    .clamp(MIN_RENDER_SCALE, MAX_RENDER_SCALE);
            }
        }
    }
}

/// This lever's registry row. Panics when the id has no row — the pinning tests
/// make that unreachable.
pub fn lever(lever_id: LeverId) -> &'static Lever {
    REGISTRY
        .iter()
        .find(|lever| lever.id == lever_id)
        .unwrap_or_else(|| panic!("lever {lever_id:?} has no REGISTRY row"))
}

/// The rows of one subsystem, in registry order.
pub fn levers_of(subsystem: LeverSubsystem) -> impl Iterator<Item = &'static Lever> {
    REGISTRY
        .iter()
        .filter(move |lever| lever.subsystem == subsystem)
}

/// Every bench column of one section, in registry order.
pub fn bench_points_of(section: BenchSection) -> impl Iterator<Item = &'static BenchPoint> {
    REGISTRY
        .iter()
        .flat_map(|lever| lever.bench.iter())
        .filter(move |point| point.section == section)
}

// ---- The registry ------------------------------------------------------------

/// THE table. Verdict lines quote the measured numbers from
/// `docs/voxel-rt-bench.md` (Apple M3 Max, 2560x1440) — they are the answer the
/// overlay shows when someone asks why a lever is off.
pub const REGISTRY: &[Lever] = &[
    // ---- Traversal (S2) ----
    Lever {
        id: LeverId::DistanceSkip,
        subsystem: LeverSubsystem::Traversal,
        kind: LeverKind::ShaderConst,
        shader_const: Some("ENABLE_DISTANCE_SKIP"),
        label: "chebyshev distance skip",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "WINNER, on. The engine of the current numbers: 17-27% under the \
                  Stage 2 baseline, and its distance byte doubles as the occupancy \
                  test (500 KB grid instead of the 2 MB pointer grid).",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Traversal,
            label: "no-dist-skip",
            overrides: &[(LeverId::DistanceSkip, LeverValue::Flag(false))],
        }],
    },
    Lever {
        id: LeverId::DirectionalSkip,
        subsystem: LeverSubsystem::Traversal,
        kind: LeverKind::ShaderConst,
        shader_const: Some("ENABLE_DIRECTIONAL_SKIP"),
        label: "AADF directional skip",
        default_value: LeverValue::Flag(false),
        range: LeverRange::Discrete,
        verdict: "MEASURED LOSS, off. Slower than the chebyshev cube in every \
                  scenario: A 5.011 vs 4.748, B 6.791 vs 6.550, C 4.565 vs 4.408, \
                  D 5.055 vs 4.973 ms. The FIELD is genuinely better — 27,578 \
                  empty cells where chebyshev grants reach 0 get a mean 5.19 \
                  cells, mean reach overall 9.10 -> 10.82 — but reading it costs \
                  more than the extra reach returns: the chebyshev byte doubles \
                  as the occupancy test (one load, two answers) where a bound is \
                  a second load, 2 MB stops being cache-resident where 500 KB \
                  was, and six 5-bit fields cost shifts where a byte costs a \
                  compare. KEPT because the reach win is hardware-independent \
                  and the cache cost is not: re-evaluate on Quest. From NAADF \
                  (Ulschmid et al., CGF 2026, MIT).",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Traversal,
            label: "with-directional-skip",
            overrides: &[(LeverId::DirectionalSkip, LeverValue::Flag(true))],
        }],
    },
    Lever {
        id: LeverId::GlobalMaxTerminate,
        subsystem: LeverSubsystem::Traversal,
        kind: LeverKind::ShaderConst,
        shader_const: Some("ENABLE_GLOBAL_MAX_TERMINATE"),
        label: "global-max terminate",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "WINNER, on. Exact sky-out for upward rays above the world's \
                  tallest brick — kills sky pixels without walking the grid, for a \
                  single compare.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Traversal,
            label: "no-global-max",
            overrides: &[(LeverId::GlobalMaxTerminate, LeverValue::Flag(false))],
        }],
    },
    Lever {
        id: LeverId::ColumnFastForward,
        subsystem: LeverSubsystem::Traversal,
        kind: LeverKind::ShaderConst,
        shader_const: Some("ENABLE_COLUMN_FAST_FORWARD"),
        label: "column fast-forward (up)",
        default_value: LeverValue::Flag(false),
        range: LeverRange::Discrete,
        verdict: "LOSER on M3 Max, off: +9-17% when re-enabled. Shadow rays cross \
                  columns about once per step, so the lateral jump saves few steps \
                  while its resync math and scattered column reads cost more. \
                  Re-measure on Quest's Adreno.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Traversal,
            label: "with-column-ff",
            overrides: &[(LeverId::ColumnFastForward, LeverValue::Flag(true))],
        }],
    },
    Lever {
        id: LeverId::BrickBitGrid,
        subsystem: LeverSubsystem::Traversal,
        kind: LeverKind::ShaderConst,
        shader_const: Some("ENABLE_BRICK_BIT_GRID"),
        label: "brick bit-grid occupancy",
        default_value: LeverValue::Flag(false),
        range: LeverRange::Discrete,
        verdict: "LOSER on M3 Max, off: no measurable win — the skip-distance byte \
                  already answers occupancy, so the bit read is a second redundant \
                  load. Retry where caches are small (Quest). Its data is still \
                  read by the AO brick early-out.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Traversal,
            label: "with-bit-grid",
            overrides: &[(LeverId::BrickBitGrid, LeverValue::Flag(true))],
        }],
    },
    // ---- Ambient occlusion (E1 / E1b) ----
    Lever {
        id: LeverId::AoMode,
        subsystem: LeverSubsystem::AmbientOcclusion,
        kind: LeverKind::ShaderConst,
        shader_const: Some("AO_MODE"),
        label: "technique",
        default_value: LeverValue::Mode(1),
        range: LeverRange::Discrete,
        verdict: "The E1b shootout's subject: analytic corner AO is the shipped \
                  default (+0.25-0.31 ms) and ray-traced AO is the Beautiful tier \
                  (+4.2-8.2 ms). The analytic 3x3x3 contender was PRUNED 2026-08-07: \
                  5x corner AO's cost for over-darkening and per-voxel flat facets — \
                  slower AND worse-looking on any hardware (verdict kept in the \
                  bench doc's E1b section).",
        mode_options: &[
            ModeOption {
                value: 0,
                label: "rays",
                verdict: "RT-AO (E1's winner, 2 rays / 8 voxels / cosine / falloff): the only \
                          estimator with reach past contact — it dims recessed-but-not-touching \
                          geometry — but +4.2-8.2 ms (11.8-14.6 ms total pass), over the ~8 ms \
                          target, and 2 rays still crosshatch large near surfaces. Beautiful tier \
                          until CAGI (E4) takes over the medium-scale band.",
            },
            ModeOption {
                value: 1,
                label: "corner",
                verdict: "Analytic corner AO — WINNER, the shipped default: 8 occupancy bits \
                          around the hit face, bilinearly interpolated with the DDA's face-local \
                          UV. +0.25-0.31 ms (20x cheaper than rays) at 82% of their frame \
                          coverage, and noiseless. Contact-only: misses recessed-but-not-touching \
                          areas by ~1.1/255 mean luminance.",
            },
            ModeOption {
                value: 2,
                label: "off",
                verdict: "No occlusion — the pre-E1 renderer bit for bit. The bench floor: \
                          4.74 / 6.52 / 4.40 / 4.96 ms (A/B/C/D).",
            },
        ],
        bench: &[
            BenchPoint {
                section: BenchSection::RayTracedAo,
                label: "ao-off",
                overrides: &[(LeverId::AoMode, LeverValue::Mode(2))],
            },
            BenchPoint {
                section: BenchSection::CheapOcclusion,
                label: "ao-off",
                overrides: &[(LeverId::AoMode, LeverValue::Mode(2))],
            },
            BenchPoint {
                section: BenchSection::CheapOcclusion,
                label: "ao-corner",
                overrides: &[(LeverId::AoMode, LeverValue::Mode(1))],
            },
        ],
    },
    Lever {
        id: LeverId::AoStrength,
        subsystem: LeverSubsystem::AmbientOcclusion,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "strength",
        default_value: LeverValue::Scalar(0.8),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 1.0,
            logarithmic: false,
        },
        verdict: "Runtime (shading_params.x), free: it scales the estimator's result, \
                  never the work. 0.8 is the shipped look.",
        mode_options: &[],
        bench: &[],
    },
    Lever {
        id: LeverId::AoRayCount,
        subsystem: LeverSubsystem::AmbientOcclusion,
        kind: LeverKind::ShaderConst,
        shader_const: Some("AO_RAY_COUNT"),
        label: "rays",
        default_value: LeverValue::Count(2),
        range: LeverRange::Rungs(&[1, 2, 4]),
        verdict: "RT-AO only. 2 is the knee: 1 ray leaves a stable crosshatch on \
                  large flat ground, 4 costs ~7 ms more for no visible gain. Each \
                  marginal full-res short ray is 2.25-3.55 ms.",
        mode_options: &[],
        bench: &[
            BenchPoint {
                section: BenchSection::RayTracedAo,
                label: "ao-1ray-d16",
                overrides: &[(LeverId::AoRayCount, LeverValue::Count(1))],
            },
            BenchPoint {
                section: BenchSection::RayTracedAo,
                label: "ao-4ray-d16",
                overrides: &[(LeverId::AoRayCount, LeverValue::Count(4))],
            },
        ],
    },
    Lever {
        id: LeverId::AoMaxDistance,
        subsystem: LeverSubsystem::AmbientOcclusion,
        kind: LeverKind::ShaderConst,
        shader_const: Some("AO_MAX_DISTANCE"),
        label: "ray length (voxels)",
        default_value: LeverValue::VoxelDistance(8),
        range: LeverRange::Rungs(&[8, 16, 32]),
        verdict: "RT-AO only. 8 voxels (1 m) is 10-17% cheaper than 16 with \
                  visually equivalent grounding (the falloff already discounts far \
                  occluders); 32 costs +30-60% and only adds scene-wide dimming.",
        mode_options: &[],
        bench: &[
            BenchPoint {
                section: BenchSection::RayTracedAo,
                label: "ao-2ray-d8",
                overrides: &[(LeverId::AoMaxDistance, LeverValue::VoxelDistance(8))],
            },
            BenchPoint {
                section: BenchSection::RayTracedAo,
                label: "ao-2ray-d32",
                overrides: &[(LeverId::AoMaxDistance, LeverValue::VoxelDistance(32))],
            },
        ],
    },
    Lever {
        id: LeverId::AoDirectionMode,
        subsystem: LeverSubsystem::AmbientOcclusion,
        kind: LeverKind::ShaderConst,
        shader_const: Some("AO_DIRECTION_MODE"),
        label: "ray directions",
        default_value: LeverValue::Mode(0),
        range: LeverRange::Discrete,
        verdict: "RT-AO only. Cosine-weighted is the default — it matches the \
                  Lambert weighting of the ambient term it multiplies.",
        mode_options: &[
            ModeOption {
                value: 0,
                label: "cosine",
                verdict: "Cosine-weighted hemisphere — default: binary hits average to the \
                          correct visibility integral, and it is the cheapest of the three at \
                          equal ray count.",
            },
            ModeOption {
                value: 1,
                label: "uniform",
                verdict: "Uniform hemisphere — LOSER, off: +4.7% (A) to +14.7% (C) over cosine \
                          (its grazing rays hit sooner) AND it over-darkens: 51% coverage vs 39%, \
                          greying open flat ground.",
            },
            ModeOption {
                value: 2,
                label: "bent-up",
                verdict: "Bent-up cone — cheapest RT-AO variant (12.16 ms on A) and noise-free, \
                          but it is a sky-visibility proxy, not occlusion: 21%/14.5% coverage \
                          means it misses most lateral contact darkening. Kept for a Quest \
                          re-measure.",
            },
        ],
        bench: &[
            BenchPoint {
                section: BenchSection::RayTracedAo,
                label: "ao-uniform-d16",
                overrides: &[(LeverId::AoDirectionMode, LeverValue::Mode(1))],
            },
            BenchPoint {
                section: BenchSection::RayTracedAo,
                label: "ao-bent-d16",
                overrides: &[(LeverId::AoDirectionMode, LeverValue::Mode(2))],
            },
        ],
    },
    Lever {
        id: LeverId::AoDistanceFalloff,
        subsystem: LeverSubsystem::AmbientOcclusion,
        kind: LeverKind::ShaderConst,
        shader_const: Some("AO_DISTANCE_FALLOFF"),
        label: "distance falloff",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "RT-AO only, on. Binary occlusion costs MORE at equal distance \
                  (16.28 vs 14.77 ms on B against falloff at d8) and looks worse: \
                  uniform mid-grey patches instead of a contact gradient.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::RayTracedAo,
            label: "ao-binary-d16",
            overrides: &[(LeverId::AoDistanceFalloff, LeverValue::Flag(false))],
        }],
    },
    Lever {
        id: LeverId::AoBrickEarlyOut,
        subsystem: LeverSubsystem::AmbientOcclusion,
        kind: LeverKind::ShaderConst,
        shader_const: Some("AO_BRICK_EARLY_OUT"),
        label: "brick early-out",
        default_value: LeverValue::Flag(false),
        range: LeverRange::Discrete,
        verdict: "NEGATIVE, off: measured 0% firing rate on terrain (byte-identical \
                  output, -0.6 to -1.4% = noise). The bricks under and beside a \
                  surface brick are solid ground, so the 3x3x3 test never passes; it \
                  needs voxel-level clearance data to ever fire.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::CheapOcclusion,
            label: "ao-2ray-brickskip",
            overrides: &[(LeverId::AoBrickEarlyOut, LeverValue::Flag(true))],
        }],
    },
    Lever {
        id: LeverId::AoDistanceFade,
        subsystem: LeverSubsystem::AmbientOcclusion,
        kind: LeverKind::ShaderConst,
        shader_const: Some("AO_DISTANCE_FADE"),
        label: "distance fade",
        default_value: LeverValue::Flag(false),
        range: LeverRange::Discrete,
        verdict: "WEAK, off by default: only 0.6-2.9% at ground level, because AO \
                  cost is dominated by NEAR pixels. Its 12-49% aerial saving is the \
                  effect itself being removed (coverage 37.6% -> 0%). A legitimate \
                  aerial-camera / Potato knob, not a free win — Potato ships it at \
                  15->30 m.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::CheapOcclusion,
            label: "ao-2ray-fade30-60",
            overrides: &[(LeverId::AoDistanceFade, LeverValue::Flag(true))],
        }],
    },
    Lever {
        id: LeverId::AoFadeStart,
        subsystem: LeverSubsystem::AmbientOcclusion,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "fade start (m)",
        default_value: LeverValue::VoxelDistance(240),
        range: LeverRange::Meters {
            minimum: 2.0,
            maximum: 125.0,
        },
        verdict: "Runtime since E1c (shading_params.z) — moving it out of the shader \
                  consts measured free, so the fade range is dialable without a \
                  pipeline rebuild. 240 voxels = 30 m, the conservative rung.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::CheapOcclusion,
            label: "ao-2ray-fade15-30",
            overrides: &[
                (LeverId::AoDistanceFade, LeverValue::Flag(true)),
                (LeverId::AoFadeStart, LeverValue::VoxelDistance(120)),
                (LeverId::AoFadeEnd, LeverValue::VoxelDistance(240)),
            ],
        }],
    },
    Lever {
        id: LeverId::AoFadeEnd,
        subsystem: LeverSubsystem::AmbientOcclusion,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "fade end (m)",
        default_value: LeverValue::VoxelDistance(480),
        range: LeverRange::Meters {
            minimum: 2.0,
            maximum: 125.0,
        },
        verdict: "Runtime since E1c (shading_params.w). Past this distance the \
                  estimator is skipped entirely. 480 voxels = 60 m.",
        mode_options: &[],
        bench: &[],
    },
    Lever {
        id: LeverId::AoSunAwareRayBudget,
        subsystem: LeverSubsystem::AmbientOcclusion,
        kind: LeverKind::ShaderConst,
        shader_const: Some("AO_SUN_AWARE_RAY_BUDGET"),
        label: "sun-aware ray budget",
        default_value: LeverValue::Flag(false),
        range: LeverRange::Discrete,
        verdict: "REJECTED, off: 0-7.5% saving (A -7.5%, B +0.1%) for halving rays \
                  on exactly the bright flat sunlit ground where the 1-ray \
                  crosshatch is visible. Coverage drops 37.6% -> 29.6%.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::CheapOcclusion,
            label: "ao-2ray-sunbudget",
            overrides: &[(LeverId::AoSunAwareRayBudget, LeverValue::Flag(true))],
        }],
    },
    Lever {
        id: LeverId::AoMissRadiance,
        subsystem: LeverSubsystem::AmbientOcclusion,
        kind: LeverKind::ShaderConst,
        shader_const: Some("AO_MISS_RADIANCE"),
        label: "directional miss radiance",
        default_value: LeverValue::Flag(false),
        range: LeverRange::Discrete,
        verdict: "WINNER on the Beautiful tier, gate passed 2026-07-30 (VGI I3D'11 \
                  SS5.1). Shipped there and only there — it needs rays, and Beautiful \
                  is the only tier that traces any. An escaping occlusion ray samples the \
                  hemisphere term in its OWN direction, so ambient becomes a \
                  visibility-weighted environment integral instead of a flat constant \
                  times a scalar — one lobe mix per missed ray, no new traversal. \
                  COST: +0.18-0.41% vs ao-2ray-d16 on the two load-stable scenarios \
                  across two runs, inside the +-2% noise band. COVERAGE (C): 72.5% of \
                  frame vs ao-off at max delta 116, against the baseline's 34.1% at \
                  55 — it reaches the medium-scale band analytic corner AO gives up. \
                  CATCH: it makes the ambient term itself Monte Carlo, so the 2-ray \
                  crosshatch now lands in ambient COLOUR and is visible as grain in \
                  dark foreground; wants 4 rays (+6.8 ms, ao-4ray-d16) or B12's \
                  bilateral filter. Needs AO_MODE = rays. Sampling the raw sky \
                  function instead was measured and REVERTED (teal shadows, purple \
                  rock) — see the shader block.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::RayTracedAo,
            label: "ao-2ray-missradiance",
            overrides: &[
                (LeverId::AoMode, LeverValue::Mode(0)),
                (LeverId::AoMissRadiance, LeverValue::Flag(true)),
            ],
        }],
    },
    // ---- CAGI global illumination (E4) ----
    Lever {
        id: LeverId::GiEnabled,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_ENABLED"),
        label: "CAGI light volume",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "ON since E4, measured cost +0.40-0.51 ms in the shading pass (the \
                  volume sample) plus 0.92-1.52 ms for the CA pass itself at the \
                  shipped 2 iterations. Off folds the whole experiment away: the \
                  volume shrinks to a 12-byte placeholder and the shading pass is \
                  byte-identical to E1c (4.71/6.52/4.36/4.91 ms, the recorded \
                  baseline within 0.5%, pixel gate 19/0).",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-off",
            overrides: &[(LeverId::GiEnabled, LeverValue::Flag(false))],
        }],
    },
    Lever {
        id: LeverId::GiResolution,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "cell size (voxels)",
        default_value: LeverValue::Count(8),
        range: LeverRange::Rungs(&[4, 8]),
        verdict: "8 voxels (1 m cells) is the shipped pairing since the D5 flip — \
                  banks6 stores six words per cell, so the resolution rung buys six \
                  times the memory and CA cost it used to: banks at 8 voxels is \
                  0.97-1.10 ms and 20.9 MiB, banks at 4 extrapolates to ~7 ms and \
                  160 MiB (measured once at D1). The pre-banks isotropic ladder for \
                  reference: 4 voxels 0.92-1.52 ms / 33 MB, 8 voxels 5.8x cheaper \
                  with a visibly coarser look (46% of the frame, mean 4.2/255). The \
                  2-voxel rung was PRUNED 2026-08-07: already DEAD isotropic \
                  (258 MB, 6x cost, 7.8/255 mean), and under banks it would be \
                  ~1.5 GB — past the 128 MB binding limit. Runtime: the grid \
                  dimensions ride in the volume uniform, so changing it reallocates \
                  buffers but compiles no shader.",
        mode_options: &[],
        bench: &[
            BenchPoint {
                section: BenchSection::Cagi,
                label: "gi-cells8",
                overrides: &[(LeverId::GiResolution, LeverValue::Count(8))],
            },
        ],
    },
    Lever {
        id: LeverId::GiLayout,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_LAYOUT"),
        label: "volume layout",
        default_value: LeverValue::Mode(1),
        range: LeverRange::Discrete,
        verdict: "MEASURED and SHIPPED (D5 flip, 2026-08-07, \
                  docs/cagi-directional-banks-plan.md). At the reference pairing \
                  (banks6 + 8-voxel cells) the CA pass costs 0.97-1.10 ms — CHEAPER \
                  than the old isotropic-at-4 default's 1.33-1.47 ms — at 20.9 MiB vs \
                  45.8 (3.3x isotropic-at-8's 0.27-0.32 ms, the price of 36 light \
                  reads per cell). The directional sampler adds ~0.6 ms to the DDA \
                  pass at 1440p (5.47-6.33 vs 4.85-5.68, after unrolling the \
                  dynamically-indexed bank-weight array that spilled to scratch \
                  memory — it was +1.5 ms before) and scales with pixels. \
                  Look: banks at defaults stays NEAR the shipped image (max channel \
                  delta 11-12, vs 43 for every isotropic rule variant) while walls, \
                  shadows and bounces become directional; the D4 gates verified \
                  black walled shadows and no leaks. The D5 corridor face-luminance \
                  comparison (`corridor_faces_read_directionally_under_banks`, \
                  2026-08-07) puts numbers on the difference: isotropic reads a \
                  sky-lit wall at ~100% of open ground and a roof underside equal \
                  to the floor below it; banks read the wall at the horizon share \
                  (0.28), the underside at 0.05 of its own floor, and beam light \
                  carries under cover where averaging diffusion is near-black \
                  (0.18 vs 0.02 of the anchor). Open ground holds at 0.94 — the \
                  direction-decay skim, see banks direction decay. Banks at \
                  4-voxel cells is NOT measured as a pairing — extrapolates to \
                  ~7 ms of CA.",
        mode_options: &[
            ModeOption {
                value: 0,
                label: "isotropic",
                verdict: "One light word per cell — the pre-D5 volume, now the Quest \
                          tier's layout (a sixth of the memory, a third of the CA \
                          cost at equal cells). Cannot represent a directional \
                          bounce: reflected light deposits directionlessly and \
                          reads as glow.",
            },
            ModeOption {
                value: 1,
                label: "banks 6",
                verdict: "x1m4's reference design: six directional banks per cell \
                          (10-bit RGB each), SoA planes. Pairs with 8-voxel cells \
                          (~24 MB both ping-pong buffers vs ~200 MB at 4-voxel cells). \
                          D5-measured: faces gain the orientation axis isotropic \
                          lacks (wall 0.28 of ground, roof underside 0.05 of its \
                          floor) at a CHEAPER CA than isotropic-at-4.",
            },
        ],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-banks6",
            overrides: &[
                (LeverId::GiLayout, LeverValue::Mode(1)),
                // The reference pairing: banks at 8-voxel cells (x1m4's 1/8 res).
                (LeverId::GiResolution, LeverValue::Count(8)),
            ],
        }],
    },
    Lever {
        id: LeverId::GiBanksLossPerMeter,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_BANKS_LOSS_PER_METER"),
        label: "banks direct loss (/m)",
        default_value: LeverValue::Scalar(1.0),
        range: LeverRange::Continuous {
            minimum: 0.1,
            maximum: 64.0,
            logarithmic: true,
        },
        verdict: "UNMEASURED — D2/D3 (banks6 only; inert under the isotropic \
                  layout). The convergence EPSILON, the reference kernel's \
                  saturating_sub(1): subtractive loss per meter that trims the \
                  exponential tail to an exact 0 (what D6 dirty-culling will \
                  test). NOT the falloff — that is banks air transmission. The D3 \
                  gate measured the wrong hierarchy: at 8.0/m the subtractive \
                  term dominates, radiance decays as a straight line, and the lit \
                  region ends at a hard terminator. Raise this to harden the \
                  edge, raise transmission to push the horizon out.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-banks-loss-hard",
            overrides: &[
                (LeverId::GiLayout, LeverValue::Mode(1)),
                (LeverId::GiResolution, LeverValue::Count(8)),
                // The D3 gate's hard-terminator configuration, kept as the A/B.
                (LeverId::GiBanksLossPerMeter, LeverValue::Scalar(8.0)),
            ],
        }],
    },
    Lever {
        id: LeverId::GiBanksSideLossMultiplier,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_BANKS_SIDE_LOSS_MULTIPLIER"),
        label: "banks side loss (x direct)",
        default_value: LeverValue::Scalar(4.0),
        range: LeverRange::Continuous {
            minimum: 1.0,
            maximum: 16.0,
            logarithmic: false,
        },
        verdict: "UNMEASURED — D2's beam-spread knob (banks6 only). The lateral \
                  seep's loss as a multiple of the direct loss: 1.0 dissolves a \
                  beam into an isotropic flood, 16.0 keeps it a laser. The \
                  heat-conduction term of x1m4's quoted rule.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-banks-side-tight",
            overrides: &[
                (LeverId::GiLayout, LeverValue::Mode(1)),
                (LeverId::GiResolution, LeverValue::Count(8)),
                (LeverId::GiBanksSideLossMultiplier, LeverValue::Scalar(8.0)),
            ],
        }],
    },
    Lever {
        id: LeverId::GiBanksSkyHorizontal,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_BANKS_SKY_HORIZONTAL"),
        label: "banks sky horizontal share",
        default_value: LeverValue::Scalar(0.25),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 1.0,
            logarithmic: false,
        },
        verdict: "MEASURED (corridor comparison, 2026-08-07) — the wall-brightness \
                  knob under banks. The horizon's share of the sky: what fraction \
                  of the sky radiance a sky-seeing cell injects into its four \
                  horizontal banks (the downward bank always gets the full value). \
                  At the default 0.25 a sky-lit corridor wall reads 0.28 of open \
                  ground (transport tops the injected share up slightly) — versus \
                  1.0 under isotropic, which cannot tell a wall from a floor. \
                  0 makes walls facing the horizon sky-black; 1 double-counts the \
                  hemisphere.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-banks-sky-flat",
            overrides: &[
                (LeverId::GiLayout, LeverValue::Mode(1)),
                (LeverId::GiResolution, LeverValue::Count(8)),
                (LeverId::GiBanksSkyHorizontal, LeverValue::Scalar(0.0)),
            ],
        }],
    },
    Lever {
        id: LeverId::GiBanksBounce,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_BANKS_BOUNCE"),
        label: "banks bounce fraction",
        default_value: LeverValue::Scalar(0.5),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 1.0,
            logarithmic: false,
        },
        verdict: "UNMEASURED — D3 (banks6 + reflectance only). The propagated \
                  bounce's energy fraction ON TOP of the surface albedo: the \
                  geometry share interreflection must lose (the E5b snow-corridor \
                  light-pipe note). Loop gain is albedo x this, so anything below \
                  1.0 contracts; 1.0 trusts albedo alone to converge, which snow \
                  makes visibly slow.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-banks-bounce",
            overrides: &[
                (LeverId::GiLayout, LeverValue::Mode(1)),
                (LeverId::GiResolution, LeverValue::Count(8)),
                (LeverId::GiReflectance, LeverValue::Flag(true)),
                (LeverId::GiBanksBounce, LeverValue::Scalar(0.25)),
            ],
        }],
    },
    Lever {
        id: LeverId::GiBanksTransmission,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_BANKS_TRANSMISSION_PER_METER"),
        label: "banks air transmission (/m)",
        default_value: LeverValue::Scalar(0.7),
        range: LeverRange::Continuous {
            minimum: 0.25,
            maximum: 1.0,
            logarithmic: false,
        },
        verdict: "MEASURED (CPU probe, lava-vs-10-cell-wall, 2026-08-07) — the \
                  banks' line-of-sight knob. Multiplicative air transmission per \
                  meter on top of the subtractive losses; without it reach is \
                  LINEAR in injected energy and a ceiling-level lava out-reached \
                  the sky 8:1. The isotropic 0.884 leaves the wall's shadow at \
                  1/4-1/10 of the lit side (wrap light cruises: half-life 5.6 m); \
                  0.7 floors the shadow at level <=30 — a soft rim hugging the \
                  wall edges, black core — while the lit side keeps levels \
                  500-1800 at 6-7 m; 0.6 zeroes the shadow exactly but halves the \
                  emitter's visible radius. 1.0 = pure-subtractive behaviour.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-banks-clear-air",
            overrides: &[
                (LeverId::GiLayout, LeverValue::Mode(1)),
                (LeverId::GiResolution, LeverValue::Count(8)),
                (LeverId::GiBanksTransmission, LeverValue::Scalar(0.95)),
            ],
        }],
    },
    Lever {
        id: LeverId::GiBanksDirectionMix,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_BANKS_DIRECTION_MIX"),
        label: "banks direction decay (/m)",
        default_value: LeverValue::Scalar(0.08),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 0.5,
            logarithmic: false,
        },
        verdict: "MEASURED — D4's wrapped-light fix (banks6 only). The per-meter \
                  fraction each bank scatters into its four PERPENDICULAR banks — \
                  how fast a beam forgets its direction. Perpendicular and never \
                  the opposite, a measured deviation from the literal \
                  mix(lightpy, lightny, x): opposite-mixing manufactures \
                  backward light along every beam, which is exactly the bank a \
                  wall's dark face samples — measured in-app as light 'coming \
                  through everywhere'. Conservative across the six banks, so \
                  fog and the bank SUM are untouched — but a SURFACE read pays a \
                  skim: the corridor comparison measured open ground at 0.94 of \
                  isotropic at the default 0.08/m, because the decay moves 6% of \
                  a fresh downward injection into the horizontals the floor does \
                  not read. 0 restores the exact anchor and forever-directional \
                  beams (lava's wrapped up-column painted bottom faces orange); \
                  0.5/m is near-isotropic within a couple of cells.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-banks-no-direction-decay",
            overrides: &[
                (LeverId::GiLayout, LeverValue::Mode(1)),
                (LeverId::GiResolution, LeverValue::Count(8)),
                (LeverId::GiBanksDirectionMix, LeverValue::Scalar(0.0)),
            ],
        }],
    },
    Lever {
        id: LeverId::GiBanksSealPartial,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_BANKS_SEAL_PARTIAL"),
        label: "banks corner-seal partial",
        default_value: LeverValue::Scalar(0.25),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 1.0,
            logarithmic: false,
        },
        verdict: "UNMEASURED — D4's face-occlusion gate on the lateral seep, the \
                  reference kernel's three-tier corner seal (its porting notes \
                  call this THE leak fix). A seep from lateral neighbour L into \
                  cell C cuts the diagonal bracketed by C-upstream and L: both \
                  solid = sealed to zero (the wall-join leak); exactly one solid \
                  = this fraction survives (grazing a wall edge — what stops the \
                  over-the-wall wrap band re-seeding beams down the shadow face); \
                  neither = the full side term. 1.0 disables the partial tier \
                  (D2-D4a behaviour); 0.0 makes every edge a hard shadow line.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-banks-no-corner-seal",
            overrides: &[
                (LeverId::GiLayout, LeverValue::Mode(1)),
                (LeverId::GiResolution, LeverValue::Count(8)),
                (LeverId::GiBanksSealPartial, LeverValue::Scalar(1.0)),
            ],
        }],
    },
    Lever {
        id: LeverId::GiRule,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_RULE"),
        label: "propagation rule",
        default_value: LeverValue::Mode(1),
        range: LeverRange::Discrete,
        verdict: "The E4 A/B, isotropic layout only (the banks transport has its own \
                  kernel). Cost: diffusion-6 and max-decrement are the SAME price \
                  (0.92-1.53 ms per frame — both read 6 neighbours and the pass is \
                  bandwidth-bound), so the rule is a free look choice. Look: the two \
                  differ on 66% of the frame (mean 8.8/255). A 26-neighbour isotropy \
                  variant was PRUNED 2026-08-07: 2.1-2.7x the cost (2.64-3.27 ms) for \
                  a mean 0.5/255 change, its keep-for-E5 rationale died when \
                  GiEmitterBounce made emitters rule-independent, and the banks \
                  layout owns directionality now.",
        mode_options: &[
            ModeOption {
                value: 0,
                label: "max-decrement",
                verdict: "Minecraft-style flood, L = max(neighbours) - attenuation. Same measured \
                          cost as diffusion (0.92-1.53 ms — the pass is bandwidth-bound, the \
                          multiply is free) and its reach is exactly 12.8 m by construction, but \
                          it reads visibly FLATTER and brighter in shade (66% of the frame, mean \
                          8.8/255 vs diffusion) because a straight-line falloff puts many cells \
                          at the same level, and its iso-surfaces are L1 balls, i.e. octahedra — \
                          the anisotropy the dossier warns about, which will matter for E5's \
                          point lights.",
            },
            ModeOption {
                value: 1,
                label: "diffusion 6",
                verdict: "The dossier's reconstructed equation, L = sum(6 neighbours) * T / 6, \
                          in pure u32 — the shipped rule at no extra cost over the flood. Its \
                          equilibrium is a discrete Laplace solution, so shadowed pockets get a \
                          smooth gradient instead of a stamped reach, and because a cell's value \
                          can DECREASE it converges downward after a sun change (the flood can \
                          too, since it excludes its own previous value).",
            },
        ],
        bench: &[
            BenchPoint {
                section: BenchSection::Cagi,
                label: "gi-max-decrement",
                overrides: &[(LeverId::GiRule, LeverValue::Mode(0))],
            },
        ],
    },
    Lever {
        id: LeverId::GiSkyTest,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_SKY_TEST"),
        label: "sky test",
        default_value: LeverValue::Mode(0),
        range: LeverRange::Discrete,
        verdict: "Sky injection asks every cell whether it sees the sky; the column-max \
                  test answers it with one load of the traversal's own column-height \
                  buffer (binding 8) instead of a ray, for +0% cost against the exact \
                  trace's +33-53%. The two disagree on 33% of the frame at a mean of \
                  2.1/255 (max 59) — cheap wins, but it is an approximation, not an \
                  identity.",
        mode_options: &[
            ModeOption {
                value: 0,
                label: "column max",
                verdict: "SHIPPED: one load of the per-XZ-brick-column max occupied brick Y — the \
                          traversal's own data, reused, so sky injection costs nothing. Exact for \
                          the vertical direction but quantized to the 1 m brick column: a cell \
                          beside a tree trunk shares the trunk's column and is treated as covered \
                          until the CA carries light in laterally. Costs a mean 2.1/255 against \
                          the exact test — the diffusion fills most of it back in.",
            },
            ModeOption {
                value: 1,
                label: "upward trace",
                verdict: "A real vertical shadow ray per cell: exact per voxel, +33-53% on the CA \
                          pass (1.47-2.07 vs 0.92-1.52 ms per frame). Worth switching on where \
                          dense canopy sits over ground the player walks under; the shipped \
                          default trades that 2.1/255 mean for free sky injection.",
            },
        ],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-sky-trace",
            overrides: &[(LeverId::GiSkyTest, LeverValue::Mode(1))],
        }],
    },
    Lever {
        id: LeverId::GiSunCache,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_SUN_CACHE"),
        label: "pin sun sources",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "ON, and free: a cell that found the sun sets bit 30, and that bit \
                  stands in for the shadow RAY on later iterations while the cell keeps \
                  propagating and recomputing its bounce colour. Saves 10% (default sun) \
                  to 19% (low sun) of the CA pass at BYTE-IDENTICAL output (0 differing \
                  pixels vs re-tracing, both scenarios). Caching the cell's VALUE \
                  instead — the first implementation — froze source cells and lost their \
                  diffusion on 26% of the frame (mean 0.6/255, max 38), which is why the \
                  flag caches the ray result only. Static world + static sun; the sun \
                  sliders clear the volume.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-no-sun-cache",
            overrides: &[(LeverId::GiSunCache, LeverValue::Flag(false))],
        }],
    },
    Lever {
        id: LeverId::GiTransmission,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_TRANSMISSION"),
        label: "transmit through solids",
        default_value: LeverValue::Flag(false),
        range: LeverRange::Discrete,
        verdict: "M2, UNMEASURED — the default is off until an app run gives it a verdict. \
                  E4 v0 wrote 0 into every solid cell, so a leaf canopy absorbed like \
                  stone and the ground under a tree went black; with this on a solid cell \
                  forwards propagate(neighbours) * transmittance (bits 25-28 of its \
                  attribute word, from the M1 material table) while still receiving no \
                  emission, so foliage passes light without losing its shadow. Costs one \
                  extra propagate on solid cells, which is why it needs measuring rather \
                  than assuming. Stone transmits 0, so opaque geometry is bit-identical \
                  either way — any delta outside vegetation is a bug.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-transmission",
            overrides: &[(LeverId::GiTransmission, LeverValue::Flag(true))],
        }],
    },
    Lever {
        id: LeverId::GiReflectance,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_REFLECTANCE"),
        label: "reflect off solids (colour bleed)",
        default_value: LeverValue::Flag(false),
        range: LeverRange::Discrete,
        verdict: "E5b, UNMEASURED at the time of writing — the default is off until \
                  section 15 gives it a verdict, and section 15 exists to give it one. \
                  What it fixes: the v0 transport could not produce colour bleed AT ALL, \
                  and not for the reason it looked like. The bounce was not missing — \
                  `cagi_sun_bounce` computes a correctly albedo-tinted term at a fraction \
                  of 0.35 — it was GATED on the receiving air cell seeing the sun, so \
                  indirect light existed only in the shell the sun already lit and never \
                  reached shadow, which is the entire job of GI. L0's corridor rendered \
                  the floor five voxels out of the sunbeam BLACK between 0.8-albedo walls, \
                  and its white ceiling grey. This term is ungated and multiplies \
                  PROPAGATED light, the same move TooManyLimits' published kernel makes \
                  when a neighbour is a Block. Because it scales INCOMING light rather \
                  than injecting the surface's own colour, a white ceiling stays white \
                  only while white light reaches it — red off the floor comes back red — \
                  so it also fixes the readout surface masking its own signal, without a \
                  second rule. Costs one extra propagate on solid cells, exactly like \
                  GiTransmission, and shares the incident term with it when both are on. \
                  Combined with max, not summed: a surface cannot both transmit and \
                  reflect the same photon, and max keeps a solid cell strictly dimmer \
                  than what reaches it, which is what keeps the flood convergent.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-reflectance",
            overrides: &[(LeverId::GiReflectance, LeverValue::Flag(true))],
        }],
    },
    Lever {
        id: LeverId::GiEmissive,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_EMISSIVE"),
        label: "emissive materials",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "E5, ON by default WITHOUT a measured verdict, deliberately: world \
                  generation places no emissive voxel, so until one is placed by hand this \
                  is a shift, a mask and an indexed uniform load per cell and changes not \
                  one pixel. The bench measures an emitter-free world, so its number \
                  cannot argue the default either way — the cost only exists once the \
                  feature is being used, and then it is the feature. An emissive SOLID \
                  pins its own radiance so neighbours diffuse it outward; thin-cover \
                  emitters inject on the air path like the sky term.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-no-emissive",
            overrides: &[(LeverId::GiEmissive, LeverValue::Flag(false))],
        }],
    },
    Lever {
        id: LeverId::GiEmitterBounce,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_EMITTER_BOUNCE"),
        label: "emitter bounce (air reads emissive neighbours)",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "E5c, ON by default — and unusually for this registry the MEASUREMENT \
                  argues for the default rather than being absent. The diffusion \
                  numerator is transmission/6: near-lossless for a UNIFORM field \
                  (6V * 0.94/6 = 0.94V) but it keeps only 15.7% of a lone bright \
                  neighbour among five dark ones. Measured on the `wall + glow block` \
                  prop, the air cell in front of the emitter settles at 152/1023 under \
                  MaxDecrement (which is scale-free) and 45/1023 under the SHIPPED \
                  Diffusion6 — so a point light worked only under a rule that is not \
                  the default. With this on, the neighbour reads the emitter's own mean \
                  under every rule, which is the whole point: it makes emitters \
                  RULE-INDEPENDENT. Cost is strictly less than the sun bounce it copies \
                  — the same 6-neighbour walk with no shadow ray (the source is \
                  adjacent) and no Lambert weight (E5b's stored value is already a mean \
                  over exposed area, so weighting by direction would double-count). Off \
                  restores E5's rule-dependent behaviour exactly, which is what makes \
                  the before/after measurable.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-no-emitter-bounce",
            overrides: &[(LeverId::GiEmitterBounce, LeverValue::Flag(false))],
        }],
    },
    Lever {
        id: LeverId::GiEventLight,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_EVENT_LIGHT"),
        label: "event light (emission follows the world event field)",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "S3b, ON by default. Without it a material whose emission answers an \
                  event sensor is a decal: the wall brightens as you walk up to it and \
                  the floor in front of it does not, because the volume only ever saw \
                  the row's resting emission. With it on, a cell carrying a response \
                  index in attribute bits 29-31 senses the same event field the surface \
                  does and interpolates between the material's resting and triggered \
                  emission. What made this cheap is a fact the arc's plan had WRONG: the \
                  CA is not a one-shot flood — it dispatches iterations_per_frame steps \
                  every frame and neither rule reads a cell's own previous value, so the \
                  field brightens and darkens on its own and a time-varying emitter \
                  needs NO re-flood. The global re-flood exists only to clear the pinned \
                  sun-source flag when the sun moves. The shipped-world benchmark covers \
                  only the no-response fast path; it does NOT price a live gated emitter. \
                  That remaining scenario must use representative gated surface area and \
                  0/1/4/16 events before the default's cost is treated as accepted. DDA \
                  remains unchanged, as it must: the shading pass never reads the response \
                  table. Off is a complete look — every gated cell injects its stored peak \
                  — and the Quest-tier fallback.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-no-event-light",
            overrides: &[(LeverId::GiEventLight, LeverValue::Flag(false))],
        }],
    },
    Lever {
        id: LeverId::GiEmissiveScale,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "emissive scale",
        default_value: LeverValue::Scalar(1.0),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 16.0,
            logarithmic: false,
        },
        verdict: "Runtime multiplier on every emitter's authored radiance (gi_params.w, \
                  the slot E4 reserved for exactly this). Free — it scales a value the CA \
                  already reads — and it is the knob to reach for when a placed light \
                  blows out the exposure, rather than re-authoring the material table. \
                  Ranges to 16 rather than 4 (raised after the first app run: 4 was not \
                  enough to make a single block read as a real light source). Kept LINEAR \
                  even that wide, because 0 means lights-off and a log slider cannot \
                  reach it. Note the ceiling is a symptom: the tonemap is Reinhard on an \
                  8-bit target, so a bright emitter clips instead of blooming — E7b's HDR \
                  intermediate is what would let the scale mean something physical.",
        mode_options: &[],
        bench: &[],
    },
    Lever {
        id: LeverId::GiSampleMode,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::ShaderConst,
        shader_const: Some("CAGI_SAMPLE_MODE"),
        label: "sampling",
        default_value: LeverValue::Mode(1),
        range: LeverRange::Discrete,
        verdict: "How the shading pass reads the volume. Trilinear is shipped at \
                  +0.28-0.35 ms over nearest: at 0.5 m cells nearest sampling stamps flat \
                  cell-sized patches onto surfaces (36% of the frame, mean 2.9/255, max \
                  76). Nearest is the Quest lever if 0.15 ms matters more than banding.",
        mode_options: &[
            ModeOption {
                value: 0,
                label: "nearest",
                verdict: "One load from the cell in front of the hit face: +0.12-0.17 ms over no \
                          CAGI at all. It shows the volume's resolution as flat 0.5 m patches of \
                          indirect light — visible on 36% of the frame against trilinear (max \
                          delta 76) — the honest look of the raw data.",
            },
            ModeOption {
                value: 1,
                label: "trilinear",
                verdict: "SHIPPED, +0.40-0.51 ms over no CAGI (i.e. +0.28-0.35 over nearest): \
                          eight loads, weights renormalized over the NON-solid taps so a wall's \
                          interior (always 0) cannot bleed darkness onto the surface in front of \
                          it. It is what turns a 0.5 m grid into a smooth gradient.",
            },
        ],
        bench: &[BenchPoint {
            section: BenchSection::Cagi,
            label: "gi-nearest",
            overrides: &[(LeverId::GiSampleMode, LeverValue::Mode(0))],
        }],
    },
    Lever {
        id: LeverId::GiIterationsPerFrame,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "iterations / frame",
        default_value: LeverValue::Count(2),
        range: LeverRange::Rungs(&[1, 2, 4, 8]),
        verdict: "The per-frame CA budget (a CPU-side dispatch count — no shader \
                  const): 0.44-0.76 ms per iteration at 0.5 m cells, linear (1 it = \
                  0.52-0.77, 2 = 0.92-1.52, 8 = 3.59-5.87 ms). Light travels one cell per \
                  iteration, so 2 gives full convergence in 32 frames (0.53 s at 60 fps) \
                  and visual convergence (max delta 1) in 16.",
        mode_options: &[],
        bench: &[
            BenchPoint {
                section: BenchSection::Cagi,
                label: "gi-iterations1",
                overrides: &[(LeverId::GiIterationsPerFrame, LeverValue::Count(1))],
            },
            BenchPoint {
                section: BenchSection::Cagi,
                label: "gi-iterations8",
                overrides: &[(LeverId::GiIterationsPerFrame, LeverValue::Count(8))],
            },
        ],
    },
    Lever {
        id: LeverId::GiStrength,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "strength",
        default_value: LeverValue::Scalar(1.0),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 3.0,
            logarithmic: false,
        },
        verdict: "Runtime (gi_params.x), free: it scales the sampled volume, never the \
                  transport. 1.0 means the volume IS the indirect term at the sky \
                  radiance E1c used for its hemisphere ambient, which is why switching \
                  CAGI on redistributes the indirect light instead of brightening the \
                  scene (74-88% of the frame changes, max delta 52-64).",
        mode_options: &[],
        bench: &[],
    },
    Lever {
        id: LeverId::GiAmbientFloor,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "ambient floor",
        default_value: LeverValue::Scalar(0.0),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 1.0,
            logarithmic: false,
        },
        verdict: "Runtime (gi_params.y): the share of analytic hemisphere ambient that \
                  bypasses the light transport. Zero is authoritative: a sealed pocket \
                  with no emitter is fully dark. Raise this only as an explicit \
                  non-physical readability override.",
        mode_options: &[],
        bench: &[],
    },
    Lever {
        id: LeverId::GiSunBounce,
        subsystem: LeverSubsystem::GlobalIllumination,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "sun bounce",
        default_value: LeverValue::Scalar(0.35),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 1.0,
            logarithmic: false,
        },
        verdict: "Runtime (gi_params.z): the share of the sun's radiance a sunlit \
                  surface injects into the volume, times that surface's albedo. This is \
                  the knob that decides how coloured the bounce reads — 0 leaves a \
                  sky-only flood.",
        mode_options: &[],
        bench: &[],
    },
    // ---- Water optics (E6) ----
    Lever {
        id: LeverId::WaterMode,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::ShaderConst,
        shader_const: Some("WATER_MODE"),
        label: "optics",
        default_value: LeverValue::Mode(4),
        range: LeverRange::Discrete,
        verdict: "MEASURED (E6, bench section 8, M3 Max 2560x1440, over the island + \
                  the debug pool). Cost over opaque, on the two scenarios that look AT \
                  water (grazing from the shore / steep from 60 m): tint +0.36 / +0.74, \
                  reflection-only +2.06 / +2.87, refraction-only +2.01 / +4.50, full \
                  +2.40 / +4.64 ms. From INSIDE the water the floor is different \
                  because the opaque row is a degenerate image (the eye sits in an \
                  opaque voxel): looking up 4.15 tint / 7.51 full, sideways 8.39 tint / \
                  11.06 full. So: full costs 2.4-4.6 ms above water and is the shipped \
                  Balanced/Beautiful pick; tint costs 0.4-0.7 ms for a recognisable \
                  water surface with no scene reflection and no visible depth, which is \
                  the Potato/Quest pick.",
        mode_options: &[
            ModeOption {
                value: 0,
                label: "opaque",
                verdict: "Water is an ordinary diffuse surface — the E4 renderer bit for bit, and \
                          the bench's no-regression anchor. Also the honest answer for a tier that \
                          cannot afford ANY water work; it is what shipped up to E4 and what makes \
                          swimming impossible to debug, which is why E6 exists.",
            },
            ModeOption {
                value: 1,
                label: "fresnel tint",
                verdict: "ZERO secondary rays: the mirror term is the analytic sky function in the \
                          reflected direction (which already carries the sun glint, so a grazing \
                          water surface still glares) and the transmitted term is the surface's own \
                          diffuse shading, mixed by the same Fresnel curve as the full model. The \
                          Potato/Quest tier. It cannot show what is UNDER the water from above, but \
                          the underwater view still gets extinction and Snell's window, because \
                          there the march is the primary ray and not an extra one.",
            },
            ModeOption {
                value: 2,
                label: "reflection",
                verdict: "The mirror ray is traced (and shaded through the full path, so it sees \
                          sun, shadow, AO and CAGI); transmission stays the diffuse surface. Exists \
                          to price the reflection ray on its own — it is the half that reads as \
                          expensive because a grazing water plane fills the frame with a second \
                          full shading.",
            },
            ModeOption {
                value: 3,
                label: "refraction",
                verdict: "The refracted ray marches the water body with per-channel extinction and \
                          shades the bed; the mirror term stays the analytic sky. Exists to price \
                          refraction on its own. This is the half that makes DEPTH readable, which \
                          is the whole reason E6 was pulled ahead of E5.",
            },
            ModeOption {
                value: 4,
                label: "full",
                verdict: "Both, Fresnel-weighted: grazing angles mirror, steep angles see through. \
                          The shipped model on Balanced and Beautiful.",
            },
        ],
        bench: &[
            BenchPoint {
                section: BenchSection::Water,
                label: "water-off",
                overrides: &[(LeverId::WaterMode, LeverValue::Mode(0))],
            },
            BenchPoint {
                section: BenchSection::Water,
                label: "water-tint",
                overrides: &[(LeverId::WaterMode, LeverValue::Mode(1))],
            },
            BenchPoint {
                section: BenchSection::Water,
                label: "water-reflect",
                overrides: &[(LeverId::WaterMode, LeverValue::Mode(2))],
            },
            BenchPoint {
                section: BenchSection::Water,
                label: "water-refract",
                overrides: &[(LeverId::WaterMode, LeverValue::Mode(3))],
            },
        ],
    },
    Lever {
        id: LeverId::WaterBounces,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::ShaderConst,
        shader_const: Some("WATER_BOUNCES"),
        label: "interfaces / ray",
        default_value: LeverValue::Count(1),
        range: LeverRange::Rungs(&[1, 2]),
        verdict: "MEASURED TWICE (E6, then E6 step 1): **1 on every tier**, and the \
                  second interface is now a documented non-purchase. It was never worth \
                  anything above water (the frame times are inside noise, because a \
                  refracted ray that reaches the bed never asks for another interface). \
                  Underwater it costs a great deal — +9.9 ms on the window-rim view, \
                  +6.6 ms looking straight up — and step 1 showed WHY it looked \
                  necessary: the region outside Snell's window had nothing but a flat \
                  constant, so a second full bounce was the only thing that put \
                  geometry there. With `WATER_TIR_FALLBACK = cheap mirror` doing that \
                  for +4.1 ms instead, the two frames are near-identical and the full \
                  bounce buys a little extra shading detail in the mirrored region for \
                  2.5x the price. Beautiful dropped from 2 to 1 on this evidence. Kept \
                  as a lever because it is the only exact answer.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Water,
            label: "water-full-2bounce",
            overrides: &[(LeverId::WaterBounces, LeverValue::Count(2))],
        }],
    },
    Lever {
        id: LeverId::WaterAbsorption,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "absorption scale",
        default_value: LeverValue::Scalar(0.0),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 4.0,
            logarithmic: false,
        },
        verdict: "Runtime (water_params.x), free: it scales the medium's per-metre \
                  ABSORPTION — the light water destroys — never the work. This is the \
                  clarity/darkening axis. 1.0 = the authored coefficients \
                  (0.446 / 0.090 / 0.015 per metre, just above pure water's measured \
                  0.35 / 0.056 / 0.015, which is what dissolved organics in a lake do); \
                  with the scattering pair that puts extinction at 0.450 / 0.120 / 0.060 \
                  and transmittance at the pool's 5 m at (0.105, 0.549, 0.741) — red \
                  nearly gone, blue mostly intact, i.e. depth reads as colour. 0 turns \
                  the water into clear glass, which is how to check the refraction \
                  geometry independently of the medium. \
                  SHIPPED AT 0 (dialled in the app, 2026-08-06). That is not a bug: with \
                  water's own absorption off, the medium's extinction is E7's TURBIDITY \
                  almost entirely, and turbidity is grey — so the water darkens without \
                  colouring, which is this instinct carried to its conclusion rather than \
                  away from it. Raise it to bring back the steep blue of pure water.",
        mode_options: &[],
        bench: &[],
    },
    Lever {
        id: LeverId::WaterScattering,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "scattering scale",
        default_value: LeverValue::Scalar(0.15),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 4.0,
            logarithmic: false,
        },
        verdict: "Runtime (water_params.y), free: it scales the medium's per-metre \
                  SCATTERING — the light water redirects rather than destroys, and \
                  therefore the light a ray picks up along its path. This is the \
                  brightness axis, and together with absorption it is what the medium's \
                  COLOUR is derived from: the single-scattering albedo \
                  scattering/extinction comes out at (0.009, 0.250, 0.750) for water, \
                  i.e. deeply blue with almost no red, without anything painting it \
                  (Pascal, 2026-07-31: water must not have a colour of its own). 1.0 = \
                  the authored 0.004 / 0.030 / 0.045 per metre; 0 makes the model \
                  absorption-only and the depths go black, which is physically \
                  incomplete and reads as a hole. Deliberately NOT sampled from the CAGI \
                  volume: E4 marks a cell absorbing at a quarter fill, so cells inside a \
                  body of water hold zero light and every pool would be black. \
                  SHIPPED AT 0.15 (dialled in the app, 2026-08-06), in proportion with the \
                  absorption scale beside it, which ships at 0. The medium's colour therefore \
                  comes from turbidity rather than from these coefficients, and measures a \
                  near-neutral albedo of (0.152, 0.166, 0.174).",
        mode_options: &[],
        bench: &[],
    },
    Lever {
        id: LeverId::WaterUnderwaterInterface,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::ShaderConst,
        shader_const: Some("WATER_UNDERWATER_INTERFACE"),
        label: "surface seen from below",
        default_value: LeverValue::Mode(1),
        range: LeverRange::Discrete,
        verdict: "PENDING MEASUREMENT (E6 step 3). What the surface interface does for a \
                  ray reaching it from BELOW. Pascal asked for the underwater side to be \
                  plainly transparent and for the reflection to live only on top, so \
                  `transparent` is the shipped default; `fresnel` is the physical \
                  interface E6 shipped through step 1, kept selectable because a Quest \
                  re-measure or wave normals may well flip the verdict back. It GATES \
                  two other levers: with `transparent` there is no mirror to bounce and \
                  no region outside a window, so `interfaces / ray` and `outside Snell's \
                  window` are inert from below.",
        mode_options: &[
            ModeOption {
                value: 0,
                label: "fresnel",
                verdict: "The physical interface: Snell's bend, a Fresnel-weighted split, and total \
                          internal reflection past the 48.607-degree critical angle, whose mirrored \
                          region the `outside Snell's window` lever then fills. This is what E6 \
                          shipped through step 1 and what the step-1 numbers describe. Kept as the \
                          off-lever because it is the CORRECT physics and because Snell's window is \
                          a genuinely striking effect — the objection was to it dominating the \
                          underwater view, not to it existing.",
            },
            ModeOption {
                value: 1,
                label: "transparent (unbent)",
                verdict: "SHIPPED: fully transmissive, and UNBENT. The ray continues straight \
                          through the surface with only the path's absorption and scattering \
                          applied. Unbent is not a shortcut, it is the only coherent version of \
                          \"just transparent\": total internal reflection is not a separable \
                          effect, it IS what Snell's law yields when sin(theta_transmitted) > 1, so \
                          past the critical angle there is no transmitted direction to bend toward \
                          — keep the bend and there is nothing to draw beyond the window. \
                          ACCEPTED CONSEQUENCES, not defects: Snell's window disappears from below \
                          and the surface becomes invisible from underneath, with no boundary cue \
                          at all. Cheaper as well as simpler, because the mirrored stand-in march \
                          never runs.",
            },
        ],
        bench: &[BenchPoint {
            section: BenchSection::Water,
            label: "water-fresnel-from-below",
            overrides: &[(LeverId::WaterUnderwaterInterface, LeverValue::Mode(0))],
        }],
    },
    Lever {
        id: LeverId::WaterTirFallback,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::ShaderConst,
        shader_const: Some("WATER_TIR_FALLBACK"),
        label: "outside Snell's window",
        default_value: LeverValue::Mode(1),
        range: LeverRange::Discrete,
        verdict: "MEASURED (E6 step 1) and it is the fix the look gate demanded. Cost \
                  of the cheap mirror over the flat constant: **free above water** \
                  (grazing and steep views inside noise — the fallback never fires \
                  there), **+4.1 ms on the window-rim view** (10.3 -> 14.5), +6.0 \
                  looking straight up, +4.4 sideways. Against the alternative — a \
                  second FULL interface — it is **40% of the cost for a near-identical \
                  frame** (+4.1 vs +9.9 ms on the rim view), which is why Beautiful \
                  dropped back to one interface.",
        mode_options: &[
            ModeOption {
                value: 0,
                label: "flat",
                verdict: "DOCUMENTED NEGATIVE — the E6 gate failure (Pascal: \"the looking up out \
                          of the water part is completely broken\"). Past the critical angle the \
                          refraction totally internally reflects, Fresnel is 1, and with one \
                          interface of budget there was nothing left to add — so the whole \
                          mirrored region was ONE FLAT COLOUR. Since the window is only a \
                          ~97-degree cone, tilting the head underwater fills most of the screen \
                          with it. It is also why the view read as uniformly tinted and why the \
                          cone's rim read as harsh: a bright cone against a featureless surround \
                          has nothing to sit against. Kept selectable ONLY so the bench can price \
                          the fix and so the failure stays reproducible.",
            },
            ModeOption {
                value: 1,
                label: "cheap mirror",
                verdict: "SHIPPED: one more medium march, shaded CHEAPLY — albedo x downwelling x \
                          the face's own up-facing share, with NO shadow ray, NO ambient occlusion \
                          and NO light-volume sample. That keeps the GEOMETRY (the bed and the \
                          pool walls, mirrored) which is the whole point, while dropping the term \
                          that actually costs: underwater the dominant cost of a full bounce is \
                          the sun shadow ray, which has to walk metres of water. Same principle \
                          as the above-water half-modes — substitute a cheap stand-in for a term \
                          you cannot afford to trace properly, never a constant.",
            },
        ],
        bench: &[BenchPoint {
            section: BenchSection::Water,
            label: "water-tir-flat",
            overrides: &[(LeverId::WaterTirFallback, LeverValue::Mode(0))],
        }],
    },
    Lever {
        id: LeverId::WaterRayCutoff,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "ray cutoff (Fresnel weight)",
        default_value: LeverValue::Scalar(0.0),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 0.5,
            logarithmic: false,
        },
        verdict: "MEASURED (E6): -7.1% on the steep aerial view (10.156 vs 10.931 ms \
                  with the cutoff disabled) and inside noise on the other three, which \
                  is exactly where the reasoning said it would land — a steep view is \
                  where the mirror term is worth 2% and gets cut on almost every water \
                  pixel, so `full` costs the same as `refraction-only` there (10.156 vs \
                  10.019). It does not help the underwater views, where the expensive \
                  term (the medium march) is the one carrying the weight. Runtime \
                  (water_params.z), and the one \
                  optimization in E6 that costs nothing to look at: Fresnel already \
                  says how much each half of a water pixel is WORTH, so a term below \
                  the threshold takes its cheap analytic stand-in instead of a \
                  secondary ray. Head-on, the mirror carries F0 = 2% of the pixel and \
                  was being paid for with a full traced reflection AND a full \
                  shading; at grazing angles the transmitted term is the negligible \
                  one. 0.04 cuts the reflection ray for incidences steeper than ~57 \
                  degrees off the normal, which is most of an aerial view, and the \
                  substituted sky differs from the traced mirror by at most 4% of the \
                  pixel. 0 = always trace, which is the row this is measured against. \
                  SHIPPED AT 0 (dialled in the app, 2026-08-06) — i.e. ALWAYS TRACE. The \
                  stand-ins are visible on a surface being judged this closely, so the \
                  measured -7.1% above is now a cost being paid deliberately. It is the \
                  first thing to raise if the water pass needs time back.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Water,
            label: "water-full-nocutoff",
            overrides: &[(LeverId::WaterRayCutoff, LeverValue::Scalar(0.0))],
        }],
    },
    Lever {
        id: LeverId::WaterSunThroughLiquid,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::ShaderConst,
        shader_const: Some("WATER_SUN_THROUGH_LIQUID"),
        label: "sun reaches through water",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "MEASURED (E6): the cost is concentrated in ONE view and the look is \
                  not optional, so it ships on the tiers that can see under water. \
                  Sideways underwater it is **6.24 -> 11.06 ms (+77%)**, because a \
                  shadow ray from the bed walks metres of water voxel by voxel; the \
                  steep aerial view pays +8% (9.393 -> 10.156) and the grazing and \
                  looking-up views are inside noise (+1.9%, +0.1%). A \
                  correctness/cost trade rather than a free win. \
                  OFF, every submerged surface is in SHADOW, so a pool bed one metre \
                  down is lit by ambient alone and shallow water reads DARKER than \
                  the opaque water it replaced (measured on the top-down lakes view: \
                  dark navy against opaque water's bright cyan) — i.e. refraction \
                  stops paying for itself. ON, a sunlit bed is sunlit, and the CA \
                  pass's per-cell sun test follows through the same shared \
                  `trace_shadow_visibility`, so the light volume lights under water \
                  too. The cost is structural: a shadow ray that no longer stops at \
                  the surface walks the whole body VOXEL BY VOXEL, because water \
                  bricks are occupied and the chebyshev skip cannot help. Ships on \
                  the tiers that draw water properly; the zero-ray tiers turn it off, \
                  where it buys little because they cannot see under the surface \
                  anyway.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Water,
            label: "water-full-sunblocked",
            overrides: &[(LeverId::WaterSunThroughLiquid, LeverValue::Flag(false))],
        }],
    },
    Lever {
        id: LeverId::WaterWaves,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::ShaderConst,
        shader_const: Some("WATER_WAVES"),
        label: "wind waves on the surface",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "NOT YET MEASURED (W1/W2) — the bench row exists so the sweep prices \
                  it, and the number replaces this sentence. \
                  WHAT IT FIXES: a water voxel's surface is a perfectly flat \
                  axis-aligned face, which makes its Fresnel mirror a PERFECT mirror. \
                  Consequences: no sun glitter (the single strongest cue that a \
                  surface is liquid), and a reflected shoreline that never moves. ON, \
                  the surface normal comes from the analytic gradient of a sum of four \
                  directional gravity waves whose wind is the SAME history that drives \
                  the cloud deck and the weather — waves are its third consumer, not a \
                  fourth noise field. \
                  WHY IT SHOULD BE CHEAP: four sin/cos pairs and a dot each, ~30 ALU \
                  per water-surface pixel, zero memory traffic and no extra rays — \
                  against the 2.25-3.55 ms E1 measured for the marginal full-res \
                  secondary ray the same pixel already pays for. If the sweep says \
                  otherwise, that is the finding. \
                  OFF folds the whole field away and the surface is the flat face the \
                  pre-E6-waves renderer shaded, bit for bit, which is the isolation \
                  rule's requirement and the no-regression anchor.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Water,
            label: "water-full-flat",
            overrides: &[(LeverId::WaterWaves, LeverValue::Flag(false))],
        }],
    },
    Lever {
        id: LeverId::WaterWaveAmplitude,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "wave amplitude",
        default_value: LeverValue::Scalar(0.21),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 1.0,
            logarithmic: false,
        },
        verdict: "LOOK KNOB, runtime (water_optics.z), no rebuild — draggable while the \
                  app runs, which is the only way to judge glitter. 1.0 is the shipped \
                  look and 0.0 is exactly flat water. \
                  There is deliberately NO value above 1: WAVE_MAX_STEEPNESS (0.35) is \
                  a PHYSICAL ceiling — a deep-water wave breaks at the Stokes limiting \
                  steepness A*k ~ 0.443 — and it is also what guarantees the shading \
                  path's two safety properties (the normal tilts at most 19.3 degrees, \
                  so a mirror ray is thrown at most 38.6 degrees below the face and a \
                  refracted ray always stays under it). A slider that walked past the \
                  cap would quietly break both. Wanting rougher water is a change to \
                  that constant and to its argument, not a wider slider. \
                  SHIPPED AT 0.21 (dialled in the app, 2026-08-06). Cox & Munk describes \
                  OPEN water; a courtyard pool is nearly still, and the field at full \
                  amplitude reads as chop at this scale. The cap's argument is untouched — \
                  this only asks for less of what the wind justifies, never more.",
        mode_options: &[],
        bench: &[],
    },
    Lever {
        id: LeverId::WaterVisibilityDepth,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "visibility depth (blocks)",
        default_value: LeverValue::Scalar(10.0),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 24.0,
            logarithmic: false,
        },
        verdict: "LOOK KNOB, runtime (water_optics.w via `water::turbidity_per_meter`), no \
                  rebuild — and it costs NOTHING: turbidity is two adds in the coefficient \
                  accessors the medium march already calls, so every setting prices the \
                  same. \
                  WHAT IT FIXES: the material table carries PURE water, and pure water is \
                  far clearer than any pond. Measured against the look Pascal asked for \
                  (\"not more then 3 blocks deep and should fade deeper you go\"): at 3 m \
                  our blue channel still passed 0.835 where the reference passes 0.050 — \
                  16.8x too see-through, so a shallow bed read as one flat cyan sheet to \
                  the horizon with no fade at all. What hides a real lake's bed is \
                  SUSPENDED SEDIMENT, a term the model simply did not have. \
                  WHY IT IS ITS OWN GREY TERM rather than a scale on the spectral pair: \
                  reaching a 3 m blue horizon by scaling needs 16.6x, which takes red to \
                  0.001 within ONE block — the bed goes blue-black instantly instead of \
                  fading through its own colour. Sediment is broadband because the \
                  particles are much larger than the wavelength, and scattering-dominant \
                  (85%) for the same reason, which is also what keeps murky depths MILKY \
                  rather than black. \
                  At the shipped 3 blocks the total is (1.218, 0.888, 0.828)/m: a bed keeps \
                  0.30/0.41/0.44 at one block and 0.026/0.070/0.083 at three. 0 disables \
                  turbidity entirely and restores the pure-water model exactly, which is \
                  the isolation anchor rather than a setting anyone wants. \
                  SHIPPED AT 10 BLOCKS (dialled in the app, 2026-08-06), not the 3 the first \
                  E7 build shipped. Once the fade was actually VISIBLE it wanted to be much \
                  further out: 3 blocks hid the bed almost immediately, and what was wanted \
                  was water you can see into but not through. At 10, with the scales beside \
                  it, a bed keeps 0.79 of its light at one block, 0.49 at three and 0.10 at \
                  ten.",
        mode_options: &[],
        bench: &[],
    },
    Lever {
        id: LeverId::WaterTurbidityScattering,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "turbidity milkiness (scattering share)",
        default_value: LeverValue::Scalar(0.15),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 1.0,
            logarithmic: false,
        },
        verdict: "LOOK KNOB, runtime (water_params.w), no rebuild, and free — turbidity is \
                  two adds either way, so this only moves WHERE the extinction goes. \
                  WHAT IT IS: turbidity's split between scattering and absorption, which is \
                  a choice of what is SUSPENDED rather than a number to derive. Mineral silt \
                  is much larger than the wavelength, so it scatters broadband and absorbs \
                  little — a silty river genuinely is milky-bright, and 0.85 renders exactly \
                  that. What limits visibility in most standing water is instead dissolved \
                  organic matter and phytoplankton, which ABSORB: a pond you cannot see the \
                  bottom of is dark, not white. \
                  WHY IT EXISTS: the first E7 build shipped 0.85 and Pascal's report was \
                  \"now it looks al hazy and white\". Measured, that is exactly right — at \
                  0.85 ONE block of water in-scatters 0.38-0.47 of the sky's radiance, so \
                  even shallow water reads as a white sheet. At the shipped 0.15 it is \
                  0.07-0.11 and the deep-water albedo lands at (0.10, 0.16, 0.19) — dark, \
                  still ordered blue-over-green-over-red, and the bed shows through the top \
                  block. \
                  0 makes turbidity purely absorbing: the depths go nearly black and keep \
                  water's own steep blue tint. 1 is full milk. This is the dial to drag \
                  against a real pool, because how milky water should look is not something \
                  the model can decide.",
        mode_options: &[],
        bench: &[],
    },
    Lever {
        id: LeverId::WaterCaustics,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::ShaderConst,
        shader_const: Some("WATER_CAUSTICS"),
        label: "caustics on submerged surfaces",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "NOT YET MEASURED (E7) — the bench row exists so the sweep prices it. \
                  WHY IT SHOULD BE NEARLY FREE, and the reason to expect that rather than \
                  hope it: caustics ride on `water_sun_transmission`, which has ALREADY \
                  marched from the bed up to the surface for the sun's own transmittance. \
                  The entry point and the depth are in hand, so the added work is one \
                  HESSIAN of the wave field — the same four-component loop the gradient \
                  runs, with sin instead of cos — and no extra ray. It is therefore gated \
                  behind WATER_SUN_THROUGH_LIQUID, which is the expensive part. \
                  WHAT IT FIXES: a sunlit pool bed was uniformly lit, which reads as a \
                  photograph rather than water. Caustics are the focusing of the refracted \
                  sun by the surface's CURVATURE, so with W1's analytic height field in \
                  hand they are a Jacobian (`1 / |det(I + d(1 - 1/n)H)|`) rather than a \
                  noise texture — which means they respond to wind speed, bearing and \
                  wavelength for free, and the reference Shadertoy's two Perlin lobes \
                  could respond to none of the three. \
                  Measured, one block down: mean gain 1.01 at 1 m/s (range 0.86-1.19) and \
                  1.19 at 12 m/s (range 0.36-4.00) — light MOVED, not manufactured. \
                  OFF returns the sun term untouched, bit for bit.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Water,
            label: "water-full-no-caustics",
            overrides: &[(LeverId::WaterCaustics, LeverValue::Flag(false))],
        }],
    },
    Lever {
        id: LeverId::WaterBounceLight,
        subsystem: LeverSubsystem::Water,
        kind: LeverKind::ShaderConst,
        shader_const: Some("WATER_BOUNCE_LIGHT"),
        label: "water bounce light on terrain",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "NOT YET MEASURED (E7), and the one E7 lever that could genuinely cost — \
                  the bench row is here because it must be priced before it is trusted. \
                  IT SPENDS A RAY PER SHADED SURFACE, capped at \
                  WATER_BOUNCE_MAX_DISTANCE_METERS (16 m) and skipped outright for surfaces \
                  facing away from the mirrored sun, for liquids, and for submerged points. \
                  Note it also fires on SECONDARY hits (`shade_secondary` calls \
                  `shade_surface`), so a water pixel with a traced mirror pays it twice; if \
                  the sweep says that is the cost, restricting it to primary hits is the \
                  first thing to try. \
                  WHAT IT FIXES: the wobbling bright band a pool throws onto the wall \
                  beside it. Nothing else in the renderer produces it — CAGI's volume \
                  carries DIFFUSE bounce, and a mirror-smooth specular bounce is not \
                  diffuse, so without this a bank next to bright water is lit as if the \
                  water were matte. \
                  THE TRICK that makes it one ray: for a flat plane the reflected sun is a \
                  virtual sun BELOW it, so the direction toward that reflection is the sun \
                  direction mirrored in Y. The reflected direction then comes off the WAVE \
                  normal, which is what makes the band shimmer with the same field the \
                  surface glitters with. \
                  ITS APPROXIMATION IS STATED: one sample cannot know the solid angle the \
                  water subtends, so WATER_BOUNCE_STRENGTH (0.35) stands in for it. That is \
                  a look bound, not a derived quantity, and it is the reason this ships as \
                  a lever rather than as physics.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Water,
            label: "water-full-no-bounce-light",
            overrides: &[(LeverId::WaterBounceLight, LeverValue::Flag(false))],
        }],
    },
    // ---- World edits (E2) ----
    Lever {
        id: LeverId::EditWorldThread,
        subsystem: LeverSubsystem::WorldEdit,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "world thread",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "ON — E2's verdict, and the ONLY variant that meets the gate. The \
                  authority (the CPU brickmap) lives on its own thread; the frame \
                  thread sends a request and drains owned deltas, so it never waits \
                  for an edit, a clearance rebuild or a CAGI attribute rebuild. Off \
                  (variant A, inline) is identical in output and cheap for the common \
                  edit, but every rare-but-real cost lands INSIDE a frame: the \
                  clearance full-rebuild strategy and E4's ~0.5 s attribute rebuild on \
                  a GI resolution switch are frame hitches there and invisible here. \
                  Numbers in the bench doc's E2 section.",
        mode_options: &[],
        bench: &[
            BenchPoint {
                section: BenchSection::EditStorm,
                label: "edit-inline",
                overrides: &[(LeverId::EditWorldThread, LeverValue::Flag(false))],
            },
            // The decisive row: the SAME rare cost (a full clearance rebuild) is a
            // frame hitch inline and mere latency on the world thread.
            BenchPoint {
                section: BenchSection::EditStorm,
                label: "edit-inline-clearance-rebuild",
                overrides: &[
                    (LeverId::EditWorldThread, LeverValue::Flag(false)),
                    (LeverId::EditClearanceUpdate, LeverValue::Mode(1)),
                ],
            },
        ],
    },
    Lever {
        id: LeverId::EditClearanceUpdate,
        subsystem: LeverSubsystem::WorldEdit,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "clearance update",
        default_value: LeverValue::Mode(0),
        range: LeverRange::Discrete,
        verdict: "How the chebyshev clearance field (binding 10, S2's engine) is \
                  repaired when a removal EMPTIES a brick — the one asymmetric part of \
                  an edit: adding solid only shrinks clearance (exact, bounded, always \
                  local), removing it can grow clearance arbitrarily far away. The \
                  bounded local box ships; the full rebuild is the correctness \
                  reference and a real hitch.",
        mode_options: &[
            ModeOption {
                value: 0,
                label: "local box",
                verdict: "SHIPPED: recompute the exact transform inside a box around the freed \
                          brick, seeded from the ring outside it. Never overestimates (an \
                          overestimate would tunnel through geometry) and underestimates by at \
                          most the freed brick's own new clearance D — so for any edit into \
                          terrain, where D = 1, it is exact. Microseconds instead of tens of \
                          milliseconds.",
            },
            ModeOption {
                value: 1,
                label: "full rebuild",
                verdict: "Exact everywhere: two chamfer sweeps over all 500 000 brick cells. This \
                          is the number the local update is judged against, and it is a frame \
                          hitch on the inline variant — on the world thread it is merely a few \
                          frames of latency, which is why the pairing of the two levers matters.",
            },
        ],
        bench: &[BenchPoint {
            section: BenchSection::EditStorm,
            label: "edit-clearance-rebuild",
            overrides: &[(LeverId::EditClearanceUpdate, LeverValue::Mode(1))],
        }],
    },
    Lever {
        id: LeverId::EditClearanceRadius,
        subsystem: LeverSubsystem::WorldEdit,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "clearance radius (bricks)",
        default_value: LeverValue::Count(8),
        range: LeverRange::Rungs(&[2, 4, 8, 16]),
        verdict: "Half-width of the local box, in bricks. It buys HOW MANY cells become \
                  exact, not safety: the deficit bound (the freed brick's own new \
                  clearance) is radius-independent, and cost grows as the cube — a \
                  17-brick box is ~6 900 cells of chamfer, a 33-brick one ~36 000. 8 is \
                  the measured knee; 2 is the Quest rung.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::EditStorm,
            label: "edit-clearance-radius16",
            overrides: &[(LeverId::EditClearanceRadius, LeverValue::Count(16))],
        }],
    },
    Lever {
        id: LeverId::EditGiReflood,
        subsystem: LeverSubsystem::WorldEdit,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "re-flood GI on edit",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "ON: an edit changes geometry, so the light volume must respond. E2 does \
                  it the only way E4 offers — a GLOBAL re-flood, which costs no extra \
                  ms per frame (a cold flood is the same 0.46-0.76 ms per iteration as \
                  the steady state) but costs FRAMES: 32 frames / 0.53 s to bit-exact \
                  convergence. Acceptable for a placed block, wrong for a placed lamp, \
                  which is exactly why E5 owns dirty-region re-flooding. Off leaves the \
                  volume stale (visibly lit air where a block now stands) and is only \
                  there to isolate the edit pipeline's own cost in the bench.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::EditStorm,
            label: "edit-no-reflood",
            overrides: &[(LeverId::EditGiReflood, LeverValue::Flag(false))],
        }],
    },
    // ---- Materials (S1) ----
    Lever {
        id: LeverId::MaterialFaceRoles,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("MATERIAL_FACE_ROLES"),
        label: "per-face roles (top/side/bottom)",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "S1, ships ON on every tier except Potato: the hit already knows its \
                  face, so the cost is one flag test and one select. Potato disables it \
                  with the rest of the material detail to keep its cheap shader path. \
                  Cost is a flag test and a select on a hit that already knows its face: \
                  the DDA records the stepped axis and the ray's sign along it for E1's \
                  analytic corner AO, so the face is free and no traversal changes. Rows \
                  without authored roles upload their base values in all three slots, so \
                  they are bit-identical either way — any delta outside `grass` is a bug. \
                  Grass is the demonstration case: earth sides with a green top, which is \
                  what stops a cut bank reading as green rock.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Materials,
            label: "material-face-roles",
            overrides: &[(LeverId::MaterialFaceRoles, LeverValue::Flag(true))],
        }],
    },
    // ---- Materials (S2) ----
    Lever {
        id: LeverId::MaterialPatterns,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("MATERIAL_PATTERNS"),
        label: "pattern layers",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "S2, ships ON with the four-layer cap on every tier except Potato: \
                  the cost is a bit test on an unpatterned row and one generator \
                  evaluation per active layer per HIT on a patterned one — never per \
                  traversal step, which is the whole reason detail lives on the \
                  material rather than in the hot loop. The bench sweeps 0/1/2/4 \
                  layers because that per-layer slope is the number that decides how \
                  many layers a Quest tier can afford. Potato sets the cap to zero and \
                  disables the path; the other tiers show all authored layers.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Materials,
            label: "material-patterns",
            overrides: &[(LeverId::MaterialPatterns, LeverValue::Flag(true))],
        }],
    },
    Lever {
        id: LeverId::MaterialPatternCache,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("MATERIAL_PATTERN_CACHE"),
        label: "pattern field cache",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "WINNER, on with texel LOD. A direct-mapped cache over the \
                  texel lattice, filled by the shading pass itself: \
                  `pattern_snap_to_texels` already quantises the coordinate, so \
                  every pixel on a 1.56 cm texel asks the generator the same \
                  question — about a hundred of them at 2 m from a wall. It needs \
                  no extension, because only read_write storage TEXTURES are \
                  gated and this is a read_write storage BUFFER. Entries are one \
                  atomic u32, a 16-bit tag over a 16-bit value. NOT bit-exact: the \
                  value is quantised to 16 bits, so \
                  the pixel gates move by design and the comparison to make is \
                  against the uncached frame at a tolerance, not at zero.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Materials,
            label: "material-pattern-cache",
            // BOTH, and the first version of this row had only the second.
            // Section 9's baseline is `materials_off`, so a point that enables
            // the cache alone renders an UNPATTERNED frame — it came back 60%
            // faster than `material-patterns` and 0 differing pixels against
            // `material-flat`, which is what gave it away.
            overrides: &[
                (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                (LeverId::MaterialPatternCache, LeverValue::Flag(true)),
            ],
        }],
    },
    Lever {
        id: LeverId::MaterialPatternTexelLod,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("MATERIAL_PATTERN_TEXEL_LOD"),
        label: "pattern texel LOD",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "WINNER, on with the pattern cache. It makes the cache work AT RANGE. The \
                  cache pays in proportion to how many pixels land on one texel — \
                  about a hundred at 2 m, fewer than one far away — which is why \
                  it removed nothing on aerial views until its shader switch was wired. \
                  Now it cuts A from 5.124 to 4.507 ms and B from 6.242 to 5.518 ms. \
                  Halving the grid per doubling of distance \
                  manufactures the reuse that distance destroys, and anti-aliases \
                  while doing it, since what it removes is detail finer than a \
                  pixel. NOT octave LOD, which lost at ground level (+0.24 ms) \
                  because per-pixel octave counts are divergence: a coarser grid \
                  makes neighbouring pixels agree MORE. Powers of two so the grids \
                  nest and the pattern does not swim across an LOD change.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Materials,
            label: "material-pattern-cache-lod",
            overrides: &[
                (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                (LeverId::MaterialPatternCache, LeverValue::Flag(true)),
                (LeverId::MaterialPatternTexelLod, LeverValue::Flag(true)),
            ],
        }],
    },
    Lever {
        id: LeverId::MaterialPatternEntryProbe,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("MATERIAL_PATTERN_ENTRY_PROBE"),
        label: "entry-cost probe (measurement only)",
        default_value: LeverValue::Mode(0),
        range: LeverRange::Discrete,
        verdict: "AN INSTRUMENT, NOT A TIER. Every rung above 0 renders wrong \
                  output on purpose, so the pixel gates only pass at 0 and this \
                  never belongs in a preset. It exists because the cached ground \
                  residual is dominated by a layer whose GENERATOR COMPUTES \
                  NOTHING — ~1.47 ms for the first layer against ~0.15 ms for the \
                  second — which rules out both the noise and the layer count and \
                  leaves the per-layer scaffolding, unattributed. The rungs are \
                  cumulative and removed innermost first, so consecutive deltas \
                  sum to the total and the top rung lands on the layers-off floor: \
                  a decomposition the hardware itself closes. The cache is \
                  deliberately outside the ladder — rung 1 takes it with the \
                  generator, so every rung above prices the entry path cache-free.",
        mode_options: &[
            ModeOption {
                value: 0,
                label: "off",
                verdict: "The shipped path. The reference every rung is read against.",
            },
            ModeOption {
                value: 1,
                label: "no-generator",
                verdict: "The generator and the cache lookup replaced by \
                          `pattern_entry_sink`, which consumes the coordinate, salt \
                          and octave count so naga cannot delete the entry path \
                          along with its consumer. THIS RUNG IS THE ENTRY COST — \
                          everything above decomposes it.",
            },
            ModeOption {
                value: 2,
                label: "no-fade",
                verdict: "+ `pattern_fade` folded to 1.0. Prices two uniform \
                          component reads, two compares and the ease curve.",
            },
            ModeOption {
                value: 3,
                label: "no-salt",
                verdict: "+ `pattern_variation_salt` folded to 0. Only live on a \
                          face-frame layer with variation on, so a world-frame \
                          sweep should read zero here — which makes this rung a \
                          control as much as a measurement.",
            },
            ModeOption {
                value: 4,
                label: "no-snap",
                verdict: "+ `pattern_snap_to_texels` folded to identity. Prices the \
                          texel divide, the floor and the half-texel recentre — the \
                          quantisation the cache depends on.",
            },
            ModeOption {
                value: 5,
                label: "no-period",
                verdict: "+ the final `snapped / period`. The first of five rungs \
                          splitting `pattern_coordinate`, which the first pass \
                          through this ladder measured at 49-59% of the whole \
                          pattern path — more than the generator, and the same \
                          absolute cost aerial as at ground level, which is why \
                          neither the cache nor the texel LOD touches it.",
            },
            ModeOption {
                value: 6,
                label: "no-tile-frame",
                verdict: "+ the tile frame collapsed to world. Prices the tile \
                          branch's PRESENCE, not its use: the frame is runtime data, \
                          so `pattern_tile_of` — the face projection, the bonded \
                          tessellation and a per-tile hash, the largest block in the \
                          function — is resident and register-allocated on every \
                          layer whether or not anything authors a tile. A large \
                          delta here means the fix is to get the tessellation out of \
                          the hot function, not to make it faster.",
            },
            ModeOption {
                value: 7,
                label: "no-frames",
                verdict: "+ the voxel and face frames collapsed to world, which is \
                          what kills the `sample.voxel / vec3<i32>(8)` SIGNED \
                          INTEGER VECTOR DIVIDE — those two branches are its only \
                          readers. Conflates the branch with the divide on purpose: \
                          stubbing the index alone lets both branches fold to \
                          constants and over-credits the divide.",
            },
            ModeOption {
                value: 8,
                label: "no-drift",
                verdict: "+ the drift subtraction, and `pattern_drift_meters` with \
                          it — this is that function's only consumer, and with the \
                          texel LOD on it carries a `log2` of its own.",
            },
            ModeOption {
                value: 9,
                label: "no-coordinate",
                verdict: "+ the hit position itself. `sample.world_meters` becomes \
                          dead, so `pattern_sample`'s position reconstruct and clamp \
                          go too — which is why this rung is 'the coordinate stage \
                          including producing what it reads', not the function body \
                          alone.",
            },
            ModeOption {
                value: 10,
                label: "no-strength",
                verdict: "+ `pattern_strength` folded to the authored amount. Loses \
                          the face-mask test and the animation gain but stays \
                          per-layer and non-zero, so the blend cannot fold and the \
                          `strength <= 0` early-out cannot start deleting layers \
                          instead of pricing them.",
            },
            ModeOption {
                value: 11,
                label: "no-layers",
                verdict: "+ the slot loop never runs. The floor and the closure \
                          check: this must land on the layers-off measurement, and \
                          what the rung below costs over it is the per-slot row \
                          load, the target branch and the blend.",
            },
        ],
        bench: &[
            BenchPoint {
                section: BenchSection::Materials,
                label: "entry-1-no-generator",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternEntryProbe, LeverValue::Mode(1)),
                ],
            },
            BenchPoint {
                section: BenchSection::Materials,
                label: "entry-2-no-fade",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternEntryProbe, LeverValue::Mode(2)),
                ],
            },
            BenchPoint {
                section: BenchSection::Materials,
                label: "entry-3-no-salt",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternEntryProbe, LeverValue::Mode(3)),
                ],
            },
            BenchPoint {
                section: BenchSection::Materials,
                label: "entry-4-no-snap",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternEntryProbe, LeverValue::Mode(4)),
                ],
            },
            BenchPoint {
                section: BenchSection::Materials,
                label: "entry-5-no-period",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternEntryProbe, LeverValue::Mode(5)),
                ],
            },
            BenchPoint {
                section: BenchSection::Materials,
                label: "entry-6-no-tile-frame",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternEntryProbe, LeverValue::Mode(6)),
                ],
            },
            BenchPoint {
                section: BenchSection::Materials,
                label: "entry-7-no-frames",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternEntryProbe, LeverValue::Mode(7)),
                ],
            },
            BenchPoint {
                section: BenchSection::Materials,
                label: "entry-8-no-drift",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternEntryProbe, LeverValue::Mode(8)),
                ],
            },
            BenchPoint {
                section: BenchSection::Materials,
                label: "entry-9-no-coordinate",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternEntryProbe, LeverValue::Mode(9)),
                ],
            },
            BenchPoint {
                section: BenchSection::Materials,
                label: "entry-10-no-strength",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternEntryProbe, LeverValue::Mode(10)),
                ],
            },
            BenchPoint {
                section: BenchSection::Materials,
                label: "entry-11-no-layers",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternEntryProbe, LeverValue::Mode(11)),
                ],
            },
        ],
    },
    Lever {
        id: LeverId::MaterialPatternAnimation,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("MATERIAL_PATTERN_ANIMATION"),
        label: "pattern animation (gain + drift)",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "LOSER, stays ON (2026-08-03): folding the whole animation value \
                  away costs +0.060 / +0.053 / +0.004 / +0.010 ms. The register- \
                  residency theory was wrong — naga already folds \
                  `pattern_animation_identity()` through the call, so there was \
                  never an `array<vec4<f32>, 4>` live on the no-graph path to \
                  reclaim, and the const only adds a branch the optimiser then has \
                  to see through. Kept as a lever because it is the switch a \
                  derived per-graph const would flip if a future backend stops \
                  folding, and because the negative is worth keeping its evidence. \
                  IT ALSO CORRECTED THE PROBE: rung 7->8 reads 0.63-0.87 ms not \
                  because drift is expensive but because, with the snap already \
                  stubbed at rung 4, drift is the LAST consumer of \
                  `pattern_texels_at` — so the ladder charges the texel-grid \
                  computation to whichever rung removes its final reader. Read \
                  rungs 4 and 8 together as one ~0.8 ms item.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Materials,
            label: "material-patterns-no-animation",
            overrides: &[
                (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                (LeverId::MaterialPatternAnimation, LeverValue::Flag(false)),
            ],
        }],
    },
    Lever {
        id: LeverId::MaterialPatternGeneratorMask,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("MATERIAL_PATTERN_GENERATOR_MASK"),
        label: "generator mask (compiled-in generators)",
        default_value: LeverValue::Count(voxel_material::pattern::PATTERN_GENERATOR_MASK_ALL),
        // Two rungs, not a free bitmask field: the panel offers "everything" and
        // "only what the bench table authors", because those are the two values a
        // measurement compares. A derived mask would be computed, never dialled.
        range: LeverRange::Rungs(&[
            PATTERN_GENERATOR_MASK_SECTION_NINE,
            voxel_material::pattern::PATTERN_GENERATOR_MASK_ALL,
        ]),
        verdict: "WINNER (2026-08-03), and the proof that this pass is \
                  RESIDENCY-bound rather than ALU-bound. Pruning the nine \
                  generators bench section 9's table never reaches cut the pattern \
                  path by 6.0 / 5.5 / 3.5 / 6.2 percent (A/B/C/D) while leaving all \
                  four frames BIT-IDENTICAL — same differing-pixel count, same max \
                  channel delta — so this is speed for nothing, not a quality \
                  trade. All fourteen generators are resident in one function \
                  inlined into the shading pass, and the pass is latency-bound, so \
                  their register footprint becomes milliseconds through occupancy. \
                  Two earlier readings pointed here and are now explained: entry \
                  rung 6 charged 0.146 ms to the tile FRAME merely being present, \
                  and the whole pattern path costs ~4000 lane-ops per pixel for a \
                  few hundred ops of arithmetic. It also explains a NEGATIVE — \
                  folding the animation value away won nothing because naga had \
                  already folded it, so there were no registers there to reclaim. \
                  NEXT: make the mask DERIVED from the authored table and the \
                  material graphs, the way the cacheability analysis reads the node \
                  declarations. Shipping it as a hand-set lever would be a footgun; \
                  computed, it is free and cannot go stale.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Materials,
            label: "material-patterns-pruned-generators",
            overrides: &[
                (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                (
                    LeverId::MaterialPatternGeneratorMask,
                    LeverValue::Count(PATTERN_GENERATOR_MASK_SECTION_NINE),
                ),
            ],
        }],
    },
    Lever {
        id: LeverId::MaterialPatternStrength,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("MATERIAL_PATTERN_STRENGTH"),
        label: "pattern strength",
        default_value: LeverValue::Scalar(1.0),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 1.0,
            logarithmic: false,
        },
        verdict: "A global scale on every layer's amount. NOT a performance lever — \
                  the generator still runs at strength 0, and the bench row exists to \
                  show that rather than to find a saving. It is the taste knob, and \
                  the honest way to answer \"is this too much\" without editing 26 \
                  rows and losing the tuning. Use MaterialPatternMaxLayers to buy \
                  frames; use this to buy restraint.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Materials,
            label: "material-patterns-half-strength",
            overrides: &[
                (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                (LeverId::MaterialPatternStrength, LeverValue::Scalar(0.5)),
            ],
        }],
    },
    Lever {
        id: LeverId::MaterialParallax,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("MATERIAL_PARALLAX"),
        label: "parallax occlusion mapping",
        default_value: LeverValue::Flag(false),
        range: LeverRange::Discrete,
        verdict: "P1 — march the primary ray through the relief height field so \
                  the shading point lands on the raised plates: parallax, plates \
                  occluding what is behind them, and visible plate sides. A \
                  shading effect only — voxel silhouettes stay straight. Costs \
                  nothing on materials without displacement (the ceiling test \
                  folds it away) and the texel cache serves the march's height \
                  taps, so the price scales with relief coverage on screen.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Materials,
            label: "material-parallax-off",
            overrides: &[
                (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                (LeverId::MaterialParallax, LeverValue::Flag(false)),
            ],
        }],
    },
    Lever {
        id: LeverId::MaterialParallaxSamples,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("MATERIAL_PARALLAX_SAMPLES"),
        label: "parallax march samples",
        default_value: LeverValue::Count(24),
        // Zero is the march disabled with the flag still on — the row that
        // prices the ceiling test alone. LabPBR packs recommend 64 for
        // texture-sampled height fields; a texel-quantised procedural field
        // resolves plateau tops with fewer because the binary refine finishes
        // the job.
        range: LeverRange::Rungs(&[0, 8, 16, 24, 32, 64]),
        verdict: "Linear search steps from the relief ceiling to the face. Too \
                  few misses THIN walls at grazing angles (a plate edge sliver \
                  between two samples); the refine then never sees it. 24 holds \
                  up at plate scale; dense sub-texel relief wants more.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Materials,
            label: "material-parallax-8-samples",
            overrides: &[
                (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                (LeverId::MaterialParallax, LeverValue::Flag(true)),
                (LeverId::MaterialParallaxSamples, LeverValue::Count(8)),
            ],
        }],
    },
    Lever {
        id: LeverId::MaterialParallaxShadowSamples,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("MATERIAL_PARALLAX_SHADOW_SAMPLES"),
        label: "parallax self-shadow samples",
        default_value: LeverValue::Count(16),
        range: LeverRange::Rungs(&[0, 8, 16, 32]),
        verdict: "P2 — height-field march from the displaced point toward the \
                  sun, multiplying DIRECT sun visibility only. Soft by \
                  penetration depth, so plate joints get contact shadows without \
                  a second shadow map. Zero disables self-shadowing and leaves \
                  the traced voxel shadow untouched.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Materials,
            label: "material-parallax-shadow-off",
            overrides: &[
                (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                (LeverId::MaterialParallax, LeverValue::Flag(true)),
                (LeverId::MaterialParallaxShadowSamples, LeverValue::Count(0)),
            ],
        }],
    },
    Lever {
        id: LeverId::MaterialParallaxEnd,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("MATERIAL_PARALLAX_END_METERS"),
        label: "parallax distance cap (m)",
        default_value: LeverValue::Scalar(48.0),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 256.0,
            logarithmic: false,
        },
        verdict: "THE parallax perf knob. The march used to run to the pattern \
                  fade — hundreds of metres of terrain marching at full budget \
                  for sub-pixel offsets, which is what turned the DDA pass into \
                  10 ms on a grass world. A 5 cm relief offset at 48 m is about \
                  a pixel; past that the march is cost without picture. Zero \
                  disables parallax by distance alone.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Materials,
            label: "material-parallax-16m-cap",
            overrides: &[
                (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                (LeverId::MaterialParallax, LeverValue::Flag(true)),
                (LeverId::MaterialParallaxEnd, LeverValue::Scalar(16.0)),
            ],
        }],
    },
    Lever {
        id: LeverId::MaterialPatternMaxLayers,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("MATERIAL_PATTERN_MAX_LAYERS"),
        label: "max pattern layers per hit",
        default_value: LeverValue::Count(MAX_PATTERN_LAYERS as u32),
        // Every rung the cap can take, INCLUDING zero: the overlay renders a `Count`
        // lever as a radio row over these, and zero is the row that proves the
        // mechanism is free on a row that authors nothing (0 differing pixels over a
        // fully patterned table). Listing them all rather than a range because the
        // bench sweeps exactly these and the panel should offer exactly what was
        // measured.
        range: LeverRange::Rungs(&[0, 1, 2, 4]),
        verdict: "The tier knob for S2, and the only one that buys frames: a row with \
                  four layers costs four generator evaluations per hit, and this caps \
                  them whatever the row authored. Drops the TAIL of the stack, so \
                  lowering it degrades a material gracefully — the first layer is the \
                  one carrying the base look and the fourth is the one adding a \
                  detail nobody at Quest resolution will resolve. Swept 0/1/2/4 by \
                  the bench, which is where the per-layer slope comes from.",
        mode_options: &[],
        bench: &[
            BenchPoint {
                section: BenchSection::Materials,
                label: "material-patterns-1-layer",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternMaxLayers, LeverValue::Count(1)),
                ],
            },
            BenchPoint {
                section: BenchSection::Materials,
                label: "material-patterns-2-layers",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternMaxLayers, LeverValue::Count(2)),
                ],
            },
            BenchPoint {
                section: BenchSection::Materials,
                label: "material-patterns-0-layers",
                overrides: &[
                    (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                    (LeverId::MaterialPatternMaxLayers, LeverValue::Count(0)),
                ],
            },
        ],
    },
    Lever {
        id: LeverId::MaterialPatternOctaveLod,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::ShaderConst,
        shader_const: Some("PATTERN_OCTAVE_LOD"),
        label: "drop sub-pixel noise octaves",
        default_value: LeverValue::Flag(false),
        range: LeverRange::Discrete,
        verdict: "Stops a fractal generator summing octaves whose feature size has \
                  fallen below a pixel. Unlike every other knob in this subsystem it \
                  is not a quality trade: an octave under a pixel cannot be resolved \
                  and contributes only aliasing, so dropping it is the same argument \
                  mip-mapping makes, applied to a procedural sum. Never drops below \
                  one octave, so a distant layer softens toward its base frequency \
                  rather than popping; disappearing is what the fade distances are \
                  for. MEASURED 2026-08-02, and the result is SPLIT rather than the \
                  win it was predicted to be: -0.24 ms top-down (A) and -0.20 ms (B), \
                  but +0.09 ms at ground level (C) and +0.12 ms (D) — i.e. it COSTS \
                  frames in the scenario it was expected to help most. The likely \
                  mechanism is coherence, not arithmetic: a top-down shot puts every \
                  hit at a similar distance so a warp agrees on its octave budget, \
                  while a ground-level shot spans metres to the horizon within a \
                  warp and the budget loop diverges. That is a hypothesis and not a \
                  measurement. Default OFF, and kept as a lever rather than deleted \
                  because an aerial or map view is exactly where it does pay.",
        mode_options: &[],
        bench: &[BenchPoint {
            section: BenchSection::Materials,
            label: "material-patterns-octave-lod",
            overrides: &[
                (LeverId::MaterialPatterns, LeverValue::Flag(true)),
                (LeverId::MaterialPatternOctaveLod, LeverValue::Flag(true)),
            ],
        }],
    },
    Lever {
        id: LeverId::MaterialPatternFadeStart,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "texture fade start (m)",
        default_value: LeverValue::Scalar(PATTERN_FADE_START_METERS),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 250.0,
            logarithmic: false,
        },
        verdict: "Material detail remains fully sharp through this camera distance, \
                  then blends toward the unpatterned base until the fade-end distance. \
                  Dragging is runtime-only and does not rebuild shaders.",
        mode_options: &[],
        bench: &[],
    },
    Lever {
        id: LeverId::MaterialPatternFadeEnd,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "texture fade end (m)",
        default_value: LeverValue::Scalar(PATTERN_FADE_END_METERS),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 250.0,
            logarithmic: false,
        },
        verdict: "Material detail is fully gone at this camera distance. Values at or \
                  below fade start produce a sharp cutoff; zero disables fading. \
                  Dragging is runtime-only and does not rebuild shaders.",
        mode_options: &[],
        bench: &[],
    },
    Lever {
        id: LeverId::MaterialAnimationSpeed,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "animation speed",
        default_value: LeverValue::Scalar(1.0),
        range: LeverRange::Continuous {
            minimum: 0.0,
            maximum: 4.0,
            logarithmic: false,
        },
        verdict: "Scales how fast the animation clock advances. It scales the DELTA, \
                  not the total, so dragging changes tempo without the wave jumping. \
                  Zero holds every oscillator still but does NOT freeze event sensors, \
                  whose inputs still move with the camera — see the deterministic \
                  lever for that. Runtime-only, no shader rebuild.",
        mode_options: &[],
        bench: &[],
    },
    Lever {
        id: LeverId::MaterialAnimationDeterministic,
        subsystem: LeverSubsystem::Materials,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "deterministic animation",
        default_value: LeverValue::Flag(false),
        range: LeverRange::Discrete,
        verdict: "Pins the animation clock at zero AND empties the world-event field, \
                  so every sensor reads zero and a still camera renders identically \
                  frame over frame. This is what the pixel-diff bench sets. It does \
                  not reproduce an un-animated material: a frozen oscillator still \
                  returns a value, so animated scenes carry their own baselines.",
        mode_options: &[],
        bench: &[],
    },
    // ---- Direct lighting ----
    Lever {
        id: LeverId::SunShadow,
        subsystem: LeverSubsystem::Lighting,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "sun shadow",
        default_value: LeverValue::Flag(true),
        range: LeverRange::Discrete,
        verdict: "LOOK lever, on (shading_params.y — the slot the pruned penumbra \
                  scale vacated): the shading pass's traced sun shadow. Off renders \
                  every sun-facing surface fully sunlit, isolating what the shadow \
                  ray contributes next to AO and CAGI. Runtime mix, so the ray \
                  still traces when off — a look toggle, not a perf lever. CAGI's \
                  per-cell sun test and relief self-shadowing are deliberately \
                  untouched.",
        mode_options: &[],
        bench: &[],
    },
    // ---- Resolution (S0) ----
    Lever {
        id: LeverId::RenderScale,
        subsystem: LeverSubsystem::Resolution,
        kind: LeverKind::Runtime,
        shader_const: None,
        label: "render scale",
        default_value: LeverValue::Scalar(MAX_RENDER_SCALE),
        range: LeverRange::Continuous {
            minimum: MIN_RENDER_SCALE,
            maximum: MAX_RENDER_SCALE,
            logarithmic: false,
        },
        verdict: "The tier knob: DDA cost is per-pixel, so cost scales with the \
                  square of the scale. 1.0 on desktop (5.0-7.2 ms full stack); \
                  Potato ships 0.7 and Quest ~0.8 (E9 tunes it on device). Swept by \
                  the preset section, which dispatches at each preset's real size.",
        mode_options: &[],
        bench: &[],
    },
];

// ---- Quality settings + presets ----------------------------------------------

/// The named tiers. Presets differ by TECHNIQUE, not only by counts (plan E1c).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QualityPreset {
    Potato,
    Quest,
    Balanced,
    Beautiful,
    /// Whatever the user dialed: touching any knob switches here, and selecting
    /// a named preset overwrites the knobs again.
    Custom,
}

/// One preset row: a SPARSE override list over [`RenderQuality::baseline`].
/// Sparse on purpose — a future experiment (E4 CAGI iterations, E6 reflection
/// depth, E7 post effects) adds its lever to the registry with a default, and
/// only the presets that want something else grow a line. No preset needs
/// rewriting.
pub struct QualityPresetSpec {
    pub preset: QualityPreset,
    pub label: &'static str,
    pub summary: &'static str,
    pub overrides: &'static [(LeverId, LeverValue)],
}

impl QualityPresetSpec {
    /// The full settings this preset means.
    pub fn resolve(&self) -> RenderQuality {
        let mut quality = RenderQuality::baseline();
        quality.preset = self.preset;
        for (lever_id, value) in self.overrides {
            lever_id.apply(&mut quality, *value);
        }
        quality
    }
}

/// The preset table (E1b's per-tier recommendation, installed).
pub const QUALITY_PRESETS: &[QualityPresetSpec] = &[
    QualityPresetSpec {
        preset: QualityPreset::Potato,
        label: "Potato",
        summary: "corner AO + hard shadows, NO light volume, flat materials, zero-ray \
                  Fresnel-tinted water, render scale 0.7, AO fade 15->30 m",
        overrides: &[
            (LeverId::AoMode, LeverValue::Mode(1)),
            (LeverId::AoDistanceFade, LeverValue::Flag(true)),
            (LeverId::AoFadeStart, LeverValue::VoxelDistance(120)),
            (LeverId::AoFadeEnd, LeverValue::VoxelDistance(240)),
            // Potato is the one intentionally flat material tier.
            (LeverId::MaterialFaceRoles, LeverValue::Flag(false)),
            (LeverId::MaterialPatterns, LeverValue::Flag(false)),
            (LeverId::MaterialPatternMaxLayers, LeverValue::Count(0)),
            // The only tier without CAGI: it also proves the experiment is
            // excludable, and it is the E1c renderer bit for bit.
            (LeverId::GiEnabled, LeverValue::Flag(false)),
            // E6: the cheapest water that is still water. No secondary rays at
            // all, but the Fresnel curve and the underwater extinction are the
            // same ones the full model uses.
            (LeverId::WaterMode, LeverValue::Mode(1)),
            // The zero-ray tiers cannot see under the surface, so paying for the
            // sun to get there buys almost nothing.
            (LeverId::WaterSunThroughLiquid, LeverValue::Flag(false)),
            (LeverId::RenderScale, LeverValue::Scalar(0.7)),
        ],
    },
    QualityPresetSpec {
        preset: QualityPreset::Quest,
        label: "Quest",
        summary: "corner AO + hard shadows, ISOTROPIC CAGI at 1 m cells x 2 \
                  iterations, zero-ray Fresnel-tinted water, material fade \
                  10->50 m, render scale 0.8 — E9 tunes this tier on device",
        overrides: &[
            (LeverId::AoMode, LeverValue::Mode(1)),
            // The D5 flip made banks6 the baseline; Quest pins the isotropic
            // layout — a sixth of the light memory (3.5 vs 20.9 MiB), a third
            // of the CA cost (0.27-0.32 vs 0.97-1.10 ms) and none of the
            // directional sampler's +0.6 ms of DDA. E9 re-evaluates on device.
            (LeverId::GiLayout, LeverValue::Mode(0)),
            (LeverId::GiResolution, LeverValue::Count(8)),
            (LeverId::GiIterationsPerFrame, LeverValue::Count(2)),
            (LeverId::MaterialPatternFadeStart, LeverValue::Scalar(10.0)),
            (LeverId::MaterialPatternFadeEnd, LeverValue::Scalar(50.0)),
            (LeverId::WaterMode, LeverValue::Mode(1)),
            (LeverId::WaterSunThroughLiquid, LeverValue::Flag(false)),
            (LeverId::RenderScale, LeverValue::Scalar(0.8)),
        ],
    },
    QualityPresetSpec {
        preset: QualityPreset::Balanced,
        label: "Balanced",
        summary: "corner AO + hard shadows + directional-banks CAGI at 1 m cells \
                  x 2 iterations + Fresnel reflection & refraction at 1 water \
                  interface, material fade 10->50 m, full resolution — the \
                  shipped default",
        overrides: &[
            (LeverId::AoMode, LeverValue::Mode(1)),
            (LeverId::GiEnabled, LeverValue::Flag(true)),
            // The D5 reference pairing: banks6 (the baseline layout) at 8-voxel
            // cells. Banks at 4-voxel cells extrapolates to ~7 ms of CA.
            (LeverId::GiResolution, LeverValue::Count(8)),
            (LeverId::GiIterationsPerFrame, LeverValue::Count(2)),
            (LeverId::MaterialPatternFadeStart, LeverValue::Scalar(10.0)),
            (LeverId::MaterialPatternFadeEnd, LeverValue::Scalar(50.0)),
            (LeverId::WaterMode, LeverValue::Mode(4)),
            (LeverId::WaterBounces, LeverValue::Count(1)),
            (LeverId::WaterSunThroughLiquid, LeverValue::Flag(true)),
            (LeverId::RenderScale, LeverValue::Scalar(MAX_RENDER_SCALE)),
        ],
    },
    QualityPresetSpec {
        preset: QualityPreset::Beautiful,
        label: "Beautiful",
        summary: "ray-traced AO (2 rays / 8 voxels / cosine / falloff) + directional \
                  miss radiance + hard shadows + banks CAGI at 1 m cells x 4 \
                  iterations + full water optics, material fade 100->200 m, full \
                  resolution",
        overrides: &[
            (LeverId::AoMode, LeverValue::Mode(0)),
            (LeverId::AoRayCount, LeverValue::Count(2)),
            (LeverId::AoMaxDistance, LeverValue::VoxelDistance(8)),
            (LeverId::AoDirectionMode, LeverValue::Mode(0)),
            (LeverId::AoDistanceFalloff, LeverValue::Flag(true)),
            // Gate passed 2026-07-30: free next to the rays it reuses, and the
            // only tier that traces any. Cosine directions keep the estimate
            // unbiased; the 2-ray grain in dark foreground ships with it.
            (LeverId::AoMissRadiance, LeverValue::Flag(true)),
            // Twice Balanced's propagation budget: the volume converges in half
            // the frames after a sun change, at twice the CA cost.
            (LeverId::GiIterationsPerFrame, LeverValue::Count(4)),
            (LeverId::MaterialPatternFadeStart, LeverValue::Scalar(100.0)),
            (LeverId::MaterialPatternFadeEnd, LeverValue::Scalar(200.0)),
            // E6: ONE interface, like Balanced. The second one was measured against
            // the cheap mirrored stand-in on the window-rim view and the two frames
            // are near-identical, while the full bounce costs 2.5x what the stand-in
            // does — so Beautiful buys nothing here (step 1 verdict).
            (LeverId::WaterMode, LeverValue::Mode(4)),
            (LeverId::WaterBounces, LeverValue::Count(1)),
            (LeverId::WaterSunThroughLiquid, LeverValue::Flag(true)),
            (LeverId::RenderScale, LeverValue::Scalar(MAX_RENDER_SCALE)),
        ],
    },
    QualityPresetSpec {
        preset: QualityPreset::Custom,
        label: "Custom",
        summary: "the knobs as dialed — selecting this preset changes nothing",
        overrides: &[],
    },
];

/// The preset table row of `preset`.
pub(crate) fn preset_spec(preset: QualityPreset) -> &'static QualityPresetSpec {
    QUALITY_PRESETS
        .iter()
        .find(|spec| spec.preset == preset)
        .unwrap_or_else(|| panic!("preset {preset:?} has no QUALITY_PRESETS row"))
}

/// S1 — the material model's levers.
///
/// Its own struct rather than loose fields on [`RenderQuality`] because this arc
/// adds several more (pattern layers, animation, sub-voxel models), and each will
/// want the same treatment: a shader const, a registry row, a bench column.
// No `Eq`: the authored controls contain floats. Pipeline invalidation below
// compares only shader-constant fields; fade distance is a runtime uniform.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialSettings {
    /// S1 — read per-face-role albedo and roughness instead of the row's base.
    pub face_roles: bool,
    /// S2 — run the row's pattern layer stack.
    pub patterns: bool,
    /// S3 — reuse a generator's answer for every pixel that lands on the same
    /// texel, through a lazily-filled direct-mapped cache.
    pub pattern_cache: bool,
    /// S3 — halve the texel grid per doubling of distance, so distant pixels
    /// share texels and the cache has something to hit.
    pub pattern_texel_lod: bool,
    /// S3 — evaluate per-slot animation gain and drift. Off folds the whole
    /// `PatternAnimation` value away, which is what a material with no graph
    /// wants: its drift array is 16 registers of zeroes held live across the
    /// layer loop.
    pub pattern_animation: bool,
    /// S3 — one bit per generator code; a clear bit compiles that generator's
    /// body out of the shading pass. Derivable from the authored table and the
    /// material graphs, so it never trades detail for speed. All bits set is the
    /// shipped renderer.
    pub pattern_generator_mask: u32,
    /// S3 — the entry-cost bisection rung. A MEASUREMENT INSTRUMENT: anything
    /// above zero renders deliberately wrong output, and only zero ships.
    pub pattern_entry_probe: u32,
    /// S2 — global scale on every layer's amount, `0.0..=1.0`. The taste knob.
    pub pattern_strength: f32,
    /// P1 — parallax occlusion mapping: march the shading point onto the relief.
    pub parallax: bool,
    /// P1 — linear march steps from the relief ceiling to the face; 0 disables
    /// the march with the flag still compiled in.
    pub parallax_samples: u32,
    /// P2 — self-shadow march steps toward the sun; 0 disables self-shadowing.
    pub parallax_shadow_samples: u32,
    /// Camera distance in metres past which the march is skipped; the parallax
    /// perf knob, since terrain is mostly far pixels.
    pub parallax_end_meters: f32,
    /// S2 — layers evaluated per hit, whatever the row authored. The tier knob, and
    /// the only one of the three that buys frames.
    pub pattern_max_layers: u32,
    /// Tier 1b — stop summing fractal octaves whose feature size has fallen below a
    /// pixel. Quality-positive: those octaves only contributed aliasing.
    pub pattern_octave_lod: bool,
    /// Absolute camera distance in metres at which detail starts fading.
    pub pattern_fade_start_meters: f32,
    /// Absolute camera distance in metres where detail has fully faded out.
    pub pattern_fade_end_meters: f32,
    /// S3 — how fast the animation clock advances. Scales the per-frame delta,
    /// never the accumulated total, so changing it mid-drag alters tempo rather
    /// than teleporting every wave.
    pub animation_speed: f32,
    /// S3 — pin the clock at zero and empty the world-event field. The bench
    /// mode: it buys frame-to-frame stability, NOT parity with an un-animated
    /// material.
    pub animation_deterministic: bool,
}

impl MaterialSettings {
    /// Whether switching to `self` needs the shading pipeline recompiled: true when
    /// any SHADER-CONST lever moved. Pattern fade distance is intentionally absent:
    /// it rides the frame uniform and must remain smooth while dragged.
    ///
    /// Note that `pattern_strength` is in here, so dragging it recompiles. That is
    /// the deliberate trade S2 makes: a shader const lets naga fold a
    /// strength-of-zero layer away entirely, and the alternative — a uniform — would
    /// put a buffer read in the shading path to serve a knob that moves a handful of
    /// times in a session. The per-material `amount` sliders are the ones dragged
    /// while looking, and those live in the material table, which uploads without a
    /// rebuild.
    pub fn requires_pipeline_rebuild(&self, applied: &MaterialSettings) -> bool {
        self.face_roles != applied.face_roles
            || self.patterns != applied.patterns
            || self.pattern_cache != applied.pattern_cache
            || self.pattern_texel_lod != applied.pattern_texel_lod
            || self.pattern_entry_probe != applied.pattern_entry_probe
            || self.pattern_animation != applied.pattern_animation
            || self.pattern_generator_mask != applied.pattern_generator_mask
            || self.pattern_strength != applied.pattern_strength
            || self.parallax != applied.parallax
            || self.parallax_samples != applied.parallax_samples
            || self.parallax_shadow_samples != applied.parallax_shadow_samples
            || self.parallax_end_meters != applied.parallax_end_meters
            || self.pattern_max_layers != applied.pattern_max_layers
            || self.pattern_octave_lod != applied.pattern_octave_lod
    }

    /// Declare this group's compile-time consts into `sink`.
    ///
    /// `MATERIAL_PATTERN_STRENGTH` is the renderer's only genuinely real-valued lever, so it is
    /// the one carried as a scaled integer. Per-mille is finer than the slider's own pixel
    /// resolution, and the value every preset and test uses (1.0) is exact. It stays a const
    /// rather than a uniform for the reason [`Self::requires_pipeline_rebuild`] records: a
    /// strength of zero lets naga fold the whole layer away.
    pub(crate) fn declare_consts(&self, sink: &mut dyn ShaderConstSink) {
        sink.boolean("MATERIAL_FACE_ROLES", self.face_roles);
        sink.boolean("MATERIAL_PATTERNS", self.patterns);
        sink.boolean("MATERIAL_PATTERN_CACHE", self.pattern_cache);
        sink.boolean("MATERIAL_PATTERN_TEXEL_LOD", self.pattern_texel_lod);
        sink.unsigned(
            "MATERIAL_PATTERN_ENTRY_PROBE",
            self.pattern_entry_probe.min(PATTERN_ENTRY_PROBE_TOP),
        );
        sink.boolean("MATERIAL_PATTERN_ANIMATION", self.pattern_animation);
        sink.unsigned(
            "MATERIAL_PATTERN_GENERATOR_MASK",
            self.pattern_generator_mask & voxel_material::pattern::PATTERN_GENERATOR_MASK_ALL,
        );
        sink.scaled_float("MATERIAL_PATTERN_STRENGTH", self.pattern_strength, 1000);
        sink.boolean("MATERIAL_PARALLAX", self.parallax);
        sink.unsigned("MATERIAL_PARALLAX_SAMPLES", self.parallax_samples.min(128));
        sink.unsigned(
            "MATERIAL_PARALLAX_SHADOW_SAMPLES",
            self.parallax_shadow_samples.min(128),
        );
        sink.scaled_float("MATERIAL_PARALLAX_END_METERS", self.parallax_end_meters, 10);
        sink.unsigned(
            "MATERIAL_PATTERN_MAX_LAYERS",
            self.pattern_max_layers.min(MAX_PATTERN_LAYERS as u32),
        );
        sink.boolean("PATTERN_OCTAVE_LOD", self.pattern_octave_lod);
    }

    /// Patch this group's consts into a shader source, the same way every other
    /// lever group does — so the bench harness can A/B it by source substitution
    /// and the shipped default is literally the unpatched file.
    pub fn patch_shader_source(&self, source: &str) -> String {
        let mut patcher = SourcePatcher::new(source);
        self.declare_consts(&mut patcher);
        patcher.finish()
    }
}

impl Default for MaterialSettings {
    /// Material detail is part of the normal shipped look. Potato is the only tier
    /// that explicitly disables it; its sparse overrides preserve the cheap fallback.
    fn default() -> MaterialSettings {
        MaterialSettings {
            face_roles: true,
            patterns: true,
            pattern_cache: true,
            pattern_texel_lod: true,
            pattern_entry_probe: 0,
            pattern_animation: true,
            pattern_generator_mask: voxel_material::pattern::PATTERN_GENERATOR_MASK_ALL,
            pattern_strength: 1.0,
            parallax: false,
            parallax_samples: 24,
            parallax_shadow_samples: 16,
            parallax_end_meters: 48.0,
            pattern_max_layers: MAX_PATTERN_LAYERS as u32,
            pattern_octave_lod: false,
            animation_speed: 1.0,
            animation_deterministic: false,
            pattern_fade_start_meters: PATTERN_FADE_START_METERS,
            pattern_fade_end_meters: PATTERN_FADE_END_METERS,
        }
    }
}

/// Everything the renderer's quality is: the lever groups plus the resolution knob
/// and the preset tag. The overlay mutates it, the passes read it, and
/// `crate::passes::dda::build_shader_source` compiles it.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct RenderQuality {
    /// Which named tier these knobs came from ([`QualityPreset::Custom`] once
    /// the user touches anything).
    pub preset: QualityPreset,
    pub traversal: TraversalSettings,
    pub ambient_occlusion: AoSettings,
    /// E4 — the CAGI light volume.
    pub global_illumination: CagiSettings,
    /// E6 — water reflection, refraction and extinction.
    pub water: WaterSettings,
    /// E2 — world authority, threading and the edit pipeline. Not a *quality*
    /// knob: this struct is the app's whole lever surface, which is what the
    /// registry, the presets and the overlay panel are built on, so E2's knobs
    /// live here for the same reason every other lever does.
    pub world_edit: WorldEditSettings,
    /// S1 — the material model.
    pub materials: MaterialSettings,
    /// The shading pass's traced sun shadow (runtime, `shading_params.y`).
    /// A look lever: off renders sun-facing surfaces fully sunlit.
    pub sun_shadow: bool,
    /// Storage-texture size / surface size (1.0 = native).
    pub render_scale: f32,
}

impl RenderQuality {
    /// The compile-time consts the SHADING pass applies.
    ///
    /// Deliberately not the same list as [`Self::declare_volume_consts`], and the difference is
    /// not tidiness — the two passes patch different subsets today, and one of those differences
    /// is observable. `MATERIAL_FACE_ROLES` lives in the shared `world.wgsl`, and only the
    /// shading pass patches it, so at Potato the CA pass compiles it as `true` while the shading
    /// pass compiles it as `false`. Harmless today (the CA shader reads no material rows), but a
    /// single global def set would have quietly changed the CA pass's compiled value. Mirroring
    /// what each pass patches keeps behaviour identical by construction.
    ///
    /// Order matches [`crate::passes::dda::build_shader_source`].
    pub(crate) fn declare_shading_consts(&self, sink: &mut dyn ShaderConstSink) {
        self.traversal.declare_consts(sink);
        self.ambient_occlusion.declare_consts(sink);
        self.global_illumination.declare_volume_consts(sink);
        self.water.declare_consts(sink);
        self.materials.declare_consts(sink);
    }

    /// The compile-time consts the CA (light volume) pass applies.
    ///
    /// No AO and no material levers — it shades nothing. It does take the water levers, because
    /// `LIQUIDS_CAST_NO_SHADOW` is in the shared `world.wgsl` and a liquid that stops the shading
    /// pass's sun ray but not the volume's would light the bed under water in one and not the
    /// other. Order matches [`crate::passes::cagi::build_shader_source`].
    pub(crate) fn declare_volume_consts(&self, sink: &mut dyn ShaderConstSink) {
        self.traversal.declare_consts(sink);
        self.water.declare_consts(sink);
        self.global_illumination.declare_volume_consts(sink);
        self.global_illumination.declare_propagation_consts(sink);
    }

    /// The shading pass's consts as preprocessor definitions.
    pub fn shading_shader_defs(&self) -> ShaderDefs {
        let mut defs = ShaderDefs::default();
        self.declare_shading_consts(&mut defs);
        defs
    }

    /// The CA pass's consts as preprocessor definitions.
    pub fn volume_shader_defs(&self) -> ShaderDefs {
        let mut defs = ShaderDefs::default();
        self.declare_volume_consts(&mut defs);
        defs
    }
}

impl Default for RenderQuality {
    /// The Balanced tier — and, by `balanced_preset_is_the_shipped_baseline`,
    /// exactly [`RenderQuality::baseline`], i.e. the unpatched `dda.wgsl`.
    fn default() -> RenderQuality {
        preset_spec(QualityPreset::Balanced).resolve()
    }
}

impl RenderQuality {
    /// The typed `Default` of every lever group — the configuration the shipped
    /// shader source already contains, and the base every preset overrides.
    /// Deliberately built from the settings structs, NOT from the registry, so
    /// the registry-vs-defaults test has two independent sources to compare.
    pub fn baseline() -> RenderQuality {
        RenderQuality {
            preset: QualityPreset::Balanced,
            traversal: TraversalSettings::default(),
            ambient_occlusion: AoSettings::default(),
            global_illumination: CagiSettings::default(),
            water: WaterSettings::default(),
            materials: MaterialSettings::default(),
            world_edit: WorldEditSettings::default(),
            sun_shadow: true,
            render_scale: MAX_RENDER_SCALE,
        }
    }

    /// Overwrite every knob with `preset`'s table row. [`QualityPreset::Custom`]
    /// only re-tags — it means "the knobs as dialed".
    pub fn apply_preset(&mut self, preset: QualityPreset) {
        if preset == QualityPreset::Custom {
            self.preset = QualityPreset::Custom;
            return;
        }
        *self = preset_spec(preset).resolve();
    }

    /// Whether any KNOB differs (the preset tag ignored) — how the overlay
    /// notices that the user dialed something and flips to
    /// [`QualityPreset::Custom`].
    pub fn knobs_differ(&self, other: &RenderQuality) -> bool {
        let mut tag_matched = *self;
        tag_matched.preset = other.preset;
        tag_matched != *other
    }

    /// Whether switching from `applied` to `self` needs a new compute pipeline
    /// (a compile-time const changed). Runtime knobs and the render scale never
    /// do.
    pub fn requires_pipeline_rebuild(&self, applied: &RenderQuality) -> bool {
        self.traversal.requires_pipeline_rebuild(&applied.traversal)
            || self
                .ambient_occlusion
                .requires_pipeline_rebuild(&applied.ambient_occlusion)
            || self
                .global_illumination
                .requires_pipeline_rebuild(&applied.global_illumination)
            || self.water.requires_pipeline_rebuild(&applied.water)
            || self.materials.requires_pipeline_rebuild(&applied.materials)
    }

    /// Whether switching from `applied` to `self` needs the CAGI light volume
    /// reallocated (its resolution or the master lever changed) — a buffer
    /// rebuild, not a pipeline rebuild.
    pub fn requires_light_volume_rebuild(&self, applied: &RenderQuality) -> bool {
        self.global_illumination
            .requires_volume_rebuild(&applied.global_illumination)
    }

    /// Whether the light volume must be re-flooded from scratch: any change to
    /// what the CA *injects* or how it *transports* invalidates every cell (E4's
    /// world is static, so there is no finer invalidation until E5's edit API).
    pub fn requires_light_volume_reflood(&self, applied: &RenderQuality) -> bool {
        let live = &self.global_illumination;
        let previous = &applied.global_illumination;
        live.rule != previous.rule
            || live.sky_test != previous.sky_test
            || live.sun_cache != previous.sun_cache
            || live.sun_bounce != previous.sun_bounce
    }

    /// The runtime knobs, for this frame's lighting uniform.
    pub fn shading_params(&self) -> ShadingParams {
        ShadingParams {
            ambient_occlusion_strength: self.ambient_occlusion.strength,
            sun_shadow: if self.sun_shadow { 1.0 } else { 0.0 },
            ambient_occlusion_fade_start_voxels: self.ambient_occlusion.fade_start_voxels as f32,
            ambient_occlusion_fade_end_voxels: self.ambient_occlusion.fade_end_voxels as f32,
        }
    }

    /// The runtime CAGI knobs, for this frame's lighting uniform (E4).
    pub fn gi_params(&self) -> GiParams {
        GiParams {
            strength: self.global_illumination.strength,
            ambient_floor: self.global_illumination.ambient_floor,
            sun_bounce: self.global_illumination.sun_bounce,
            emissive_scale: self.global_illumination.emissive_scale,
        }
    }

    /// `render_height_pixels` is the DISPATCH height, not the window height: the
    /// octave cutoff is about what a shaded pixel can resolve, and a preset that
    /// renders at half scale resolves half as much.
    pub fn material_params(&self, render_height_pixels: u32) -> MaterialParams {
        MaterialParams {
            pattern_fade_start_meters: self.materials.pattern_fade_start_meters,
            pattern_fade_end_meters: self.materials.pattern_fade_end_meters,
            pixel_footprint_at_one_meter: crate::camera::pixel_footprint_at_one_meter(
                crate::camera::DEFAULT_VERTICAL_FOV_RADIANS,
                render_height_pixels,
            ),
        }
    }

    /// The animation clock, for this frame's lighting uniform (S3).
    ///
    /// Deterministic mode pins the reading here as well as clearing the event
    /// field, so the two halves of the guarantee cannot drift apart: a caller
    /// that forgets to clear events still gets a frozen clock, and the frozen
    /// clock is visible in one place rather than inferred from a lever.
    pub fn animation_params(
        &self,
        material_clock: AnimationClockSample,
        world_clock: AnimationClockSample,
        event_count: usize,
    ) -> (AnimationParams, EventParams) {
        if self.materials.animation_deterministic {
            return (AnimationParams::default(), EventParams::default());
        }
        (
            AnimationParams {
                remainder_seconds: material_clock.remainder_seconds,
                epoch: material_clock.epoch,
                reserved_flow: 0.0,
                reserved: 0.0,
            },
            EventParams {
                remainder_seconds: world_clock.remainder_seconds,
                epoch: world_clock.epoch,
                event_count: event_count as f32,
            },
        )
    }

    /// The runtime water knobs, for this frame's lighting uniform (E6).
    pub fn water_params(&self) -> WaterParams {
        WaterParams {
            absorption_scale: self.water.absorption_scale,
            scattering_scale: self.water.scattering_scale,
            ray_cutoff: self.water.ray_cutoff,
            turbidity_scattering_fraction: self.water.turbidity_scattering_fraction,
            // E6 step 3 turns this into a registry lever (the window-width dial);
            // until then it stays at the physical index, so no taste is shipped.
            refraction_strength: 1.0,
            // E7: the lever is a visibility depth in BLOCKS and the shader wants a
            // per-metre coefficient, so the conversion happens once here rather than per
            // pixel — and it is the tested `water::turbidity_per_meter`, not a second
            // expression of the same relation.
            turbidity_per_meter: crate::water::turbidity_per_meter(
                self.water.visibility_depth_blocks,
            ),
            // The amplitude LEVER is a quality setting and belongs here; the WIND is
            // not, and this module has none. The app attaches it with
            // `WaterParams::with_wind`, the same seam argument
            // `LightingUniform::with_output_params` makes: of the callers of this
            // function only the windowed app has a wind history, and the omission
            // fails safe — a caller that forgets leaves flat water rather than
            // inventing a breeze.
            waves: WaveField {
                amplitude_scale: self.water.wave_amplitude_scale,
                ..WaveField::FLAT
            },
        }
    }

    /// CA iterations to dispatch this frame — zero when the experiment is off, so
    /// the frame composer needs no second condition.
    pub fn gi_iterations_per_frame(&self) -> u32 {
        if self.global_illumination.enabled {
            self.global_illumination.iterations_per_frame
        } else {
            0
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passes::cagi::CAGI_SHADER_SOURCE;
    use crate::passes::dda::{build_shader_source, SHADER_SOURCE};
    use crate::shader_consts::patch_shader_const;

    /// Shader consts in the lever blocks that are NOT levers: the mode NAME
    /// constants and the fixed tuning thresholds. Anything else the shaders
    /// declare with a lever-ish name must have a registry row.
    const NON_LEVER_SHADER_CONSTANTS: &[&str] = &[
        "AO_MODE_RAY_TRACED",
        "AO_MODE_ANALYTIC_CORNER",
        "AO_MODE_OFF",
        "AO_SUN_BUDGET_THRESHOLD",
        "SHADOW_BIAS",
        "USE_COLUMN_HEIGHTS",
        // E4 CAGI: mode names, the light-word / attribute bit layout and the
        // fixed-point shift are structure, not levers.
        "CAGI_SAMPLE_NEAREST",
        "CAGI_SAMPLE_TRILINEAR",
        "CAGI_LAYOUT_ISOTROPIC",
        "CAGI_LAYOUT_BANKS6",
        // D2/D3 banks: fixed-point forms derived from the levers above them, and
        // the 1/6 emitter split — structure, not levers.
        "CAGI_BANKS_SKY_HORIZONTAL_NUMERATOR",
        "CAGI_BANKS_BOUNCE_NUMERATOR",
        "CAGI_BANKS_SEAL_PARTIAL_NUMERATOR",
        "CAGI_BANKS_SIXTH_NUMERATOR",
        "CAGI_RULE_MAX_DECREMENT",
        "CAGI_RULE_DIFFUSION_6",
        "CAGI_SKY_TEST_COLUMN_MAX",
        "CAGI_SKY_TEST_UPWARD_TRACE",
        "CAGI_CELL_SOLID",
        "CAGI_CELL_DATA_WORDS",
        "CAGI_TRANSMITTANCE_SHIFT",
        "CAGI_TRANSMITTANCE_LEVELS",
        "CAGI_EVENT_RESPONSE_SHIFT",
        "CAGI_CHANNEL_MASK",
        "CAGI_CHANNEL_MAX",
        "CAGI_RADIANCE_MAX",
        "CAGI_RADIANCE_PER_STEP",
        "CAGI_SAMPLE_SEARCH_STEPS",
        "CAGI_DIFFUSION_SHIFT",
        // E6 water: mode names, the derived mode predicates, the physical
        // constants (indices of refraction, F0, the extinction coefficients) and
        // the march's own bounds are structure and physics, not levers.
        "WATER_MODE_OPAQUE",
        "WATER_MODE_FRESNEL_TINT",
        "WATER_MODE_REFLECTION",
        "WATER_MODE_REFRACTION",
        "WATER_MODE_FULL",
        "WATER_TRACES_REFLECTION",
        "WATER_TRACES_REFRACTION",
        "WATER_AIR_INDEX",
        "WATER_INDEX",
        "WATER_ETA_INTO",
        "WATER_ETA_OUT",
        "WATER_FRESNEL_F0",
        "WATER_EXTINCTION_PER_METER",
        "WATER_MEDIUM_MAX_DISTANCE",
        "WATER_MEDIUM_MAX_STEPS",
        "WATER_MEDIUM_SOLID",
        "WATER_MEDIUM_AIR",
        "WATER_MEDIUM_LIMIT",
        "WATER_NO_MEDIUM",
        "WATER_TIR_FLAT",
        "WATER_TIR_STANDIN",
        "WATER_INTERFACE_FRESNEL",
        "WATER_INTERFACE_TRANSPARENT",
        // W2: structure, in the same sense SHADOW_BIAS is. It is how far above the
        // geometric face a mirror ray must leave so the DDA does not immediately
        // re-enter the water it is bouncing off; a slider on it would only offer a
        // choice between "correct" and "black speckles".
        "WATER_REFLECTION_MIN_COSINE",
        // E7 turbidity: the scattering/absorption split of suspended sediment is a
        // property of the particles (they are much larger than the wavelength, so they
        // scatter broadband and absorb little), not a dial. The DEPTH is the lever —
        // LeverId::WaterVisibilityDepth, which sets `water_optics.w`.
        // E7 caustics and bounce light: the divergence guard at a focus, and the bounce
        // ray's reach and single-sample strength. Stated bounds on approximations, not
        // dials — the two ON/OFF levers are WATER_CAUSTICS and WATER_BOUNCE_LIGHT.
        "WATER_CAUSTIC_MAX_GAIN",
        "WATER_BOUNCE_MAX_DISTANCE_METERS",
        "WATER_BOUNCE_STRENGTH",
        "WATER_BOUNCE_MIN_LOBE_RADIANS",
    ];

    /// Both pass shader sources — a lever's const lives in exactly one of them
    /// (the shared files appear in both, which is the point).
    fn shader_sources() -> [&'static str; 2] {
        [SHADER_SOURCE.as_str(), CAGI_SHADER_SOURCE.as_str()]
    }

    fn registry_ids() -> Vec<LeverId> {
        REGISTRY.iter().map(|lever| lever.id).collect()
    }

    /// A value for this lever that is NOT its default, chosen inside whatever
    /// bounds the lever declares so `apply` cannot clamp it straight back.
    fn moved_off_default(lever: &Lever) -> LeverValue {
        match lever.default_value {
            LeverValue::Flag(value) => LeverValue::Flag(!value),
            LeverValue::Mode(value) => {
                let other = lever
                    .mode_options
                    .iter()
                    .find(|option| option.value != value)
                    .unwrap_or_else(|| panic!("mode lever {:?} offers no second option", lever.id));
                LeverValue::Mode(other.value)
            }
            // DOWN, not up. `pattern_max_layers` is clamped to the array length on
            // the way to the shader, so a default-plus-one would patch the default
            // back in and the assertion below would fail on a correctly wired lever.
            LeverValue::Count(value) => LeverValue::Count(value.saturating_sub(1)),
            LeverValue::VoxelDistance(value) => LeverValue::VoxelDistance(value.saturating_sub(1)),
            LeverValue::Scalar(value) => match lever.range {
                LeverRange::Continuous {
                    minimum, maximum, ..
                } => {
                    let midpoint = (minimum + maximum) * 0.5;
                    let moved = if (midpoint - value).abs() > 1e-4 {
                        midpoint
                    } else {
                        (minimum + midpoint) * 0.5
                    };
                    LeverValue::Scalar(moved)
                }
                _ => LeverValue::Scalar(value * 0.5 - 0.25),
            },
        }
    }

    /// REGISTRY -> shader: moving a compile-time lever off its default must
    /// actually CHANGE the source the pipeline is built from.
    ///
    /// The gap this closes was shipped once and cost a full measurement session.
    /// `MATERIAL_PATTERN_TEXEL_LOD` had a registry row, a settings field, a shader
    /// const and a bench point — everything except a line in
    /// `MaterialSettings::patch_shader_source`. Flipping it therefore rebuilt a
    /// pipeline from byte-identical source, the feature measured as "no effect",
    /// and the natural next suspicion was the feature rather than the wiring.
    ///
    /// No per-lever test could have caught it, and that is the point: a per-lever
    /// test is written alongside the wiring it checks, so whatever made someone
    /// forget the wiring makes them forget the test. This one is generic over the
    /// registry, so it covers levers nobody has written yet.
    ///
    /// `registry_defaults_match_shader_source` is the other half — that the
    /// UNPATCHED file is the shipped configuration. Together they pin both ends:
    /// the default is what ships, and every other value can be reached.
    #[test]
    fn every_shader_const_lever_reaches_the_built_source() {
        for lever in REGISTRY {
            let Some(constant_name) = lever.shader_const else {
                continue;
            };
            let moved = moved_off_default(lever);
            assert_ne!(
                moved, lever.default_value,
                "{:?}: the test picked the default as its off-default value",
                lever.id
            );
            let literal = moved
                .wgsl_literal()
                .unwrap_or_else(|| panic!("{:?} has no WGSL literal", lever.id));

            let mut quality = RenderQuality::default();
            lever.id.apply(&mut quality, moved);
            assert_eq!(
                lever.id.read(&quality),
                moved,
                "{:?} did not survive an apply/read round trip, so the assertion \
                 below would be checking the default",
                lever.id
            );

            let built = [
                build_shader_source(&quality),
                crate::passes::cagi::build_shader_source(&quality),
            ];
            // AT LEAST ONE, not all, and the difference is deliberate.
            // `world.wgsl` is concatenated in front of both passes, so a const it
            // declares exists in the CAGI source too — but CAGI shades nothing and
            // calls neither `material_face_albedo` nor the pattern stack, so
            // `MATERIAL_FACE_ROLES` sitting at its default there is dead, not
            // wrong, and patching it would only mean recompiling the CA pass
            // whenever a shading lever moves. What this test is for is a lever that
            // reaches NO source at all, which is the failure that actually happened.
            let reached = built.iter().any(|source| {
                source.contains(&format!("const {constant_name}:"))
                    && patch_shader_const(source, constant_name, &literal) == *source
            });
            assert!(
                reached,
                "{:?} was set to {literal}, but no built source holds \
                 `{constant_name}` at that value — the lever has no line in its \
                 group's `patch_shader_source`",
                lever.id
            );
        }
    }

    /// REGISTRY -> shader: every compile-time row's default must already BE the
    /// shipped shader's value (patching it in is the identity), which also
    /// proves the const exists — `patch_shader_const` panics otherwise.
    #[test]
    fn registry_defaults_match_shader_source() {
        for lever in REGISTRY {
            let Some(constant_name) = lever.shader_const else {
                assert_eq!(
                    lever.kind,
                    LeverKind::Runtime,
                    "{:?} has no shader const, so it must be a runtime lever",
                    lever.id
                );
                continue;
            };
            assert_eq!(
                lever.kind,
                LeverKind::ShaderConst,
                "{:?} names a shader const, so it must be a compile-time lever",
                lever.id
            );
            let literal = lever
                .default_value
                .wgsl_literal()
                .unwrap_or_else(|| panic!("{:?} has no WGSL literal", lever.id));
            let mut sources_holding_the_const = 0;
            for shader_source in shader_sources() {
                if !shader_source.contains(&format!("const {constant_name}:")) {
                    continue;
                }
                sources_holding_the_const += 1;
                assert_eq!(
                    patch_shader_const(shader_source, constant_name, &literal),
                    shader_source,
                    "registry default for {:?} ({literal}) drifted from `{constant_name}` \
                     in the shader source",
                    lever.id
                );
            }
            assert!(
                sources_holding_the_const > 0,
                "shader const `{constant_name}` for {:?} exists in neither pass shader",
                lever.id
            );
        }
    }

    /// REGISTRY -> typed defaults: the registry's default column and the
    /// `Default` impls of the settings structs must agree.
    #[test]
    fn registry_defaults_match_typed_settings_defaults() {
        let baseline = RenderQuality::baseline();
        for lever in REGISTRY {
            assert_eq!(
                lever.id.read(&baseline),
                lever.default_value,
                "registry default for {:?} drifted from the typed Default impl",
                lever.id
            );
        }
    }

    /// shader -> REGISTRY: a lever added to `dda.wgsl` without a registry row
    /// fails here, so the drift gate closes in BOTH directions.
    #[test]
    fn every_lever_shaped_shader_const_has_a_registry_row() {
        let registered: Vec<&str> = REGISTRY
            .iter()
            .filter_map(|lever| lever.shader_const)
            .collect();
        for shader_source in shader_sources() {
            for line in shader_source.lines() {
                let Some(declaration) = line.strip_prefix("const ") else {
                    continue;
                };
                let Some(constant_name) = declaration.split(':').next() else {
                    continue;
                };
                let lever_shaped = constant_name.starts_with("ENABLE_")
                    || constant_name.starts_with("AO_")
                    || constant_name.starts_with("SHADOW_")
                    || constant_name.starts_with("CAGI_")
                    || constant_name.starts_with("WATER_");
                if !lever_shaped || NON_LEVER_SHADER_CONSTANTS.contains(&constant_name) {
                    continue;
                }
                assert!(
                    registered.contains(&constant_name),
                    "shader const `{constant_name}` is lever-shaped but has no REGISTRY row \
                     (add one, or list it in NON_LEVER_SHADER_CONSTANTS if it is a fixed \
                     tuning constant)"
                );
            }
        }
    }

    /// Every row must declare bounds the overlay can actually draw.
    ///
    /// `draw_lever` matches on the value SHAPE and then destructures the range,
    /// panicking when the two disagree — so a registry row with the wrong `range`
    /// crashes the app the first time its subsystem's panel is opened, with nothing
    /// upstream to catch it. That is exactly what S2's max-layers row did: a `Count`
    /// lever given `Discrete` instead of `Rungs`, which compiled, passed every other
    /// pinning test, shipped, and panicked on the Materials panel.
    ///
    /// The pairing is duplicated here rather than shared with the overlay on purpose,
    /// for the same reason `RenderQuality::baseline` is built from the settings structs
    /// instead of the registry: two independent statements of the rule can disagree,
    /// and one derived from the other cannot.
    #[test]
    fn every_lever_declares_bounds_the_overlay_can_draw() {
        for lever in REGISTRY {
            let value = lever.default_value;
            match value {
                // A checkbox needs no bounds at all.
                LeverValue::Flag(_) => {}
                // A radio row over `mode_options`, so the options are the bounds.
                LeverValue::Mode(_) => {
                    assert!(
                        !lever.mode_options.is_empty(),
                        "{:?} is a Mode lever with no mode_options — the overlay would \
                         draw a label and no controls",
                        lever.id
                    );
                    assert!(
                        lever
                            .mode_options
                            .iter()
                            .any(|option| LeverValue::Mode(option.value) == value),
                        "{:?}'s default is not one of its mode_options, so the panel \
                         would open with nothing selected",
                        lever.id
                    );
                }
                // A radio row over fixed rungs.
                LeverValue::Count(_) => {
                    let LeverRange::Rungs(rungs) = lever.range else {
                        panic!(
                            "{:?} is a Count lever, so the overlay draws a rung row and \
                             its range MUST be Rungs (got {:?})",
                            lever.id, lever.range
                        );
                    };
                    assert!(!rungs.is_empty(), "{:?} has no rungs", lever.id);
                    assert!(
                        rungs.iter().any(|rung| LeverValue::Count(*rung) == value),
                        "{:?}'s default is not one of its rungs {rungs:?}, so the panel \
                         would open with nothing selected",
                        lever.id
                    );
                }
                // Either a rung row or a metres slider.
                LeverValue::VoxelDistance(voxels) => match lever.range {
                    LeverRange::Rungs(rungs) => assert!(
                        rungs.contains(&voxels),
                        "{:?}'s default is not one of its rungs {rungs:?}",
                        lever.id
                    ),
                    LeverRange::Meters { minimum, maximum } => {
                        let meters = voxels as f32 / VOXELS_PER_METER;
                        assert!(
                            (minimum..=maximum).contains(&meters),
                            "{:?}'s default of {meters} m is outside its own \
                             {minimum}..={maximum} slider",
                            lever.id
                        );
                    }
                    other => panic!(
                        "{:?} is a VoxelDistance lever, so its range MUST be Rungs or \
                         Meters (got {other:?})",
                        lever.id
                    ),
                },
                // A continuous slider.
                LeverValue::Scalar(scalar) => {
                    let LeverRange::Continuous {
                        minimum, maximum, ..
                    } = lever.range
                    else {
                        panic!(
                            "{:?} is a Scalar lever, so the overlay draws a slider and \
                             its range MUST be Continuous (got {:?})",
                            lever.id, lever.range
                        );
                    };
                    assert!(
                        (minimum..=maximum).contains(&scalar),
                        "{:?}'s default of {scalar} is outside its own \
                         {minimum}..={maximum} slider",
                        lever.id
                    );
                }
            }
        }
    }

    /// The settings structs cannot grow an unregistered field: this
    /// destructuring stops compiling until the new field is added here, and the
    /// assertions then force it to have a registry row.
    #[test]
    fn every_settings_field_has_a_registry_lever() {
        let baseline = RenderQuality::baseline();
        let RenderQuality {
            preset: _,
            traversal,
            ambient_occlusion,
            global_illumination,
            water,
            world_edit,
            materials,
            sun_shadow,
            render_scale,
        } = baseline;
        let MaterialSettings {
            face_roles,
            patterns,
            pattern_cache,
            pattern_texel_lod,
            pattern_entry_probe,
            pattern_animation,
            pattern_generator_mask,
            pattern_strength,
            parallax,
            parallax_samples,
            parallax_shadow_samples,
            parallax_end_meters,
            pattern_max_layers,
            pattern_octave_lod,
            pattern_fade_start_meters,
            pattern_fade_end_meters,
            animation_speed,
            animation_deterministic,
        } = materials;
        let TraversalSettings {
            column_fast_forward,
            global_max_terminate,
            brick_bit_grid,
            distance_skip,
            directional_skip,
        } = traversal;
        let AoSettings {
            mode,
            strength,
            ray_count,
            max_distance_voxels,
            direction_mode,
            distance_falloff,
            brick_early_out,
            distance_fade,
            fade_start_voxels,
            fade_end_voxels,
            sun_aware_ray_budget,
            miss_radiance,
        } = ambient_occlusion;
        let CagiSettings {
            enabled: gi_enabled,
            cell_voxels,
            layout: gi_layout,
            banks_loss_per_meter,
            banks_side_loss_multiplier,
            banks_sky_horizontal,
            banks_bounce,
            banks_transmission_per_meter,
            banks_direction_mix,
            banks_seal_partial,
            rule,
            sample_mode,
            sky_test,
            sun_cache,
            transmission,
            reflectance,
            emissive,
            emitter_bounce,
            event_light,
            iterations_per_frame,
            strength: gi_strength,
            ambient_floor,
            sun_bounce,
            emissive_scale,
        } = global_illumination;
        let WaterSettings {
            mode: water_mode,
            bounces,
            tir_fallback,
            underwater_interface,
            absorption_scale,
            scattering_scale,
            ray_cutoff,
            sun_through_liquid,
            waves,
            wave_amplitude_scale,
            visibility_depth_blocks,
            caustics,
            bounce_light,
            turbidity_scattering_fraction,
        } = water;
        let WorldEditSettings {
            world_thread,
            clearance_update,
            clearance_radius_cells,
            gi_reflood,
        } = world_edit;

        let expected: Vec<(LeverId, LeverValue)> = vec![
            (
                LeverId::ColumnFastForward,
                LeverValue::Flag(column_fast_forward),
            ),
            (
                LeverId::GlobalMaxTerminate,
                LeverValue::Flag(global_max_terminate),
            ),
            (LeverId::BrickBitGrid, LeverValue::Flag(brick_bit_grid)),
            (LeverId::DistanceSkip, LeverValue::Flag(distance_skip)),
            (LeverId::DirectionalSkip, LeverValue::Flag(directional_skip)),
            (LeverId::AoMode, LeverValue::Mode(mode.shader_value())),
            (LeverId::AoStrength, LeverValue::Scalar(strength)),
            (LeverId::AoRayCount, LeverValue::Count(ray_count)),
            (
                LeverId::AoMaxDistance,
                LeverValue::VoxelDistance(max_distance_voxels),
            ),
            (
                LeverId::AoDirectionMode,
                LeverValue::Mode(direction_mode.shader_value()),
            ),
            (
                LeverId::AoDistanceFalloff,
                LeverValue::Flag(distance_falloff),
            ),
            (LeverId::AoBrickEarlyOut, LeverValue::Flag(brick_early_out)),
            (LeverId::AoDistanceFade, LeverValue::Flag(distance_fade)),
            (
                LeverId::AoFadeStart,
                LeverValue::VoxelDistance(fade_start_voxels),
            ),
            (
                LeverId::AoFadeEnd,
                LeverValue::VoxelDistance(fade_end_voxels),
            ),
            (
                LeverId::AoSunAwareRayBudget,
                LeverValue::Flag(sun_aware_ray_budget),
            ),
            (LeverId::AoMissRadiance, LeverValue::Flag(miss_radiance)),
            (LeverId::GiEnabled, LeverValue::Flag(gi_enabled)),
            (LeverId::GiResolution, LeverValue::Count(cell_voxels)),
            (
                LeverId::GiLayout,
                LeverValue::Mode(gi_layout.shader_value()),
            ),
            (
                LeverId::GiBanksLossPerMeter,
                LeverValue::Scalar(banks_loss_per_meter),
            ),
            (
                LeverId::GiBanksSideLossMultiplier,
                LeverValue::Scalar(banks_side_loss_multiplier),
            ),
            (
                LeverId::GiBanksSkyHorizontal,
                LeverValue::Scalar(banks_sky_horizontal),
            ),
            (LeverId::GiBanksBounce, LeverValue::Scalar(banks_bounce)),
            (
                LeverId::GiBanksTransmission,
                LeverValue::Scalar(banks_transmission_per_meter),
            ),
            (
                LeverId::GiBanksDirectionMix,
                LeverValue::Scalar(banks_direction_mix),
            ),
            (
                LeverId::GiBanksSealPartial,
                LeverValue::Scalar(banks_seal_partial),
            ),
            (LeverId::GiRule, LeverValue::Mode(rule.shader_value())),
            (
                LeverId::GiSkyTest,
                LeverValue::Mode(sky_test.shader_value()),
            ),
            (LeverId::GiSunCache, LeverValue::Flag(sun_cache)),
            (LeverId::GiTransmission, LeverValue::Flag(transmission)),
            (LeverId::GiReflectance, LeverValue::Flag(reflectance)),
            (LeverId::GiEmissive, LeverValue::Flag(emissive)),
            (LeverId::GiEmitterBounce, LeverValue::Flag(emitter_bounce)),
            (LeverId::GiEventLight, LeverValue::Flag(event_light)),
            (LeverId::GiEmissiveScale, LeverValue::Scalar(emissive_scale)),
            (
                LeverId::GiSampleMode,
                LeverValue::Mode(sample_mode.shader_value()),
            ),
            (
                LeverId::GiIterationsPerFrame,
                LeverValue::Count(iterations_per_frame),
            ),
            (LeverId::GiStrength, LeverValue::Scalar(gi_strength)),
            (LeverId::GiAmbientFloor, LeverValue::Scalar(ambient_floor)),
            (LeverId::GiSunBounce, LeverValue::Scalar(sun_bounce)),
            (
                LeverId::WaterMode,
                LeverValue::Mode(water_mode.shader_value()),
            ),
            (LeverId::WaterBounces, LeverValue::Count(bounces)),
            (
                LeverId::WaterTirFallback,
                LeverValue::Mode(tir_fallback.shader_value()),
            ),
            (
                LeverId::WaterUnderwaterInterface,
                LeverValue::Mode(underwater_interface.shader_value()),
            ),
            (
                LeverId::WaterAbsorption,
                LeverValue::Scalar(absorption_scale),
            ),
            (
                LeverId::WaterScattering,
                LeverValue::Scalar(scattering_scale),
            ),
            (LeverId::WaterRayCutoff, LeverValue::Scalar(ray_cutoff)),
            (
                LeverId::WaterSunThroughLiquid,
                LeverValue::Flag(sun_through_liquid),
            ),
            (LeverId::WaterWaves, LeverValue::Flag(waves)),
            (
                LeverId::WaterWaveAmplitude,
                LeverValue::Scalar(wave_amplitude_scale),
            ),
            (
                LeverId::WaterVisibilityDepth,
                LeverValue::Scalar(visibility_depth_blocks),
            ),
            (LeverId::WaterCaustics, LeverValue::Flag(caustics)),
            (LeverId::WaterBounceLight, LeverValue::Flag(bounce_light)),
            (
                LeverId::WaterTurbidityScattering,
                LeverValue::Scalar(turbidity_scattering_fraction),
            ),
            (LeverId::EditWorldThread, LeverValue::Flag(world_thread)),
            (
                LeverId::EditClearanceUpdate,
                LeverValue::Mode(clearance_update.shader_value()),
            ),
            (
                LeverId::EditClearanceRadius,
                LeverValue::Count(clearance_radius_cells),
            ),
            (LeverId::EditGiReflood, LeverValue::Flag(gi_reflood)),
            (LeverId::MaterialFaceRoles, LeverValue::Flag(face_roles)),
            (LeverId::MaterialPatterns, LeverValue::Flag(patterns)),
            (
                LeverId::MaterialPatternCache,
                LeverValue::Flag(pattern_cache),
            ),
            (
                LeverId::MaterialPatternTexelLod,
                LeverValue::Flag(pattern_texel_lod),
            ),
            (
                LeverId::MaterialPatternEntryProbe,
                LeverValue::Mode(pattern_entry_probe),
            ),
            (
                LeverId::MaterialPatternAnimation,
                LeverValue::Flag(pattern_animation),
            ),
            (
                LeverId::MaterialPatternGeneratorMask,
                LeverValue::Count(pattern_generator_mask),
            ),
            (
                LeverId::MaterialPatternStrength,
                LeverValue::Scalar(pattern_strength),
            ),
            (LeverId::MaterialParallax, LeverValue::Flag(parallax)),
            (
                LeverId::MaterialParallaxSamples,
                LeverValue::Count(parallax_samples),
            ),
            (
                LeverId::MaterialParallaxShadowSamples,
                LeverValue::Count(parallax_shadow_samples),
            ),
            (
                LeverId::MaterialParallaxEnd,
                LeverValue::Scalar(parallax_end_meters),
            ),
            (
                LeverId::MaterialPatternMaxLayers,
                LeverValue::Count(pattern_max_layers),
            ),
            (
                LeverId::MaterialPatternOctaveLod,
                LeverValue::Flag(pattern_octave_lod),
            ),
            (
                LeverId::MaterialPatternFadeStart,
                LeverValue::Scalar(pattern_fade_start_meters),
            ),
            (
                LeverId::MaterialPatternFadeEnd,
                LeverValue::Scalar(pattern_fade_end_meters),
            ),
            (
                LeverId::MaterialAnimationSpeed,
                LeverValue::Scalar(animation_speed),
            ),
            (
                LeverId::MaterialAnimationDeterministic,
                LeverValue::Flag(animation_deterministic),
            ),
            (LeverId::SunShadow, LeverValue::Flag(sun_shadow)),
            (LeverId::RenderScale, LeverValue::Scalar(render_scale)),
        ];

        let ids = registry_ids();
        assert_eq!(
            ids.len(),
            expected.len(),
            "REGISTRY has {} rows for {} settings fields",
            ids.len(),
            expected.len()
        );
        for (lever_id, value) in expected {
            assert!(
                ids.contains(&lever_id),
                "settings field behind {lever_id:?} has no REGISTRY row"
            );
            assert_eq!(
                lever_id.read(&baseline),
                value,
                "{lever_id:?} reads a different field than the one it documents"
            );
        }
    }

    #[test]
    fn registry_rows_are_unique_and_findable() {
        let ids = registry_ids();
        for (index, lever_id) in ids.iter().enumerate() {
            assert!(
                !ids[..index].contains(lever_id),
                "{lever_id:?} has two REGISTRY rows"
            );
            assert_eq!(lever(*lever_id).id, *lever_id);
        }
    }

    /// Every compile-time lever is swept by the harness forever after — the
    /// point of deriving the bench tables from this table. Runtime levers are
    /// exempt: the preset sweep varies them (render scale, fade range) or they
    /// are provably free (a multiply on the result).
    #[test]
    fn every_compile_time_lever_is_swept_by_the_bench() {
        let swept: Vec<LeverId> = REGISTRY
            .iter()
            .flat_map(|lever| lever.bench.iter())
            .flat_map(|point| point.overrides.iter().map(|(lever_id, _)| *lever_id))
            .collect();
        for lever in REGISTRY {
            if lever.kind != LeverKind::ShaderConst {
                continue;
            }
            assert!(
                swept.contains(&lever.id),
                "{:?} is compile-time but no bench point varies it — add a BenchPoint",
                lever.id
            );
        }
    }

    #[test]
    fn bench_labels_are_unique_within_a_section() {
        for section in [
            BenchSection::Traversal,
            BenchSection::RayTracedAo,
            BenchSection::CheapOcclusion,
            BenchSection::Cagi,
            BenchSection::EditStorm,
            BenchSection::Water,
        ] {
            let labels: Vec<&str> = bench_points_of(section).map(|point| point.label).collect();
            for (index, label) in labels.iter().enumerate() {
                assert!(
                    !labels[..index].contains(label),
                    "bench label `{label}` appears twice in {section:?}"
                );
            }
        }
    }

    /// Every bench point must apply cleanly (right value shape for its lever).
    #[test]
    fn every_bench_point_applies() {
        for point in REGISTRY.iter().flat_map(|lever| lever.bench.iter()) {
            let mut quality = RenderQuality::baseline();
            for (lever_id, value) in point.overrides {
                lever_id.apply(&mut quality, *value);
                assert_eq!(
                    lever_id.read(&quality),
                    *value,
                    "bench point `{}` did not take on {lever_id:?}",
                    point.label
                );
            }
        }
    }

    #[test]
    fn every_lever_round_trips_through_apply_and_read() {
        for lever in REGISTRY {
            let mut quality = RenderQuality::baseline();
            let probe = match lever.default_value {
                LeverValue::Flag(value) => LeverValue::Flag(!value),
                LeverValue::Mode(value) => {
                    // A different option of the same lever.
                    let other = lever
                        .mode_options
                        .iter()
                        .map(|option| option.value)
                        .find(|option_value| *option_value != value)
                        .expect("a mode lever needs at least two options");
                    LeverValue::Mode(other)
                }
                LeverValue::Count(value) => LeverValue::Count(value + 1),
                LeverValue::VoxelDistance(value) => LeverValue::VoxelDistance(value / 2),
                LeverValue::Scalar(value) => LeverValue::Scalar(value * 0.5),
            };
            lever.id.apply(&mut quality, probe);
            assert_eq!(
                lever.id.read(&quality),
                probe,
                "{:?} does not round-trip through apply/read",
                lever.id
            );
        }
    }

    /// Mode levers must offer exactly the options the shader has branches for,
    /// and every option must be selectable.
    #[test]
    fn mode_options_are_consistent() {
        for lever in REGISTRY {
            let is_mode = matches!(lever.default_value, LeverValue::Mode(_));
            assert_eq!(
                is_mode,
                !lever.mode_options.is_empty(),
                "{:?}: mode options and a Mode default must come together",
                lever.id
            );
            if !is_mode {
                continue;
            }
            for (index, option) in lever.mode_options.iter().enumerate() {
                assert_eq!(
                    option.value, index as u32,
                    "{:?} option `{}` must sit at its own shader value",
                    lever.id, option.label
                );
                assert!(
                    !option.verdict.is_empty(),
                    "{:?} option `{}` needs a verdict — it is the in-app answer to \
                     \"why is this off\"",
                    lever.id,
                    option.label
                );
            }
        }
    }

    #[test]
    fn every_lever_has_a_verdict() {
        for lever in REGISTRY {
            assert!(
                lever.verdict.len() > 20,
                "{:?} needs a real one-line verdict with numbers",
                lever.id
            );
        }
    }

    // ---- Presets ----

    /// Balanced IS the shipped configuration: the app's default pipeline is the
    /// unpatched `dda.wgsl`, so the bench's `current` column measures what the
    /// app ships.
    #[test]
    fn balanced_preset_is_the_shipped_baseline() {
        let balanced = preset_spec(QualityPreset::Balanced).resolve();
        assert_eq!(balanced, RenderQuality::baseline());
        assert_eq!(RenderQuality::default(), balanced);
        assert_eq!(build_shader_source(&balanced), SHADER_SOURCE.as_str());
    }

    /// The value of `const NAME: type = LITERAL;` in `source`, or `None` when this shader has no
    /// such const.
    fn declared_literal(source: &str, name: &str) -> Option<String> {
        let start = source.find(&format!("const {name}:"))?;
        let equals = source[start..].find('=')? + start;
        let semicolon = source[equals..].find(';')? + equals;
        Some(source[equals + 1..semicolon].trim().to_string())
    }

    /// The two sinks must agree on every lever, for every preset, in both pass sources.
    ///
    /// This is the load-bearing test of the whole `shader_consts` refactor. The def map is about
    /// to become what the composer compiles from, while `patch_shader_source` is what the shipped
    /// renderer compiles from today — and a lever whose def carried a different value than its
    /// patch would be a wrong pixel with no error anywhere. Because both come from one
    /// `declare_consts` list, disagreement can only mean a bug in a sink, and this catches that.
    #[test]
    fn shader_defs_agree_with_the_patched_source_for_every_preset() {
        let mut checked = 0;
        for spec in QUALITY_PRESETS {
            let quality = spec.resolve();
            for (label, source, defs) in [
                (
                    "dda",
                    build_shader_source(&quality),
                    quality.shading_shader_defs(),
                ),
                (
                    "cagi",
                    crate::passes::cagi::build_shader_source(&quality),
                    quality.volume_shader_defs(),
                ),
            ] {
                assert!(!defs.is_empty());
                for (name, value) in defs.iter() {
                    // A const absent from this pass's shader is correct — `water.wgsl` is not in
                    // the CA pass and `cagi.wgsl` is not in the shading pass.
                    let Some(actual) = declared_literal(&source, name) else {
                        continue;
                    };
                    assert_eq!(
                        actual,
                        value.wgsl_literal(),
                        "{:?}/{label}: def {name} = {value:?} renders to `{}` but the patched \
                         source declares `{actual}`",
                        spec.preset,
                        value.wgsl_literal()
                    );
                    checked += 1;
                }
            }
        }
        // Guard against the assertions being vacuous: if `declared_literal` stopped finding
        // anything, every comparison above would be skipped and the test would still pass.
        assert!(
            checked > 200,
            "only {checked} const comparisons ran — the lookup is probably broken"
        );
    }

    /// The def set must distinguish every configuration the shader source distinguishes, because
    /// it is about to become the pipeline cache key. Two presets that compile to different
    /// sources but produce equal defs would silently share a pipeline.
    #[test]
    fn presets_with_different_sources_have_different_shader_defs() {
        let named: Vec<_> = QUALITY_PRESETS
            .iter()
            .filter(|spec| spec.preset != QualityPreset::Custom)
            .map(|spec| {
                let quality = spec.resolve();
                (
                    spec.preset,
                    build_shader_source(&quality),
                    quality.shading_shader_defs(),
                )
            })
            .collect();
        for (preset, source, defs) in &named {
            for (other_preset, other_source, other_defs) in &named {
                if preset == other_preset {
                    continue;
                }
                if source != other_source {
                    assert_ne!(
                        defs, other_defs,
                        "{preset:?} and {other_preset:?} compile different sources but share a \
                         def set, so they would share a pipeline"
                    );
                }
            }
        }
    }

    #[test]
    fn every_preset_resolves_independently_of_the_previous_state() {
        for spec in QUALITY_PRESETS {
            if spec.preset == QualityPreset::Custom {
                continue;
            }
            let mut from_default = RenderQuality::default();
            from_default.apply_preset(spec.preset);

            let mut from_junk = RenderQuality::default();
            from_junk.ambient_occlusion.strength = 0.1;
            from_junk.traversal.column_fast_forward = true;
            from_junk.render_scale = MIN_RENDER_SCALE;
            from_junk.apply_preset(spec.preset);

            assert_eq!(
                from_default, from_junk,
                "{:?} must fully define the knobs it ships",
                spec.preset
            );
            assert_eq!(from_default.preset, spec.preset);
        }
    }

    #[test]
    fn custom_preset_keeps_the_dialed_knobs() {
        let mut quality = RenderQuality::default();
        quality.ambient_occlusion.strength = 0.33;
        quality.traversal.brick_bit_grid = true;
        let dialed = quality;
        quality.apply_preset(QualityPreset::Custom);
        assert_eq!(quality.preset, QualityPreset::Custom);
        assert!(!quality.knobs_differ(&dialed));
    }

    #[test]
    fn pattern_fade_slider_is_runtime_and_reaches_the_uniform() {
        let applied = RenderQuality::default();
        let mut edited = applied;
        edited.materials.pattern_fade_start_meters = 40.0;
        edited.materials.pattern_fade_end_meters = 90.0;
        assert!(!edited.requires_pipeline_rebuild(&applied));
        assert_eq!(edited.material_params(1440).pattern_fade_start_meters, 40.0);
        assert_eq!(edited.material_params(1440).pattern_fade_end_meters, 90.0);
    }

    #[test]
    fn pattern_cache_patches_the_shader_and_rebuilds_the_pipeline() {
        let applied = RenderQuality::default();
        let mut edited = applied;
        edited.materials.pattern_cache = false;

        assert!(edited.requires_pipeline_rebuild(&applied));
        assert!(
            build_shader_source(&applied).contains("const MATERIAL_PATTERN_CACHE: bool = true;")
        );
        assert!(
            build_shader_source(&edited).contains("const MATERIAL_PATTERN_CACHE: bool = false;")
        );
    }

    #[test]
    fn pattern_texel_lod_patches_the_shader_and_rebuilds_the_pipeline() {
        let applied = RenderQuality::default();
        let mut edited = applied;
        edited.materials.pattern_texel_lod = false;

        assert!(edited.requires_pipeline_rebuild(&applied));
        assert!(build_shader_source(&applied)
            .contains("const MATERIAL_PATTERN_TEXEL_LOD: bool = true;"));
        assert!(build_shader_source(&edited)
            .contains("const MATERIAL_PATTERN_TEXEL_LOD: bool = false;"));
    }

    #[test]
    fn preset_table_matches_the_e1b_per_tier_recommendation() {
        let potato = preset_spec(QualityPreset::Potato).resolve();
        assert_eq!(potato.ambient_occlusion.mode, AoMode::AnalyticCorner);
        assert!(potato.ambient_occlusion.distance_fade);
        assert_eq!(potato.ambient_occlusion.fade_start_voxels, 120); // 15 m
        assert_eq!(potato.ambient_occlusion.fade_end_voxels, 240); // 30 m
        assert_eq!(potato.render_scale, 0.7);
        assert!(!potato.materials.face_roles);
        assert!(!potato.materials.patterns);
        assert_eq!(potato.materials.pattern_max_layers, 0);

        let quest = preset_spec(QualityPreset::Quest).resolve();
        assert_eq!(quest.ambient_occlusion.mode, AoMode::AnalyticCorner);
        assert!(!quest.ambient_occlusion.distance_fade);
        assert_eq!(quest.materials.pattern_fade_start_meters, 10.0);
        assert_eq!(quest.materials.pattern_fade_end_meters, 50.0);
        assert_eq!(quest.render_scale, 0.8);

        let balanced = preset_spec(QualityPreset::Balanced).resolve();
        let beautiful = preset_spec(QualityPreset::Beautiful).resolve();
        assert_eq!(balanced.materials.pattern_fade_start_meters, 10.0);
        assert_eq!(balanced.materials.pattern_fade_end_meters, 50.0);
        assert_eq!(beautiful.materials.pattern_fade_start_meters, 100.0);
        assert_eq!(beautiful.materials.pattern_fade_end_meters, 200.0);

        for tier in [quest, balanced, beautiful] {
            assert!(tier.materials.face_roles);
            assert!(tier.materials.patterns);
            assert_eq!(tier.materials.pattern_max_layers, MAX_PATTERN_LAYERS as u32);
        }

        // E4 + D5: the GI tiering. Potato is the only tier without a light
        // volume, so it stays the "CAGI is excludable" proof; Quest keeps the
        // pre-D5 isotropic layout for memory and cost; Balanced and Beautiful
        // ship the banks6 reference pairing, Beautiful buying convergence
        // speed with iterations.
        assert!(!potato.global_illumination.enabled);
        assert_eq!(potato.gi_iterations_per_frame(), 0);
        assert!(quest.global_illumination.enabled);
        assert_eq!(quest.global_illumination.layout, CagiLayout::Isotropic);
        assert_eq!(quest.global_illumination.cell_voxels, 8);
        assert_eq!(quest.global_illumination.iterations_per_frame, 2);

        assert_eq!(balanced.global_illumination.layout, CagiLayout::Banks6);
        assert_eq!(balanced.global_illumination.cell_voxels, 8);

        assert!(beautiful.global_illumination.enabled);
        assert_eq!(beautiful.global_illumination.layout, CagiLayout::Banks6);
        assert_eq!(beautiful.global_illumination.cell_voxels, 8);
        assert_eq!(beautiful.global_illumination.iterations_per_frame, 4);
        assert_eq!(beautiful.ambient_occlusion.mode, AoMode::RayTraced);
        assert_eq!(beautiful.ambient_occlusion.ray_count, 2);
        assert_eq!(beautiful.ambient_occlusion.max_distance_voxels, 8);
        assert_eq!(
            beautiful.ambient_occlusion.direction_mode,
            AoDirectionMode::CosineHemisphere
        );
        assert!(beautiful.ambient_occlusion.distance_falloff);
        assert_eq!(beautiful.render_scale, MAX_RENDER_SCALE);
    }

    /// Four of the five presets need their own pipeline, and Custom starts as
    /// Balanced. This is what the startup pipeline cache pays for.
    ///
    /// It was THREE up to E4: Quest and Balanced differed by render scale alone,
    /// which is not a shader const. E6 gave Quest the zero-ray water mode, so the
    /// two tiers now compile different sources — a deliberate ~2 ms more of
    /// startup compile for the tier that most needs the cheap water.
    #[test]
    fn presets_need_four_distinct_pipelines() {
        let mut sources: Vec<String> = QUALITY_PRESETS
            .iter()
            .filter(|spec| spec.preset != QualityPreset::Custom)
            .map(|spec| build_shader_source(&spec.resolve()))
            .collect();
        sources.sort();
        sources.dedup();
        assert_eq!(sources.len(), 4);
    }

    #[test]
    fn preset_switches_that_only_move_runtime_knobs_skip_the_rebuild() {
        let balanced = preset_spec(QualityPreset::Balanced).resolve();
        // Quest needed no rebuild up to E4 (it differed by render scale and the
        // GI cell size, both runtime); since E6 gave it the zero-ray water mode it
        // does. The GI-resolution-only move is the case that must still not.
        let coarser_gi = {
            let mut quality = balanced;
            LeverId::GiResolution.apply(&mut quality, LeverValue::Count(8));
            quality
        };
        assert!(!coarser_gi.requires_pipeline_rebuild(&balanced));

        let potato = preset_spec(QualityPreset::Potato).resolve();
        let quest = preset_spec(QualityPreset::Quest).resolve();
        let beautiful = preset_spec(QualityPreset::Beautiful).resolve();
        assert!(potato.requires_pipeline_rebuild(&balanced));
        assert!(quest.requires_pipeline_rebuild(&balanced));
        assert!(beautiful.requires_pipeline_rebuild(&balanced));
    }

    /// E6: the per-tier water picks, and the two properties that make the tiering
    /// meaningful — Potato/Quest trace NO water rays, Balanced/Beautiful trace
    /// both halves, and only Beautiful spends a second interface.
    #[test]
    fn preset_table_tiers_the_water_optics_by_ray_budget() {
        let potato = preset_spec(QualityPreset::Potato).resolve();
        let quest = preset_spec(QualityPreset::Quest).resolve();
        let balanced = preset_spec(QualityPreset::Balanced).resolve();
        let beautiful = preset_spec(QualityPreset::Beautiful).resolve();

        for cheap in [potato, quest] {
            assert_eq!(cheap.water.mode, WaterMode::FresnelTint);
            assert!(!cheap.water.mode.traces_reflection());
            assert!(!cheap.water.mode.traces_refraction());
        }
        for full in [balanced, beautiful] {
            assert_eq!(full.water.mode, WaterMode::Full);
            assert!(full.water.mode.traces_reflection());
            assert!(full.water.mode.traces_refraction());
        }
        // E6 step 1: BOTH full tiers ship one interface. The second one was
        // measured against the cheap mirrored stand-in and buys a near-identical
        // frame for 2.5x the cost.
        assert_eq!(balanced.water.bounces, 1);
        assert_eq!(beautiful.water.bounces, 1);
        for tier in [potato, quest, balanced, beautiful] {
            assert_eq!(
                tier.water.tir_fallback,
                WaterTirFallback::CheapMirror,
                "no tier may ship the flat fallback — it is the E6 gate failure"
            );
            // E6 step 3: every tier shows a plainly transparent surface from below.
            // It is a LOOK decision, so it does not differ by tier — and it is also
            // the cheaper option, so no tier has a cost reason to differ either.
            assert_eq!(
                tier.water.underwater_interface,
                WaterUnderwaterInterface::Transparent,
                "the underwater interface is a look decision and must not vary by tier"
            );
        }
        // The sun-through-water lever follows the same split: the tiers that can
        // see under the surface pay for the sun to get there, the zero-ray tiers
        // do not.
        assert!(!potato.water.sun_through_liquid);
        assert!(!quest.water.sun_through_liquid);
        assert!(balanced.water.sun_through_liquid);
        assert!(beautiful.water.sun_through_liquid);
        // No tier ships opaque water: E6 exists because opaque water makes
        // swimming impossible to judge.
        for spec in QUALITY_PRESETS
            .iter()
            .filter(|spec| spec.preset != QualityPreset::Custom)
        {
            assert_ne!(
                spec.resolve().water.mode,
                WaterMode::Opaque,
                "{:?} ships opaque water",
                spec.preset
            );
        }
    }

    #[test]
    fn shading_params_carry_the_runtime_levers() {
        let potato = preset_spec(QualityPreset::Potato).resolve();
        let shading_params = potato.shading_params();
        assert_eq!(shading_params.ambient_occlusion_strength, 0.8);
        assert_eq!(shading_params.ambient_occlusion_fade_start_voxels, 120.0);
        assert_eq!(shading_params.ambient_occlusion_fade_end_voxels, 240.0);
    }

    /// E4: which kind of rebuild each CAGI lever forces. Getting this wrong means
    /// either a stale volume after a lever change or a pointless 20 MB
    /// reallocation every frame.
    #[test]
    fn cagi_levers_force_the_right_kind_of_rebuild() {
        let applied = RenderQuality::baseline();

        let finer = {
            let mut quality = applied;
            LeverId::GiResolution.apply(&mut quality, LeverValue::Count(4));
            quality
        };
        assert!(!finer.requires_pipeline_rebuild(&applied));
        assert!(finer.requires_light_volume_rebuild(&applied));

        let other_rule = {
            let mut quality = applied;
            LeverId::GiRule.apply(&mut quality, LeverValue::Mode(0));
            quality
        };
        assert!(other_rule.requires_pipeline_rebuild(&applied));
        assert!(!other_rule.requires_light_volume_rebuild(&applied));
        assert!(other_rule.requires_light_volume_reflood(&applied));

        // The sun bounce changes what the CA injects, so it needs a re-flood even
        // though it is a free runtime uniform for the shading pass.
        let dimmer_bounce = {
            let mut quality = applied;
            LeverId::GiSunBounce.apply(&mut quality, LeverValue::Scalar(0.1));
            quality
        };
        assert!(!dimmer_bounce.requires_pipeline_rebuild(&applied));
        assert!(dimmer_bounce.requires_light_volume_reflood(&applied));

        // Pure look knobs touch nothing.
        let stronger = {
            let mut quality = applied;
            LeverId::GiStrength.apply(&mut quality, LeverValue::Scalar(2.0));
            LeverId::GiAmbientFloor.apply(&mut quality, LeverValue::Scalar(0.5));
            LeverId::GiIterationsPerFrame.apply(&mut quality, LeverValue::Count(8));
            quality
        };
        assert!(!stronger.requires_pipeline_rebuild(&applied));
        assert!(!stronger.requires_light_volume_rebuild(&applied));
        assert!(!stronger.requires_light_volume_reflood(&applied));
    }

    #[test]
    fn render_scale_is_clamped_to_the_lever_range() {
        let mut quality = RenderQuality::baseline();
        LeverId::RenderScale.apply(&mut quality, LeverValue::Scalar(0.1));
        assert_eq!(quality.render_scale, MIN_RENDER_SCALE);
        LeverId::RenderScale.apply(&mut quality, LeverValue::Scalar(4.0));
        assert_eq!(quality.render_scale, MAX_RENDER_SCALE);
    }

    #[test]
    #[should_panic(expected = "takes a Flag")]
    fn applying_the_wrong_value_shape_panics() {
        let mut quality = RenderQuality::baseline();
        LeverId::DistanceSkip.apply(&mut quality, LeverValue::Mode(1));
    }
}
