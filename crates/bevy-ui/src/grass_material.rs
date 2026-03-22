//! Procedural grass material using multi-octave simplex noise.
//!
//! The visual output is driven entirely by a WGSL fragment shader
//! (`assets/shaders/grass.wgsl`) — no texture assets required.
//! Uniforms are updated each frame for time-based wind animation
//! and weather-reactive wetness.

use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

/// Procedural grass ground material. All visual logic lives in the WGSL shader;
/// this struct provides the uniform bindings that the shader reads.
#[derive(Asset, AsBindGroup, TypePath, Debug, Clone)]
pub struct GrassMaterial {
    /// Elapsed time in seconds (drives wind animation).
    #[uniform(0)]
    pub time: f32,

    /// Wind animation speed multiplier.
    #[uniform(0)]
    pub wind_speed: f32,

    /// How much noise variation to apply (0 = flat base color, 1 = full noise).
    #[uniform(0)]
    pub variation_strength: f32,

    /// Wetness factor (0 = dry, 1 = soaked). Darkens and adds slight sheen.
    #[uniform(0)]
    pub wetness: f32,

    /// Wind strength (0 = calm, 1 = strong gusts).
    #[uniform(0)]
    pub wind_strength: f32,

    /// Wind direction X component (normalized).
    #[uniform(0)]
    pub wind_direction_x: f32,

    /// Wind direction Y component (normalized).
    #[uniform(0)]
    pub wind_direction_y: f32,

    /// Padding to align to 16 bytes.
    #[uniform(0)]
    pub _padding: f32,
}

impl Default for GrassMaterial {
    fn default() -> Self {
        Self {
            time: 0.0,
            wind_speed: 0.3,
            variation_strength: 1.0,
            wetness: 0.0,
            wind_strength: 0.3,
            wind_direction_x: 1.0,
            wind_direction_y: 0.0,
            _padding: 0.0,
        }
    }
}

impl Material for GrassMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/grass.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Opaque
    }
}

/// System that updates the `time` uniform on all `GrassMaterial` instances each frame.
pub fn update_grass_time(time: Res<Time>, mut materials: ResMut<Assets<GrassMaterial>>) {
    let elapsed = time.elapsed_secs();
    for (_handle, material) in materials.iter_mut() {
        material.time = elapsed;
    }
}
