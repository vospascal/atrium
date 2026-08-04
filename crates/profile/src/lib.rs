//! Performance instrumentation shared by every crate in the tree: CPU spans,
//! frame pacing, GPU pass timing, and one text report format for all of them.
//!
//! # Why this is its own crate
//!
//! The renderer and the audio engine have the same question — *where did the
//! time go, and is it periodic?* — and neither wants the other's dependencies.
//! So the measuring lives here, once, and the **consumer owns the span names**:
//! nothing in this crate knows what a DDA pass or a convolver is. A consumer
//! declares a `&'static [&'static str]` label list and indexes into it.
//!
//! The CPU half ([`cpu`], [`frame`], [`stats`], [`report`]) has **no
//! dependencies at all** and is safe on a realtime audio thread — no
//! allocation, no locking, every buffer preallocated. The GPU half ([`gpu`])
//! needs wgpu and sits behind the off-by-default `gpu` feature.
//!
//! # The four questions, and which module answers each
//!
//! | Question | Module |
//! |---|---|
//! | How long did *this work* take on the CPU? | [`cpu`] |
//! | How far apart did the frames actually land? | [`frame`] |
//! | How long did the *GPU* take on this pass? | [`gpu`] (feature `gpu`) |
//! | How many bytes is each subsystem holding, and is it growing? | [`memory`] |
//!
//! Asking all of them together is the point. CPU spans summing to 6 ms while frames land
//! 16.7 ms apart means 10 ms went somewhere none of the spans cover — and that
//! gap is usually the actual bug.
//!
//! # Read the median, not the mean
//!
//! Every statistic here is percentile-based on purpose. A mean over a window
//! hides oscillation: 4 ms / 20 ms alternating and a flat 12 ms have the same
//! mean and feel nothing alike. [`stats::RollingWindow::dominant_period`] goes
//! further and reports *whether the cost is periodic and at what rate*, which is
//! what turns "it feels like it comes in waves" into a number.
//!
//! # Typical wiring
//!
//! ```
//! use atrium_profile::{cpu::{SpanHistory, SpanRecorder}, frame::FrameClock, report};
//!
//! const SPANS: &[&str] = &["input", "simulate", "encode"];
//! const SPAN_SIMULATE: usize = 1;
//! const HISTORY_FRAMES: usize = 240; // four seconds at 60 Hz
//!
//! let recorder = SpanRecorder::new(SPANS);
//! let mut history = SpanHistory::new(SPANS, HISTORY_FRAMES);
//! let mut clock = FrameClock::new(HISTORY_FRAMES);
//!
//! // ... once per frame:
//! clock.tick();
//! {
//!     let _measured = recorder.scope(SPAN_SIMULATE);
//!     // the work
//! }
//! history.sample(&recorder);
//!
//! // ... and to print it (the benchmark path):
//! let rows = history.table_rows();
//! let text = report::format_table("CPU spans", &rows, clock.frames_per_second().unwrap_or(60.0));
//! # let _ = text;
//! ```

pub mod cpu;
pub mod frame;
pub mod memory;
pub mod report;
pub mod stats;

#[cfg(feature = "gpu")]
pub mod gpu;

/// History depth for a per-frame window: four seconds at 60 Hz.
///
/// Long enough to show several cycles of anything down to ~2 Hz, which covers
/// the cadences that background work (cascade updates, streaming, buffer
/// uploads) actually beats at. Shorter windows can fail to see a slow wave at
/// all; longer ones keep stale frames alive after a scene change.
pub const DEFAULT_HISTORY_FRAMES: usize = 240;
