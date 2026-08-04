//! Frame pacing: how far apart the frames actually landed.
//!
//! This is a different question from [`crate::cpu`] spans, which measure work
//! *inside* a frame. A clock measures the interval *between* frames, and the two
//! disagreeing is itself the finding: spans summing to 6 ms while frames land
//! 16.7 ms apart means 10 ms went somewhere unmeasured — blocked in swapchain
//! acquire, blocked in present, or waiting on vsync.
//!
//! Reusable beyond rendering: an audio callback is a frame clock too, and
//! [`FrameClock::intervals`] over callback arrivals is a dropout detector.

use std::time::Instant;

use crate::stats::{PeriodEstimate, RollingWindow};

/// Measures the interval between successive [`FrameClock::tick`] calls and keeps
/// a rolling history of them.
pub struct FrameClock {
    previous_tick: Option<Instant>,
    intervals: RollingWindow,
}

impl FrameClock {
    /// `capacity` is history depth in frames. 240 is four seconds at 60 Hz.
    pub fn new(capacity: usize) -> Self {
        Self {
            previous_tick: None,
            intervals: RollingWindow::new(capacity),
        }
    }

    /// Mark a frame boundary. Returns the interval since the previous tick in
    /// milliseconds, or `None` on the very first call (no interval exists yet —
    /// which is why this returns an `Option` rather than a misleading 0.0).
    pub fn tick(&mut self) -> Option<f32> {
        let now = Instant::now();
        let interval = self
            .previous_tick
            .map(|previous| (now - previous).as_secs_f32() * 1_000.0);
        self.previous_tick = Some(now);
        if let Some(milliseconds) = interval {
            self.intervals.push(milliseconds);
        }
        interval
    }

    /// Discard history and the previous timestamp. Call after a deliberate
    /// discontinuity — a vsync toggle, a resize, a mode switch — so the
    /// transient does not sit in the window looking like a regression.
    pub fn reset(&mut self) {
        self.previous_tick = None;
        self.intervals.clear();
    }

    pub fn intervals(&self) -> &RollingWindow {
        &self.intervals
    }

    pub fn intervals_mut(&mut self) -> &mut RollingWindow {
        &mut self.intervals
    }

    pub fn latest_milliseconds(&self) -> Option<f32> {
        self.intervals.latest()
    }

    pub fn median_milliseconds(&mut self) -> Option<f32> {
        self.intervals.median()
    }

    pub fn p95_milliseconds(&mut self) -> Option<f32> {
        self.intervals.p95()
    }

    /// Frames per second from the *median* interval, not the mean.
    ///
    /// A mean fps over a window is dominated by the fastest frames — the number
    /// that made a 2-second average read "1200 fps" while the display was
    /// visibly hitching. The median interval is the honest typical frame.
    pub fn frames_per_second(&mut self) -> Option<f32> {
        self.median_milliseconds()
            .filter(|milliseconds| *milliseconds > 0.0)
            .map(|milliseconds| 1_000.0 / milliseconds)
    }

    /// Spread between the typical and the bad frames, in milliseconds. Near zero
    /// is smooth; a large value is a hitch even when the median looks healthy.
    pub fn jitter_milliseconds(&mut self) -> Option<f32> {
        Some(self.p95_milliseconds()? - self.median_milliseconds()?)
    }

    /// Is the frame interval itself oscillating, and how fast? A peak here says
    /// the hitching is periodic — a fixed-cadence background job or a swapchain
    /// starving on a cycle — rather than random.
    pub fn dominant_period(&mut self, minimum_strength: f32) -> Option<PeriodEstimate> {
        self.intervals.dominant_period(minimum_strength)
    }

    /// Frames whose interval exceeded `budget_milliseconds`, as a fraction of the
    /// window. The "how often do we miss" number: at a 120 Hz target, pass 8.33.
    pub fn over_budget_fraction(&self, budget_milliseconds: f32) -> Option<f32> {
        if self.intervals.is_empty() {
            return None;
        }
        let total = self.intervals.len() as f32;
        let over = self
            .intervals
            .chronological()
            .filter(|milliseconds| *milliseconds > budget_milliseconds)
            .count() as f32;
        Some(over / total)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_first_tick_has_no_interval_and_the_second_one_does() {
        let mut clock = FrameClock::new(16);
        assert_eq!(
            clock.tick(),
            None,
            "no interval exists before a second tick"
        );
        std::thread::sleep(Duration::from_millis(4));
        let interval = clock.tick().expect("second tick has an interval");
        assert!(interval >= 3.0, "expected >=3 ms, got {interval}");
        assert_eq!(clock.intervals().len(), 1, "only intervals are recorded");
    }

    /// The bug this replaces: a mean-based fps read absurdly high when a few
    /// near-zero frames were mixed in with real ones. A median cannot.
    #[test]
    fn frames_per_second_uses_the_median_so_fast_outliers_cannot_inflate_it() {
        let mut clock = FrameClock::new(64);
        for index in 0..64 {
            // Mostly 16.7 ms frames with a handful of near-instant skipped ones.
            clock
                .intervals_mut()
                .push(if index % 16 == 0 { 0.05 } else { 16.7 });
        }
        let fps = clock.frames_per_second().expect("history exists");
        assert!(
            (fps - 59.88).abs() < 1.0,
            "median-based fps must stay near 60, got {fps}"
        );
        assert!(clock.intervals_mut().mean().expect("mean") < 16.7);
    }

    #[test]
    fn jitter_separates_a_smooth_stream_from_a_hitching_one() {
        let mut smooth = FrameClock::new(64);
        let mut hitching = FrameClock::new(64);
        for index in 0..64 {
            smooth.intervals_mut().push(16.7);
            hitching
                .intervals_mut()
                .push(if index % 10 == 0 { 40.0 } else { 14.0 });
        }
        assert_eq!(smooth.jitter_milliseconds(), Some(0.0));
        assert!(hitching.jitter_milliseconds().expect("history") > 20.0);
    }

    #[test]
    fn the_over_budget_fraction_counts_missed_frames() {
        let mut clock = FrameClock::new(100);
        for index in 0..100 {
            clock
                .intervals_mut()
                .push(if index < 25 { 20.0 } else { 8.0 });
        }
        let missed = clock.over_budget_fraction(8.33).expect("history");
        assert!((missed - 0.25).abs() < 1e-3, "expected 0.25, got {missed}");
        assert_eq!(FrameClock::new(8).over_budget_fraction(8.33), None);
    }

    #[test]
    fn a_reset_drops_the_transient_instead_of_averaging_it_in() {
        let mut clock = FrameClock::new(16);
        clock.intervals_mut().push(300.0);
        clock.tick();
        clock.reset();
        assert!(clock.intervals().is_empty());
        assert_eq!(clock.tick(), None, "reset also forgets the previous tick");
    }
}
