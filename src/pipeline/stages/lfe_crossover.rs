//! LFE low-pass crossover filter (~120 Hz Butterworth).
//!
//! Applied post-mix to the LFE channel only. Uses a 2nd-order IIR biquad
//! (Direct Form I) to roll off content above the crossover frequency.
//! Ensures the subwoofer channel only receives bass energy.

use crate::pipeline::mix_stage::{MixContext, MixStage};

/// LFE crossover cutoff frequency in Hz.
const LFE_CUTOFF_HZ: f32 = 120.0;

/// 2nd-order IIR biquad filter (Direct Form I).
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
        let a1 = -2.0 * cos_w;
        let a2 = 1.0 - alpha;

        Self {
            b0: b0 / a0,
            b1: b1 / a0,
            b2: b2 / a0,
            a1: a1 / a0,
            a2: a2 / a0,
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

/// Post-mix LFE crossover stage.
///
/// Only active when the layout has an LFE channel. Filters the LFE channel
/// in-place with a 120 Hz Butterworth lowpass. No-op for layouts without LFE.
pub struct LfeCrossoverStage {
    filter: Option<Biquad>,
}

impl LfeCrossoverStage {
    pub fn new() -> Self {
        Self { filter: None }
    }
}

impl MixStage for LfeCrossoverStage {
    fn init(&mut self, ctx: &MixContext) {
        if ctx.layout.lfe_channel().is_some() {
            self.filter = Some(Biquad::lowpass(LFE_CUTOFF_HZ, ctx.sample_rate));
        }
    }

    fn process(&mut self, buffer: &mut [f32], ctx: &MixContext) {
        let lfe = match ctx.layout.lfe_channel() {
            Some(ch) => ch,
            None => return,
        };
        let filter = match self.filter.as_mut() {
            Some(f) => f,
            None => return,
        };

        let channels = ctx.channels;
        let num_frames = buffer.len() / channels;
        for frame in 0..num_frames {
            let idx = frame * channels + lfe;
            buffer[idx] = filter.process(buffer[idx]);
        }
    }

    fn reset(&mut self) {
        if let Some(ref mut f) = self.filter {
            f.reset();
        }
    }

    fn name(&self) -> &str {
        "lfe_crossover"
    }
}
