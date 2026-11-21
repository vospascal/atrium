//! The wavering of a path through moving air.

use crate::audio::turbulence::{AirPath, AirWaver, Wavering};
use crate::pipeline::path::{PathEffect, PathEffectContext};

/// Per-path wavering of level and treble from wind and sun stirring the air
/// (`crate::audio::turbulence`). Under a roof, in still air and close by it
/// is not in the signal at all.
pub struct TurbulenceEffect {
    waver: AirWaver,
}

impl TurbulenceEffect {
    /// `seed` gives the path a wander of its own.
    pub fn new(seed: u64) -> Self {
        Self {
            waver: AirWaver::new(seed),
        }
    }
}

impl PathEffect for TurbulenceEffect {
    fn update(&mut self, ctx: &PathEffectContext) {
        let wavering = Wavering::along(&AirPath {
            air: &ctx.environment.air,
            open_air: ctx.environment.is_open_air(),
            from: ctx.source_pos,
            to: ctx.target_pos,
            ground: ctx.environment.min.z,
            distance: ctx.path.distance,
            temperature_c: ctx.environment.atmosphere.temperature_c,
            speed_of_sound: ctx.environment.atmosphere.speed_of_sound(),
        });
        self.waver.set(wavering, ctx.sample_rate);
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        self.waver.process(sample)
    }

    fn name(&self) -> &str {
        "turbulence"
    }

    fn reset(&mut self) {
        self.waver.reset();
    }
}
