//! Shared Hillaire/Jolifanto environment lighting for the voxel renderer.
//!
//! This crate is intentionally structured as a stable facade over interchangeable
//! providers. The renderer depends on [`EnvironmentProvider`], while the analytic and
//! Hillaire implementations can evolve independently behind this API.

pub mod analytic;
pub mod api;
pub mod gpu;
pub mod hillaire;
pub mod scale;
pub mod state;

pub use analytic::AnalyticProvider;
pub use api::{EnvironmentFrame, EnvironmentProvider, FroxelCamera};
pub use hillaire::{
    AtmosphereBindings, AtmosphereLutPasses, AtmosphereResources, AtmosphereUniform,
    HillaireProvider, LutConfig, LutKind, LutUpdate, ATMOSPHERE_BIND_GROUP,
};
pub use scale::{from_kilometers_scale, FROM_KILOMETERS_SCALE};
pub use state::{
    SunSettings, AMBIENT_STRENGTH, GROUND_AMBIENT_COLOR, SKY_AMBIENT_COLOR, SUN_COLOR,
    SUN_INTENSITY,
};

/// Matching WGSL helpers for the renderer's `Lighting` and `Camera` uniforms.
pub const WGSL: &str = include_str!("../shaders/environment.wgsl");
