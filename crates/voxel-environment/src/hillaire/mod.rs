//! Hillaire/Jolifanto atmosphere implementation.
//!
//! Crate-private by design. The selectable policy that pairs these resources with the
//! renderer contract is [`crate::adapters::HillaireEnvironment`]; everything here — the
//! uniform layout, the four LUTs, the compute passes, the WGSL assembly — sits behind it.
//!
//! One file per concern: [`lut`] owns the sizes and the compute passes, [`resources`] owns
//! the textures, sampler and uniform, [`shaders`] owns the WGSL splicing.

pub mod shaders;

mod lut;
mod resources;

pub use lut::{AtmosphereLutPasses, LutConfig};
pub use resources::AtmosphereBindings;
