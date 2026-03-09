//! ISO 9613-2 ground effect — broadband gain modifier.
//!
//! SourceStage for listener-relative modes. WorldLockedRenderer computes
//! ground effect per-speaker inline.

use crate::audio::propagation::ground_effect_gain;
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
            ctx.atmosphere.speed_of_sound(),
        );
        output.gain_modifier *= gain;
    }

    fn name(&self) -> &str {
        "ground_effect"
    }
}
