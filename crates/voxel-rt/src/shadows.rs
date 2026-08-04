//! E1b — sun-shadow settings: pure data + shader-source patching, no wgpu,
//! no windowing (plan architecture rule). Mirrors the "E1b: shadow levers"
//! block in `shaders/dda.wgsl`, exactly as [`crate::ao`] mirrors the AO block.
//!
//! `mode` is a compile-time shader const (pipeline rebuild on change);
//! `penumbra_scale` is a RUNTIME knob riding in the lighting uniform
//! (`shading_params.y`), so the overlay slider needs no rebuild.

use crate::shader_consts::{ShaderConstSink, SourcePatcher};

/// Sun-shadow technique — mirrors `SHADOW_MODE` in `dda.wgsl`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ShadowMode {
    /// One binary any-hit shadow ray: fully lit or fully shadowed. The
    /// voxel-purist default and the reference of the Stage 2 correctness gate.
    Hard,
    /// IQ's single-ray penumbra trick over the chebyshev distance field
    /// (technique bank T1): the SAME shadow ray tracks
    /// `min(penumbra_scale * clearance / t)`, smoothstepped into a soft
    /// visibility factor. No extra rays.
    SoftDistanceField,
}

impl ShadowMode {
    /// The `SHADOW_MODE` u32 this technique compiles to — the one place the
    /// Rust↔WGSL numbering lives (the registry's mode options and the overlay
    /// radio buttons both go through it).
    pub fn shader_value(self) -> u32 {
        match self {
            ShadowMode::Hard => 0,
            ShadowMode::SoftDistanceField => 1,
        }
    }

    /// Inverse of [`ShadowMode::shader_value`]; panics on a value the shader has
    /// no branch for.
    pub fn from_shader_value(shader_value: u32) -> ShadowMode {
        match shader_value {
            0 => ShadowMode::Hard,
            1 => ShadowMode::SoftDistanceField,
            other => panic!("no SHADOW_MODE {other} in dda.wgsl"),
        }
    }
}

/// User-facing shadow configuration.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ShadowSettings {
    /// Which shadow technique to compile (`SHADOW_MODE`).
    pub mode: ShadowMode,
    /// Runtime penumbra scale (`lighting.shading_params.y`): the multiplier on
    /// `clearance / t`. It IS the reciprocal of the light source's angular
    /// radius — the penumbra softens whatever subtends more than
    /// `1 / penumbra_scale` radians — so larger = tighter, smaller = wider and
    /// darker. Ignored in [`ShadowMode::Hard`].
    pub penumbra_scale: f32,
}

impl Default for ShadowSettings {
    /// Hard shadows, matching the shadow lever defaults in `dda.wgsl` (guarded
    /// by `default_settings_match_shader_source`). The penumbra scale is the
    /// real sun's angular radius (0.5 degrees across, so 1 / 0.0087 rad ~ 115)
    /// — the only physically motivated value, and the one E1b's sweep measured
    /// as least broken.
    fn default() -> ShadowSettings {
        ShadowSettings {
            mode: ShadowMode::Hard,
            penumbra_scale: 115.0,
        }
    }
}

impl ShadowSettings {
    /// Declare this group's compile-time consts into `sink`.
    pub fn declare_consts(&self, sink: &mut dyn ShaderConstSink) {
        sink.unsigned("SHADOW_MODE", self.mode.shader_value());
    }

    /// `shader_source` with this configuration's compile-time consts patched
    /// in. Identity for the default settings.
    pub fn patch_shader_source(&self, shader_source: &str) -> String {
        let mut patcher = SourcePatcher::new(shader_source);
        self.declare_consts(&mut patcher);
        patcher.finish()
    }

    /// Whether switching from `applied` to `self` changes a compile-time const
    /// (only `mode` — `penumbra_scale` lives in the lighting uniform).
    pub fn requires_pipeline_rebuild(&self, applied: &ShadowSettings) -> bool {
        self.mode != applied.mode
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passes::dda::SHADER_SOURCE;

    #[test]
    fn default_settings_match_shader_source() {
        assert_eq!(
            ShadowSettings::default().patch_shader_source(&SHADER_SOURCE),
            SHADER_SOURCE.as_str(),
            "ShadowSettings::default() drifted from the shadow lever defaults in dda.wgsl"
        );
    }

    /// `SHADOW_MODE` must not be confused with the `SHADOW_MODE_*` name
    /// constants declared right above it.
    #[test]
    fn patched_source_carries_the_mode_without_touching_the_name_constants() {
        let shader_source = ShadowSettings {
            mode: ShadowMode::SoftDistanceField,
            penumbra_scale: 200.0,
        }
        .patch_shader_source(&SHADER_SOURCE);
        assert!(shader_source.contains("const SHADOW_MODE: u32 = 1u;"));
        assert!(shader_source.contains("const SHADOW_MODE_HARD: u32 = 0u;"));
        assert!(shader_source.contains("const SHADOW_MODE_SOFT_DISTANCE_FIELD: u32 = 1u;"));
    }

    #[test]
    fn penumbra_scale_alone_never_forces_a_rebuild() {
        let applied = ShadowSettings::default();
        let mut runtime_only_change = applied;
        runtime_only_change.penumbra_scale = 12.0;
        assert!(!runtime_only_change.requires_pipeline_rebuild(&applied));

        let mode_change = ShadowSettings {
            mode: ShadowMode::SoftDistanceField,
            ..applied
        };
        assert!(mode_change.requires_pipeline_rebuild(&applied));
    }
}
