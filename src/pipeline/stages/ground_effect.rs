//! ISO 9613-2 ground effect — broadband gain modifier.
//!
//! SourceStage version uses source→listener distance and heights.
//! PathStage version uses source→speaker (target) distance and heights.

use crate::audio::propagation::ground_effect_gain;
use crate::pipeline::path_stage::{PathContext, PathStage};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

// ─────────────────────────────────────────────────────────────────────────────
// SourceStage: per-source, listener-relative
// ─────────────────────────────────────────────────────────────────────────────

/// Per-source ground effect. Modifies `output.gain_modifier`.
pub struct GroundEffectStage;

impl SourceStage for GroundEffectStage {
    fn process(&mut self, ctx: &SourceContext, output: &mut SourceOutput) {
        let dx = ctx.source_pos.x - ctx.listener.position.x;
        let dy = ctx.source_pos.y - ctx.listener.position.y;
        let horizontal_dist = (dx * dx + dy * dy).sqrt();
        let gain = ground_effect_gain(
            horizontal_dist,
            ctx.source_pos.z.max(0.0),
            ctx.listener.position.z.max(0.0),
            ctx.ground,
        );
        output.gain_modifier *= gain;
    }

    fn name(&self) -> &str {
        "ground_effect"
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// PathStage: per source × speaker, world-locked
// ─────────────────────────────────────────────────────────────────────────────

/// Per-path ground effect. Returns broadband gain via `gain_modifier()`.
pub struct GroundEffectPath {
    cached_gain: f32,
}

impl Default for GroundEffectPath {
    fn default() -> Self {
        Self { cached_gain: 1.0 }
    }
}

impl GroundEffectPath {
    pub fn new() -> Self {
        Self { cached_gain: 1.0 }
    }
}

impl PathStage for GroundEffectPath {
    fn update(&mut self, ctx: &PathContext) {
        let dx = ctx.source_pos.x - ctx.target_pos.x;
        let dy = ctx.source_pos.y - ctx.target_pos.y;
        let horizontal_dist = (dx * dx + dy * dy).sqrt();
        self.cached_gain = ground_effect_gain(
            horizontal_dist,
            ctx.source_pos.z.max(0.0),
            ctx.target_pos.z.max(0.0),
            ctx.ground,
        );
    }

    fn gain_modifier(&self) -> f32 {
        self.cached_gain
    }

    fn name(&self) -> &str {
        "ground_effect_path"
    }

    fn reset(&mut self) {
        self.cached_gain = 1.0;
    }
}
