//! Procedural scatter system for loading .glb models and distributing
//! them across the ground plane as decorative props.

use bevy::prelude::*;
use rand::Rng;

use crate::scene::schema::SceneDescription;

/// Marker for scattered prop entities.
#[derive(Component)]
pub struct ScatteredProp;

struct ScatterLayer {
    asset_path: &'static str,
    count: usize,
    /// Desired world size in meters (largest axis).
    target_size: f32,
    /// Measured largest axis from the glb (via Python accessor inspection).
    measured_size: f32,
    scale_variation: f32,
    spread: f32,
    random_yaw: bool,
}

impl ScatterLayer {
    fn scale(&self) -> f32 {
        self.target_size / self.measured_size
    }
}

pub fn scatter_props(
    mut commands: Commands,
    asset_server: Res<AssetServer>,
    description: Res<SceneDescription>,
) {
    let center = crate::scene::atrium_to_bevy(description.environment.spawn);
    let atrium_half_w = description.atrium.width / 2.0;
    let atrium_half_d = description.atrium.depth / 2.0;

    // measured_size: from Python glb accessor min/max inspection
    let layers = [
        ScatterLayer {
            asset_path: "models/Grass.glb",
            count: 1000,
            target_size: 0.15,
            measured_size: 28.0,
            scale_variation: 0.3,
            spread: 15.0,
            random_yaw: true,
        },
        ScatterLayer {
            asset_path: "models/Grass2.glb",
            count: 100,
            target_size: 0.3,
            measured_size: 0.8,
            scale_variation: 0.3,
            spread: 15.0,
            random_yaw: true,
        },
        ScatterLayer {
            asset_path: "models/Birch Trees.glb",
            count: 8,
            target_size: 2.5,
            measured_size: 2.5, // node-transform based, bounds ≈0
            scale_variation: 0.2,
            spread: 12.0,
            random_yaw: true,
        },
        ScatterLayer {
            asset_path: "models/Rocks.glb",
            count: 10,
            target_size: 0.3,
            measured_size: 0.3, // node-transform based
            scale_variation: 0.3,
            spread: 10.0,
            random_yaw: true,
        },
        ScatterLayer {
            asset_path: "models/Flowers.glb",
            count: 20,
            target_size: 0.1,
            measured_size: 0.2, // node-transform based
            scale_variation: 0.2,
            spread: 10.0,
            random_yaw: true,
        },
        ScatterLayer {
            asset_path: "models/Bush.glb",
            count: 6,
            target_size: 1.2,
            measured_size: 2.0,
            scale_variation: 0.2,
            spread: 10.0,
            random_yaw: true,
        },
        ScatterLayer {
            asset_path: "models/Bush with Flowers.glb",
            count: 4,
            target_size: 1.2,
            measured_size: 2.0,
            scale_variation: 0.2,
            spread: 10.0,
            random_yaw: true,
        },
    ];

    let mut rng = rand::rng();

    for layer in &layers {
        let scene_handle: Handle<Scene> =
            asset_server.load(GltfAssetLabel::Scene(0).from_asset(layer.asset_path));

        let base_scale = layer.scale();

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

            commands.spawn((
                ScatteredProp,
                SceneRoot(scene_handle.clone()),
                Transform::from_translation(center + Vec3::new(x, 0.0, z))
                    .with_rotation(Quat::from_rotation_y(yaw))
                    .with_scale(Vec3::splat(scale)),
            ));
        }
    }
}
