//! voxel-rt library surface: every module of the renderer, exported so that
//! the binary (`main.rs`) AND the permanent headless benchmark
//! (`examples/bench_dda.rs`) build on the exact same code — the benchmark
//! must measure the real pass, not a copy that can drift.
//!
//! Module seams (plan architecture rule): platform/windowing (`main.rs`,
//! `gpu`) ↔ render passes (`passes`, `render`) ↔ world data (`brickmap`) ↔
//! camera (`camera`) ↔ lighting (`lighting`) ↔ ambient occlusion (`ao`, E1)
//! ↔ overlay (`overlay`) ↔ GPU timing (`frame_timing`).

pub mod ao;
pub mod brickmap;
pub mod camera;
pub mod frame_timing;
pub mod gpu;
pub mod lighting;
pub mod overlay;
pub mod passes;
pub mod render;
