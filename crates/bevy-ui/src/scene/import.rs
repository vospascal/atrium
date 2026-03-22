//! SceneDescription → ECS import.
//!
//! Spawns Bevy entities from a `SceneDescription`. Used by `setup_scene`
//! at startup and can be called at runtime for scene reloads.

use bevy::prelude::*;

use super::atrium_to_bevy;
use super::schema::{parse_hex_color, SceneDescription};
use crate::ecs::*;
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

    // ── Ground plane (environment floor) ──
    let ground_size = env.width.max(env.depth);
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(ground_size, ground_size))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.15, 0.18, 0.12),
            perceptual_roughness: 0.95,
            ..default()
        })),
        Transform::from_translation(atrium_to_bevy(spawn)),
    ));

    // ── Atrium floor overlay ──
    let atrium_center = atrium_to_bevy(spawn);
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(atrium.width, atrium.depth))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(0.3, 0.4, 0.5, 0.15),
            alpha_mode: AlphaMode::Blend,
            ..default()
        })),
        Transform::from_translation(atrium_center + Vec3::Y * 0.01),
    ));

    // ── Speakers ──
    let speaker_mesh = meshes.add(Cuboid::new(0.15, 0.15, 0.15));
    let speaker_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.6, 0.6, 0.7),
        ..default()
    });
    for speaker in &description.speakers.speakers {
        commands.spawn((
            SoundSpeaker {
                id: speaker.id.clone(),
                label: speaker.label.clone(),
                channel: speaker.channel,
            },
            Mesh3d(speaker_mesh.clone()),
            MeshMaterial3d(speaker_material.clone()),
            Transform::from_translation(atrium_to_bevy(speaker.position)),
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

    // ── Sources ──
    let source_mesh = meshes.add(Sphere::new(0.2));
    for (index, source) in description.sources.iter().enumerate() {
        let rgb = parse_hex_color(&source.color);
        let color = Color::srgb(rgb[0], rgb[1], rgb[2]);
        let material = materials.add(StandardMaterial {
            base_color: color,
            emissive: LinearRgba::new(rgb[0] * 0.5, rgb[1] * 0.5, rgb[2] * 0.5, 1.0),
            ..default()
        });

        let id = if source.id.is_empty() {
            format!("source_{}", index)
        } else {
            source.id.clone()
        };

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
                MeshMaterial3d(material),
                Transform::from_translation(atrium_to_bevy(source.position)),
            ))
            .with_children(|parent| {
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

    // ── Listener ──
    let listener_mesh = meshes.add(Capsule3d::new(0.08, 0.3));
    let listener_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.2, 0.8, 0.4),
        emissive: LinearRgba::new(0.1, 0.4, 0.2, 1.0),
        ..default()
    });
    commands.spawn((
        SoundListener {
            id: "listener".into(),
            yaw_degrees: description.listener.yaw_degrees,
        },
        Mesh3d(listener_mesh),
        MeshMaterial3d(listener_material),
        Transform::from_translation(atrium_to_bevy(description.listener.position)),
    ));

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
            illuminance: 2000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, 0.3, 0.0)),
    ));
    commands.spawn(AmbientLight {
        color: Color::srgb(0.6, 0.65, 0.8),
        brightness: 200.0,
        affects_lightmapped_meshes: true,
    });
}
