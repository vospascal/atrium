//! Built-in environment adapters.
//!
//! An adapter is one coherent answer to "what is the sky": a CPU frame evaluator, a WGSL
//! implementation of the four dispatch entry points, and whatever GPU resources that
//! implementation needs, selected together. The renderer picks an adapter once and then
//! talks only to [`EnvironmentProvider`](crate::EnvironmentProvider) and
//! [`EnvironmentGpu`](crate::EnvironmentGpu).
//!
//! # There is exactly one, and that is the honest count
//!
//! [`HillaireEnvironment`] is it. The crate briefly carried a second, `AnalyticProvider`,
//! described as a fallback for when the atmosphere resources cannot be allocated — but it
//! was a field-for-field copy of the Hillaire provider whose `shader_source` returned the
//! *LUT sampler*. Selecting it would have bound a module that reads four textures nobody
//! had populated. Nothing constructed it, so nothing caught that.
//!
//! A real second adapter is a reasonable thing to want; a Quest tier that cannot afford
//! four persistent LUTs is the obvious motivation. What it owes is a
//! `shaders/environment/` implementation of its own — `common.wgsl` plus its own answers
//! to the `dispatch.wgsl` entry points, with no texture reads it cannot back. The
//! appearance layer is already LUT-free and would carry most of it. That is real work, so
//! it is absent rather than faked.

mod hillaire;

pub use hillaire::{HillaireEnvironment, LutConfig};
