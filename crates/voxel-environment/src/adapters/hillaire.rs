//! The Hillaire/Jolifanto adapter: the shipped environment.
//!
//! This file is the *policy* — construct the resources, decide what a frame invalidates,
//! encode the minimum. The physics and the GPU resources live in [`crate::hillaire`], one
//! file per concern (`lut.rs`, `resources.rs`, `shaders.rs`), and none of it is reachable
//! from outside the crate.

use crate::api::{EnvironmentInvalidation, EnvironmentRequest};
use crate::gpu::EnvironmentGpu;
use crate::hillaire::{shaders, AtmosphereBindings, AtmosphereLutPasses};

pub use crate::hillaire::LutConfig;

/// Hillaire's four-LUT atmosphere, wired to the renderer contract.
///
/// Holds the resources and the passes *together with* the last submitted frame, which is
/// the point: the "what changed" question is answered here, once, instead of at whatever
/// call site happens to own the previous uniform.
pub struct HillaireEnvironment {
    bindings: AtmosphereBindings,
    lut_passes: AtmosphereLutPasses,
    /// `None` until the first [`EnvironmentGpu::submit`] — which is exactly the condition
    /// that makes the view-independent tables stale, so it doubles as the first-frame flag.
    submitted: Option<EnvironmentRequest>,
}

impl HillaireEnvironment {
    /// This adapter's WGSL half, as a const.
    ///
    /// [`EnvironmentGpu::shader_source`] is the same string and is what runtime code should
    /// use. This exists for the one thing a trait method cannot do: appear inside `concat!`,
    /// which a consumer assembling its shader module at compile time needs. Naming the
    /// adapter there is the honest cost — a shader spliced before any device exists really
    /// is committed to one implementation, and a provider-neutral `WGSL` const would have
    /// hidden that rather than removed it.
    pub const WGSL: &'static str = shaders::WGSL;

    /// Allocate the atmosphere at Jolifanto's starting LUT sizes.
    pub fn new(device: &wgpu::Device) -> Self {
        Self::with_lut_config(device, LutConfig::default())
    }

    /// Allocate the atmosphere at explicit LUT sizes. The sizes are a cost/quality lever,
    /// so a mobile tier can shrink them without the renderer knowing they exist.
    pub fn with_lut_config(device: &wgpu::Device, lut_config: LutConfig) -> Self {
        let bindings = AtmosphereBindings::new(device, lut_config);
        let lut_passes = AtmosphereLutPasses::new(device, &bindings);
        Self {
            bindings,
            lut_passes,
            submitted: None,
        }
    }

    // There is deliberately NO sun state on this adapter, and no `settings`/`settings_mut`/
    // `frame` accessors. It held a `SunSettings` that nothing in the workspace ever wrote, so
    // every reader got a default noon sun forever — which is what froze the cloud deck's ground
    // bounce. The sun arrives per frame on `EnvironmentRequest` and nowhere else; a second copy
    // to keep in sync is a bug waiting to happen, not a convenience.

    /// How many times the atmosphere uniform has been rewritten. A counter that does *not*
    /// advance while the viewer stands still under a frozen sun, which is the cheapest way
    /// to see the update policy working in a running app.
    pub fn upload_count(&self) -> u64 {
        self.bindings.resources.generation
    }
}

impl EnvironmentGpu for HillaireEnvironment {
    fn sample_bind_group_layout(&self) -> &wgpu::BindGroupLayout {
        self.bindings.sample_bind_group_layout()
    }

    fn sample_bind_group(&self) -> &wgpu::BindGroup {
        self.bindings.sample_bind_group()
    }

    fn shader_source(&self) -> &'static str {
        Self::WGSL
    }

    fn submit(
        &mut self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        request: &EnvironmentRequest,
    ) -> EnvironmentInvalidation {
        let invalidation = match self.submitted {
            Some(previous) => request.invalidation_since(&previous),
            None => EnvironmentInvalidation::all(),
        };
        if !invalidation.any() {
            return invalidation;
        }

        let mut uniform = self.bindings.uniform;
        uniform.apply_request(request);
        self.bindings.update_uniform(queue, uniform);
        self.lut_passes.encode(encoder, invalidation);
        self.submitted = Some(*request);
        invalidation
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The LUT sizes the crate promises in its README. Jolifanto's starting values, and
    /// the numbers every cost estimate in the docs is quoted against.
    #[test]
    fn default_lut_config_matches_jolifantos_starting_sizes() {
        let config = LutConfig::default();
        assert_eq!(config.transmittance, [256, 64]);
        assert_eq!(config.multiple_scattering, [32, 32]);
        assert_eq!(config.sky_view, [192, 108]);
        assert_eq!(config.aerial_perspective, [32, 32, 32]);
    }

    /// The adapter must splice a module that answers all four dispatch entry points and
    /// declares the bindings they read — otherwise a consumer's shader fails to compile
    /// with an error that points at the consumer, not here.
    #[test]
    fn spliced_wgsl_is_self_contained_for_the_dispatch_contract() {
        let source = shaders::WGSL;
        for entry in [
            "fn sky_color(",
            "fn sky_color_at_distance(",
            "fn ambient_light(",
            "fn environment_diffuse_radiance(",
        ] {
            assert!(source.contains(entry), "missing dispatch entry {entry}");
        }
        assert!(source.contains("struct AtmosphereUniform"));
        assert!(source.contains(&format!(
            "@group({}) @binding(0)",
            crate::gpu::ENVIRONMENT_BIND_GROUP
        )));
    }

    /// The fragments must appear in dependency order. WGSL allows module-scope
    /// declarations in any order, so this is not a compile requirement — it is a
    /// readability one, and it is the property that makes the aggregate reviewable.
    #[test]
    fn fragments_are_spliced_common_first_dispatch_last() {
        let source = shaders::WGSL;
        let position = |needle: &str| source.find(needle).expect(needle);
        assert!(position("struct AtmosphereUniform") < position("fn environment_hillaire_sky("));
        assert!(
            position("fn environment_hillaire_sky(") < position("fn environment_sky_radiance(")
        );
        assert!(position("fn environment_sky_radiance(") < position("fn sky_color("));
    }
}
