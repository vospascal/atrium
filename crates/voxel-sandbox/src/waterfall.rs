//! Waterfalls where the river spills off the island's rim.
//!
//! For every river-rim exit the world reports, a curved ribbon mesh hangs
//! off the edge, drawn with an animated shader: streaks of foam race down
//! translucent water, brightest at the spill lip, dissolving into the fog
//! sea below. Spawned with the world (tagged `WorldMesh`), so `R`
//! regenerates them with the terrain.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::Material;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use voxel_core::world::{RiverExit, VOXEL_SIZE, WATER_LEVEL};

/// The ribbon dissolves into the fog sea around this height (m).
const WATERFALL_BOTTOM_METERS: f32 = 3.0;

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct WaterfallUniform {
    /// x = flow speed, y = streak density, z = per-fall seed,
    /// w = scene light factor (day/night, set per frame).
    pub params: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct WaterfallMaterial {
    #[uniform(0)]
    pub water: WaterfallUniform,
}

impl Material for WaterfallMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/waterfall.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

/// Spawn one ribbon per river-rim exit (tagged as world meshes, so they
/// regenerate with the terrain). Returns how many were placed.
pub fn spawn_waterfalls(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<WaterfallMaterial>,
    exits: &[RiverExit],
) -> usize {
    for (index, exit) in exits.iter().enumerate() {
        commands.spawn((
            Mesh3d(meshes.add(build_ribbon_mesh(exit))),
            MeshMaterial3d(materials.add(WaterfallMaterial {
                water: WaterfallUniform {
                    params: Vec4::new(2.4, 7.0, index as f32 * 17.3, 1.0),
                },
            })),
            Transform::default(),
            bevy::light::NotShadowCaster,
            crate::water::reflective_layers(),
            crate::WorldMesh,
        ));
    }
    exits.len()
}

/// Waterfalls are unlit — dim them with the sky so they don't glow at
/// night (moonlight keeps them faintly silver).
pub fn update_waterfalls(
    celestial: Res<crate::day_night::CelestialState>,
    mut materials: ResMut<Assets<WaterfallMaterial>>,
) {
    let light = 0.06 + 0.94 * celestial.daylight.max(celestial.moonlight * 0.35);
    for (_, material) in materials.iter_mut() {
        material.water.params.w = light;
    }
}

/// A vertical ribbon at the rim exit: bows slightly outward at the lip
/// (water spilling over), then falls straight down into the fog. Both
/// windings, so it reads from on-island and from the orbit side view.
fn build_ribbon_mesh(exit: &RiverExit) -> Mesh {
    const COLUMNS: u32 = 8;
    const ROWS: u32 = 10;

    let top_y = (WATER_LEVEL as f32 + 1.0) * VOXEL_SIZE;
    let outward = Vec2::new(exit.outward_x, exit.outward_z);
    let tangent = Vec2::new(-outward.y, outward.x);
    let center = Vec2::new(exit.center_x, exit.center_z);

    let mut positions = Vec::new();
    let mut normals = Vec::new();
    let mut uvs = Vec::new();
    for row in 0..=ROWS {
        let down = row as f32 / ROWS as f32;
        let y = top_y + (WATERFALL_BOTTOM_METERS - top_y) * down;
        // Spill bow: pushes out fast near the lip, then hangs vertically.
        let bow = 0.30 + 0.85 * (down * 2.4).min(1.0);
        for column in 0..=COLUMNS {
            let across = column as f32 / COLUMNS as f32 - 0.5;
            let planar = center + tangent * (across * exit.width) + outward * bow;
            positions.push([planar.x, y, planar.y]);
            normals.push([outward.x, 0.0, outward.y]);
            // u in meters so streak density is width-independent.
            uvs.push([(across + 0.5) * exit.width, down]);
        }
    }

    let mut indices = Vec::new();
    let stride = COLUMNS + 1;
    for row in 0..ROWS {
        for column in 0..COLUMNS {
            let a = row * stride + column;
            let b = a + 1;
            let c = a + stride;
            let d = c + 1;
            indices.extend([a, c, b, b, c, d]);
            indices.extend([a, b, c, b, d, c]);
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
