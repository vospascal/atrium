//! The `wgpu` seam: what a renderer needs from an environment backend, and nothing more.
//!
//! Four methods, because four is what the renderer actually uses. A pass needs the
//! bind-group layout when it builds its pipeline, the bind group when it encodes, the
//! matching WGSL when it assembles its shader module, and a way to hand over this frame's
//! inputs. Everything else — how many lookup tables exist, what sizes they are, which
//! compute passes populate them, what the uniform layout is — is the backend's business
//! and stops at this boundary.
//!
//! Before this module existed, it did not stop: `AtmosphereBindings`, `AtmosphereUniform`
//! and `LutConfig` were the crate's public surface, so `voxel-rt` named Hillaire's
//! resources in four files and hand-rolled the invalidation diff itself. Swapping the
//! provider would have been a renderer-wide edit.

use crate::api::{EnvironmentInvalidation, EnvironmentRequest};

/// The bind-group index the environment occupies in every consuming shader.
///
/// `shaders/environment/common.wgsl` declares `@group(1)`. A consumer that binds
/// elsewhere gets a `wgpu` validation error rather than a wrong image, but binding
/// through this constant means the two cannot drift silently in the first place.
pub const ENVIRONMENT_BIND_GROUP: u32 = 1;

/// A GPU-resident environment backend.
///
/// Object-safe on purpose: consuming passes take `&dyn EnvironmentGpu` and therefore
/// compile against the contract rather than against Hillaire's resource types.
pub trait EnvironmentGpu {
    /// Layout for [`ENVIRONMENT_BIND_GROUP`], needed at pipeline-construction time.
    fn sample_bind_group_layout(&self) -> &wgpu::BindGroupLayout;

    /// The bind group to set at [`ENVIRONMENT_BIND_GROUP`] when encoding a pass.
    fn sample_bind_group(&self) -> &wgpu::BindGroup;

    /// The WGSL implementing this backend's half of the environment contract — the four
    /// entry points in `shaders/environment/dispatch.wgsl`. A consumer splices this into
    /// its own module; it never defines those functions itself.
    fn shader_source(&self) -> &'static str;

    /// Upload this frame's inputs and encode only the work they invalidated.
    ///
    /// Returns what was considered stale, so a caller can assert the update policy or
    /// report it. An unchanged frame must encode nothing and return
    /// [`EnvironmentInvalidation::default`].
    fn submit(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        request: &EnvironmentRequest,
    ) -> EnvironmentInvalidation;
}
