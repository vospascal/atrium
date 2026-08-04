//! voxel-rt library surface: every module of the renderer, exported so that
//! the binary (`main.rs`) AND the permanent headless benchmark
//! (`examples/bench_dda.rs`) build on the exact same code — the benchmark
//! must measure the real pass, not a copy that can drift.
//!
//! Module seams (plan architecture rule): platform/windowing (`main.rs`,
//! `gpu`) ↔ render passes (`passes`, `render`) ↔ world data (`brickmap`) ↔
//! world authority + edit threading (`world_edit`, `world_host`, E2) ↔ CPU
//! traversal for picking and audio (`voxel_dda`, E2/E8) ↔
//! camera (`camera`) ↔ character body + voxel collision (`character`, E2b — the
//! audio listener at E8 and the VR player at E9) ↔
//! lighting (`lighting`) ↔ traversal levers (`traversal`,
//! S2) ↔ ambient occlusion (`ao`, E1) ↔ sun shadows (`shadows`, E1b) ↔ CAGI light
//! volume (`cagi`, E4) ↔ water optics (`water`, E6) ↔ lever registry + quality
//! presets (`variants`, E1c) ↔ overlay (`overlay`) ↔ GPU timing
//! (`frame_timing`).
//!
//! `variants` is the single source of truth for what levers exist: the overlay,
//! the benchmark and the pinning tests all read its registry, so a lever cannot
//! live in the shader without a documented verdict and a bench column.

pub mod ao;
pub mod biome;
pub mod brickmap;
pub mod cagi;
pub mod camera;
pub mod character;
pub mod engine_runtime;
pub mod environment;
pub mod frame_timing;
pub mod gpu;
pub mod graph;
pub mod light_fixture;
pub mod lighting;
pub mod material_edit;
pub mod material_graph_assets;
pub mod material_table;
pub mod material_tune;
pub mod overlay;
pub mod passes;
pub mod render;
pub mod shadows;
pub mod studio;
pub mod studio_assets;
pub mod traversal;
pub mod variants;
pub mod vox_material;
pub mod voxel_dda;
pub mod water;
pub mod world_edit;
pub mod world_host;
pub mod world_profile;
pub mod world_profile_runtime;
