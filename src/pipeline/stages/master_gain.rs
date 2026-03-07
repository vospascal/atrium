//! Applies runtime master gain from AudioScene.
//!
//! Stateless — reads `ctx.master_gain` each buffer. AudioScene is the
//! single source of truth; this stage just multiplies and soft-clips.

use crate::pipeline::mix_stage::{MixContext, MixStage};
use crate::pipeline::stages::soft_clip;

#[derive(Default)]
pub struct MasterGainStage;

impl MixStage for MasterGainStage {
    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        let gain = ctx.master_gain;
        for sample in buffer.iter_mut() {
            *sample = soft_clip(*sample * gain);
        }
    }

    fn name(&self) -> &str {
        "master_gain"
    }
}
