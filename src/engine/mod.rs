//! Audio engine internals: scene graph, command queue, telemetry, and profiling.

pub mod commands;
/// Audio-thread allocation tracking. Only available with `--features memprof`.
#[cfg(feature = "memprof")]
pub mod memprof;
/// Zero-cost profiling spans (`profile_span!` macro). See module docs for usage.
pub mod profiler;
pub mod scene;
pub mod telemetry;
