//! Volumetric fog sea hiding the world edge.
//!
//! The plateau floats in a raymarched sea of fog: a world-space density
//! field (zero inside the rim, dense beyond it, with a noisy billowing
//! top surface) is marched per-pixel in a fullscreen dome pass. The march
//! is clamped to the scene depth from the depth prepass, so cliffs and
//! trees sit *in* the fog — there is no curtain geometry and no visible
//! edge or seam from any angle. Wind drifts the fog; the day/night cycle
//! tints it, so it dims to darkness at night instead of glowing.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::Material;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::day_night::CelestialState;
use crate::weather::WeatherState;

/// Fog density rises from zero at this radius (m from world center)…
const FOG_RADIUS_START: f32 = 42.0;
/// …to full a little beyond the rim.
const FOG_RADIUS_FULL: f32 = 53.0;
/// Mean height of the fog ring's billowing top surface (m) — just below
/// the island's rim lip, wrapping the sculpted underside.
const FOG_TOP_METERS: f32 = 7.0;
/// The fog fills everything below the top, down past the island's belly.
const FOG_BOTTOM_METERS: f32 = -40.0;

/// Tiny proxy sphere around the camera: always in front of everything, so
/// the depth test never culls it — occlusion is handled inside the shader
/// by clamping the march to the prepass depth. (A big dome proxy fails:
/// its far surface sits *behind* terrain and gets depth-culled, taking
/// the fog in front of that terrain with it.)
const FOG_DOME_RADIUS: f32 = 0.5;

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct FogSeaUniform {
    /// rgb = fog color (linear), w = master opacity.
    pub color: Vec4,
    /// xy = wind scroll offset (m), z = noise scale, w = daylight.
    pub drift: Vec4,
    /// x = radius where fog starts, y = radius of full fog,
    /// z = mean top height, w = bottom of the marched slab.
    pub band: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct FogSeaMaterial {
    #[uniform(0)]
    pub fog: FogSeaUniform,
}

impl Material for FogSeaMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/fog_ring.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

/// Marker for the fog proxy dome.
#[derive(Component)]
pub struct FogSea;

pub fn spawn_fog_ring(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FogSeaMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(build_proxy_dome())),
        MeshMaterial3d(materials.add(FogSeaMaterial {
            fog: FogSeaUniform {
                color: Vec4::new(0.6, 0.62, 0.63, 1.0),
                drift: Vec4::new(0.0, 0.0, 0.055, 1.0),
                band: Vec4::new(
                    FOG_RADIUS_START,
                    FOG_RADIUS_FULL,
                    FOG_TOP_METERS,
                    FOG_BOTTOM_METERS,
                ),
            },
        })),
        Transform::default(),
        bevy::light::NotShadowCaster,
        FogSea,
    ));
}

/// Low-poly sphere around the camera; the fragment shader does all the
/// work, this just guarantees every pixel runs it. Both windings so the
/// inside is always visible (camera never leaves it).
fn build_proxy_dome() -> Mesh {
    const RINGS: u32 = 12;
    const SEGMENTS: u32 = 18;

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    for ring in 0..=RINGS {
        let theta = std::f32::consts::PI * ring as f32 / RINGS as f32;
        for segment in 0..=SEGMENTS {
            let phi = std::f32::consts::TAU * segment as f32 / SEGMENTS as f32;
            let direction = Vec3::new(
                theta.sin() * phi.cos(),
                theta.cos(),
                theta.sin() * phi.sin(),
            );
            positions.push((direction * FOG_DOME_RADIUS).to_array());
            normals.push((-direction).to_array());
            uvs.push([segment as f32 / SEGMENTS as f32, ring as f32 / RINGS as f32]);
        }
    }
    let mut indices = Vec::new();
    let stride = SEGMENTS + 1;
    for ring in 0..RINGS {
        for segment in 0..SEGMENTS {
            let a = ring * stride + segment;
            let b = a + 1;
            let c = a + stride;
            let d = c + 1;
            indices.extend([a, b, c, b, d, c]);
            indices.extend([a, c, b, b, c, d]);
        }
    }
    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, uvs)
    .with_inserted_indices(Indices::U32(indices))
}

/// Follow the camera and keep the fog tinted by the sky and drifting
/// with the wind.
pub fn update_fog_ring(
    celestial: Res<CelestialState>,
    weather: Res<WeatherState>,
    mut materials: ResMut<Assets<FogSeaMaterial>>,
    mut dome_query: Query<(&mut Transform, &MeshMaterial3d<FogSeaMaterial>), With<FogSea>>,
    camera_query: Query<
        &Transform,
        (
            With<Camera3d>,
            Without<FogSea>,
            Without<crate::water::ReflectionCamera>,
        ),
    >,
) {
    let Ok((mut dome_transform, material_handle)) = dome_query.single_mut() else {
        return;
    };
    if let Ok(camera_transform) = camera_query.single() {
        dome_transform.translation = camera_transform.translation;
    }
    let Some(material) = materials.get_mut(&material_handle.0) else {
        return;
    };
    // Sunlit cloud bank by day, sinking to the sky tone at night.
    let color = celestial
        .horizon_color
        .lerp(Vec3::ONE, 0.32 * celestial.daylight);
    let scroll = weather.cloud_scroll * 0.05;
    material.fog.color = color.extend(1.0);
    material.fog.drift.x = scroll.x;
    material.fog.drift.y = scroll.y;
    material.fog.drift.w = celestial.daylight;
}
