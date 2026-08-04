//! What a surface *is*: the material table and the pattern layers that vary it.
//!
//! Deliberately ignorant of how any of it is rendered. There is no `wgpu` here, no pass, no
//! shader — only the row layout the GPU will read ([`GpuMaterial`], [`GpuPatternLayer`]) and
//! the CPU reference evaluation that must agree with the shader's.
//!
//! This is the bottom of the voxel stack: `voxel-material` → `voxel-material-graph` →
//! `voxel-rt`. It was extracted first because everything above it needs it, and it needed
//! nothing — the two modules had zero dependencies outside themselves.
//!
//! # Layout
//!
//! - [`material`] — the table, its rows, media, face roles, and the GPU row encoding.
//! - [`pattern`] — pattern layers, generators, frames, blends, and the CPU noise reference.
//! - [`animation_clock`] — the clock a material animates against.
//! - [`world_event`] — the event field a material responds to, and its GPU row.
//!
//! The last two are here because they are *inputs to evaluating a surface*: an oscillator
//! node needs the clock, an event-sensor node needs the field. Both were leaves in
//! `voxel-rt`, so keeping them above the material table would have meant the lowering crate
//! could not exist.
//!
//! Known follow-up: `pattern` still holds every noise generator in one file
//! (`value_noise`, `perlin_noise`, `simplex_noise`, `ridged_noise`, `turbulence`,
//! `worley_distances`, `wave`, `checker`) behind shared helpers. Those are independently
//! selectable implementations, so by this workspace's convention each belongs in its own
//! file with the helpers in a `common`. Not done yet.

pub mod animation_clock;
pub mod material;
pub mod pattern;
pub mod world_event;
