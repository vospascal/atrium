//! Memory gauges: how many bytes each subsystem is holding, and whether that
//! number is growing.
//!
//! # Why gauges and not spans
//!
//! Time is a *duration* — it accumulates within a frame and resets. Memory is a
//! *level* — it persists across frames and the interesting questions are "how much
//! now", "how much at peak", and "is it drifting upward". So this is a separate
//! type from [`crate::cpu`], not a reuse of it.
//!
//! # Attributed bytes, not a process total
//!
//! Every gauge is a number the consumer *reports about itself*: a buffer size it
//! allocated, an array length it owns. That is deliberately more useful than an
//! operating-system resident-set figure, because it is **attributable** — "the
//! light volume is 48 MB" tells you what to change, "the process is at 900 MB"
//! does not. It is also portable and dependency-free, which an RSS query is not.
//!
//! The cost is that this measures what you remember to report, and nothing else.
//! Totals here are therefore a lower bound on real usage, never a claim about it,
//! and [`MemoryLedger::report`] labels them as such.
//!
//! # Peak and drift
//!
//! Each gauge keeps its own peak and its value from the previous
//! [`MemoryLedger::mark_frame`], so the readout can show growth per frame. A
//! steady positive drift on a gauge that should be stable is a leak, and it is
//! visible here long before an out-of-memory failure.

use std::sync::atomic::{AtomicU64, Ordering};

/// One named byte gauge, updated by whoever owns the memory.
///
/// Atomics rather than a lock: gauges are written from wherever the allocation
/// happens — including the world thread — and a readout must never be able to
/// block the thread it is measuring.
#[derive(Default)]
struct Gauge {
    bytes: AtomicU64,
    peak_bytes: AtomicU64,
    /// Value at the previous `mark_frame`, for the drift column.
    previous_bytes: AtomicU64,
}

/// A fixed set of named byte gauges. Like [`crate::cpu::SpanRecorder`], the
/// consumer owns the names and indexes into its own label list.
///
/// ```
/// # use atrium_profile::memory::MemoryLedger;
/// const CATEGORIES: &[&str] = &["world", "light volume"];
/// const WORLD: usize = 0;
///
/// let ledger = MemoryLedger::new(CATEGORIES);
/// ledger.set(WORLD, 7 * 1024 * 1024);
/// assert_eq!(ledger.bytes(WORLD), 7 * 1024 * 1024);
/// ```
pub struct MemoryLedger {
    labels: &'static [&'static str],
    gauges: Box<[Gauge]>,
}

impl MemoryLedger {
    pub fn new(labels: &'static [&'static str]) -> Self {
        Self {
            labels,
            gauges: (0..labels.len()).map(|_| Gauge::default()).collect(),
        }
    }

    pub fn labels(&self) -> &'static [&'static str] {
        self.labels
    }

    pub fn category_count(&self) -> usize {
        self.gauges.len()
    }

    pub fn label(&self, category: usize) -> &'static str {
        self.labels
            .get(category)
            .copied()
            .unwrap_or("<unknown category>")
    }

    /// Report the current size of a category, in bytes. Updates the peak.
    ///
    /// Out-of-range indices are a `debug_assert` and then a no-op, for the same
    /// reason as in [`crate::cpu`]: instrumentation must not be able to kill the
    /// thing it measures.
    pub fn set(&self, category: usize, bytes: u64) {
        debug_assert!(
            category < self.gauges.len(),
            "category {category} out of range for {} gauges",
            self.gauges.len()
        );
        let Some(gauge) = self.gauges.get(category) else {
            return;
        };
        gauge.bytes.store(bytes, Ordering::Relaxed);
        gauge.peak_bytes.fetch_max(bytes, Ordering::Relaxed);
    }

    pub fn bytes(&self, category: usize) -> u64 {
        self.gauges
            .get(category)
            .map(|gauge| gauge.bytes.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    pub fn peak_bytes(&self, category: usize) -> u64 {
        self.gauges
            .get(category)
            .map(|gauge| gauge.peak_bytes.load(Ordering::Relaxed))
            .unwrap_or(0)
    }

    /// Change since the previous [`Self::mark_frame`], in bytes. Signed: a
    /// negative value is memory released.
    pub fn drift_bytes(&self, category: usize) -> i64 {
        let Some(gauge) = self.gauges.get(category) else {
            return 0;
        };
        let current = gauge.bytes.load(Ordering::Relaxed) as i64;
        let previous = gauge.previous_bytes.load(Ordering::Relaxed) as i64;
        current - previous
    }

    /// Sum of every gauge. A LOWER BOUND on real usage — only what was reported.
    pub fn total_bytes(&self) -> u64 {
        (0..self.gauges.len()).map(|index| self.bytes(index)).sum()
    }

    pub fn total_peak_bytes(&self) -> u64 {
        (0..self.gauges.len())
            .map(|index| self.peak_bytes(index))
            .sum()
    }

    /// Latch the current values as the baseline for [`Self::drift_bytes`]. Call
    /// once per frame, after the frame's gauges have been reported.
    pub fn mark_frame(&self) {
        for gauge in self.gauges.iter() {
            let current = gauge.bytes.load(Ordering::Relaxed);
            gauge.previous_bytes.store(current, Ordering::Relaxed);
        }
    }

    /// Reset every gauge, including peaks. For a deliberate discontinuity such as
    /// loading a different world.
    pub fn clear(&self) {
        for gauge in self.gauges.iter() {
            gauge.bytes.store(0, Ordering::Relaxed);
            gauge.peak_bytes.store(0, Ordering::Relaxed);
            gauge.previous_bytes.store(0, Ordering::Relaxed);
        }
    }

    /// One row per category, plus the shape the benchmark prints.
    pub fn rows(&self) -> Vec<MemoryRow> {
        (0..self.gauges.len())
            .map(|category| MemoryRow {
                label: self.label(category),
                bytes: self.bytes(category),
                peak_bytes: self.peak_bytes(category),
                drift_bytes: self.drift_bytes(category),
            })
            .collect()
    }

    /// Fixed-width table for stdout or a benchmark log.
    pub fn report(&self, title: &str) -> String {
        use std::fmt::Write as _;

        let rows = self.rows();
        let label_width = rows
            .iter()
            .map(|row| row.label.len())
            .chain(std::iter::once("TOTAL (reported only)".len()))
            .max()
            .unwrap_or(0);

        let mut out = String::new();
        let _ = writeln!(out, "{title}");
        let _ = writeln!(
            out,
            "{:<label_width$}  {:>10}  {:>10}  drift",
            "category", "bytes", "peak"
        );
        let _ = writeln!(out, "{}", "-".repeat(label_width + 2 + 10 + 2 + 10 + 7));
        for row in &rows {
            let _ = writeln!(
                out,
                "{:<label_width$}  {:>10}  {:>10}  {}",
                row.label,
                format_bytes(row.bytes),
                format_bytes(row.peak_bytes),
                format_drift(row.drift_bytes),
            );
        }
        // Named "reported only" rather than "total": this crate cannot know about
        // memory nobody told it about, and a bare "TOTAL" would imply it could.
        let _ = writeln!(
            out,
            "{:<label_width$}  {:>10}  {:>10}",
            "TOTAL (reported only)",
            format_bytes(self.total_bytes()),
            format_bytes(self.total_peak_bytes()),
        );
        out
    }
}

/// One category's numbers, for rendering.
#[derive(Clone, Copy, Debug)]
pub struct MemoryRow {
    pub label: &'static str,
    pub bytes: u64,
    pub peak_bytes: u64,
    /// Signed change since the previous frame.
    pub drift_bytes: i64,
}

/// Human-readable bytes in **binary** units, labelled `KiB`/`MiB`/`GiB`.
///
/// The suffixes are explicit rather than the friendlier `MB`, because mixing the
/// two conventions has already cost us a confused reading: a light volume that
/// this reports as `45.8` and a startup log reports as `48.0` are the SAME
/// allocation, one in MiB and one in decimal MB. Ambiguous units turn a 4.6%
/// labelling difference into a hunt for a bug that does not exist.
pub fn format_bytes(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KIB {
        format!("{bytes} B")
    } else if value < KIB * KIB {
        format!("{:.1} KiB", value / KIB)
    } else if value < KIB * KIB * KIB {
        format!("{:.1} MiB", value / (KIB * KIB))
    } else {
        format!("{:.2} GiB", value / (KIB * KIB * KIB))
    }
}

/// Signed byte delta, with an explicit sign so growth is unmistakable. Exactly
/// zero prints as a dash rather than "0 B" — the common case should be quiet.
pub fn format_drift(bytes: i64) -> String {
    match bytes {
        0 => "-".to_string(),
        positive if positive > 0 => format!("+{}", format_bytes(positive as u64)),
        negative => format!("-{}", format_bytes(negative.unsigned_abs())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CATEGORIES: &[&str] = &["world", "light volume"];
    const WORLD: usize = 0;
    const LIGHT_VOLUME: usize = 1;

    #[test]
    fn a_gauge_holds_its_level_and_remembers_its_peak() {
        let ledger = MemoryLedger::new(CATEGORIES);
        ledger.set(WORLD, 1_000);
        ledger.set(WORLD, 5_000);
        ledger.set(WORLD, 2_000);

        assert_eq!(ledger.bytes(WORLD), 2_000, "the level is the latest value");
        assert_eq!(
            ledger.peak_bytes(WORLD),
            5_000,
            "the peak survives shrinking"
        );
    }

    /// The leak detector: drift is measured against the previous frame, so a
    /// gauge that keeps climbing is visible per frame rather than only at OOM.
    #[test]
    fn drift_reports_growth_since_the_previous_frame_and_is_signed() {
        let ledger = MemoryLedger::new(CATEGORIES);
        ledger.set(WORLD, 1_000);
        ledger.mark_frame();
        assert_eq!(ledger.drift_bytes(WORLD), 0, "no change since the mark");

        ledger.set(WORLD, 1_500);
        assert_eq!(ledger.drift_bytes(WORLD), 500);

        ledger.mark_frame();
        ledger.set(WORLD, 900);
        assert_eq!(
            ledger.drift_bytes(WORLD),
            -600,
            "released memory reads negative"
        );
    }

    #[test]
    fn totals_sum_every_gauge_and_are_labelled_as_reported_only() {
        let ledger = MemoryLedger::new(CATEGORIES);
        ledger.set(WORLD, 7 * 1024 * 1024);
        ledger.set(LIGHT_VOLUME, 48 * 1024 * 1024);
        assert_eq!(ledger.total_bytes(), 55 * 1024 * 1024);

        let report = ledger.report("Memory");
        assert!(report.contains("48.0 MiB"), "{report}");
        assert!(report.contains("55.0 MiB"), "{report}");
        // A bare "TOTAL" would claim to be the process total, which it is not.
        assert!(report.contains("TOTAL (reported only)"), "{report}");
    }

    #[test]
    fn a_cleared_ledger_forgets_its_peak_too() {
        let ledger = MemoryLedger::new(CATEGORIES);
        ledger.set(WORLD, 9_000);
        ledger.clear();
        assert_eq!(ledger.bytes(WORLD), 0);
        assert_eq!(ledger.peak_bytes(WORLD), 0);
        assert_eq!(ledger.drift_bytes(WORLD), 0);
    }

    #[test]
    fn bytes_format_in_binary_units_across_every_magnitude() {
        assert_eq!(format_bytes(512), "512 B");
        assert_eq!(format_bytes(2_048), "2.0 KiB");
        assert_eq!(format_bytes(48 * 1024 * 1024), "48.0 MiB");
        assert_eq!(format_bytes(3 * 1024 * 1024 * 1024), "3.00 GiB");
    }

    /// An unchanged gauge is the common case and must stay visually quiet, or the
    /// drift column becomes noise that hides the one row that is growing.
    #[test]
    fn drift_is_quiet_when_nothing_changed_and_signed_when_it_did() {
        assert_eq!(format_drift(0), "-");
        assert_eq!(format_drift(2_048), "+2.0 KiB");
        assert_eq!(format_drift(-2_048), "-2.0 KiB");
    }

    #[test]
    #[cfg(not(debug_assertions))]
    fn an_out_of_range_category_is_a_no_op_rather_than_a_panic() {
        let ledger = MemoryLedger::new(CATEGORIES);
        ledger.set(99, 1_000);
        assert_eq!(ledger.bytes(99), 0);
        assert_eq!(ledger.label(99), "<unknown category>");
    }
}
