//! Stable environment contracts shared by providers and renderer adapters.

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

/// A provider evaluates the environment state without exposing its implementation.
/// GPU-specific adapters can use [`EnvironmentProvider::shader_source`] to splice the
/// matching WGSL and can use [`EnvironmentProvider::settings`] to drive LUT updates.
pub trait EnvironmentProvider {
    /// The current CPU-side environment state consumed by lighting and CAGI.
    fn frame(&self) -> EnvironmentFrame;

    /// The provider's matching WGSL implementation, if the renderer needs one.
    fn shader_source(&self) -> &'static str;

    /// The sun/day-night inputs that determine whether cached GPU state is stale.
    fn settings(&self) -> SunSettings;
}
