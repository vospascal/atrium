//! Renderer-agnostic voxel core.
//!
//! Pure data + CPU logic with no engine dependency: voxel storage (RLE),
//! world generation, the derived-biome classifier, terrain import, and noise.
//! The Bevy adapter (`voxel-sandbox`) and any future backend (e.g. a
//! ray-marched renderer) consume this crate's data without it knowing about
//! them. See `docs/voxel-engine-plan.md` (Stage 0).

pub mod noise;
pub mod terrain_import;
pub mod world;
