//! Zero-cost profiling spans for the audio render pipeline.
//!
//! # Feature gate
//!
//! All profiling is behind the **`profiler`** Cargo feature and compiles to
//! nothing when disabled.
//!
//! ```toml
//! cargo run --features profiler -- scenes/default.yaml --profile perfetto
//! ```
//!
//! # Usage
//!
//! Wrap any code section with [`profile_span!`] to measure its duration:
//!
//! ```rust,ignore
//! use crate::profile_span;
//!
//! let _total = profile_span!("render", sources = 4).entered();
//! {
//!     let _s = profile_span!("source_tick").entered();
//!     // ... work ...
//! }
//! // _s drops here → span recorded
//! ```
//!
//! The span guard is RAII — timing starts on `.entered()` and stops when the
//! guard drops (goes out of scope). Use block scopes `{ }` to control the
//! measurement window precisely.
//!
//! # Output backends (`--profile` flag)
//!
//! | Flag | Output | View with |
//! |------|--------|-----------|
//! | `fmt` | Terminal span timing | Terminal |
//! | `perfetto` | `trace.pftrace` (native protobuf) | [ui.perfetto.dev](https://ui.perfetto.dev) |
//! | `flame` | `tracing.folded` (folded stacks) | `inferno-flamegraph` / speedscope |
//!
//! Without `--profile`, no subscriber is installed and spans are ~1ns no-ops.
//!
//! # How it works
//!
//! - With `profiler` feature: `profile_span!` expands to `tracing::info_span!`
//! - Without: expands to [`NoopSpan`] which the compiler eliminates entirely

pub struct NoopSpan;

pub struct NoopSpanGuard;

impl NoopSpan {
    #[inline]
    pub fn entered(self) -> NoopSpanGuard {
        NoopSpanGuard
    }
}

impl Drop for NoopSpanGuard {
    #[inline]
    fn drop(&mut self) {}
}

#[cfg(feature = "profiler")]
#[macro_export]
macro_rules! profile_span {
    ($name:expr $(, $($field:tt)*)?) => {
        tracing::info_span!($name $(, $($field)*)?)
    };
}

#[cfg(not(feature = "profiler"))]
#[macro_export]
macro_rules! profile_span {
    ($name:expr $(, $($field:tt)*)?) => {
        $crate::engine::profiler::NoopSpan
    };
}
