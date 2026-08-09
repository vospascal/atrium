//! Renderer-agnostic voxel core.
//!
//! Pure data + CPU logic with no engine dependency: voxel storage (RLE),
//! world generation, terrain import, asset parsing, water, wind, and noise.
//! Renderers consume this crate's data without it knowing about them.

pub mod cave;
pub mod chunk;
pub mod noise;
pub mod scatter;
pub mod stage_report;
pub mod terrain_import;
pub mod vox;
pub mod water_sim;
pub mod weather;
pub mod wind;
pub mod world;
