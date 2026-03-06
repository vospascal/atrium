//! ISO 9613-1 air absorption — frequency-dependent atmospheric absorption.
//!
//! Shared inner filter used by both SourceStage (per-source, listener-relative)
//! and PathStage (per source × speaker, world-locked).

use crate::audio::atmosphere::{iso9613_cutoff, AtmosphericParams};
use crate::pipeline::path_stage::{PathContext, PathStage};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// 2nd-order IIR biquad lowpass (Direct Form I).
struct Biquad {
    b0: f32,
    b1: f32,
    b2: f32,
    a1: f32,
    a2: f32,
    x1: f32,
    x2: f32,
    y1: f32,
    y2: f32,
}

impl Biquad {
    fn lowpass(cutoff_hz: f32, sample_rate: f32) -> Self {
        let omega = 2.0 * std::f32::consts::PI * cutoff_hz / sample_rate;
        let cos_w = omega.cos();
        let sin_w = omega.sin();
        let alpha = sin_w / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let b0 = (1.0 - cos_w) / 2.0;
        let b1 = 1.0 - cos_w;
        let b2 = b0;
        let a0 = 1.0 + alpha;
        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: -2.0 * cos_w / a0,
            a2: (1.0 - alpha) / a0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// Shared filter core: tracks cutoff with hysteresis to avoid unnecessary
/// coefficient recomputation.
struct AirAbsorptionFilter {
    filter: Biquad,
    current_cutoff: f32,
    sample_rate: f32,
}

impl AirAbsorptionFilter {
    fn new(sample_rate: f32) -> Self {
        Self {
            filter: Biquad::lowpass(20000.0, sample_rate),
            current_cutoff: 20000.0,
            sample_rate,
        }
    }

    fn update(&mut self, distance: f32, atmosphere: &AtmosphericParams) {
        let target = iso9613_cutoff(distance, atmosphere);
        if (target - self.current_cutoff).abs() / self.current_cutoff > 0.05 {
            self.filter = Biquad::lowpass(target, self.sample_rate);
            self.current_cutoff = target;
        }
    }

    #[inline]
    fn process(&mut self, sample: f32) -> f32 {
        self.filter.process(sample)
    }

    fn reset(&mut self) {
        self.filter.reset();
        self.current_cutoff = 20000.0;
        self.filter = Biquad::lowpass(20000.0, self.sample_rate);
    }
}

// ─────────────────────────────────────────────────────────────────────────────
// SourceStage: per-source, listener-relative (VBAP / HRTF)
// ─────────────────────────────────────────────────────────────────────────────

/// Per-source air absorption stage. Distance = source → listener.
pub struct AirAbsorptionStage {
    inner: AirAbsorptionFilter,
}

impl AirAbsorptionStage {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            inner: AirAbsorptionFilter::new(sample_rate),
        }
    }
}

impl SourceStage for AirAbsorptionStage {
    fn process(&mut self, ctx: &SourceContext, _output: &mut SourceOutput) {
        self.inner.update(ctx.dist_to_listener, ctx.atmosphere);
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

// ─────────────────────────────────────────────────────────────────────────────
// PathStage: per source × speaker, world-locked
// ─────────────────────────────────────────────────────────────────────────────

/// Per-path air absorption. Distance = source → speaker (target).
pub struct AirAbsorptionPath {
    inner: AirAbsorptionFilter,
}

impl AirAbsorptionPath {
    pub fn new(sample_rate: f32) -> Self {
        Self {
            inner: AirAbsorptionFilter::new(sample_rate),
        }
    }
}

impl PathStage for AirAbsorptionPath {
    fn update(&mut self, ctx: &PathContext) {
        let dist = ctx.source_pos.distance_to(ctx.target_pos);
        self.inner.update(dist, ctx.atmosphere);
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        self.inner.process(sample)
    }

    fn name(&self) -> &str {
        "air_absorption_path"
    }

    fn reset(&mut self) {
        self.inner.reset();
    }
}
