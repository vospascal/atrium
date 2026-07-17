//! Campfire v2: procedural stones, real shader fire.
//!
//! Unlike the `.vox` campfire prop (whose flame is voxels), this one is
//! built in code: a ring of voxel stones around two crossed charred logs
//! (meshed through the same prop pipeline, so they sit native), topped
//! with a billboarded procedural fire — noise-distorted flame body with a
//! fire color ramp, HDR core for bloom, and rising embers — plus the
//! usual flickering warm point light.
//!
//! Press `C` to place one where you're looking; `Shift+C` clears them.
//! `VOXEL_CAMPFIRES="x,z;x,z"` pre-spawns (screenshots / future scenes).

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::Material;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::flame::FlameLight;
use crate::noise::{hash_3d, hash_to_unit};
use crate::vox_import::{build_prop_meshes, VoxModel};
use crate::ViewMode;

/// Rising embers built into the fire mesh.
const EMBER_COUNT: usize = 14;

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct FireUniform {
    /// x = flame width (m), y = flame height (m), z = HDR gain,
    /// w = flame base height above the prop origin (m).
    pub params: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct FireMaterial {
    #[uniform(0)]
    pub fire: FireUniform,
}

impl Material for FireMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/fire.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/fire.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

/// Marker for a placed campfire (all of its parts carry it).
#[derive(Component)]
pub struct Campfire;

/// Shared meshes + materials for every placed campfire.
#[derive(Resource)]
pub struct CampfireAssets {
    stones_mesh: Handle<Mesh>,
    fire_mesh: Handle<Mesh>,
    stone_material: Handle<StandardMaterial>,
    fire_material: Handle<FireMaterial>,
}

pub fn setup_campfires(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut standard_materials: ResMut<Assets<StandardMaterial>>,
    mut fire_materials: ResMut<Assets<FireMaterial>>,
) {
    let stones_model = build_stone_ring_model();
    let stones_mesh = build_prop_meshes(&stones_model, 71)
        .solid
        .expect("stone ring model is never empty");
    commands.insert_resource(CampfireAssets {
        stones_mesh: meshes.add(stones_mesh),
        fire_mesh: meshes.add(build_fire_mesh()),
        stone_material: standard_materials.add(StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.95,
            ..default()
        }),
        fire_material: fire_materials.add(FireMaterial {
            fire: FireUniform {
                params: Vec4::new(0.62, 0.85, 4.5, 0.26),
            },
        }),
    });
}

/// Voxel stone ring + two crossed charred logs, engine axes, base at y 0.
/// Everything through the prop mesher, so it gets the terrain's AO look.
fn build_stone_ring_model() -> VoxModel {
    const SIZE: i32 = 20;
    const HEIGHT: i32 = 7;
    let mut cells: Vec<Option<[f32; 4]>> = vec![None; (SIZE * HEIGHT * SIZE) as usize];
    let center = (SIZE as f32 - 1.0) / 2.0;
    let mut set = |x: i32, y: i32, z: i32, color: [f32; 4]| {
        if x >= 0 && y >= 0 && z >= 0 && x < SIZE && y < HEIGHT && z < SIZE {
            cells[((y * SIZE + z) * SIZE + x) as usize] = Some(color);
        }
    };

    // Ring of stones: blobs of gray, each with its own tint and size.
    const STONE_COUNT: i32 = 10;
    for stone in 0..STONE_COUNT {
        let unit = |salt: i32| hash_to_unit(hash_3d(stone, salt, 3, 909));
        let angle =
            stone as f32 / STONE_COUNT as f32 * std::f32::consts::TAU + (unit(0) - 0.5) * 0.35;
        let ring_radius = 7.2 + (unit(1) - 0.5) * 1.2;
        let stone_x = center + angle.cos() * ring_radius;
        let stone_z = center + angle.sin() * ring_radius;
        let half_x = 1.3 + unit(2) * 0.9;
        let half_y = 1.1 + unit(3) * 0.9;
        let half_z = 1.3 + unit(4) * 0.9;
        // Cells hold LINEAR colors: ~0.13-0.23 linear ≈ mid-gray in sRGB.
        let gray = 0.13 + unit(5) * 0.10;
        let color = [gray, gray * 1.02, gray * 1.06, 1.0];

        for y in 0..HEIGHT {
            for z in 0..SIZE {
                for x in 0..SIZE {
                    let dx = (x as f32 - stone_x) / half_x;
                    let dy = y as f32 / half_y;
                    let dz = (z as f32 - stone_z) / half_z;
                    let jitter = (hash_to_unit(hash_3d(x, y + stone * 8, z, 911)) - 0.5) * 0.35;
                    if dx * dx + dy * dy + dz * dz + jitter < 1.0 {
                        set(x, y, z, color);
                    }
                }
            }
        }
    }

    // Two crossed logs through the middle, charred toward the center
    // (linear space: dark bark brown and near-black char).
    let log_brown = [0.095, 0.042, 0.015, 1.0];
    let charred = [0.012, 0.010, 0.009, 1.0];
    let center_voxel = center.round() as i32;
    for along in -5_i32..=5 {
        let color = if along.abs() <= 2 { charred } else { log_brown };
        for thickness in 0..2 {
            // One log along x at ground level, one along z stacked above.
            set(center_voxel + along, thickness, center_voxel + 1, color);
            set(center_voxel + along, thickness, center_voxel, color);
            set(center_voxel, 1 + thickness, center_voxel + along, color);
            set(center_voxel + 1, 1 + thickness, center_voxel + along, color);
        }
    }

    VoxModel::from_cells(SIZE, HEIGHT, SIZE, cells)
}

/// One big flame quad (sentinel `uv_b.x = -1`) plus small ember quads with
/// per-ember seeds. Both windings so billboards never vanish.
fn build_fire_mesh() -> Mesh {
    let mut positions: Vec<[f32; 3]> = Vec::new();
    let mut normals: Vec<[f32; 3]> = Vec::new();
    let mut corner_uvs: Vec<[f32; 2]> = Vec::new();
    let mut seed_uvs: Vec<[f32; 2]> = Vec::new();
    let mut indices: Vec<u32> = Vec::new();

    let push_quad = |seeds: [f32; 2],
                     positions: &mut Vec<[f32; 3]>,
                     normals: &mut Vec<[f32; 3]>,
                     corner_uvs: &mut Vec<[f32; 2]>,
                     seed_uvs: &mut Vec<[f32; 2]>,
                     indices: &mut Vec<u32>| {
        let first_vertex = positions.len() as u32;
        for corner in [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]] {
            positions.push([0.0, 0.0, 0.0]);
            normals.push([0.0, 1.0, 0.0]);
            corner_uvs.push(corner);
            seed_uvs.push(seeds);
        }
        for triangle in [[0, 1, 2], [0, 2, 3], [0, 2, 1], [0, 3, 2]] {
            indices.extend(triangle.map(|offset| first_vertex + offset));
        }
    };

    push_quad(
        [-1.0, 0.0],
        &mut positions,
        &mut normals,
        &mut corner_uvs,
        &mut seed_uvs,
        &mut indices,
    );
    for ember in 0..EMBER_COUNT as i32 {
        let seeds = [
            hash_to_unit(hash_3d(ember, 5, 17, 4_040)),
            hash_to_unit(hash_3d(ember, 9, 23, 4_040)),
        ];
        push_quad(
            seeds,
            &mut positions,
            &mut normals,
            &mut corner_uvs,
            &mut seed_uvs,
            &mut indices,
        );
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, corner_uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, seed_uvs)
    .with_inserted_indices(Indices::U32(indices))
}

fn spawn_campfire_at(commands: &mut Commands, assets: &CampfireAssets, position: Vec3) {
    commands.spawn((
        Mesh3d(assets.stones_mesh.clone()),
        MeshMaterial3d(assets.stone_material.clone()),
        Transform::from_translation(position),
        crate::water::reflective_layers(),
        Campfire,
    ));
    commands.spawn((
        Mesh3d(assets.fire_mesh.clone()),
        MeshMaterial3d(assets.fire_material.clone()),
        Transform::from_translation(position),
        bevy::light::NotShadowCaster,
        crate::water::reflective_layers(),
        // The animated flame is sub-pixel from afar (the point light still
        // glows) — fade the quads out past ~80 m.
        bevy::camera::visibility::VisibilityRange {
            start_margin: 0.0..0.0,
            end_margin: 80.0..100.0,
            use_aabb: false,
        },
        Campfire,
    ));
    commands.spawn((
        PointLight {
            color: Color::srgb(1.0, 0.62, 0.28),
            intensity: 60_000.0,
            range: 12.0,
            shadows_enabled: false,
            ..default()
        },
        FlameLight {
            base_intensity: 60_000.0,
        },
        Transform::from_translation(position + Vec3::Y * 0.6),
        crate::water::reflective_layers(),
        Campfire,
    ));
}

/// `C` places a campfire where you're looking; `Shift+C` clears them all.
#[allow(clippy::too_many_arguments)]
pub fn campfire_controls(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    assets: Option<Res<CampfireAssets>>,
    view_mode: Res<ViewMode>,
    orbit_state: Res<crate::OrbitCameraState>,
    ground_heights: Option<Res<crate::GroundHeights>>,
    camera_query: Query<&Transform, (With<Camera3d>, Without<crate::water::ReflectionCamera>)>,
    campfires: Query<Entity, With<Campfire>>,
) {
    if !keyboard.just_pressed(KeyCode::KeyC) {
        return;
    }
    if keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight) {
        for campfire in &campfires {
            commands.entity(campfire).despawn();
        }
        return;
    }
    let Some(assets) = assets else {
        return;
    };

    let anchor = match *view_mode {
        ViewMode::FirstPerson => {
            let Ok(camera_transform) = camera_query.single() else {
                return;
            };
            let ahead = camera_transform.translation + camera_transform.forward() * 4.0;
            Vec2::new(ahead.x, ahead.z)
        }
        ViewMode::Orbit => Vec2::new(orbit_state.focus.x, orbit_state.focus.z),
    };
    let ground = ground_heights
        .map(|heights| heights.ground_at(anchor.x, anchor.y))
        .unwrap_or(10.0);
    spawn_campfire_at(
        &mut commands,
        &assets,
        Vec3::new(anchor.x, ground, anchor.y),
    );
}

/// Pre-spawn campfires from `VOXEL_CAMPFIRES="x,z;x,z"` once terrain
/// heights exist.
pub fn spawn_env_campfires(
    mut commands: Commands,
    mut done: Local<bool>,
    assets: Option<Res<CampfireAssets>>,
    ground_heights: Option<Res<crate::GroundHeights>>,
) {
    if *done {
        return;
    }
    let (Some(assets), Some(ground_heights)) = (assets, ground_heights) else {
        return;
    };
    *done = true;
    let Ok(layout) = std::env::var("VOXEL_CAMPFIRES") else {
        return;
    };
    for entry in layout.split(';') {
        let Some((x_text, z_text)) = entry.trim().split_once(',') else {
            continue;
        };
        let (Ok(x), Ok(z)) = (x_text.trim().parse(), z_text.trim().parse()) else {
            continue;
        };
        let ground = ground_heights.ground_at(x, z);
        spawn_campfire_at(&mut commands, &assets, Vec3::new(x, ground, z));
    }
}
