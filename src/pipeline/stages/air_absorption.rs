//! ISO 9613-1 air absorption — frequency-dependent atmospheric absorption.
//!
//! Inner filter (`AirAbsorptionFilter`) is shared by the SourceStage
//! (listener-relative modes) and WorldLockedRenderer (per-speaker).

use crate::audio::atmosphere::{air_absorption_lp_cutoff, AtmosphericParams};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// 2nd-order IIR biquad lowpass (Direct Form I).
pub(crate) struct Biquad {
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
    pub(crate) fn lowpass(cutoff_hz: f32, sample_rate: f32) -> Self {
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
    pub(crate) fn process(&mut self, x: f32) -> f32 {
        let y = self.b0 * x + self.b1 * self.x1 + self.b2 * self.x2
            - self.a1 * self.y1
            - self.a2 * self.y2;
        self.x2 = self.x1;
        self.x1 = x;
        self.y2 = self.y1;
        self.y1 = y;
        y
    }

    /// Update coefficients only, preserving filter state.
    pub(crate) fn set_lowpass(&mut self, cutoff_hz: f32, sample_rate: f32) {
        let omega = 2.0 * std::f32::consts::PI * cutoff_hz / sample_rate;
        let cos_w = omega.cos();
        let sin_w = omega.sin();
        let alpha = sin_w / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let b0 = (1.0 - cos_w) / 2.0;
        let b1 = 1.0 - cos_w;
        let b2 = b0;
        let a0 = 1.0 + alpha;
        self.b0 = b0 / a0;
        self.b1 = b1 / a0;
        self.b2 = b2 / a0;
        self.a1 = -2.0 * cos_w / a0;
        self.a2 = (1.0 - alpha) / a0;
    }

    pub(crate) fn reset(&mut self) {
        self.x1 = 0.0;
        self.x2 = 0.0;
        self.y1 = 0.0;
        self.y2 = 0.0;
    }
}

/// Shared filter core: tracks cutoff with hysteresis to avoid unnecessary
/// coefficient recomputation.
pub(crate) struct AirAbsorptionFilter {
    filter: Biquad,
    current_cutoff: f32,
    sample_rate: f32,
}

impl AirAbsorptionFilter {
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self {
            filter: Biquad::lowpass(20000.0, sample_rate),
            current_cutoff: 20000.0,
            sample_rate,
        }
    }

    pub(crate) fn update(&mut self, distance: f32, atmosphere: &AtmosphericParams) {
        let target = air_absorption_lp_cutoff(distance, atmosphere);
        if (target - self.current_cutoff).abs() / self.current_cutoff > 0.05 {
            self.filter.set_lowpass(target, self.sample_rate);
            self.current_cutoff = target;
        }
    }

    #[inline]
    pub(crate) fn process(&mut self, sample: f32) -> f32 {
        self.filter.process(sample)
    }

    pub(crate) fn reset(&mut self) {
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
