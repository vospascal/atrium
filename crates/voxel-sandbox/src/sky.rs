//! Procedural sky dome.
//!
//! A big inward-facing sphere follows the camera and is drawn with a fully
//! custom shader: atmospheric gradient with a scattering glow around the
//! sun, a raymarched volumetric cloud layer (coverage / type / wind from
//! [`WeatherState`]), an HDR sun disc, a moon with phases, and a hash-based
//! star field that rotates with the earth. Everything is driven by
//! [`CelestialState`] so the sky always agrees with the directional light.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::Material;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::day_night::CelestialState;
use crate::weather::WeatherState;

/// Must stay inside the camera far plane (default 1000).
const SKY_DOME_RADIUS: f32 = 780.0;

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct SkyUniform {
    /// xyz = direction to the sun, w = daylight 0..1.
    pub sun_direction: Vec4,
    /// xyz = direction to the moon, w = moon phase 0..1 (0.5 = full).
    pub moon_direction: Vec4,
    /// rgb = current celestial light color (linear), w = star rotation rad.
    pub light_color: Vec4,
    /// rgb = zenith color (linear), w = effective cloud coverage 0..1.
    pub zenith_color: Vec4,
    /// rgb = horizon color (linear), w = cloud type 0..2.
    pub horizon_color: Vec4,
    /// xy = cloud scroll (m), z = moonlight 0..1, w = fog 0..1.
    pub scroll: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct SkyMaterial {
    #[uniform(0)]
    pub sky: SkyUniform,
}

impl Material for SkyMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/sky.wgsl".into()
    }
}

/// Marker for the dome entity.
#[derive(Component)]
pub struct SkyDome;

pub fn spawn_sky(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<SkyMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(build_dome_mesh())),
        MeshMaterial3d(materials.add(SkyMaterial {
            sky: SkyUniform {
                sun_direction: Vec4::new(0.0, 1.0, 0.0, 1.0),
                moon_direction: Vec4::new(0.0, -1.0, 0.0, 0.5),
                light_color: Vec4::ONE,
                zenith_color: Vec4::new(0.09, 0.22, 0.57, 0.4),
                horizon_color: Vec4::new(0.60, 0.64, 0.60, 1.0),
                scroll: Vec4::ZERO,
            },
        })),
        Transform::default(),
        bevy::light::NotShadowCaster,
        // The sky must exist in water reflections too.
        crate::water::reflective_layers(),
        SkyDome,
    ));
}

/// UV sphere with both windings emitted, so the inside is always visible
/// regardless of cull mode (the outward copy is back-face culled).
fn build_dome_mesh() -> Mesh {
    const RINGS: u32 = 20;
    const SEGMENTS: u32 = 36;

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
            positions.push((direction * SKY_DOME_RADIUS).to_array());
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

/// Keep the dome centered on the camera and its uniforms in sync with the
/// day/night cycle and the weather.
#[allow(clippy::type_complexity)]
pub fn update_sky(
    celestial: Res<CelestialState>,
    weather: Res<WeatherState>,
    mut materials: ResMut<Assets<SkyMaterial>>,
    cycle: Res<crate::day_night::DayNightCycle>,
    mut dome_query: Query<(&mut Transform, &MeshMaterial3d<SkyMaterial>), With<SkyDome>>,
    camera_query: Query<
        &Transform,
        (
            With<Camera3d>,
            Without<SkyDome>,
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
    material.sky.sun_direction = celestial.sun_direction.extend(celestial.daylight);
    material.sky.moon_direction = celestial.moon_direction.extend(cycle.moon_phase);
    material.sky.light_color = celestial.light_color.extend(celestial.star_rotation);
    material.sky.zenith_color = celestial
        .zenith_color
        .extend(weather.effective_cloud_coverage());
    material.sky.horizon_color = celestial
        .horizon_color
        .extend(weather.current.cloud_type.clamp(0.0, 2.0));
    material.sky.scroll = Vec4::new(
        weather.cloud_scroll.x,
        weather.cloud_scroll.y,
        celestial.moonlight,
        weather.current.fog,
    );
}
