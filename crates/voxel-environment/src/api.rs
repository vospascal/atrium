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
