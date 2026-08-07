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
// The cloud tables' sizes. Re-exported through the crate root, not because the resources are
// public — they are not — but because these three numbers have to be checkable against the world's
// scale from outside, and getting that relationship wrong is invisible in the image.
pub use resources::{
    CLOUD_NDF_EDGE, CLOUD_NDF_EXTENT_WORLD, CLOUD_NOISE_EDGE, CLOUD_NOISE_MIP_LEVELS,
    CLOUD_SHADOW_EDGE, CLOUD_SHADOW_EXTENT_WORLD,
};
