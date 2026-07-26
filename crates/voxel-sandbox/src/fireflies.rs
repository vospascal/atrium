//! Firefly swarms.
//!
//! Press `F` to release a swarm where you're looking (in front of the
//! camera in first-person, at the orbit focus otherwise); `Shift+F`
//! clears them all. Each swarm is one mesh of billboarded motes whose
//! vertex shader wanders them on per-particle sine paths and blinks them
//! with individual rhythms — HDR-bright, so the bloom pass makes them
//! glow. Fireflies only come out when the daylight fades; a soft green
//! point light per swarm touches the grass beneath them.
//!
//! `VOXEL_FIREFLIES="x,z;x,z"` pre-spawns swarms (render-space meters) —
//! for screenshots now, biome presets later.

use bevy::asset::RenderAssetUsages;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::Material;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::day_night::CelestialState;
use crate::ViewMode;
use voxel_core::noise::{hash_3d, hash_to_unit};

/// Quads built into the shared swarm mesh — the ceiling for the panel's
/// amount slider (unused motes collapse in the vertex shader).
const MAX_FIREFLIES: usize = 150;

/// Live-tweakable firefly style, shared by every swarm (the `V` panel
/// edits this; `update_fireflies` pushes it into the shared material).
#[derive(Resource)]
pub struct FireflySettings {
    /// Motes per swarm (up to [`MAX_FIREFLIES`]).
    pub amount: u32,
    /// Horizontal wander radius, meters.
    pub width: f32,
    /// Vertical wander extent, meters.
    pub height: f32,
    /// Mote billboard size, meters.
    pub size: f32,
    /// HDR emit power (higher = stronger bloom halo).
    pub glow: f32,
    /// Blink tempo multiplier.
    pub blink_speed: f32,
    /// Mote color (sRGB, for the panel's color picker).
    pub color: [f32; 3],
    /// Per-swarm point light at full night, lumens.
    pub light_intensity: f32,
}

impl Default for FireflySettings {
    fn default() -> Self {
        // User's baked sweet spot (2026-07-16): a soft champagne-colored
        // drift, tight and bright.
        Self {
            amount: 75,
            width: 2.0,
            height: 1.5,
            size: 0.045,
            glow: 25.0,
            blink_speed: 0.85,
            color: [235.0 / 255.0, 185.0 / 255.0, 145.0 / 255.0],
            light_intensity: 8_000.0,
        }
    }
}

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct FireflyUniform {
    /// x = swarm width, y = mote size, z = glow gain, w = night factor.
    pub params: Vec4,
    /// rgb = mote color (linear), w = amount fraction (seed cull).
    pub tint: Vec4,
    /// x = blink tempo, y = swarm height, zw = unused.
    pub motion: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct FireflyMaterial {
    #[uniform(0)]
    pub firefly: FireflyUniform,
}

impl Material for FireflyMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/firefly.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/firefly.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }
}

/// Marker for a spawned swarm.
#[derive(Component)]
pub struct FireflySwarm;

/// Shared mesh + material: every swarm reuses them (per-swarm variety
/// comes from the entity position salting the shader's wander phases).
#[derive(Resource)]
pub struct FireflyAssets {
    mesh: Handle<Mesh>,
    material: Handle<FireflyMaterial>,
}

pub fn setup_fireflies(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<FireflyMaterial>>,
) {
    commands.insert_resource(FireflyAssets {
        mesh: meshes.add(build_swarm_mesh()),
        material: materials.add(FireflyMaterial {
            firefly: FireflyUniform {
                params: Vec4::new(2.0, 0.045, 25.0, 1.0),
                tint: Vec4::new(0.92, 0.72, 0.57, 0.5),
                motion: Vec4::new(0.85, 2.0, 0.0, 0.0),
            },
        }),
    });
}

/// One quad per mote. POSITION carries the mote's home offset in a unit
/// sphere (scaled by the radius uniform); UV0 the corner; UV1 two seeds
/// driving wander speeds and blink rhythm.
fn build_swarm_mesh() -> Mesh {
    let mut positions = Vec::with_capacity(MAX_FIREFLIES * 4);
    let mut normals = Vec::with_capacity(MAX_FIREFLIES * 4);
    let mut corner_uvs = Vec::with_capacity(MAX_FIREFLIES * 4);
    let mut seed_uvs = Vec::with_capacity(MAX_FIREFLIES * 4);
    let mut indices = Vec::with_capacity(MAX_FIREFLIES * 12);

    for mote in 0..MAX_FIREFLIES as i32 {
        let unit = |salt: i32| hash_to_unit(hash_3d(mote, salt, 137, 4_242));
        // Rejection-free ball point: cube sample scaled toward the center.
        let home = [
            (unit(0) * 2.0 - 1.0) * 0.95,
            (unit(1) * 2.0 - 1.0) * 0.70,
            (unit(2) * 2.0 - 1.0) * 0.95,
        ];
        let seeds = [unit(3), unit(4)];

        let first_vertex = positions.len() as u32;
        for corner in [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]] {
            positions.push(home);
            normals.push([0.0, 1.0, 0.0]);
            corner_uvs.push(corner);
            seed_uvs.push(seeds);
        }
        for triangle in [[0, 1, 2], [0, 2, 3], [0, 2, 1], [0, 3, 2]] {
            indices.extend(triangle.map(|offset| first_vertex + offset));
        }
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

fn spawn_swarm_at(commands: &mut Commands, assets: &FireflyAssets, position: Vec3) {
    commands.spawn((
        Mesh3d(assets.mesh.clone()),
        MeshMaterial3d(assets.material.clone()),
        Transform::from_translation(position),
        bevy::light::NotShadowCaster,
        PointLight {
            color: Color::srgb(1.0, 0.72, 0.35),
            intensity: 0.0,
            range: 7.0,
            shadows_enabled: false,
            ..default()
        },
        crate::water::reflective_layers(),
        // Draw distance: motes are sub-pixel from afar — fade the swarm
        // out between 55 and 70 m instead of paying for its quads.
        bevy::camera::visibility::VisibilityRange {
            start_margin: 0.0..0.0,
            end_margin: 55.0..70.0,
            use_aabb: false,
        },
        FireflySwarm,
    ));
}

/// `F` spawns a swarm where you're looking; `Shift+F` clears them all.
#[allow(clippy::too_many_arguments)]
pub fn firefly_controls(
    mut commands: Commands,
    keyboard: Res<ButtonInput<KeyCode>>,
    assets: Option<Res<FireflyAssets>>,
    view_mode: Res<ViewMode>,
    orbit_state: Res<crate::OrbitCameraState>,
    ground_heights: Option<Res<crate::GroundHeights>>,
    camera_query: Query<&Transform, (With<Camera3d>, Without<crate::water::ReflectionCamera>)>,
    swarms: Query<Entity, With<FireflySwarm>>,
) {
    if !keyboard.just_pressed(KeyCode::KeyF) {
        return;
    }
    if keyboard.pressed(KeyCode::ShiftLeft) || keyboard.pressed(KeyCode::ShiftRight) {
        for swarm in &swarms {
            commands.entity(swarm).despawn();
        }
        return;
    }
    let Some(assets) = assets else {
        return;
    };

    let anchor = match *view_mode {
        ViewMode::FirstPerson | ViewMode::Fly => {
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
    spawn_swarm_at(
        &mut commands,
        &assets,
        Vec3::new(anchor.x, ground + 1.3, anchor.y),
    );
}

/// Pre-spawn swarms from `VOXEL_FIREFLIES="x,z;x,z"` once the terrain
/// heights exist (screenshots and, later, biome presets).
pub fn spawn_env_fireflies(
    mut commands: Commands,
    mut done: Local<bool>,
    assets: Option<Res<FireflyAssets>>,
    ground_heights: Option<Res<crate::GroundHeights>>,
) {
    if *done {
        return;
    }
    let (Some(assets), Some(ground_heights)) = (assets, ground_heights) else {
        return;
    };
    *done = true;
    let Ok(layout) = std::env::var("VOXEL_FIREFLIES") else {
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
        spawn_swarm_at(&mut commands, &assets, Vec3::new(x, ground + 1.3, z));
    }
}

/// Fireflies come out as the light fades. Pushes the live panel settings
/// and the night factor into the shared material and the swarm lights.
pub fn update_fireflies(
    celestial: Res<CelestialState>,
    settings: Res<FireflySettings>,
    mut materials: ResMut<Assets<FireflyMaterial>>,
    mut lights: Query<&mut PointLight, With<FireflySwarm>>,
) {
    let night = (1.0 - celestial.daylight).clamp(0.0, 1.0);
    let emergence = night * night;
    let tint = Color::srgb(settings.color[0], settings.color[1], settings.color[2]).to_linear();
    let amount_fraction = settings.amount.min(MAX_FIREFLIES as u32) as f32 / MAX_FIREFLIES as f32;
    for (_, material) in materials.iter_mut() {
        material.firefly.params =
            Vec4::new(settings.width, settings.size, settings.glow, emergence);
        material.firefly.tint = Vec4::new(tint.red, tint.green, tint.blue, amount_fraction);
        material.firefly.motion.x = settings.blink_speed;
        material.firefly.motion.y = settings.height;
    }
    let light_color = Color::srgb(
        settings.color[0].max(0.05),
        settings.color[1].max(0.05),
        settings.color[2].max(0.05),
    );
    for mut light in &mut lights {
        light.intensity = settings.light_intensity * emergence;
        light.color = light_color;
    }
}
