//! Shared environment lighting for the voxel renderer.
//!
//! One environment, two consumers: the sky a camera sees and the sky that illuminates a
//! surface must be the same thing. That invariant is why this is a crate and not a module
//! in the renderer.
//!
//! # The facade
//!
//! Consumers depend on two traits and one request type:
//!
//! - [`EnvironmentGpu`] — the GPU state. A bind-group layout, a bind group, the matching
//!   WGSL, and one `submit` per frame.
//! - [`EnvironmentRequest`] — what the renderer states each frame. The adapter decides
//!   what that invalidated; the renderer never diffs GPU uniform fields itself.
//!
//! [`adapters::HillaireEnvironment`] is the shipped implementation of both traits. Its
//! resources, uniform layout, LUT sizes and compute passes are deliberately *not* exported
//! — that is what makes the provider replaceable rather than merely abstract.

pub mod adapters;
pub mod api;
pub mod clouds;
pub mod gpu;
pub mod scale;
pub mod state;

mod hillaire;

pub use adapters::HillaireEnvironment;
pub use api::{EnvironmentFrame, EnvironmentInvalidation, EnvironmentRequest, FroxelCamera};
pub use clouds::{CloudRequest, CloudSettings};
pub use gpu::{EnvironmentGpu, ENVIRONMENT_BIND_GROUP};
/// The cloud shadow map's resolution and world extent.
///
/// Exported because the relationship that matters — metres per texel versus the size of a world
/// voxel — can only be checked by a crate that sees both this and `voxel-core`'s world dimensions,
/// and this crate deliberately does not depend on the world.
pub use hillaire::{
    CLOUD_NDF_EDGE, CLOUD_NDF_EXTENT_WORLD, CLOUD_NOISE_EDGE, CLOUD_NOISE_MIP_LEVELS,
    CLOUD_SHADOW_EDGE, CLOUD_SHADOW_EXTENT_WORLD,
};
pub use scale::{from_kilometers_scale, FROM_KILOMETERS_SCALE};
pub use state::{
    SunSettings, AMBIENT_STRENGTH, GROUND_AMBIENT_COLOR, SKY_AMBIENT_COLOR, SUN_COLOR,
    SUN_INTENSITY,
};
