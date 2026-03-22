//! SceneDescription → ECS import.
//!
//! Spawns Bevy entities from a `SceneDescription`. Used by `setup_scene`
//! at startup and can be called at runtime for scene reloads.

use bevy::light::{CascadeShadowConfigBuilder, DirectionalLightShadowMap};
use bevy::prelude::*;

use super::atrium_to_bevy;
use super::schema::{parse_hex_color, SceneDescription};
use crate::ecs::*;
use crate::grass_material::GrassMaterial;
use crate::scene::{EarLabel, SourceLabel, SourceLight, SpeakerLabel};

/// Spawn all scene entities from a `SceneDescription`.
///
/// Creates: environment, atrium, speakers, sources (with lights + labels),
/// listener (with ear labels), and lighting. Does NOT despawn existing entities
/// — caller is responsible for clearing the scene first if needed.
pub fn spawn_scene(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    _grass_materials: &mut Assets<GrassMaterial>,
    asset_server: &AssetServer,
    description: &SceneDescription,
) {
    let env = &description.environment;
    let atrium = &description.atrium;
    let spawn = env.spawn;

    // ── Environment entity ──
    commands.spawn(SoundEnvironment {
        id: "environment".into(),
        width: env.width,
        depth: env.depth,
        height: env.height,
        spawn,
    });

    // ── Atrium entity ──
    commands.spawn(SoundAtrium {
        id: "atrium".into(),
        width: atrium.width,
        depth: atrium.depth,
        height: atrium.height,
    });

    // ── Ground plane (lit green surface) ──
    let ground_size = env.width.max(env.depth) * 2.0;
    let scene_center = Vec3::ZERO;
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(ground_size, ground_size))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.25, 0.45, 0.15),
            perceptual_roughness: 0.95,
            ..default()
        })),
        Transform::from_translation(scene_center),
    ));

    // ── Atrium floor overlay (stone clearing) ──
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(atrium.width, atrium.depth))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.55, 0.50, 0.40),
            perceptual_roughness: 0.9,
            ..default()
        })),
        Transform::from_translation(scene_center + Vec3::Y * 0.02),
    ));

    // ── Atrium boundary walls (replace wireframe) ──
    let wall_height = 0.3;
    let wall_thickness = 0.08;
    let half_w = atrium.width / 2.0;
    let half_d = atrium.depth / 2.0;
    let wall_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.45, 0.42, 0.40),
        perceptual_roughness: 0.9,
        ..default()
    });

    // Four walls: front, back, left, right
    let wall_configs = [
        // (width, depth, x_offset, z_offset)
        (atrium.width + wall_thickness, wall_thickness, 0.0, -half_d),
        (atrium.width + wall_thickness, wall_thickness, 0.0, half_d),
        (wall_thickness, atrium.depth + wall_thickness, -half_w, 0.0),
        (wall_thickness, atrium.depth + wall_thickness, half_w, 0.0),
    ];
    for (wall_w, wall_d, offset_x, offset_z) in wall_configs {
        commands.spawn((
            Mesh3d(meshes.add(Cuboid::new(wall_w, wall_height, wall_d))),
            MeshMaterial3d(wall_material.clone()),
            Transform::from_translation(
                scene_center + Vec3::new(offset_x, wall_height / 2.0, offset_z),
            ),
        ));
    }

    // ── Speakers (glb model, facing toward center) ──
    // Model is ~15.6 units tall; scale to ~0.4m
    let speaker_scale = 0.4 / 15.6;
    let speaker_scene: Handle<Scene> =
        asset_server.load(GltfAssetLabel::Scene(0).from_asset("models/speaker.glb"));
    for speaker in &description.speakers.speakers {
        let pos = atrium_to_bevy(speaker.position);
        // Compute yaw so speaker faces toward the center, with per-channel correction
        let to_center = -pos;
        let base_yaw = to_center.x.atan2(to_center.z);
        let correction = match speaker.label.to_lowercase().as_str() {
            "fl" => std::f32::consts::FRAC_PI_2,        // 90° CCW
            "c" => std::f32::consts::FRAC_PI_4,         // 45° CCW
            "rl" => std::f32::consts::PI,                // 180°
            "rr" => -std::f32::consts::FRAC_PI_2,       // 90° CW
            _ => 0.0,                                     // FR and others: no correction
        };
        let yaw = base_yaw + correction;
        commands.spawn((
            SoundSpeaker {
                id: speaker.id.clone(),
                label: speaker.label.clone(),
                channel: speaker.channel,
            },
            SceneRoot(speaker_scene.clone()),
            Transform::from_translation(pos)
                .with_rotation(Quat::from_rotation_y(yaw))
                .with_scale(Vec3::splat(speaker_scale)),
        ));

        // Speaker label
        commands.spawn((
            SpeakerLabel {
                channel: speaker.channel,
            },
            Text::new(&speaker.label),
            TextFont {
                font_size: 11.0,
                ..default()
            },
            TextColor(Color::srgba(0.6, 0.6, 0.7, 0.8)),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(-1000.0),
                top: Val::Px(-1000.0),
                ..default()
            },
        ));
    }

    // ── Sources (glowing orbs) ──
    let source_mesh = meshes.add(Sphere::new(0.15));
    let halo_mesh = meshes.add(Sphere::new(0.35));
    for (index, source) in description.sources.iter().enumerate() {
        let rgb = parse_hex_color(&source.color);
        let color = Color::srgb(rgb[0], rgb[1], rgb[2]);

        // Inner orb: solid with strong emissive
        let inner_material = materials.add(StandardMaterial {
            base_color: color,
            emissive: LinearRgba::new(rgb[0] * 2.0, rgb[1] * 2.0, rgb[2] * 2.0, 1.0),
            ..default()
        });

        // Outer halo: translucent glow
        let halo_material = materials.add(StandardMaterial {
            base_color: Color::srgba(rgb[0], rgb[1], rgb[2], 0.12),
            emissive: LinearRgba::new(rgb[0] * 0.8, rgb[1] * 0.8, rgb[2] * 0.8, 1.0),
            alpha_mode: AlphaMode::Blend,
            ..default()
        });

        let id = if source.id.is_empty() {
            format!("source_{}", index)
        } else {
            source.id.clone()
        };

        let mut pos = atrium_to_bevy(source.position);
        pos.y = pos.y.max(0.25); // keep orbs slightly above ground

        commands
            .spawn((
                SoundSource {
                    id,
                    name: source.name.clone(),
                    color: rgb,
                    spl: source.spl,
                    ref_distance: source.ref_distance,
                    directivity: source.directivity.clone(),
                    directivity_alpha: source.directivity_alpha,
                    spread: source.spread,
                    orbit_radius: source.orbit_radius,
                    orbit_speed: source.orbit_speed,
                },
                SoundSourceIndex(index),
                Mesh3d(source_mesh.clone()),
                MeshMaterial3d(inner_material),
                Transform::from_translation(pos),
            ))
            .with_children(|parent| {
                // Glow halo
                parent.spawn((Mesh3d(halo_mesh.clone()), MeshMaterial3d(halo_material)));
                // Point light
                parent.spawn((
                    SourceLight {
                        source_index: index,
                    },
                    PointLight {
                        color,
                        intensity: 5000.0,
                        radius: 0.2,
                        range: 8.0,
                        shadows_enabled: false,
                        ..default()
                    },
                ));
            });

        // Screen-space label
        commands.spawn((
            SourceLabel { index },
            Text::new(&source.name),
            TextFont {
                font_size: 13.0,
                ..default()
            },
            TextColor(color),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(-1000.0),
                top: Val::Px(-1000.0),
                ..default()
            },
        ));
    }

    // ── Listener (character marker + direction cone) ──
    let listener_mesh = meshes.add(Capsule3d::new(0.08, 0.3));
    let listener_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.2, 0.8, 0.4),
        emissive: LinearRgba::new(0.1, 0.4, 0.2, 1.0),
        ..default()
    });
    let cone_mesh = meshes.add(Cone::new(0.06, 0.15));
    let cone_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.3, 0.9, 0.5),
        emissive: LinearRgba::new(0.15, 0.5, 0.25, 1.0),
        ..default()
    });
    commands
        .spawn((
            SoundListener {
                id: "listener".into(),
                yaw_degrees: description.listener.yaw_degrees,
            },
            Mesh3d(listener_mesh),
            MeshMaterial3d(listener_material),
            Transform::from_translation(atrium_to_bevy(description.listener.position)),
        ))
        .with_children(|parent| {
            // Direction cone: points forward on the ground plane
            // Cone default points up (+Y), rotate -90° on X to point forward (-Z),
            // then it will be yaw-rotated each frame by update_listener_direction_cone.
            parent.spawn((
                crate::scene::ListenerDirectionCone,
                Mesh3d(cone_mesh),
                MeshMaterial3d(cone_material),
                Transform::from_translation(Vec3::new(0.0, -0.15, -0.2))
                    .with_rotation(Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2)),
            ));
        });

    // Ear labels
    let ear_label_color = Color::srgba(0.2, 0.8, 0.4, 0.9);
    for (is_right, text) in [(false, "L"), (true, "R")] {
        commands.spawn((
            EarLabel { is_right },
            Text::new(text),
            TextFont {
                font_size: 14.0,
                ..default()
            },
            TextColor(ear_label_color),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(-1000.0),
                top: Val::Px(-1000.0),
                ..default()
            },
        ));
    }

    // ── Lighting ──
    commands.spawn((
        DirectionalLight {
            illuminance: 10000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.9, 0.4, 0.0)),
        CascadeShadowConfigBuilder {
            first_cascade_far_bound: 50.0,
            maximum_distance: 200.0,
            ..default()
        }
        .build(),
    ));
    commands.insert_resource(DirectionalLightShadowMap { size: 4096 });

    commands.insert_resource(ClearColor(Color::srgb(0.12, 0.20, 0.08)));
}
