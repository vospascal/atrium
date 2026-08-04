//! CPU span timing: named sub-frame work, measured on the thread that does it.
//!
//! Two halves, deliberately split by thread role:
//!
//! - [`SpanRecorder`] is the **write** side. Lock-free, allocation-free, `Sync`,
//!   and safe to call from the audio callback thread — one `Instant::now` pair
//!   and one relaxed atomic add per span. Share it with `Arc` and let any number
//!   of threads record into it.
//! - [`SpanHistory`] is the **read** side. Drains the recorder once per frame
//!   into a [`RollingWindow`] per span, and owns all the statistics. Lives on
//!   whichever thread draws the readout.
//!
//! Spans are identified by `usize` index into a `&'static [&'static str]` label
//! list that the *consumer* supplies — this crate never learns the names of the
//! things it measures. Register the list as a const in the calling crate and use
//! named index constants alongside it.
//!
//! ```
//! # use atrium_profile::cpu::{SpanRecorder, SpanHistory};
//! const SPANS: &[&str] = &["character step", "world upload"];
//! const SPAN_CHARACTER: usize = 0;
//!
//! let recorder = SpanRecorder::new(SPANS);
//! let mut history = SpanHistory::new(SPANS, 240);
//!
//! {
//!     let _measured = recorder.scope(SPAN_CHARACTER);
//!     // ... the work being measured; the guard records on drop.
//! }
//! history.sample(&recorder);
//! assert!(history.median_milliseconds(SPAN_CHARACTER).is_some());
//! ```
//!
//! **An instrumentation crate must never be the reason an app dies.** An
//! out-of-range span index is a `debug_assert` in development and a silent no-op
//! in release, rather than a panic in the middle of someone's frame.

use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::time::{Duration, Instant};

use crate::report::TableRow;
use crate::stats::{PeriodEstimate, RollingWindow};

/// One span's accumulator. Split into time and count so a span entered several
/// times per frame reports both its total cost and how often it ran — a span
/// whose cost doubled because it ran twice is a different bug from one whose
/// cost doubled per call.
#[derive(Default)]
struct Slot {
    nanoseconds: AtomicU64,
    hits: AtomicU32,
}

/// The write side: accumulates per-span elapsed time until a reader drains it.
///
/// Cheap enough for the audio thread — no allocation, no locking, no syscall
/// beyond the monotonic clock read that any timing must do.
pub struct SpanRecorder {
    labels: &'static [&'static str],
    slots: Box<[Slot]>,
}

impl SpanRecorder {
    pub fn new(labels: &'static [&'static str]) -> Self {
        Self {
            labels,
            slots: (0..labels.len()).map(|_| Slot::default()).collect(),
        }
    }

    pub fn labels(&self) -> &'static [&'static str] {
        self.labels
    }

    pub fn span_count(&self) -> usize {
        self.slots.len()
    }

    /// Measure a scope: the returned guard records elapsed time into `span` when
    /// it drops. Hold it in a binding — `let _measured = ...`, never `let _ =`,
    /// which drops it immediately and measures nothing.
    #[must_use = "the span is measured when the guard drops; `let _ = ` measures nothing"]
    pub fn scope(&self, span: usize) -> SpanGuard<'_> {
        debug_assert!(
            span < self.slots.len(),
            "span index {span} out of range for {} spans",
            self.slots.len()
        );
        SpanGuard {
            slot: self.slots.get(span),
            started: Instant::now(),
        }
    }

    /// Record an already-measured duration. For work whose timing does not fit a
    /// lexical scope — a span that starts in one callback and ends in another.
    pub fn record(&self, span: usize, elapsed: Duration) {
        debug_assert!(
            span < self.slots.len(),
            "span index {span} out of range for {} spans",
            self.slots.len()
        );
        if let Some(slot) = self.slots.get(span) {
            slot.add(elapsed);
        }
    }

    /// Read and zero one span's accumulator. `Relaxed` ordering throughout: the
    /// only consumer is a statistics readout, and a value landing one frame later
    /// than it might have is invisible in a rolling window.
    fn take(&self, span: usize) -> SpanTotal {
        match self.slots.get(span) {
            Some(slot) => SpanTotal {
                nanoseconds: slot.nanoseconds.swap(0, Ordering::Relaxed),
                hits: slot.hits.swap(0, Ordering::Relaxed),
            },
            None => SpanTotal::default(),
        }
    }
}

impl Slot {
    fn add(&self, elapsed: Duration) {
        // Saturating: a span left open across a debugger pause could otherwise
        // wrap the accumulator and report a nonsense negative-looking delta.
        self.nanoseconds.fetch_add(
            u64::try_from(elapsed.as_nanos()).unwrap_or(u64::MAX),
            Ordering::Relaxed,
        );
        self.hits.fetch_add(1, Ordering::Relaxed);
    }
}

/// What one span accumulated between two drains.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SpanTotal {
    pub nanoseconds: u64,
    /// How many times the span was entered.
    pub hits: u32,
}

impl SpanTotal {
    pub fn milliseconds(&self) -> f32 {
        self.nanoseconds as f32 / 1_000_000.0
    }
}

/// RAII guard from [`SpanRecorder::scope`]. Records on drop.
pub struct SpanGuard<'recorder> {
    /// `None` when the span index was out of range — the guard then measures
    /// nothing instead of panicking.
    slot: Option<&'recorder Slot>,
    started: Instant,
}

impl Drop for SpanGuard<'_> {
    fn drop(&mut self) {
        if let Some(slot) = self.slot {
            slot.add(self.started.elapsed());
        }
    }
}

/// The read side: per-span rolling history and the statistics over it.
pub struct SpanHistory {
    labels: &'static [&'static str],
    windows: Vec<RollingWindow>,
    /// Hit count from the most recent [`Self::sample`], per span.
    latest_hits: Vec<u32>,
}

impl SpanHistory {
    /// `capacity` is history depth in samples — i.e. in frames, when sampled
    /// once per frame. 240 is four seconds at 60 Hz.
    pub fn new(labels: &'static [&'static str], capacity: usize) -> Self {
        Self {
            labels,
            windows: (0..labels.len())
                .map(|_| RollingWindow::new(capacity))
                .collect(),
            latest_hits: vec![0; labels.len()],
        }
    }

    /// Drain every span from `recorder` and push one sample each. Call exactly
    /// once per frame: this is what defines the sample rate that
    /// [`Self::dominant_period`] reports periods against.
    ///
    /// Spans that did not run this frame push 0.0 rather than nothing, so the
    /// series stays evenly spaced in time — a gap would shift every later sample
    /// and corrupt the period estimate.
    pub fn sample(&mut self, recorder: &SpanRecorder) {
        for span in 0..self.windows.len() {
            let total = recorder.take(span);
            self.windows[span].push(total.milliseconds());
            self.latest_hits[span] = total.hits;
        }
    }

    /// Push an externally measured value for one span, instead of draining a
    /// recorder. This is how GPU timestamp results ([`crate::gpu`]) reach the
    /// same statistics, percentiles and wave detection as CPU spans — the
    /// alternative was a second, parallel copy of all of it.
    ///
    /// Call at most once per span per frame, or the series stops being evenly
    /// spaced and the period estimate goes with it.
    pub fn push_milliseconds(&mut self, span: usize, milliseconds: f32) {
        if let Some(window) = self.windows.get_mut(span) {
            window.push(milliseconds);
            self.latest_hits[span] = 1;
        }
    }

    pub fn labels(&self) -> &'static [&'static str] {
        self.labels
    }

    pub fn span_count(&self) -> usize {
        self.windows.len()
    }

    pub fn label(&self, span: usize) -> &'static str {
        self.labels.get(span).copied().unwrap_or("<unknown span>")
    }

    /// Full history for one span, oldest-first — the series to plot.
    pub fn series(&self, span: usize) -> impl Iterator<Item = f32> + '_ {
        self.windows
            .get(span)
            .into_iter()
            .flat_map(|window| window.chronological())
    }

    pub fn latest_milliseconds(&self, span: usize) -> Option<f32> {
        self.windows.get(span)?.latest()
    }

    pub fn latest_hits(&self, span: usize) -> u32 {
        self.latest_hits.get(span).copied().unwrap_or(0)
    }

    pub fn median_milliseconds(&mut self, span: usize) -> Option<f32> {
        self.windows.get_mut(span)?.median()
    }

    pub fn p95_milliseconds(&mut self, span: usize) -> Option<f32> {
        self.windows.get_mut(span)?.p95()
    }

    pub fn max_milliseconds(&self, span: usize) -> Option<f32> {
        self.windows.get(span)?.max()
    }

    /// Is this span's cost oscillating, and how fast? See
    /// [`RollingWindow::dominant_period`].
    pub fn dominant_period(
        &mut self,
        span: usize,
        minimum_strength: f32,
    ) -> Option<PeriodEstimate> {
        self.windows
            .get_mut(span)?
            .dominant_period(minimum_strength)
    }

    /// Sum of every span's median. Useful as a sanity check against a measured
    /// frame time: a large unexplained remainder means real work is unmeasured.
    pub fn total_median_milliseconds(&mut self) -> f32 {
        (0..self.span_count())
            .filter_map(|span| self.median_milliseconds(span))
            .sum()
    }

    /// One [`TableRow`] per span, for [`crate::report::format_table`] — the
    /// path the headless benchmark prints through.
    pub fn table_rows(&mut self) -> Vec<TableRow> {
        (0..self.span_count())
            .map(|span| TableRow {
                label: self.label(span),
                median_milliseconds: self.median_milliseconds(span),
                p95_milliseconds: self.p95_milliseconds(span),
                max_milliseconds: self.max_milliseconds(span),
                hits: self.latest_hits(span),
                period: self.dominant_period(span, crate::report::PERIOD_REPORT_STRENGTH),
            })
            .collect()
    }

    pub fn clear(&mut self) {
        for window in &mut self.windows {
            window.clear();
        }
        self.latest_hits.fill(0);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SPANS: &[&str] = &["first", "second"];
    const FIRST: usize = 0;
    const SECOND: usize = 1;

    #[test]
    fn a_recorded_duration_survives_the_drain_and_zeroes_the_accumulator() {
        let recorder = SpanRecorder::new(SPANS);
        recorder.record(FIRST, Duration::from_micros(1_500));

        let drained = recorder.take(FIRST);
        assert_eq!(drained.hits, 1);
        assert!((drained.milliseconds() - 1.5).abs() < 1e-3);

        // Draining zeroes: the next frame starts from nothing, so a span that
        // stops running reads 0.0 instead of holding its last value forever.
        assert_eq!(recorder.take(FIRST), SpanTotal::default());
    }

    #[test]
    fn several_entries_in_one_frame_sum_their_time_and_count_their_hits() {
        let recorder = SpanRecorder::new(SPANS);
        for _ in 0..3 {
            recorder.record(SECOND, Duration::from_millis(2));
        }
        let mut history = SpanHistory::new(SPANS, 8);
        history.sample(&recorder);

        assert_eq!(history.latest_hits(SECOND), 3);
        let total = history.latest_milliseconds(SECOND).expect("sampled");
        assert!((total - 6.0).abs() < 0.1, "expected ~6 ms, got {total}");
    }

    #[test]
    fn the_scope_guard_measures_the_scope_it_is_held_in() {
        let recorder = SpanRecorder::new(SPANS);
        {
            let _measured = recorder.scope(FIRST);
            std::thread::sleep(Duration::from_millis(3));
        }
        let mut history = SpanHistory::new(SPANS, 8);
        history.sample(&recorder);
        let measured = history.latest_milliseconds(FIRST).expect("sampled");
        assert!(measured >= 2.5, "guard must record on drop, got {measured}");
    }

    /// A span that stopped running must read 0.0, not stay stuck at its last
    /// value — and the series must stay evenly spaced for period detection.
    #[test]
    fn an_idle_span_samples_zero_and_keeps_the_series_evenly_spaced() {
        let recorder = SpanRecorder::new(SPANS);
        let mut history = SpanHistory::new(SPANS, 8);

        recorder.record(FIRST, Duration::from_millis(4));
        history.sample(&recorder);
        history.sample(&recorder);
        history.sample(&recorder);

        assert_eq!(history.latest_milliseconds(FIRST), Some(0.0));
        assert_eq!(history.latest_hits(FIRST), 0);
        assert_eq!(history.series(FIRST).count(), 3);
    }

    /// The audio-thread requirement: `SpanRecorder` must be shareable across
    /// threads without a lock.
    #[test]
    fn the_recorder_is_shared_across_threads_without_locking() {
        use std::sync::Arc;

        let recorder = Arc::new(SpanRecorder::new(SPANS));
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let recorder = Arc::clone(&recorder);
                std::thread::spawn(move || {
                    for _ in 0..10 {
                        recorder.record(FIRST, Duration::from_micros(100));
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().expect("worker panicked");
        }

        let drained = recorder.take(FIRST);
        assert_eq!(drained.hits, 40, "every record must land");
        assert!((drained.milliseconds() - 4.0).abs() < 0.5);
    }

    /// Instrumentation must not kill the app it measures. In release an
    /// out-of-range index is a no-op; `debug_assert` catches it in development,
    /// so this only runs without debug assertions.
    #[test]
    #[cfg(not(debug_assertions))]
    fn an_out_of_range_span_is_a_no_op_rather_than_a_panic() {
        let recorder = SpanRecorder::new(SPANS);
        recorder.record(99, Duration::from_millis(1));
        drop(recorder.scope(99));

        let mut history = SpanHistory::new(SPANS, 4);
        history.sample(&recorder);
        assert_eq!(history.latest_milliseconds(99), None);
        assert_eq!(history.label(99), "<unknown span>");
    }

    #[test]
    fn a_periodic_span_reports_its_cadence_through_the_history() {
        let recorder = SpanRecorder::new(SPANS);
        let mut history = SpanHistory::new(SPANS, 240);
        for frame in 0..240 {
            // Expensive every 8th frame — a cascade-style cadence.
            let cost = if frame % 8 == 0 { 5_000 } else { 500 };
            recorder.record(FIRST, Duration::from_micros(cost));
            history.sample(&recorder);
        }
        let estimate = history
            .dominant_period(FIRST, 0.25)
            .expect("an every-8th-frame cost is periodic");
        assert_eq!(estimate.period_samples, 8);
    }
}
