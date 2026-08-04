//! S2 — traversal-lever settings: pure data + shader-source patching, no wgpu,
//! no windowing (plan architecture rule). Mirrors the "A/B benchmark levers"
//! block in `shaders/dda.wgsl`, exactly as [`crate::ao`] mirrors the AO block
//! and [`crate::shadows`] the shadow block.
//!
//! EVERY field here is a compile-time WGSL const, deliberately: these levers
//! sit inside the two coarse DDA loops, so naga must fold the disabled ones
//! away entirely — that folding IS the Stage 2 optimization round's result
//! (bench doc, "Standing verdicts"). None of them may become a runtime uniform
//! without a fresh measurement; the registry in [`crate::variants`] records
//! that decision per lever.

use crate::shader_consts::{ShaderConstSink, SourcePatcher};

/// The seven traversal fast paths. Defaults = the fastest combination measured on
/// Apple M3 Max; the measured verdict of each is one line in
/// [`crate::variants::REGISTRY`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TraversalSettings {
    /// `ENABLE_COLUMN_FAST_FORWARD`: upward rays above their XZ column's max
    /// occupied brick jump to the next column boundary in one step.
    pub column_fast_forward: bool,
    /// `ENABLE_DESCEND_FAST_FORWARD`: downward rays above their column's max
    /// jump straight to the top plane of the highest occupied brick.
    pub descend_fast_forward: bool,
    /// `ENABLE_GLOBAL_MAX_TERMINATE`: an upward ray above the tallest brick in
    /// the WORLD terminates as a miss immediately.
    pub global_max_terminate: bool,
    /// `ENABLE_ANY_HIT_SHADOW`: shadow rays run a specialized any-hit coarse
    /// loop instead of reusing the closest-hit `trace`.
    pub any_hit_shadow: bool,
    /// `ENABLE_BRICK_BIT_GRID`: the 1-bit-per-brick occupancy grid answers the
    /// coarse occupancy test instead of the skip-distance byte.
    pub brick_bit_grid: bool,
    /// `ENABLE_DISTANCE_SKIP`: chebyshev empty-space skip — jump a
    /// guaranteed-empty cube of bricks in one re-seeded step.
    pub distance_skip: bool,
    /// `ENABLE_DIRECTIONAL_SKIP`: AADF empty-space skip — jump the box spanned by
    /// the cell's six directional bounds instead of the chebyshev cube.
    pub directional_skip: bool,
}

impl Default for TraversalSettings {
    /// The shipped configuration: distance skip + global-max terminate on,
    /// everything else off. MUST match the lever block in `dda.wgsl` (guarded
    /// by `default_settings_match_shader_source`).
    fn default() -> TraversalSettings {
        TraversalSettings {
            column_fast_forward: false,
            descend_fast_forward: false,
            global_max_terminate: true,
            any_hit_shadow: false,
            brick_bit_grid: false,
            distance_skip: true,
            directional_skip: false,
        }
    }
}

impl TraversalSettings {
    /// Declare this group's compile-time consts into `sink`.
    ///
    /// The single list. [`Self::patch_shader_source`] renders it as shader text and
    /// [`crate::shader_consts::ShaderDefs`] renders it as preprocessor definitions, so the two
    /// cannot disagree.
    pub(crate) fn declare_consts(&self, sink: &mut dyn ShaderConstSink) {
        sink.boolean("ENABLE_COLUMN_FAST_FORWARD", self.column_fast_forward);
        sink.boolean("ENABLE_DESCEND_FAST_FORWARD", self.descend_fast_forward);
        sink.boolean("ENABLE_GLOBAL_MAX_TERMINATE", self.global_max_terminate);
        sink.boolean("ENABLE_ANY_HIT_SHADOW", self.any_hit_shadow);
        sink.boolean("ENABLE_BRICK_BIT_GRID", self.brick_bit_grid);
        sink.boolean("ENABLE_DISTANCE_SKIP", self.distance_skip);
        sink.boolean("ENABLE_DIRECTIONAL_SKIP", self.directional_skip);
    }

    /// `shader_source` with this configuration's compile-time consts patched
    /// in. Identity for the default settings.
    pub fn patch_shader_source(&self, shader_source: &str) -> String {
        let mut patcher = SourcePatcher::new(shader_source);
        self.declare_consts(&mut patcher);
        patcher.finish()
    }

    /// Every traversal lever is a compile-time const, so ANY difference needs a
    /// new pipeline.
    pub fn requires_pipeline_rebuild(&self, applied: &TraversalSettings) -> bool {
        self != applied
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::passes::dda::SHADER_SOURCE;

    #[test]
    fn default_settings_match_shader_source() {
        assert_eq!(
            TraversalSettings::default().patch_shader_source(&SHADER_SOURCE),
            SHADER_SOURCE.as_str(),
            "TraversalSettings::default() drifted from the lever block in dda.wgsl"
        );
    }

    #[test]
    fn patched_source_carries_every_lever() {
        let shader_source = TraversalSettings {
            column_fast_forward: true,
            descend_fast_forward: true,
            global_max_terminate: false,
            any_hit_shadow: true,
            brick_bit_grid: true,
            distance_skip: false,
            directional_skip: false,
        }
        .patch_shader_source(&SHADER_SOURCE);
        assert!(shader_source.contains("const ENABLE_COLUMN_FAST_FORWARD: bool = true;"));
        assert!(shader_source.contains("const ENABLE_DESCEND_FAST_FORWARD: bool = true;"));
        assert!(shader_source.contains("const ENABLE_GLOBAL_MAX_TERMINATE: bool = false;"));
        assert!(shader_source.contains("const ENABLE_ANY_HIT_SHADOW: bool = true;"));
        assert!(shader_source.contains("const ENABLE_BRICK_BIT_GRID: bool = true;"));
        assert!(shader_source.contains("const ENABLE_DISTANCE_SKIP: bool = false;"));
        assert!(shader_source.contains("const ENABLE_DIRECTIONAL_SKIP: bool = false;"));
    }

    #[test]
    fn any_lever_change_forces_a_rebuild() {
        let applied = TraversalSettings::default();
        for changed in [
            TraversalSettings {
                column_fast_forward: true,
                ..applied
            },
            TraversalSettings {
                descend_fast_forward: true,
                ..applied
            },
            TraversalSettings {
                global_max_terminate: false,
                ..applied
            },
            TraversalSettings {
                any_hit_shadow: true,
                ..applied
            },
            TraversalSettings {
                brick_bit_grid: true,
                ..applied
            },
            TraversalSettings {
                distance_skip: false,
                ..applied
            },
            TraversalSettings {
                directional_skip: true,
                ..applied
            },
        ] {
            assert!(
                changed.requires_pipeline_rebuild(&applied),
                "{changed:?} must force a pipeline rebuild"
            );
        }
        assert!(!applied.requires_pipeline_rebuild(&applied));
    }
}
