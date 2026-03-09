//! ISO 9613-1 air absorption — frequency-dependent atmospheric absorption.
//!
//! Models how air absorbs high frequencies more than lows using two shelving
//! filters derived from ISO 9613-1 absorption coefficients:
//!
//! - **Low shelf at 500 Hz**: O₂ relaxation absorption — the baseline loss
//!   affecting all frequencies.
//! - **High shelf at 4 kHz**: additional HF rolloff from N₂ relaxation +
//!   classical absorption.
//!
//! This dual-shelf approach captures the frequency-dependent shape of atmospheric
//! absorption far more accurately than a single lowpass cutoff, which over-attenuates
//! mid frequencies and under-attenuates low frequencies.
//!
//! Inner filter (`AirAbsorptionFilter`) is shared by the SourceStage
//! (listener-relative modes), PathEffect (per-path), and WorldLockedRenderer
//! (per-speaker).

use crate::audio::atmosphere::{air_absorption_shelf_gains, AtmosphericParams};
use crate::pipeline::source_stage::{SourceContext, SourceOutput, SourceStage};

/// Shelving filter center frequencies (Hz).
const LOW_SHELF_FREQ: f32 = 500.0;
const HIGH_SHELF_FREQ: f32 = 4000.0;

/// 2nd-order IIR biquad filter (Direct Form I).
///
/// Supports lowpass, low-shelf, and high-shelf configurations.
/// All coefficient formulas from RBJ Audio EQ Cookbook.
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
    /// Initialize as unity passthrough.
    fn unity() -> Self {
        Self {
            b0: 1.0,
            b1: 0.0,
            b2: 0.0,
            a1: 0.0,
            a2: 0.0,
            x1: 0.0,
            x2: 0.0,
            y1: 0.0,
            y2: 0.0,
        }
    }

    /// Set low-shelf coefficients (RBJ Audio EQ Cookbook).
    /// Preserves filter state to avoid clicks on parameter changes.
    pub(crate) fn set_low_shelf(&mut self, freq_hz: f32, gain_db: f32, sample_rate: f32) {
        if gain_db.abs() < 0.01 {
            self.set_unity();
            return;
        }
        let a = 10.0_f32.powf(gain_db / 40.0); // sqrt of linear gain
        let omega = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
        let cos_w = omega.cos();
        let sin_w = omega.sin();
        let alpha = sin_w / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let a0 = (a + 1.0) + (a - 1.0) * cos_w + two_sqrt_a_alpha;
        let a0_inv = 1.0 / a0;

        self.b0 = (a * ((a + 1.0) - (a - 1.0) * cos_w + two_sqrt_a_alpha)) * a0_inv;
        self.b1 = (2.0 * a * ((a - 1.0) - (a + 1.0) * cos_w)) * a0_inv;
        self.b2 = (a * ((a + 1.0) - (a - 1.0) * cos_w - two_sqrt_a_alpha)) * a0_inv;
        self.a1 = (-2.0 * ((a - 1.0) + (a + 1.0) * cos_w)) * a0_inv;
        self.a2 = ((a + 1.0) + (a - 1.0) * cos_w - two_sqrt_a_alpha) * a0_inv;
    }

    /// Set high-shelf coefficients (RBJ Audio EQ Cookbook).
    pub(crate) fn set_high_shelf(&mut self, freq_hz: f32, gain_db: f32, sample_rate: f32) {
        if gain_db.abs() < 0.01 {
            self.set_unity();
            return;
        }
        let a = 10.0_f32.powf(gain_db / 40.0);
        let omega = 2.0 * std::f32::consts::PI * freq_hz / sample_rate;
        let cos_w = omega.cos();
        let sin_w = omega.sin();
        let alpha = sin_w / (2.0 * std::f32::consts::FRAC_1_SQRT_2);
        let two_sqrt_a_alpha = 2.0 * a.sqrt() * alpha;

        let a0 = (a + 1.0) - (a - 1.0) * cos_w + two_sqrt_a_alpha;
        let a0_inv = 1.0 / a0;

        self.b0 = (a * ((a + 1.0) + (a - 1.0) * cos_w + two_sqrt_a_alpha)) * a0_inv;
        self.b1 = (-2.0 * a * ((a - 1.0) + (a + 1.0) * cos_w)) * a0_inv;
        self.b2 = (a * ((a + 1.0) + (a - 1.0) * cos_w - two_sqrt_a_alpha)) * a0_inv;
        self.a1 = (2.0 * ((a - 1.0) - (a + 1.0) * cos_w)) * a0_inv;
        self.a2 = ((a + 1.0) - (a - 1.0) * cos_w - two_sqrt_a_alpha) * a0_inv;
    }

    fn set_unity(&mut self) {
        self.b0 = 1.0;
        self.b1 = 0.0;
        self.b2 = 0.0;
        self.a1 = 0.0;
        self.a2 = 0.0;
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
}

/// Shared 2-band air absorption filter.
///
/// Uses two shelving biquads to model the frequency-dependent shape of
/// ISO 9613-1 atmospheric absorption:
/// - Low shelf at 500 Hz (O₂ relaxation region)
/// - High shelf at 4 kHz (steep HF rolloff)
///
/// Gains are derived from `air_absorption_shelf_gains()` and updated with
/// 5% hysteresis to avoid unnecessary coefficient recomputation.
pub(crate) struct AirAbsorptionFilter {
    low_shelf: Biquad,
    high_shelf: Biquad,
    current_low_db: f32,
    current_high_db: f32,
    sample_rate: f32,
}

impl AirAbsorptionFilter {
    pub(crate) fn new(sample_rate: f32) -> Self {
        Self {
            low_shelf: Biquad::unity(),
            high_shelf: Biquad::unity(),
            current_low_db: 0.0,
            current_high_db: 0.0,
            sample_rate,
        }
    }

    pub(crate) fn update(&mut self, distance: f32, atmosphere: &AtmosphericParams) {
        let (target_low, target_high) = air_absorption_shelf_gains(distance, atmosphere);

        // Hysteresis: only recompute if gain changed by > 0.5 dB.
        if (target_low - self.current_low_db).abs() > 0.5 {
            self.low_shelf
                .set_low_shelf(LOW_SHELF_FREQ, target_low, self.sample_rate);
            self.current_low_db = target_low;
        }
        if (target_high - self.current_high_db).abs() > 0.5 {
            self.high_shelf
                .set_high_shelf(HIGH_SHELF_FREQ, target_high, self.sample_rate);
            self.current_high_db = target_high;
        }
    }

    #[inline]
    pub(crate) fn process(&mut self, sample: f32) -> f32 {
        let s = self.low_shelf.process(sample);
        self.high_shelf.process(s)
    }

    pub(crate) fn reset(&mut self) {
        self.low_shelf = Biquad::unity();
        self.high_shelf = Biquad::unity();
        self.current_low_db = 0.0;
        self.current_high_db = 0.0;
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
