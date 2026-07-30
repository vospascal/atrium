//! E1 — ray-traced ambient occlusion settings: pure data + shader-source
//! patching, no wgpu, no windowing (plan architecture rule).
//!
//! The AO variant knobs (ray count, max distance, direction strategy,
//! falloff) are COMPILE-TIME consts in `shaders/dda.wgsl` so naga folds every
//! disabled path away — the "E1 AO levers" block there is the source of
//! truth for the defaults. [`AoSettings`] mirrors those knobs on the Rust
//! side: the overlay mutates it, and when a compile-time field changes the
//! platform layer rebuilds the DDA pipeline from
//! [`AoSettings::shader_source`]. `strength` is the one RUNTIME knob — it
//! rides in the lighting uniform (`ao_params.x`) and never needs a rebuild.
//!
//! The headless benchmark builds its AO contenders through the same
//! [`AoSettings::shader_source`] path, so the bench measures exactly the
//! pipelines the app can ship.

use crate::passes::dda::SHADER_SOURCE;

/// AO ray direction strategy — mirrors `AO_DIRECTION_MODE` in `dda.wgsl`.
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
    fn wgsl_literal(self) -> &'static str {
        match self {
            AoDirectionMode::CosineHemisphere => "0u",
            AoDirectionMode::UniformHemisphere => "1u",
            AoDirectionMode::BentUp => "2u",
        }
    }
}

/// User-facing AO configuration. All fields except `strength` are
/// compile-time shader consts — changing them requires a pipeline rebuild
/// (see [`AoSettings::requires_pipeline_rebuild`]).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct AoSettings {
    /// Master lever (`ENABLE_AO`). Off = the shader folds AO away entirely
    /// and renders bit-identically to the pre-E1 renderer.
    pub enabled: bool,
    /// Runtime attenuation scale in [0, 1] (`lighting.ao_params.x`): the
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
}

impl Default for AoSettings {
    /// The recommended E1 variant — MUST match the "E1 AO levers" defaults in
    /// `dda.wgsl` (guarded by the `default_settings_match_shader_source`
    /// test), so the app's default pipeline is the unpatched shipped shader.
    fn default() -> AoSettings {
        AoSettings {
            enabled: true,
            strength: 0.8,
            ray_count: 2,
            max_distance_voxels: 8,
            direction_mode: AoDirectionMode::CosineHemisphere,
            distance_falloff: true,
        }
    }
}

impl AoSettings {
    /// The DDA shader source with this configuration's compile-time consts
    /// patched in. Equal to [`SHADER_SOURCE`] for the default settings.
    pub fn shader_source(&self) -> String {
        let mut shader_source = patch_shader_const(
            SHADER_SOURCE,
            "ENABLE_AO",
            if self.enabled { "true" } else { "false" },
        );
        shader_source = patch_shader_const(
            &shader_source,
            "AO_RAY_COUNT",
            &format!("{}u", self.ray_count),
        );
        shader_source = patch_shader_const(
            &shader_source,
            "AO_MAX_DISTANCE",
            &format!("{}.0", self.max_distance_voxels),
        );
        shader_source = patch_shader_const(
            &shader_source,
            "AO_DIRECTION_MODE",
            self.direction_mode.wgsl_literal(),
        );
        patch_shader_const(
            &shader_source,
            "AO_DISTANCE_FALLOFF",
            if self.distance_falloff {
                "true"
            } else {
                "false"
            },
        )
    }

    /// Whether switching from `applied` to `self` changes a compile-time
    /// const (everything except `strength`, which lives in the lighting
    /// uniform).
    pub fn requires_pipeline_rebuild(&self, applied: &AoSettings) -> bool {
        self.enabled != applied.enabled
            || self.ray_count != applied.ray_count
            || self.max_distance_voxels != applied.max_distance_voxels
            || self.direction_mode != applied.direction_mode
            || self.distance_falloff != applied.distance_falloff
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

    /// The Rust-side defaults and the shader's lever block must be the same
    /// configuration: the app's default pipeline is built from the UNPATCHED
    /// shipped shader, and the bench's `current` column must measure it.
    #[test]
    fn default_settings_match_shader_source() {
        assert_eq!(
            AoSettings::default().shader_source(),
            SHADER_SOURCE,
            "AoSettings::default() drifted from the E1 lever defaults in dda.wgsl"
        );
    }

    #[test]
    fn patched_source_carries_every_knob() {
        let settings = AoSettings {
            enabled: false,
            strength: 1.0,
            ray_count: 4,
            max_distance_voxels: 32,
            direction_mode: AoDirectionMode::BentUp,
            distance_falloff: false,
        };
        let shader_source = settings.shader_source();
        assert!(shader_source.contains("const ENABLE_AO: bool = false;"));
        assert!(shader_source.contains("const AO_RAY_COUNT: u32 = 4u;"));
        assert!(shader_source.contains("const AO_MAX_DISTANCE: f32 = 32.0;"));
        assert!(shader_source.contains("const AO_DIRECTION_MODE: u32 = 2u;"));
        assert!(shader_source.contains("const AO_DISTANCE_FALLOFF: bool = false;"));
    }

    #[test]
    #[should_panic(expected = "not found in dda.wgsl")]
    fn patching_a_missing_const_panics() {
        patch_shader_const(SHADER_SOURCE, "ENABLE_NONEXISTENT", "true");
    }

    #[test]
    fn strength_alone_never_forces_a_rebuild() {
        let applied = AoSettings::default();
        let mut runtime_only_change = applied;
        runtime_only_change.strength = 0.25;
        assert!(!runtime_only_change.requires_pipeline_rebuild(&applied));

        let mut compile_time_change = applied;
        compile_time_change.ray_count = 4;
        assert!(compile_time_change.requires_pipeline_rebuild(&applied));
    }
}
