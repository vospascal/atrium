//! The session's clock: output samples since the scene started.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};

/// Counted by the audio thread and read by the planner, without a lock.
///
/// Every plan is timed on this clock, not on the time of day, so stopping it
/// would pause every voice together.
#[derive(Debug, Default)]
pub struct Clock {
    samples: AtomicU64,
    /// The output rate as `f32` bits; 0 until the device is open.
    rate: AtomicU32,
}

impl Clock {
    /// Output samples since the scene started.
    pub fn samples(&self) -> u64 {
        self.samples.load(Ordering::Acquire)
    }

    /// The output rate, or `None` before the device is open.
    pub fn sample_rate(&self) -> Option<f32> {
        let rate = f32::from_bits(self.rate.load(Ordering::Acquire));
        (rate > 0.0).then_some(rate)
    }

    /// Seconds since the scene started, or 0 before the device is open.
    pub fn seconds(&self) -> f64 {
        match self.sample_rate() {
            Some(rate) => self.samples() as f64 / rate as f64,
            None => 0.0,
        }
    }

    /// The audio thread has rendered `frames` more at `sample_rate`.
    pub fn advance(&self, frames: u64, sample_rate: f32) {
        self.rate.store(sample_rate.to_bits(), Ordering::Release);
        self.samples.fetch_add(frames, Ordering::AcqRel);
    }
}
