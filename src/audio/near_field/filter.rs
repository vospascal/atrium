//! The filter that gives one ear its correction.
//!
//! It splits the sound with a one-pole low-pass at the corner and weighs the
//! two parts: `high · x + (low − high) · lowpass(x)`, which is the response
//! [`EarCorrection`] describes. The low-pass is the topology-preserving
//! (trapezoidal) one-pole, the bilinear transform with its corner prewarped,
//! which stays well-behaved while its coefficient moves.
//!
//! Its targets change once a buffer and every parameter glides there sample
//! by sample, so a source flying past never clicks. Asked for no correction
//! it glides home to unity, and once there it is out of the signal: samples
//! pass bit for bit, as if it did not exist.

use super::shelf::EarCorrection;

/// The corner is kept below this share of the sample rate, short of the
/// Nyquist frequency where the prewarping tangent grows without bound. A
/// corner there would shape nothing audible anyway.
pub(super) const HIGHEST_CORNER_SHARE: f32 = 0.45;

/// One ear's near-field filter.
pub(crate) struct EarFilter {
    low: f32,
    high: f32,
    /// The trapezoidal one-pole's g / (1 + g), g = tan(π corner / fs).
    coefficient: f32,
    low_step: f32,
    high_step: f32,
    coefficient_step: f32,
    /// Where the glide in progress ends, which the next one starts from
    /// exactly, so rounding in the steps never accumulates.
    end: (f32, f32, f32),
    /// Samples left of the glide in progress.
    remaining: usize,
    /// The low-pass's integrator.
    state: f32,
    /// False while the filter is out of the signal.
    active: bool,
}

impl EarFilter {
    /// A filter out of the signal.
    pub(crate) fn new() -> Self {
        Self {
            low: 1.0,
            high: 1.0,
            coefficient: 0.0,
            low_step: 0.0,
            high_step: 0.0,
            coefficient_step: 0.0,
            end: (1.0, 1.0, 0.0),
            remaining: 0,
            state: 0.0,
            active: false,
        }
    }

    /// Glide to `target` over the next `frames` samples at `sample_rate`;
    /// `None` (or unity) takes the filter home and then out of the signal.
    pub(crate) fn glide_to(
        &mut self,
        target: Option<EarCorrection>,
        frames: usize,
        sample_rate: f32,
    ) {
        (self.low, self.high, self.coefficient) = self.end;
        let target = target.filter(|correction| !correction.is_unity());
        let end = match target {
            None if !self.active => return,
            None if self.low == 1.0 && self.high == 1.0 => {
                // Home: from here on the sound passes untouched.
                self.active = false;
                self.state = 0.0;
                self.remaining = 0;
                return;
            }
            None => (1.0, 1.0, self.coefficient),
            Some(correction) => {
                let corner = correction.corner_hz.min(HIGHEST_CORNER_SHARE * sample_rate);
                let warped = (std::f32::consts::PI * corner / sample_rate).tan();
                let coefficient = warped / (1.0 + warped);
                if !self.active {
                    // Start from unity with the corner already in place: the
                    // gains glide in, nothing sweeps.
                    self.active = true;
                    self.state = 0.0;
                    self.coefficient = coefficient;
                }
                (correction.low_gain, correction.high_gain, coefficient)
            }
        };
        let frames = frames.max(1);
        let per_sample = 1.0 / frames as f32;
        self.low_step = (end.0 - self.low) * per_sample;
        self.high_step = (end.1 - self.high) * per_sample;
        self.coefficient_step = (end.2 - self.coefficient) * per_sample;
        self.end = end;
        self.remaining = frames;
    }

    #[inline]
    pub(crate) fn process(&mut self, sample: f32) -> f32 {
        if !self.active {
            return sample;
        }
        if self.remaining > 0 {
            self.remaining -= 1;
            self.low += self.low_step;
            self.high += self.high_step;
            self.coefficient += self.coefficient_step;
        }
        let half_step = (sample - self.state) * self.coefficient;
        let lowpassed = half_step + self.state;
        self.state = lowpassed + half_step;
        self.high * sample + (self.low - self.high) * lowpassed
    }

    /// Out of the signal, with nothing remembered.
    pub(crate) fn reset(&mut self) {
        *self = Self::new();
    }
}
