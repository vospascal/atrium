//! E1/E1b — ambient-occlusion settings: pure data + shader-source patching,
//! no wgpu, no windowing (plan architecture rule).
//!
//! The AO variant knobs that live INSIDE the estimator (technique, ray count,
//! max distance, direction strategy, falloff, and E1b's brick early-out /
//! sun-aware budget) are COMPILE-TIME consts in `shaders/dda.wgsl` so naga
//! folds every disabled path away — the "E1/E1b: ambient occlusion levers"
//! block there is the source of truth for the defaults. [`AoSettings`] mirrors
//! those knobs on the Rust side: the overlay mutates it, and when a
//! compile-time field changes the platform layer rebuilds the DDA pipeline from
//! [`crate::passes::dda::build_shader_source`].
//!
//! The RUNTIME knobs — `strength` and the two `fade_*_voxels` distances — ride
//! in the lighting uniform (`shading_params.x`, `.z`, `.w`) and never need a
//! rebuild; E1c measured the fade distances as free to move out of the shader
//! consts (verdict in the bench doc's E1c section).
//!
//! The headless benchmark builds its AO contenders through the same patching
//! path, so the bench measures exactly the pipelines the app can ship, and
//! [`crate::variants::REGISTRY`] carries one row per knob (kind, default,
//! measured verdict) that the bench sweep, the overlay and the pinning tests
//! all read.

/// AO technique — mirrors `AO_MODE` in `dda.wgsl`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AoMode {
    /// E1's estimator: `ray_count` short occlusion rays through the shared
    /// `trace` core.
    RayTraced,
    /// E1b analytic corner occlusion: zero rays, the 8 occupancy bits around
    /// the hit face, bilinearly interpolated across it (technique bank T7).
    AnalyticCorner,
    /// E1b analytic 3x3x3: zero rays, hemisphere-weighted occupancy of the 26
    /// voxels around the face-front voxel.
    AnalyticNeighborhood,
    /// No occlusion at all — the shader folds AO away and renders
    /// bit-identically to the pre-E1 renderer.
    Off,
}

impl AoMode {
    /// The `AO_MODE` u32 this technique compiles to — the one place the
    /// Rust↔WGSL numbering lives (the registry's mode options and the overlay
    /// radio buttons both go through it).
    pub fn shader_value(self) -> u32 {
        match self {
            AoMode::RayTraced => 0,
            AoMode::AnalyticCorner => 1,
            AoMode::AnalyticNeighborhood => 2,
            AoMode::Off => 3,
        }
    }

    /// Inverse of [`AoMode::shader_value`]; panics on a value the shader has no
    /// branch for.
    pub fn from_shader_value(shader_value: u32) -> AoMode {
        match shader_value {
            0 => AoMode::RayTraced,
            1 => AoMode::AnalyticCorner,
            2 => AoMode::AnalyticNeighborhood,
            3 => AoMode::Off,
            other => panic!("no AO_MODE {other} in dda.wgsl"),
        }
    }

    fn wgsl_literal(self) -> String {
        format!("{}u", self.shader_value())
    }
}

/// AO ray direction strategy — mirrors `AO_DIRECTION_MODE` in `dda.wgsl`.
/// Only read by [`AoMode::RayTraced`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AoDirectionMode {
    /// Cosine-weighted hemisphere around the surface normal (matches the
    /// Lambert weighting of the ambient term).
    CosineHemisphere,
    /// Uniform hemisphere around the surface normal.
    UniformHemisphere,
    /// Fixed cone around the normal bent toward world up (sky-visibility
    /// proxy).
    BentUp,
}

impl AoDirectionMode {
    /// The `AO_DIRECTION_MODE` u32 this strategy compiles to.
    pub fn shader_value(self) -> u32 {
        match self {
            AoDirectionMode::CosineHemisphere => 0,
            AoDirectionMode::UniformHemisphere => 1,
            AoDirectionMode::BentUp => 2,
        }
    }

    /// Inverse of [`AoDirectionMode::shader_value`]; panics on a value the
    /// shader has no branch for.
    pub fn from_shader_value(shader_value: u32) -> AoDirectionMode {
        match shader_value {
            0 => AoDirectionMode::CosineHemisphere,
            1 => AoDirectionMode::UniformHemisphere,
            2 => AoDirectionMode::BentUp,
            other => panic!("no AO_DIRECTION_MODE {other} in dda.wgsl"),
        }
    }

    fn wgsl_literal(self) -> String {
        format!("{}u", self.shader_value())
    }
}

/// User-facing AO configuration. `strength` and the two `fade_*_voxels`
/// distances are runtime uniform fields; every other field is a compile-time
/// shader const and changing it requires a pipeline rebuild (see
/// [`AoSettings::requires_pipeline_rebuild`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AoSettings {
    /// Which estimator to run (`AO_MODE`), [`AoMode::Off`] included.
    pub mode: AoMode,
    /// Runtime attenuation scale in [0, 1] (`lighting.shading_params.x`): the
    /// ambient term is multiplied by `1 - strength * occlusion`.
    pub strength: f32,
    /// Occlusion rays per primary hit (`AO_RAY_COUNT`): 1, 2 or 4.
    pub ray_count: u32,
    /// Max occlusion-ray length in voxels (`AO_MAX_DISTANCE`): 8, 16 or 32.
    pub max_distance_voxels: u32,
    /// Ray direction strategy (`AO_DIRECTION_MODE`).
    pub direction_mode: AoDirectionMode,
    /// Distance-weighted occlusion (`AO_DISTANCE_FALLOFF`); false = binary.
    pub distance_falloff: bool,
    /// E1b lever 1 (`AO_BRICK_EARLY_OUT`): fall back to the analytic corner
    /// estimate on pixels whose 3x3x3 brick neighbourhood is empty, so no ray
    /// can find an occluder outside the hit's own brick.
    pub brick_early_out: bool,
    /// E1b lever 2 (`AO_DISTANCE_FADE`): fade occlusion out with primary-hit
    /// distance and skip the rays entirely past `fade_end_voxels`.
    pub distance_fade: bool,
    /// Start of the fade ramp, voxels (RUNTIME, `shading_params.z`; 8 voxels =
    /// 1 m). Only read when `distance_fade` is compiled in.
    pub fade_start_voxels: u32,
    /// End of the fade ramp, voxels (RUNTIME, `shading_params.w`) — beyond it
    /// no AO work happens at all.
    pub fade_end_voxels: u32,
    /// E1b lever 3 (`AO_SUN_AWARE_RAY_BUDGET`): halve the ray count on pixels
    /// where the direct sun term dominates.
    pub sun_aware_ray_budget: bool,
    /// Directional miss radiance (`AO_MISS_RADIANCE`, VGI I3D'11 §5.1): an
    /// occlusion ray that escapes reads the sky dome in its OWN direction, so
    /// the hemisphere term becomes a visibility-weighted sky integral instead
    /// of a flat constant times a scalar. Needs [`AoMode::RayTraced`] — the
    /// analytic estimators trace no rays, so there is no miss direction to
    /// sample.
    pub miss_radiance: bool,
}

impl Default for AoSettings {
    /// E1b's measured winner: analytic corner AO — ~20x cheaper than the
    /// ray-traced mode, noiseless, and it keeps the full stack under the
    /// plan's frame-time target. MUST match the AO lever defaults in
    /// `dda.wgsl` (guarded by the `default_settings_match_shader_source`
    /// test), so the app's default pipeline is the unpatched shipped shader.
    fn default() -> AoSettings {
        AoSettings {
            mode: AoMode::AnalyticCorner,
            strength: 0.8,
            ray_count: 2,
            max_distance_voxels: 8,
            direction_mode: AoDirectionMode::CosineHemisphere,
            distance_falloff: true,
            brick_early_out: false,
            distance_fade: false,
            fade_start_voxels: 240,
            fade_end_voxels: 480,
            sun_aware_ray_budget: false,
            miss_radiance: false,
        }
    }
}

impl AoSettings {
    /// `shader_source` with this configuration's compile-time consts patched
    /// in. Identity for the default settings.
    pub fn patch_shader_source(&self, shader_source: &str) -> String {
        let mut patched = patch_shader_const(shader_source, "AO_MODE", &self.mode.wgsl_literal());
        patched = patch_shader_const(&patched, "AO_RAY_COUNT", &format!("{}u", self.ray_count));
        patched = patch_shader_const(
            &patched,
            "AO_MAX_DISTANCE",
            &format!("{}.0", self.max_distance_voxels),
        );
        patched = patch_shader_const(
            &patched,
            "AO_DIRECTION_MODE",
            &self.direction_mode.wgsl_literal(),
        );
        patched = patch_shader_const(
            &patched,
            "AO_DISTANCE_FALLOFF",
            boolean_literal(self.distance_falloff),
        );
        patched = patch_shader_const(
            &patched,
            "AO_BRICK_EARLY_OUT",
            boolean_literal(self.brick_early_out),
        );
        patched = patch_shader_const(
            &patched,
            "AO_DISTANCE_FADE",
            boolean_literal(self.distance_fade),
        );
        patched = patch_shader_const(
            &patched,
            "AO_SUN_AWARE_RAY_BUDGET",
            boolean_literal(self.sun_aware_ray_budget),
        );
        patch_shader_const(
            &patched,
            "AO_MISS_RADIANCE",
            boolean_literal(self.miss_radiance),
        )
    }

    /// Whether switching from `applied` to `self` changes a compile-time
    /// const — i.e. everything except the runtime uniform fields (`strength`
    /// and the two fade distances).
    pub fn requires_pipeline_rebuild(&self, applied: &AoSettings) -> bool {
        let mut compile_time_only = *self;
        compile_time_only.strength = applied.strength;
        compile_time_only.fade_start_voxels = applied.fade_start_voxels;
        compile_time_only.fade_end_voxels = applied.fade_end_voxels;
        compile_time_only != *applied
    }
}

/// A WGSL `f32` literal for `value`.
///
/// `{:?}` rather than `{}` on purpose: Rust's `Display` for floats prints `1` for
/// `1.0`, which WGSL reads as an `i32` and rejects where an `f32` is expected.
/// `Debug` always emits the decimal point, so `1.0` stays `1.0` and `0.5` stays
/// `0.5`. The first shader const with a real-valued lever was S2's pattern strength;
/// every earlier scalar reached the shader as a uniform instead.
pub fn float_literal(value: f32) -> String {
    format!("{value:?}")
}

fn boolean_literal(value: bool) -> &'static str {
    if value {
        "true"
    } else {
        "false"
    }
}

/// Replace the value of `const {constant_name}: {type} = {value};` in a WGSL
/// source with `new_value_literal`. Panics when the const is missing — a
/// patch must never silently no-op (bench/overlay and shader cannot drift).
pub fn patch_shader_const(
    shader_source: &str,
    constant_name: &str,
    new_value_literal: &str,
) -> String {
    let declaration_prefix = format!("const {constant_name}:");
    let declaration_start = shader_source
        .find(&declaration_prefix)
        .unwrap_or_else(|| panic!("shader const `{constant_name}` not found in dda.wgsl"));
    let equals_offset = shader_source[declaration_start..]
        .find('=')
        .unwrap_or_else(|| panic!("shader const `{constant_name}` has no `=`"))
        + declaration_start;
    let semicolon_offset = shader_source[equals_offset..]
        .find(';')
        .unwrap_or_else(|| panic!("shader const `{constant_name}` has no `;`"))
        + equals_offset;
    format!(
        "{}= {new_value_literal}{}",
        &shader_source[..equals_offset],
        &shader_source[semicolon_offset..]
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passes::dda::SHADER_SOURCE;

    /// The Rust-side defaults and the shader's lever block must be the same
    /// configuration: the app's default pipeline is built from the UNPATCHED
    /// shipped shader, and the bench's `current` column must measure it.
    #[test]
    fn default_settings_match_shader_source() {
        assert_eq!(
            AoSettings::default().patch_shader_source(&SHADER_SOURCE),
            SHADER_SOURCE.as_str(),
            "AoSettings::default() drifted from the AO lever defaults in dda.wgsl"
        );
    }

    #[test]
    fn patched_source_carries_every_knob() {
        let settings = AoSettings {
            mode: AoMode::AnalyticNeighborhood,
            strength: 1.0,
            ray_count: 4,
            max_distance_voxels: 32,
            direction_mode: AoDirectionMode::BentUp,
            distance_falloff: false,
            brick_early_out: true,
            distance_fade: true,
            fade_start_voxels: 120,
            fade_end_voxels: 240,
            sun_aware_ray_budget: true,
            miss_radiance: true,
        };
        let shader_source = settings.patch_shader_source(&SHADER_SOURCE);
        assert!(shader_source.contains("const AO_MODE: u32 = 2u;"));
        assert!(shader_source.contains("const AO_RAY_COUNT: u32 = 4u;"));
        assert!(shader_source.contains("const AO_MAX_DISTANCE: f32 = 32.0;"));
        assert!(shader_source.contains("const AO_DIRECTION_MODE: u32 = 2u;"));
        assert!(shader_source.contains("const AO_DISTANCE_FALLOFF: bool = false;"));
        assert!(shader_source.contains("const AO_BRICK_EARLY_OUT: bool = true;"));
        assert!(shader_source.contains("const AO_DISTANCE_FADE: bool = true;"));
        assert!(shader_source.contains("const AO_SUN_AWARE_RAY_BUDGET: bool = true;"));
        assert!(shader_source.contains("const AO_MISS_RADIANCE: bool = true;"));
        // The fade DISTANCES are runtime uniform fields (E1c) — they must not
        // leave a const behind in the shader for the patcher to hit.
        assert!(!shader_source.contains("const AO_FADE_START_VOXELS"));
        assert!(!shader_source.contains("const AO_FADE_END_VOXELS"));
    }

    /// `AO_MODE` must not be confused with the `AO_MODE_*` name constants
    /// sitting right above it — patching the mode may only touch its own
    /// declaration.
    #[test]
    fn patching_the_mode_leaves_the_mode_name_constants_alone() {
        let shader_source = AoSettings {
            mode: AoMode::Off,
            ..AoSettings::default()
        }
        .patch_shader_source(&SHADER_SOURCE);
        assert!(shader_source.contains("const AO_MODE: u32 = 3u;"));
        assert!(shader_source.contains("const AO_MODE_RAY_TRACED: u32 = 0u;"));
        assert!(shader_source.contains("const AO_MODE_ANALYTIC_CORNER: u32 = 1u;"));
        assert!(shader_source.contains("const AO_MODE_ANALYTIC_NEIGHBORHOOD: u32 = 2u;"));
        assert!(shader_source.contains("const AO_MODE_OFF: u32 = 3u;"));
    }

    #[test]
    #[should_panic(expected = "not found in dda.wgsl")]
    fn patching_a_missing_const_panics() {
        patch_shader_const(&SHADER_SOURCE, "ENABLE_NONEXISTENT", "true");
    }

    #[test]
    fn runtime_knobs_alone_never_force_a_rebuild() {
        let applied = AoSettings::default();
        for runtime_only_change in [
            AoSettings {
                strength: 0.25,
                ..applied
            },
            AoSettings {
                fade_start_voxels: 120,
                ..applied
            },
            AoSettings {
                fade_end_voxels: 240,
                ..applied
            },
        ] {
            assert!(
                !runtime_only_change.requires_pipeline_rebuild(&applied),
                "{runtime_only_change:?} rides in the lighting uniform — no rebuild"
            );
        }

        for compile_time_change in [
            AoSettings {
                ray_count: 4,
                ..applied
            },
            AoSettings {
                mode: AoMode::RayTraced,
                ..applied
            },
            AoSettings {
                brick_early_out: true,
                ..applied
            },
            AoSettings {
                distance_fade: true,
                ..applied
            },
            AoSettings {
                sun_aware_ray_budget: true,
                ..applied
            },
        ] {
            assert!(
                compile_time_change.requires_pipeline_rebuild(&applied),
                "{compile_time_change:?} must force a pipeline rebuild"
            );
        }
    }
}
