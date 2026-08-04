//! The performance window — press **P**.
//!
//! Deliberately a toggled window rather than another always-on corner readout:
//! the overlay already costs a lot of permanent screen area, and per-span perf
//! numbers are something you go looking for, not something you need in frame.
//!
//! # What it is for
//!
//! Answering "why does it come in waves" without guessing. Three independently
//! measured quantities are shown side by side:
//!
//! 1. **Frame interval** — how far apart frames actually landed.
//! 2. **CPU spans** — where the frame's CPU time went ([`crate::profiling`]).
//! 3. **GPU spans** — where the device's time went (timestamp queries).
//!
//! The number that usually cracks it is [`PerformancePanel`]'s *unaccounted*
//! row: frame interval minus CPU total. CPU work and GPU work overlap by design,
//! so a healthy frame has a small remainder. A large one means the frame loop
//! spent its time **blocked** — and since `acquire` and `present` are measured
//! separately, the panel says which side of the swapchain it blocked on.
//!
//! Every statistic is a median or a p95, never a mean, and every row can report a
//! detected oscillation period. See [`atrium_profile`] for why.

use atrium_profile::cpu::{SpanHistory, SpanRecorder};
use atrium_profile::frame::FrameClock;
use atrium_profile::memory::{format_bytes, format_drift, MemoryLedger};
use atrium_profile::report::{self, TableRow, PERIOD_REPORT_STRENGTH};
use atrium_profile::DEFAULT_HISTORY_FRAMES;

use crate::profiling::{self, FrameTimings};

const GRAPH_MAX_WIDTH: f32 = 420.0;
const GRAPH_HEIGHT: f32 = 84.0;

const FRAME_INTERVAL_COLOR: egui::Color32 = egui::Color32::from_rgb(117, 180, 255);
const CPU_TOTAL_COLOR: egui::Color32 = egui::Color32::from_rgb(255, 186, 96);
const GPU_TOTAL_COLOR: egui::Color32 = egui::Color32::from_rgb(110, 220, 155);

/// Frame-interval budget used for the "missed" percentage, milliseconds. 120 Hz —
/// the refresh of the development display, and the target the renderer is tuned
/// against.
const FRAME_BUDGET_MILLISECONDS: f32 = 1_000.0 / 120.0;

pub struct PerformancePanel {
    /// Toggled by **P**. Public so the platform layer flips it without a setter
    /// that would do nothing else.
    pub visible: bool,
    frame_clock: FrameClock,
    cpu_spans: SpanHistory,
    /// GPU pass timings, fed through the same history type as CPU spans so both
    /// get the same percentiles and wave detection.
    gpu_spans: SpanHistory,
    /// Per-frame sum of the CPU spans, and of the GPU spans — kept as their own
    /// series so the graph can draw them against the frame interval.
    cpu_total: atrium_profile::stats::RollingWindow,
    gpu_total: atrium_profile::stats::RollingWindow,
    /// `collect` returns its previous result on frames where no new readback has
    /// landed. Without this the same GPU sample enters the history repeatedly and
    /// flattens every statistic over it.
    last_gpu_sample_sequence: Option<u64>,
    /// False when the device has no timestamp-query support, so the panel says so
    /// instead of showing an empty GPU table.
    gpu_timing_available: bool,
    /// Byte gauges. Public through [`Self::memory`] so the platform layer reports
    /// sizes it owns; the panel only reads and draws them.
    memory: MemoryLedger,
}

impl Default for PerformancePanel {
    fn default() -> Self {
        Self::new()
    }
}

impl PerformancePanel {
    pub fn new() -> Self {
        Self {
            visible: false,
            frame_clock: FrameClock::new(DEFAULT_HISTORY_FRAMES),
            cpu_spans: SpanHistory::new(profiling::CPU_SPANS, DEFAULT_HISTORY_FRAMES),
            gpu_spans: SpanHistory::new(profiling::GPU_SPANS, DEFAULT_HISTORY_FRAMES),
            cpu_total: atrium_profile::stats::RollingWindow::new(DEFAULT_HISTORY_FRAMES),
            gpu_total: atrium_profile::stats::RollingWindow::new(DEFAULT_HISTORY_FRAMES),
            last_gpu_sample_sequence: None,
            gpu_timing_available: false,
            memory: MemoryLedger::new(profiling::MEMORY_CATEGORIES),
        }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// The byte gauges, for the platform layer to report sizes into once per
    /// frame. See [`profiling::MEMORY_CATEGORIES`] for the indices.
    pub fn memory(&self) -> &MemoryLedger {
        &self.memory
    }

    /// Fold one frame's measurements in. Call once per frame, whether or not the
    /// window is visible — history has to be warm the moment it is opened, and a
    /// wave takes seconds of history to detect.
    ///
    /// Draining the recorder every frame is also what keeps its accumulators from
    /// summing several frames together.
    pub fn record_frame(&mut self, recorder: &SpanRecorder, gpu_timings: Option<FrameTimings>) {
        self.frame_clock.tick();
        // Latch this frame's gauges as the baseline, so `drift` means "since the
        // previous frame" rather than "since startup".
        self.memory.mark_frame();
        self.cpu_spans.sample(recorder);
        self.cpu_total.push(
            (0..self.cpu_spans.span_count())
                .filter_map(|span| self.cpu_spans.latest_milliseconds(span))
                .sum(),
        );

        let Some(timings) = gpu_timings else {
            return;
        };
        self.gpu_timing_available = true;
        // Only fold a readback in once — see `last_gpu_sample_sequence`.
        if self.last_gpu_sample_sequence == Some(timings.sample_sequence) {
            return;
        }
        self.last_gpu_sample_sequence = Some(timings.sample_sequence);
        for span in 0..profiling::GPU_SPANS.len() {
            if let Some(milliseconds) = timings.span_milliseconds(span) {
                self.gpu_spans.push_milliseconds(span, milliseconds);
            }
        }
        if let Some(total) = timings.total_milliseconds() {
            self.gpu_total.push(total);
        }
    }

    /// Discard history after a deliberate discontinuity — a vsync toggle, an
    /// output-format change — so the transient is not read as a regression.
    pub fn reset(&mut self) {
        self.frame_clock.reset();
        self.cpu_spans.clear();
        self.gpu_spans.clear();
        self.cpu_total.clear();
        self.gpu_total.clear();
        self.last_gpu_sample_sequence = None;
    }

    /// Frame rate from the median frame interval, for any caller that wants one
    /// number (the compact readout still shows a frame rate).
    pub fn frames_per_second(&mut self) -> Option<f32> {
        self.frame_clock.frames_per_second()
    }

    /// The whole readout as plain text: the same tables the window draws, in the
    /// form the headless benchmark prints. This is the shared path that keeps the
    /// live numbers and the gate's numbers identical.
    pub fn text_report(&mut self) -> String {
        let samples_per_second = self.frames_per_second().unwrap_or(60.0);
        let mut out = String::new();
        out.push_str(&self.pacing_summary());
        out.push('\n');
        out.push_str(&report::format_table(
            "CPU spans",
            &self.cpu_spans.table_rows(),
            samples_per_second,
        ));
        out.push('\n');
        out.push_str(&report::format_table(
            "GPU passes",
            &self.gpu_span_rows(),
            samples_per_second,
        ));
        out.push('\n');
        out.push_str(&self.memory.report("Memory"));
        out
    }

    /// One-line pacing summary, and the accounting line that names backpressure.
    fn pacing_summary(&mut self) -> String {
        let median = self.frame_clock.median_milliseconds();
        let p95 = self.frame_clock.p95_milliseconds();
        let jitter = self.frame_clock.jitter_milliseconds();
        let missed = self
            .frame_clock
            .over_budget_fraction(FRAME_BUDGET_MILLISECONDS);
        let cpu = self.cpu_total.median();
        let gpu = self.gpu_total.median();

        let mut out = String::new();
        out.push_str(&format!(
            "frame interval  p50 {}  p95 {}  jitter {}  over {:.0}%\n",
            format_milliseconds(median),
            format_milliseconds(p95),
            format_milliseconds(jitter),
            missed.unwrap_or(0.0) * 100.0,
        ));
        out.push_str(&format!(
            "CPU total {}  GPU total {}  unaccounted {}\n",
            format_milliseconds(cpu),
            format_milliseconds(gpu),
            format_milliseconds(unaccounted_milliseconds(median, cpu)),
        ));
        out
    }

    fn gpu_span_rows(&mut self) -> Vec<TableRow> {
        if !self.gpu_timing_available {
            return profiling::GPU_SPANS
                .iter()
                .map(|label| TableRow::missing(label))
                .collect();
        }
        self.gpu_spans.table_rows()
    }

    /// Draw the window when visible. `egui::Window` handles dragging, resizing
    /// and the close button; the caller only owns the toggle.
    pub fn draw(&mut self, context: &egui::Context) {
        if !self.visible {
            return;
        }
        // `Window::open` borrows the flag mutably, which would conflict with the
        // `&mut self` the body needs — so the flag round-trips through a local.
        let mut visible = self.visible;
        egui::Window::new("Performance  (P)")
            .open(&mut visible)
            .default_width(460.0)
            .resizable(true)
            .show(context, |ui| self.draw_body(ui));
        self.visible = visible;
    }

    fn draw_body(&mut self, ui: &mut egui::Ui) {
        self.draw_pacing(ui);
        ui.separator();
        self.draw_graph(ui);
        ui.separator();

        let samples_per_second = self.frame_clock.frames_per_second().unwrap_or(60.0);
        let cpu_rows = self.cpu_spans.table_rows();
        draw_span_table(ui, "CPU spans", &cpu_rows, samples_per_second);

        ui.add_space(4.0);
        if self.gpu_timing_available {
            let gpu_rows = self.gpu_spans.table_rows();
            draw_span_table(ui, "GPU passes", &gpu_rows, samples_per_second);
        } else {
            ui.label("GPU passes: timestamp queries unavailable on this device.");
        }

        ui.add_space(4.0);
        self.draw_memory(ui);

        ui.add_space(6.0);
        if ui
            .button("print report to stdout")
            .on_hover_text(
                "Prints exactly the table the headless benchmark prints, so a \
                 live observation can be pasted next to a gate run.",
            )
            .clicked()
        {
            println!("{}", self.text_report());
        }
    }

    /// Byte gauges: what each subsystem holds, its peak, and whether it moved.
    ///
    /// Attributed bytes, NOT a process total — every row is a size the renderer
    /// reports about itself, which is what makes it actionable. The total is
    /// labelled "reported only" for that reason.
    fn draw_memory(&mut self, ui: &mut egui::Ui) {
        ui.strong("Memory");
        egui::Grid::new("performance memory")
            .num_columns(4)
            .spacing(egui::vec2(10.0, 2.0))
            .striped(true)
            .show(ui, |ui| {
                ui.label("category");
                ui.label("bytes");
                ui.label("peak");
                ui.label("drift/frame");
                ui.end_row();

                for row in self.memory.rows() {
                    ui.label(row.label);
                    ui.label(format_bytes(row.bytes));
                    ui.label(format_bytes(row.peak_bytes));
                    // Growth is the interesting case, so colour it and leave a
                    // steady gauge visually quiet.
                    if row.drift_bytes > 0 {
                        ui.colored_label(CPU_TOTAL_COLOR, format_drift(row.drift_bytes));
                    } else {
                        ui.weak(format_drift(row.drift_bytes));
                    }
                    ui.end_row();
                }

                ui.strong("total (reported only)");
                ui.strong(format_bytes(self.memory.total_bytes()));
                ui.label(format_bytes(self.memory.total_peak_bytes()));
                ui.weak("-");
                ui.end_row();
            });
    }

    fn draw_pacing(&mut self, ui: &mut egui::Ui) {
        let median = self.frame_clock.median_milliseconds();
        let p95 = self.frame_clock.p95_milliseconds();
        let jitter = self.frame_clock.jitter_milliseconds();
        let frames_per_second = self.frame_clock.frames_per_second();
        let missed = self
            .frame_clock
            .over_budget_fraction(FRAME_BUDGET_MILLISECONDS);
        let cpu = self.cpu_total.median();
        let gpu = self.gpu_total.median();
        let wave = self.frame_clock.dominant_period(PERIOD_REPORT_STRENGTH);

        ui.label(format!(
            "frame interval  p50 {}  p95 {}  ({} FPS)",
            format_milliseconds(median),
            format_milliseconds(p95),
            frames_per_second
                .map(|value| format!("{value:.0}"))
                .unwrap_or_else(|| "--".to_string()),
        ))
        .on_hover_text(
            "Measured between successive redraws, and reported as a MEDIAN — a \
             mean over the window is dominated by the fastest frames and once \
             read '1200 FPS' while the display was visibly hitching.",
        );
        ui.label(format!(
            "jitter {}  |  over {:.1} ms budget: {:.0}% of frames",
            format_milliseconds(jitter),
            FRAME_BUDGET_MILLISECONDS,
            missed.unwrap_or(0.0) * 100.0,
        ))
        .on_hover_text(
            "Jitter is p95 minus p50: how much worse a bad frame is than a \
             typical one. Near zero is smooth even when the median is slow.",
        );

        let unaccounted = unaccounted_milliseconds(median, cpu);
        ui.label(format!(
            "CPU total {}  GPU total {}  unaccounted {}",
            format_milliseconds(cpu),
            format_milliseconds(gpu),
            format_milliseconds(unaccounted),
        ))
        .on_hover_text(
            "UNACCOUNTED = frame interval minus measured CPU work. CPU and GPU \
             work overlap, so a small remainder is healthy. A LARGE one means the \
             frame loop was blocked rather than busy — check the `acquire` and \
             `present` spans below to see which side of the swapchain it blocked \
             on. Under vsync some remainder is expected and correct: the loop is \
             waiting for the display on purpose.",
        );

        match wave {
            Some(period) => {
                ui.colored_label(
                    egui::Color32::from_rgb(255, 186, 96),
                    format!(
                        "wave detected: {} frames ({:.1} Hz, r={:.2})",
                        period.period_samples,
                        period.hertz(self.frame_clock.frames_per_second().unwrap_or(60.0)),
                        period.strength,
                    ),
                )
                .on_hover_text(
                    "The frame interval is OSCILLATING on a fixed cadence, not \
                     merely noisy. Look for a span below reporting the same \
                     period: that is the work causing it. A period that matches \
                     a cascade or upload cadence is that job beating against the \
                     frame budget.",
                );
            }
            None => {
                ui.weak("no periodic wave in the frame interval");
            }
        }
    }

    /// Millisecond history: frame interval against measured CPU and GPU totals.
    ///
    /// Milliseconds rather than frames-per-second on purpose. FPS is a
    /// reciprocal, so it compresses exactly the region a slow frame lives in —
    /// a 4 ms spike is visually obvious in milliseconds and nearly invisible in
    /// FPS once the baseline is high.
    fn draw_graph(&mut self, ui: &mut egui::Ui) {
        let frame_series: Vec<f32> = self.frame_clock.intervals().chronological().collect();
        let cpu_series: Vec<f32> = self.cpu_total.chronological().collect();
        let gpu_series: Vec<f32> = self.gpu_total.chronological().collect();

        // Ceiling from the p95, not the max: one 300 ms hitch (a shader rebuild,
        // a world regeneration) would otherwise flatten the whole graph to the
        // bottom axis for the next four seconds.
        let ceiling = self
            .frame_clock
            .p95_milliseconds()
            .into_iter()
            .chain(self.cpu_total.p95())
            .chain(self.gpu_total.p95())
            .fold(FRAME_BUDGET_MILLISECONDS, f32::max);
        let ceiling = (ceiling * 1.25 / 5.0).ceil() * 5.0;

        ui.horizontal_wrapped(|ui| {
            ui.colored_label(FRAME_INTERVAL_COLOR, "— frame interval");
            ui.colored_label(CPU_TOTAL_COLOR, "— CPU total");
            ui.colored_label(GPU_TOTAL_COLOR, "— GPU total");
            ui.weak(format!("0–{ceiling:.0} ms"));
        });

        let (response, painter) = ui.allocate_painter(
            egui::vec2(ui.available_width().min(GRAPH_MAX_WIDTH), GRAPH_HEIGHT),
            egui::Sense::hover(),
        );
        let plot = response.rect.shrink2(egui::vec2(4.0, 4.0));
        painter.rect_filled(plot, 3.0, ui.visuals().faint_bg_color);

        // The budget line: the frame interval to beat, so a wave crossing it is
        // visible as a wave crossing a line rather than a number to compare.
        let budget_y = egui::remap_clamp(
            FRAME_BUDGET_MILLISECONDS,
            0.0..=ceiling,
            plot.bottom()..=plot.top(),
        );
        painter.line_segment(
            [
                egui::pos2(plot.left(), budget_y),
                egui::pos2(plot.right(), budget_y),
            ],
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );

        for (series, color) in [
            (&frame_series, FRAME_INTERVAL_COLOR),
            (&cpu_series, CPU_TOTAL_COLOR),
            (&gpu_series, GPU_TOTAL_COLOR),
        ] {
            draw_series(&painter, plot, series, ceiling, color);
        }
    }
}

/// Frame interval minus measured CPU work: time the frame loop was not busy.
/// `None` unless both are known, so an unmeasured half never reads as zero.
fn unaccounted_milliseconds(frame_interval: Option<f32>, cpu_total: Option<f32>) -> Option<f32> {
    Some((frame_interval? - cpu_total?).max(0.0))
}

fn format_milliseconds(value: Option<f32>) -> String {
    match value {
        Some(milliseconds) => format!("{milliseconds:.2} ms"),
        None => "-- ms".to_string(),
    }
}

fn draw_series(
    painter: &egui::Painter,
    plot: egui::Rect,
    samples: &[f32],
    ceiling: f32,
    color: egui::Color32,
) {
    if samples.len() < 2 {
        return;
    }
    let last_index = (samples.len() - 1) as f32;
    let points = samples
        .iter()
        .enumerate()
        .map(|(index, milliseconds)| {
            let x = egui::remap(index as f32, 0.0..=last_index, plot.left()..=plot.right());
            let y = egui::remap_clamp(*milliseconds, 0.0..=ceiling, plot.bottom()..=plot.top());
            egui::pos2(x, y)
        })
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, color)));
}

fn draw_span_table(ui: &mut egui::Ui, title: &str, rows: &[TableRow], samples_per_second: f32) {
    ui.strong(title);
    egui::Grid::new(title)
        .num_columns(5)
        .spacing(egui::vec2(10.0, 2.0))
        .striped(true)
        .show(ui, |ui| {
            ui.label("span");
            ui.label("p50");
            ui.label("p95");
            ui.label("max");
            ui.label("wave");
            ui.end_row();

            for row in rows {
                ui.label(row.label);
                ui.label(format_milliseconds(row.median_milliseconds));
                ui.label(format_milliseconds(row.p95_milliseconds));
                ui.label(format_milliseconds(row.max_milliseconds));
                match row.reportable_period() {
                    Some(period) => {
                        ui.colored_label(
                            CPU_TOTAL_COLOR,
                            format!(
                                "{} f / {:.1} Hz",
                                period.period_samples,
                                period.hertz(samples_per_second)
                            ),
                        );
                    }
                    _ => {
                        ui.weak("-");
                    }
                }
                ui.end_row();
            }
        });
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[test]
    fn the_panel_starts_hidden_and_p_toggles_it() {
        let mut panel = PerformancePanel::new();
        assert!(
            !panel.visible,
            "the window must not cost screen space unasked"
        );
        panel.toggle();
        assert!(panel.visible);
        panel.toggle();
        assert!(!panel.visible);
    }

    #[test]
    fn cpu_spans_reach_the_report_even_while_the_window_is_hidden() {
        let recorder = SpanRecorder::new(profiling::CPU_SPANS);
        let mut panel = PerformancePanel::new();
        for _ in 0..8 {
            recorder.record(profiling::CPU_ACQUIRE, Duration::from_micros(5_000));
            panel.record_frame(&recorder, None);
        }
        let report = panel.text_report();
        assert!(report.contains("acquire"), "{report}");
        assert!(report.contains("5.00"), "{report}");
        // No timestamp support was reported, so the GPU table must say so rather
        // than print zeroes.
        assert!(report.contains("--"), "{report}");
    }

    /// The same GPU readback arriving on several frames must not enter the
    /// history more than once — it would drag every percentile toward it.
    #[test]
    fn a_repeated_gpu_sample_sequence_is_folded_in_only_once() {
        let recorder = SpanRecorder::new(profiling::CPU_SPANS);
        let mut panel = PerformancePanel::new();
        let stale = FrameTimings {
            span_milliseconds: [Some(4.0), Some(1.0), Some(0.2), Some(0.3)],
            sample_sequence: 7,
        };
        for _ in 0..10 {
            panel.record_frame(&recorder, Some(stale));
        }
        assert_eq!(panel.gpu_total.len(), 1, "one sequence, one sample");

        let fresh = FrameTimings {
            span_milliseconds: [Some(4.0), Some(1.0), Some(0.2), Some(0.3)],
            sample_sequence: 8,
        };
        panel.record_frame(&recorder, Some(fresh));
        assert_eq!(panel.gpu_total.len(), 2);
    }

    /// The backpressure signal: a frame loop that is blocked rather than busy
    /// shows a large unaccounted remainder.
    #[test]
    fn unaccounted_time_is_the_gap_between_the_interval_and_measured_cpu_work() {
        let remainder = unaccounted_milliseconds(Some(16.7), Some(6.0)).expect("both measured");
        assert!((remainder - 10.7).abs() < 1e-4, "got {remainder}");
        // Never negative: CPU spans can overlap the interval boundary slightly.
        assert_eq!(unaccounted_milliseconds(Some(5.0), Some(6.0)), Some(0.0));
        // Unmeasured stays unmeasured rather than reading as zero.
        assert_eq!(unaccounted_milliseconds(None, Some(6.0)), None);
        assert_eq!(unaccounted_milliseconds(Some(16.7), None), None);
    }

    #[test]
    fn a_reset_clears_every_series_so_a_transient_is_not_read_as_a_regression() {
        let recorder = SpanRecorder::new(profiling::CPU_SPANS);
        let mut panel = PerformancePanel::new();
        for _ in 0..4 {
            recorder.record(profiling::CPU_ENCODE, Duration::from_millis(1));
            panel.record_frame(&recorder, None);
        }
        panel.reset();
        assert_eq!(panel.cpu_total.len(), 0);
        assert_eq!(panel.frame_clock.intervals().len(), 0);
        assert_eq!(panel.frames_per_second(), None);
    }

    #[test]
    fn a_missing_measurement_prints_dashes_rather_than_zero() {
        assert_eq!(format_milliseconds(None), "-- ms");
        assert_eq!(format_milliseconds(Some(1.5)), "1.50 ms");
    }
}
