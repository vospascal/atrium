//! Air absorption along a path (ISO 9613-1).

use crate::audio::filters::AirAbsorptionFilter;
use crate::pipeline::path::{PathEffect, PathEffectContext};

/// Per-path air absorption using ISO 9613-1 lowpass filter.
///
/// Reuses `AirAbsorptionFilter` — the same biquad + hysteresis logic used by
/// the WorldLockedRenderer.
pub struct AirAbsorptionEffect {
    inner: AirAbsorptionFilter,
}

impl AirAbsorptionEffect {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            inner: AirAbsorptionFilter::new(sample_rate),
        }
    }
}

impl PathEffect for AirAbsorptionEffect {
    fn update(&mut self, ctx: &PathEffectContext) {
        self.inner
            .update(ctx.path.distance, &ctx.environment.atmosphere);
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        self.inner.process(sample)
    }

    fn name(&self) -> &str {
        "air_absorption"
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}
