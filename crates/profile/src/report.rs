//! Plain-text reporting: the format the headless benchmark prints.
//!
//! Lives here rather than in the benchmark so the live overlay and the offline
//! harness read the *same* statistics through the same shape. A perf number that
//! is computed one way on screen and another way in the gate is a number you
//! cannot act on.
//!
//! Text only — no egui, no wgpu. The overlay renders [`TableRow`] itself; this
//! module is what makes a row printable.

use std::fmt::Write as _;

use crate::stats::PeriodEstimate;

/// Autocorrelation strength below which a detected period is treated as noise
/// and left out of reports. Shared by the overlay and the benchmark so both
/// agree on what counts as "periodic".
pub const PERIOD_REPORT_STRENGTH: f32 = 0.25;

/// A span must reach this p95 before any detected period is reported.
///
/// Strength alone is not enough, and a real dump proved it: spans measuring
/// 0.000 ms reported confident-looking waves at 34 Hz, which was the monotonic
/// clock's own quantisation being correlated with itself.
pub const PERIOD_REPORT_MINIMUM_P95_MILLISECONDS: f32 = 0.25;

/// A span's p95-minus-p50 swing must reach this before a period is reported.
///
/// The subtler half of the same lesson. Pearson correlation NORMALISES MAGNITUDE
/// AWAY, so a series that is 95% one identical value with two blips in it
/// correlates at r=0.93 on the blip spacing — the strongest-looking number in the
/// dump, and pure artefact. A span that does not actually swing is not waving,
/// however well it correlates.
pub const PERIOD_REPORT_MINIMUM_SWING_MILLISECONDS: f32 = 0.05;

/// One measured span, reduced to the numbers worth printing.
#[derive(Clone, Copy, Debug)]
pub struct TableRow {
    pub label: &'static str,
    /// The typical cost. Median, not mean — see [`crate::frame`].
    pub median_milliseconds: Option<f32>,
    /// The bad case, excluding lone outliers.
    pub p95_milliseconds: Option<f32>,
    pub max_milliseconds: Option<f32>,
    /// Times the span was entered in the most recent sample.
    pub hits: u32,
    /// Detected oscillation in this span's cost, if any.
    pub period: Option<PeriodEstimate>,
}

impl TableRow {
    /// A row carrying no measurement at all — printed as dashes rather than
    /// zeroes, because "not measured" and "measured as free" are different
    /// claims and conflating them has cost us a bench read before.
    pub fn missing(label: &'static str) -> Self {
        Self {
            label,
            median_milliseconds: None,
            p95_milliseconds: None,
            max_milliseconds: None,
            hits: 0,
            period: None,
        }
    }

    /// Spread between typical and bad. `None` unless both are measured.
    pub fn jitter_milliseconds(&self) -> Option<f32> {
        Some(self.p95_milliseconds? - self.median_milliseconds?)
    }

    /// The detected period, but only when it is worth believing: strong enough,
    /// on a span large enough to matter, that actually swings.
    ///
    /// **Every reader must go through this rather than reading `period`
    /// directly** — that is what keeps the live overlay and the benchmark from
    /// disagreeing about whether something is oscillating.
    pub fn reportable_period(&self) -> Option<PeriodEstimate> {
        let period = self.period?;
        if period.strength < PERIOD_REPORT_STRENGTH {
            return None;
        }
        if self.p95_milliseconds? < PERIOD_REPORT_MINIMUM_P95_MILLISECONDS {
            return None;
        }
        if self.jitter_milliseconds()? < PERIOD_REPORT_MINIMUM_SWING_MILLISECONDS {
            return None;
        }
        Some(period)
    }
}

/// Render rows as a fixed-width table with a header, suitable for stdout or a
/// benchmark log. The label column sizes itself to the longest label.
///
/// `samples_per_second` converts a detected period into a rate for the `wave`
/// column; pass the frame rate the rows were sampled at.
pub fn format_table(title: &str, rows: &[TableRow], samples_per_second: f32) -> String {
    const MISSING: &str = "  --  ";
    let label_width = rows
        .iter()
        .map(|row| row.label.len())
        .chain(std::iter::once(title.len()))
        .max()
        .unwrap_or(0)
        .max("span".len());

    let mut out = String::new();
    let _ = writeln!(out, "{title}");
    let _ = writeln!(
        out,
        "{:<label_width$}  {:>8}  {:>8}  {:>8}  {:>5}  wave",
        "span", "p50 ms", "p95 ms", "max ms", "hits"
    );
    let _ = writeln!(
        out,
        "{}",
        "-".repeat(label_width + 2 + 8 + 2 + 8 + 2 + 8 + 2 + 5 + 2 + 4)
    );

    let format_milliseconds = |value: Option<f32>| match value {
        Some(milliseconds) => format!("{milliseconds:8.3}"),
        None => format!("{MISSING:>8}"),
    };

    for row in rows {
        let wave = match row.reportable_period() {
            Some(period) => format!(
                "{} frames ({:.1} Hz, r={:.2})",
                period.period_samples,
                period.hertz(samples_per_second),
                period.strength
            ),
            _ => "-".to_string(),
        };
        let _ = writeln!(
            out,
            "{:<label_width$}  {}  {}  {}  {:>5}  {}",
            row.label,
            format_milliseconds(row.median_milliseconds),
            format_milliseconds(row.p95_milliseconds),
            format_milliseconds(row.max_milliseconds),
            row.hits,
            wave
        );
    }
    out
}

/// Sum of the rows' medians — the measured total. Compare against a frame
/// interval to find unmeasured time.
pub fn total_median_milliseconds(rows: &[TableRow]) -> f32 {
    rows.iter().filter_map(|row| row.median_milliseconds).sum()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn row(label: &'static str, median: f32) -> TableRow {
        TableRow {
            label,
            median_milliseconds: Some(median),
            p95_milliseconds: Some(median * 2.0),
            max_milliseconds: Some(median * 3.0),
            hits: 1,
            period: None,
        }
    }

    #[test]
    fn a_table_lines_up_and_names_every_span() {
        let rows = [row("dda", 4.0), row("a much longer span name", 1.25)];
        let table = format_table("CPU spans", &rows, 60.0);

        for line in table.lines().skip(3) {
            assert!(line.contains("ms") || line.contains('.'), "{line}");
        }
        assert!(table.contains("dda"));
        assert!(table.contains("a much longer span name"));
        assert!(table.contains("4.000"));
        assert!(table.contains("1.250"));
    }

    /// "Not measured" must never print as 0.000 — that reads as "this is free".
    #[test]
    fn an_unmeasured_row_prints_dashes_rather_than_zero() {
        let table = format_table("CPU spans", &[TableRow::missing("cagi")], 60.0);
        assert!(table.contains("--"), "{table}");
        assert!(!table.contains("0.000"), "{table}");
    }

    #[test]
    fn a_periodic_row_reports_frames_and_hertz_and_a_weak_one_does_not() {
        let mut strong = row("upload", 2.0);
        strong.period = Some(PeriodEstimate {
            period_samples: 12,
            strength: 0.8,
        });
        let table = format_table("CPU spans", &[strong], 60.0);
        assert!(table.contains("12 frames"), "{table}");
        assert!(table.contains("5.0 Hz"), "{table}");

        let mut weak = row("upload", 2.0);
        weak.period = Some(PeriodEstimate {
            period_samples: 12,
            strength: 0.05,
        });
        let quiet = format_table("CPU spans", &[weak], 60.0);
        assert!(!quiet.contains("12 frames"), "noise must not be reported");
    }

    /// Regression, from a real dump: a span measuring 0.000 ms reported a
    /// confident "4 frames (34.6 Hz)". That was the monotonic clock's own
    /// quantisation correlating with itself, not a wave.
    #[test]
    fn a_negligible_span_reports_no_wave_however_strongly_it_correlates() {
        let mut negligible = TableRow {
            label: "input + move",
            median_milliseconds: Some(0.000),
            p95_milliseconds: Some(0.000),
            max_milliseconds: Some(0.001),
            hits: 1,
            period: Some(PeriodEstimate {
                period_samples: 4,
                strength: 0.41,
            }),
        };
        assert_eq!(negligible.reportable_period(), None);
        let table = format_table("CPU spans", &[negligible], 138.0);
        assert!(!table.contains("34.6 Hz"), "{table}");

        // Same strength, but now on a span that is actually large and swinging:
        // this is `acquire` from the same dump, and it must still be reported.
        negligible.label = "acquire";
        negligible.median_milliseconds = Some(6.545);
        negligible.p95_milliseconds = Some(9.933);
        negligible.period = Some(PeriodEstimate {
            period_samples: 26,
            strength: 0.52,
        });
        assert!(negligible.reportable_period().is_some());
    }

    /// The subtler regression: a nearly CONSTANT series correlates at r=0.93
    /// because Pearson normalises magnitude away. `blit+ui` did exactly this —
    /// p50 = p95 = max = 6.384 — and produced the strongest-looking number in
    /// the dump while not oscillating at all.
    #[test]
    fn a_span_that_does_not_swing_reports_no_wave_even_at_high_strength() {
        let flat = TableRow {
            label: "blit+ui",
            median_milliseconds: Some(6.384),
            p95_milliseconds: Some(6.384),
            max_milliseconds: Some(6.384),
            hits: 1,
            period: Some(PeriodEstimate {
                period_samples: 2,
                strength: 0.93,
            }),
        };
        assert_eq!(
            flat.jitter_milliseconds(),
            Some(0.0),
            "the premise: this span is large but does not swing"
        );
        assert_eq!(flat.reportable_period(), None);
    }

    #[test]
    fn totals_skip_unmeasured_rows_and_jitter_needs_both_numbers() {
        let rows = [row("a", 2.0), TableRow::missing("b"), row("c", 3.0)];
        assert_eq!(total_median_milliseconds(&rows), 5.0);
        assert_eq!(rows[0].jitter_milliseconds(), Some(2.0));
        assert_eq!(rows[1].jitter_milliseconds(), None);
    }

    #[test]
    fn an_empty_table_still_prints_its_header() {
        let table = format_table("CPU spans", &[], 60.0);
        assert!(table.contains("CPU spans"));
        assert!(table.contains("p95 ms"));
    }
}
