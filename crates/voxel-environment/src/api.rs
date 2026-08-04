//! Stable environment contracts shared by providers and renderer adapters.
//!
//! Nothing in this module mentions `wgpu`. The CPU half of the environment — where the
//! sun is, how bright it is, what the backdrop palette should be — is answerable without
//! a device, and keeping it that way is what makes [`EnvironmentRequest::invalidation_since`]
//! a plain unit test instead of a headless-GPU one. The `wgpu` seam is [`crate::gpu`].

use crate::SunSettings;

use glam::Vec3;

/// CPU result of evaluating the environment at one point in time.
///
/// The field names mirror the existing renderer lighting uniform so adapters can migrate
/// without changing its ABI. `zenith` and `horizon` are linear RGB radiance samples; they
/// are not display-encoded colours.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentFrame {
    pub sun_direction: Vec3,
    pub moon_direction: Vec3,
    pub active_direction: Vec3,
    pub active_color: [f32; 3],
    pub direct_strength: f32,
    pub ambient_strength: f32,
    pub daylight: f32,
    pub moonlight: f32,
    pub zenith: [f32; 3],
    pub horizon: [f32; 3],
    pub star_rotation: f32,
}

/// Camera-relative projection data used by the aerial-perspective froxel LUT.
///
/// The basis vectors already contain the camera's FOV and aspect scaling, so
/// the environment adapter does not depend on the renderer's camera type.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FroxelCamera {
    pub forward: [f32; 3],
    pub right_scaled: [f32; 3],
    pub up_scaled: [f32; 3],
    pub near_world: f32,
    pub far_world: f32,
}

impl Default for FroxelCamera {
    fn default() -> Self {
        Self {
            forward: [1.0, 0.0, 0.0],
            right_scaled: [0.57735026, 0.0, 0.0],
            up_scaled: [0.0, 0.57735026, 0.0],
            near_world: 0.1,
            far_world: 32_000.0,
        }
    }
}

/// Everything an environment adapter needs for one frame, in renderer terms.
///
/// This type is the reason the renderer no longer touches a GPU uniform layout field by
/// field. It used to: `render.rs` copied the previous `AtmosphereUniform`, mutated nine
/// members of it, and then compared five groups of them to decide what was stale — which
/// is exactly the backend policy scattered through the renderer that the crate boundary
/// exists to prevent. Now the renderer states the environment's inputs and the adapter
/// decides what that invalidates.
///
/// The split between `sun_*` and `celestial_*`/`sky_*` is not redundancy: the first pair
/// is the physical light shared with CAGI and the transmittance LUT, the rest is the
/// camera-only appearance layer (see `shaders/environment/appearance.wgsl`).
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EnvironmentRequest {
    /// Direction from a surface toward the dominant light, physical.
    pub sun_direction: [f32; 3],
    /// Top-of-atmosphere illuminance in linear scene units, physical.
    pub sun_illuminance: [f32; 3],
    /// Appearance-layer sun direction (`xyz`) and daylight amount (`w`).
    pub celestial_sun: [f32; 4],
    /// Appearance-layer moon direction (`xyz`) and phase (`w`).
    pub celestial_moon: [f32; 4],
    /// Appearance-layer zenith radiance (`rgb`) and star rotation (`w`).
    pub sky_zenith: [f32; 4],
    /// Appearance-layer horizon radiance (`rgb`) and moonlight amount (`w`).
    pub sky_horizon: [f32; 4],
    /// Camera position in renderer world units.
    pub camera_position: [f32; 3],
    /// Camera basis and depth range for the froxel grid.
    pub camera: FroxelCamera,
}

impl Default for EnvironmentRequest {
    fn default() -> Self {
        Self {
            sun_direction: [0.55, 0.8, 0.35],
            sun_illuminance: [2.2, 2.112, 1.936],
            celestial_sun: [0.55, 0.8, 0.35, 1.0],
            celestial_moon: [-0.55, -0.8, -0.35, 0.85],
            sky_zenith: [0.08, 0.31, 2.55, 0.0],
            sky_horizon: [2.55, 1.37, 0.63, 0.0],
            camera_position: [0.0, 0.0, 0.0],
            camera: FroxelCamera::default(),
        }
    }
}

impl EnvironmentRequest {
    /// What a transition from `previous` to `self` invalidates.
    ///
    /// Pure, and deliberately so — this is the whole update policy, and it is the part
    /// most likely to regress into "recompute everything every frame" without a test
    /// noticing. Standing still must invalidate nothing; turning the head must not
    /// re-integrate the transmittance table.
    pub fn invalidation_since(&self, previous: &Self) -> EnvironmentInvalidation {
        EnvironmentInvalidation {
            // The atmosphere's own parameters (planet radii, world scale) are not
            // per-frame inputs, so no request can invalidate them. Only a first
            // submission does, and the adapter is what knows when that is.
            atmosphere: false,
            sun: self.sun_direction != previous.sun_direction
                || self.sun_illuminance != previous.sun_illuminance
                || self.celestial_sun != previous.celestial_sun
                || self.celestial_moon != previous.celestial_moon
                || self.sky_zenith != previous.sky_zenith
                || self.sky_horizon != previous.sky_horizon,
            camera: self.camera_position != previous.camera_position
                || self.camera != previous.camera,
        }
    }
}

/// Which class of cached environment state this frame's inputs made stale.
///
/// Stated as *what changed*, not as *which table to recompute*: a provider with a
/// different caching strategy maps these three onto its own resources. The Hillaire
/// adapter reads it as "transmittance + multiple scattering" versus "sky view + aerial
/// perspective", because only the latter pair depends on the camera.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct EnvironmentInvalidation {
    /// The atmosphere's fixed parameters — first submission, or a re-configuration.
    pub atmosphere: bool,
    /// The light itself moved or changed brightness.
    pub sun: bool,
    /// The viewer moved or looked elsewhere.
    pub camera: bool,
}

impl EnvironmentInvalidation {
    /// Everything is stale. The correct value for a first submission.
    pub const fn all() -> Self {
        Self {
            atmosphere: true,
            sun: true,
            camera: true,
        }
    }

    /// Whether any GPU work is needed at all. A stationary viewer under a frozen sun
    /// answers `false`, which is the point of the whole type.
    pub const fn any(self) -> bool {
        self.atmosphere || self.sun || self.camera
    }

    /// State independent of the viewer: stale only when the atmosphere or the light is.
    pub const fn view_independent(self) -> bool {
        self.atmosphere || self.sun
    }

    /// State that depends on where the camera is and where it points.
    pub const fn view_dependent(self) -> bool {
        self.atmosphere || self.sun || self.camera
    }

    /// Union of two invalidations, for a caller accumulating across skipped frames.
    pub const fn or(self, other: Self) -> Self {
        Self {
            atmosphere: self.atmosphere || other.atmosphere,
            sun: self.sun || other.sun,
            camera: self.camera || other.camera,
        }
    }
}

/// A provider evaluates the CPU environment state without exposing its implementation.
///
/// This is the half that needs no device. The GPU half — bind groups, WGSL and cache
/// invalidation — is [`crate::gpu::EnvironmentGpu`], and the two are separate traits
/// because a consumer that only needs to know where the sun is (lighting, shadows, a
/// headless test) should not need a `wgpu::Device` to find out.
///
/// This split also removed a lie. `shader_source` used to sit here, which forced every
/// provider to name a WGSL module whether or not it had a GPU path — and the non-LUT
/// provider "satisfied" it by returning the LUT *sampler*, a module that reads four
/// textures it would never have populated. Selecting it would have bound nothing.
pub trait EnvironmentProvider {
    /// The current CPU-side environment state consumed by lighting and CAGI.
    fn frame(&self) -> EnvironmentFrame;

    /// The sun/day-night inputs that determine whether cached GPU state is stale.
    fn settings(&self) -> SunSettings;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_unchanged_frame_invalidates_nothing() {
        let request = EnvironmentRequest::default();
        assert_eq!(
            request.invalidation_since(&request),
            EnvironmentInvalidation::default()
        );
        assert!(!request.invalidation_since(&request).any());
    }

    #[test]
    fn looking_around_leaves_the_view_independent_tables_alone() {
        let previous = EnvironmentRequest::default();
        let moved = EnvironmentRequest {
            camera: FroxelCamera {
                forward: [0.0, 0.0, 1.0],
                ..FroxelCamera::default()
            },
            ..previous
        };
        let invalidation = moved.invalidation_since(&previous);
        assert!(invalidation.camera);
        assert!(!invalidation.sun);
        assert!(invalidation.view_dependent());
        assert!(!invalidation.view_independent());
    }

    #[test]
    fn moving_the_sun_invalidates_both_classes() {
        let previous = EnvironmentRequest::default();
        let sunset = EnvironmentRequest {
            sun_direction: [0.9, 0.1, 0.0],
            ..previous
        };
        let invalidation = sunset.invalidation_since(&previous);
        assert!(invalidation.sun);
        assert!(!invalidation.camera);
        assert!(invalidation.view_independent());
        assert!(invalidation.view_dependent());
    }

    /// The appearance layer is camera-only in the shader, but the renderer feeds its
    /// values through the same uniform the LUT passes read, so a palette change still
    /// has to resubmit. Treating it as a sun change is the honest answer, not a
    /// conservative one.
    #[test]
    fn appearance_only_changes_still_count_as_sun_changes() {
        let previous = EnvironmentRequest::default();
        let twilight = EnvironmentRequest {
            sky_horizon: [3.0, 0.55, 0.12, 0.0],
            ..previous
        };
        assert!(twilight.invalidation_since(&previous).sun);
    }

    #[test]
    fn first_submission_invalidates_everything() {
        let all = EnvironmentInvalidation::all();
        assert!(all.any());
        assert!(all.view_independent());
        assert!(all.view_dependent());
        assert_eq!(all.or(EnvironmentInvalidation::default()), all);
    }
}
