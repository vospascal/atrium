//! voxel-rt library surface: every module of the renderer, exported so that
//! the binary (`main.rs`) AND the permanent headless benchmark
//! (`examples/bench_dda.rs`) build on the exact same code — the benchmark
//! must measure the real pass, not a copy that can drift.
//!
//! Module seams (plan architecture rule): platform/windowing (`main.rs`,
//! `gpu`) ↔ render passes (`passes`, `render`) ↔ world data (`brickmap`) ↔
//! camera (`camera`) ↔ lighting (`lighting`) ↔ traversal levers (`traversal`,
//! S2) ↔ ambient occlusion (`ao`, E1) ↔ sun shadows (`shadows`, E1b) ↔ lever
//! registry + quality presets (`variants`, E1c) ↔ overlay (`overlay`) ↔ GPU
//! timing (`frame_timing`).
//!
//! `variants` is the single source of truth for what levers exist: the overlay,
//! the benchmark and the pinning tests all read its registry, so a lever cannot
//! live in the shader without a documented verdict and a bench column.

pub mod ao;
pub mod brickmap;
pub mod camera;
pub mod frame_timing;
pub mod gpu;
pub mod lighting;
pub mod overlay;
pub mod passes;
pub mod render;
pub mod shadows;
pub mod traversal;
pub mod variants;
