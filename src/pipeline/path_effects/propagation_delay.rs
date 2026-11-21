//! Propagation delay of a reflection path.

use crate::pipeline::path::{PathEffect, PathEffectContext};

/// Fractional-delay line for propagation delay (reflections).
///
/// Uses a circular buffer with linear interpolation between adjacent samples,
/// following the pattern from `delay_comp.rs:118-126`. The delay is derived
/// from `path.delay_seconds * sample_rate` each buffer and approached per
/// sample, never jumped to: a tap that jumps skips or repeats audio, which is
/// a click on every buffer boundary for any moving source. Gliding the tap
/// is also what a moving reflection physically does (a small Doppler shift).
///
/// Buffer capacity is dynamically sized from the maximum first-order image-source
/// distance (via `room_acoustics::delay_capacity_for_seconds`), with a minimum of 8192
/// samples (~170ms at 48kHz). Power-of-2 sizing enables fast wrapping via bitmask.
pub struct PropagationDelayEffect {
    buffer: Box<[f32]>,
    capacity: usize,
    mask: usize,
    write_pos: usize,
    /// Delay the tap is at right now, in samples.
    delay_samples: f32,
    /// Delay the current path asks for, in samples.
    target_delay_samples: f32,
    /// Fraction of the remaining distance to the target covered per sample.
    glide_per_sample: f32,
    /// False until the first update: the first delay is taken as is, since
    /// there is no earlier tap position to glide from.
    primed: bool,
    sample_rate: f32,
}

impl PropagationDelayEffect {
    /// Time constant of the glide toward a new delay.
    const GLIDE_SECONDS: f32 = 0.010;
    /// Fastest the tap may move, in samples of delay per sample of audio.
    /// Half a sample per sample keeps the output's slope within 1.5× the
    /// input's, so even a source that teleports glides instead of clicking.
    const MAX_GLIDE_RATE: f32 = 0.5;

    /// Minimum capacity: 8192 samples (~170ms at 48kHz).
    const MIN_CAPACITY: usize = 8192;

    pub fn new(sample_rate: f32, capacity: usize) -> Self {
        let capacity = capacity.max(Self::MIN_CAPACITY).next_power_of_two();
        Self {
            buffer: vec![0.0; capacity].into_boxed_slice(),
            capacity,
            mask: capacity - 1,
            write_pos: 0,
            delay_samples: 0.0,
            target_delay_samples: 0.0,
            glide_per_sample: 1.0,
            primed: false,
            sample_rate,
        }
    }
}

impl PathEffect for PropagationDelayEffect {
    fn update(&mut self, ctx: &PathEffectContext) {
        self.sample_rate = ctx.sample_rate;
        self.target_delay_samples = ctx.path.delay_seconds * ctx.sample_rate;
        self.glide_per_sample = 1.0 - (-1.0 / (Self::GLIDE_SECONDS * ctx.sample_rate)).exp();
        if !self.primed {
            self.delay_samples = self.target_delay_samples;
            self.primed = true;
        }
    }

    #[inline]
    fn process_sample(&mut self, sample: f32) -> f32 {
        let wp = self.write_pos;
        self.buffer[wp] = sample;
        self.write_pos = (wp + 1) & self.mask;

        let glide = (self.target_delay_samples - self.delay_samples) * self.glide_per_sample;
        self.delay_samples += glide.clamp(-Self::MAX_GLIDE_RATE, Self::MAX_GLIDE_RATE);

        if self.delay_samples < 0.5 {
            return sample;
        }

        let delay_clamped = self.delay_samples.min((self.capacity - 2) as f32);
        let delay_int = delay_clamped as usize;
        let frac = delay_clamped - delay_int as f32;

        let idx0 = (wp + self.capacity - delay_int) & self.mask;
        let idx1 = (wp + self.capacity - delay_int - 1) & self.mask;

        let s0 = self.buffer[idx0];
        let s1 = self.buffer[idx1];
        s0 + (s1 - s0) * frac
    }

    fn name(&self) -> &str {
        "propagation_delay"
    }

    fn reset(&mut self) {
        self.buffer.fill(0.0);
        self.write_pos = 0;
        self.delay_samples = 0.0;
        self.target_delay_samples = 0.0;
        self.primed = false;
    }
}
