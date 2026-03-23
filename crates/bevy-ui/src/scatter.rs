//! Procedural scatter system for loading .glb models and distributing
//! them across the ground plane as decorative props.
//!
//! Non-grass props use SceneRoot (full glTF scene hierarchy).
//! Grass uses extracted meshes with GrassWindMaterial (no SceneRoot = no flickering).

use bevy::gltf::Gltf;
use bevy::prelude::*;
use rand::Rng;

use crate::grass_wind::{GrassWindExtension, GrassWindMaterial};
use crate::scene::schema::SceneDescription;

/// Marker for scattered prop entities.
#[derive(Component)]
pub struct ScatteredProp;

struct ScatterLayer {
    asset_path: &'static str,
    count: usize,
    target_size: f32,
    measured_size: f32,
    scale_variation: f32,
    spread: f32,
    random_yaw: bool,
    is_grass: bool,
}

impl ScatterLayer {
    fn scale(&self) -> f32 {
        self.target_size / self.measured_size
    }
}

/// Pending grass spawn: waiting for Gltf asset to load so we can extract meshes.
#[derive(Resource)]
pub struct PendingGrassSpawns {
    pub entries: Vec<PendingGrass>,
}

pub struct PendingGrass {
    pub gltf_handle: Handle<Gltf>,
    pub transforms: Vec<Transform>,
}

pub fn scatter_props(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    description: Res<SceneDescription>,
) {
    let center = crate::scene::atrium_to_bevy(description.environment.spawn);
    let atrium_half_w = description.atrium.width / 2.0;
    let atrium_half_d = description.atrium.depth / 2.0;

    let layers = [
        ScatterLayer {
            asset_path: "models/Grass.glb",
            count: 20000,
            target_size: 0.15,
            measured_size: 28.0,
            scale_variation: 0.3,
            spread: 15.0,
            random_yaw: true,
            is_grass: true,
        },
        ScatterLayer {
            asset_path: "models/Grass2.glb",
            count: 500,
            target_size: 0.3,
            measured_size: 0.8,
            scale_variation: 0.3,
            spread: 15.0,
            random_yaw: true,
            is_grass: true,
        },
        ScatterLayer {
            asset_path: "models/Birch Trees.glb",
            count: 8,
            target_size: 2.5,
            measured_size: 2.5,
            scale_variation: 0.2,
            spread: 12.0,
            random_yaw: true,
            is_grass: false,
        },
        ScatterLayer {
            asset_path: "models/Rocks.glb",
            count: 10,
            target_size: 0.3,
            measured_size: 0.3,
            scale_variation: 0.3,
            spread: 10.0,
            random_yaw: true,
            is_grass: false,
        },
        ScatterLayer {
            asset_path: "models/Flowers.glb",
            count: 20,
            target_size: 0.1,
            measured_size: 0.2,
            scale_variation: 0.2,
            spread: 10.0,
            random_yaw: true,
            is_grass: true,
        },
        ScatterLayer {
            asset_path: "models/Bush.glb",
            count: 6,
            target_size: 1.2,
            measured_size: 2.0,
            scale_variation: 0.2,
            spread: 10.0,
            random_yaw: true,
            is_grass: false,
        },
        ScatterLayer {
            asset_path: "models/Bush with Flowers.glb",
            count: 4,
            target_size: 1.2,
            measured_size: 2.0,
            scale_variation: 0.2,
            spread: 10.0,
            random_yaw: true,
            is_grass: false,
        },
    ];

    let mut rng = rand::rng();
    let mut pending_grass = Vec::new();

    for layer in &layers {
        let base_scale = layer.scale();

        // Generate all transforms for this layer
        let mut transforms = Vec::with_capacity(layer.count);
        for _ in 0..layer.count {
            let (x, z) = loop {
                let x = rng.random_range(-layer.spread..layer.spread);
                let z = rng.random_range(-layer.spread..layer.spread);
                if x.abs() < atrium_half_w + 0.5 && z.abs() < atrium_half_d + 0.5 {
                    continue;
                }
                break (x, z);
            };

            let variation = if layer.scale_variation > 0.0 {
                rng.random_range(1.0 - layer.scale_variation..1.0 + layer.scale_variation)
            } else {
                1.0
            };

            let yaw = if layer.random_yaw {
                rng.random_range(0.0..std::f32::consts::TAU)
            } else {
                0.0
            };

            let scale = base_scale * variation;
            transforms.push(
                Transform::from_translation(center + Vec3::new(x, 0.0, z))
                    .with_rotation(Quat::from_rotation_y(yaw))
                    .with_scale(Vec3::splat(scale)),
            );
        }

        if layer.is_grass {
            // Defer grass spawning until Gltf asset loads (so we can extract meshes)
            let gltf_handle: Handle<Gltf> = asset_server.load(layer.asset_path);
            pending_grass.push(PendingGrass {
                gltf_handle,
                transforms,
            });
        } else {
            // Non-grass: spawn with SceneRoot as before
            let scene_handle: Handle<Scene> =
                asset_server.load(GltfAssetLabel::Scene(0).from_asset(layer.asset_path));
            for transform in transforms {
                commands.spawn((
                    ScatteredProp,
                    SceneRoot(scene_handle.clone()),
                    transform,
                ));
            }
        }
    }

    commands.insert_resource(PendingGrassSpawns {
        entries: pending_grass,
    });
}

/// System: once Gltf assets load, extract meshes and spawn grass with GrassWindMaterial directly.
/// No SceneRoot = no material fighting = no flickering.
pub fn spawn_pending_grass(
    mut commands: Commands,
    gltf_assets: Res<Assets<Gltf>>,
    gltf_meshes: Res<Assets<bevy::gltf::GltfMesh>>,
    standard_materials: Res<Assets<StandardMaterial>>,
    mut grass_materials: ResMut<Assets<GrassWindMaterial>>,
    mut pending: ResMut<PendingGrassSpawns>,
) {
    pending.entries.retain(|entry| {
        let Some(gltf) = gltf_assets.get(&entry.gltf_handle) else {
            return true; // not loaded yet, keep waiting
        };

        // Extract all mesh+material pairs from the Gltf
        let mut mesh_materials: Vec<(Handle<Mesh>, Handle<GrassWindMaterial>)> = Vec::new();
        for gltf_mesh_handle in &gltf.meshes {
            let Some(gltf_mesh) = gltf_meshes.get(gltf_mesh_handle) else {
                return true; // sub-asset not ready
            };
            for primitive in &gltf_mesh.primitives {
                // Convert the StandardMaterial to GrassWindMaterial
                let base_mat = primitive
                    .material
                    .as_ref()
                    .and_then(|h| standard_materials.get(h))
                    .cloned()
                    .unwrap_or_default();

                let grass_mat = grass_materials.add(GrassWindMaterial {
                    base: base_mat,
                    extension: GrassWindExtension::default(),
                });
                mesh_materials.push((primitive.mesh.clone(), grass_mat));
            }
        }

        // Spawn one entity per transform, cycling through mesh variants
        if mesh_materials.is_empty() {
            return false; // no meshes found, drop it
        }

        for (i, transform) in entry.transforms.iter().enumerate() {
            let (mesh, material) = &mesh_materials[i % mesh_materials.len()];
            commands.spawn((
                ScatteredProp,
                Mesh3d(mesh.clone()),
                MeshMaterial3d(material.clone()),
                *transform,
            ));
        }

        false // done, remove from pending
    });
}
