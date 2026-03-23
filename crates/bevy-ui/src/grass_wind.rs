//! Grass wind animation via ExtendedMaterial vertex shader.
//!
//! Extends StandardMaterial with a custom vertex shader that bends
//! grass blades based on time, wind strength, and world position.
//! The fragment shader is untouched — full PBR lighting is preserved.

use bevy::pbr::{ExtendedMaterial, MaterialExtension};
use bevy::prelude::*;
use bevy::render::render_resource::*;
use bevy::shader::ShaderRef;

use crate::weather::WeatherState;

/// The wind uniform data passed to the vertex shader.
#[derive(Clone, Debug, Default, ShaderType, Reflect)]
pub struct GrassWindUniforms {
    pub time: f32,
    pub wind_strength: f32,
    pub wind_direction_x: f32,
    pub wind_direction_z: f32,
}

/// MaterialExtension that adds wind vertex displacement to StandardMaterial.
#[derive(Asset, AsBindGroup, Reflect, Debug, Clone, Default)]
pub struct GrassWindExtension {
    #[uniform(100)]
    pub wind: GrassWindUniforms,
}

impl MaterialExtension for GrassWindExtension {
    fn vertex_shader() -> ShaderRef {
        "shaders/grass_wind.wgsl".into()
    }
}

/// Combined type alias for convenience.
pub type GrassWindMaterial = ExtendedMaterial<StandardMaterial, GrassWindExtension>;

/// System: update wind uniforms each frame from weather state.
pub fn update_grass_wind(
    weather: Res<WeatherState>,
    time: Res<Time>,
    mut materials: ResMut<Assets<GrassWindMaterial>>,
) {
    let elapsed = time.elapsed_secs();
    for (_id, material) in materials.iter_mut() {
        material.extension.wind.time = elapsed;
        material.extension.wind.wind_strength = weather.wind_strength;
        material.extension.wind.wind_direction_x = weather.wind_direction.x;
        material.extension.wind.wind_direction_z = weather.wind_direction.y;
    }
}

