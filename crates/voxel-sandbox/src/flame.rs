//! Animated flame rendering for emissive prop voxels.
//!
//! Hot voxels in a `.vox` prop (fire colors) get their own mesh drawn with
//! [`FlameMaterial`]: a custom shader that sways the flame (anchored at the
//! base) and flickers its HDR emissive output for the bloom pass. A warm
//! flickering point light completes the effect.

use bevy::pbr::Material;
use bevy::prelude::*;
use bevy::render::render_resource::AsBindGroup;
use bevy::shader::ShaderRef;

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct FlameMaterial {
    /// x = flame height (m), y = sway amplitude (m), z = sway speed,
    /// w = emissive gain (HDR, >1 blooms).
    #[uniform(0)]
    pub params: Vec4,
}

impl Material for FlameMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/flame.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/flame.wgsl".into()
    }
}

/// Warm light attached to a flame; `base_intensity` is modulated per frame.
#[derive(Component)]
pub struct FlameLight {
    pub base_intensity: f32,
}

pub fn flicker_flame_lights(time: Res<Time>, mut lights: Query<(&mut PointLight, &FlameLight)>) {
    let t = time.elapsed_secs();
    for (mut point_light, flame_light) in &mut lights {
        let flicker = 0.78 + 0.16 * (t * 9.3).sin() + 0.06 * (t * 23.7).sin().abs();
        point_light.intensity = flame_light.base_intensity * flicker;
    }
}
