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
//! - [`EnvironmentProvider`] — the CPU state. Where the sun is, how bright, what the
//!   backdrop palette is. No `wgpu`, so lighting code and headless tests can ask.
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
pub mod gpu;
pub mod scale;
pub mod state;

mod hillaire;

pub use adapters::HillaireEnvironment;
pub use api::{
    EnvironmentFrame, EnvironmentInvalidation, EnvironmentProvider, EnvironmentRequest,
    FroxelCamera,
};
pub use gpu::{EnvironmentGpu, ENVIRONMENT_BIND_GROUP};
pub use scale::{from_kilometers_scale, FROM_KILOMETERS_SCALE};
pub use state::{
    SunSettings, AMBIENT_STRENGTH, GROUND_AMBIENT_COLOR, SKY_AMBIENT_COLOR, SUN_COLOR,
    SUN_INTENSITY,
};
