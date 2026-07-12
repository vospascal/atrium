//! SceneDescription → ECS import (2D top-down sound emission map).
//!
//! Spawns the schematic entities from a `SceneDescription`: source badges
//! (dark disc + colored ring + soft halo), the listener badge, speaker dots,
//! the atrium floor sprite, and the screen-space info cards that track them.
//! Icon glyphs, intensity ripples, and connection lines are drawn per-frame
//! with gizmos (see `scene::mod`).

use bevy::prelude::*;

use super::atrium_to_world;
use super::icons::{IconKind, SourceIcon};
use super::landscape::{FloorSprite, LandscapeTheme};
use super::schema::{parse_hex_color, SceneDescription, SourceDescription};
use super::{
    EarLabel, ListenerTag, SourceCard, SourceCardMetrics, SpeakerLabel, BADGE_RADIUS,
    LAYER_LISTENER, LAYER_SOURCE, LAYER_SPEAKER,
};
use crate::ecs::*;

/// Shared dark fill for source/listener badges — readable in every theme.
const BADGE_FILL: Color = Color::srgba(0.03, 0.055, 0.10, 0.92);

/// Info card background (matches the HUD panels, slightly deeper).
pub(crate) const CARD_BG: Color = Color::srgba(0.02, 0.04, 0.08, 0.86);
const CARD_TEXT: Color = Color::srgb(0.92, 0.95, 1.00);
const CARD_METRICS_TEXT: Color = Color::srgb(0.65, 0.73, 0.86);

/// Spawn all scene entities from a `SceneDescription`.
///
/// Creates: environment + atrium data entities, atrium floor sprite, speakers,
/// source badges (with info cards), and the listener badge (with tag + ear
/// labels). Does NOT despawn existing entities — the caller clears the scene
/// first if reloading.
pub fn spawn_scene(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<ColorMaterial>,
    description: &SceneDescription,
    theme: LandscapeTheme,
) {
    let env = &description.environment;
    let atrium = &description.atrium;
    let palette = theme.palette();

    // ── Data-only entities ──
    commands.spawn(SoundEnvironment {
        id: "environment".into(),
        width: env.width,
        depth: env.depth,
        height: env.height,
        spawn: env.spawn,
    });
    commands.spawn(SoundAtrium {
        id: "atrium".into(),
        width: atrium.width,
        depth: atrium.depth,
        height: atrium.height,
    });

    // ── Atrium floor (translucent fill; fixed at the spawn/room center) ──
    let atrium_center = atrium_to_world(env.spawn);
    commands.spawn((
        FloorSprite,
        Sprite::from_color(palette.floor, Vec2::new(atrium.width, atrium.depth)),
        Transform::from_xyz(atrium_center.x, atrium_center.y, -1.0),
    ));

    // ── Speakers (dim slate dots) ──
    let speaker_mesh = meshes.add(Circle::new(0.14));
    let speaker_material = materials.add(ColorMaterial::from_color(Color::srgb(0.55, 0.58, 0.66)));
    for speaker in &description.speakers.speakers {
        let world = atrium_to_world(speaker.position);
        commands.spawn((
            SoundSpeaker {
                id: speaker.id.clone(),
                label: speaker.label.clone(),
                channel: speaker.channel,
            },
            AtriumHeight(speaker.position[2]),
            Mesh2d(speaker_mesh.clone()),
            MeshMaterial2d(speaker_material.clone()),
            Transform::from_xyz(world.x, world.y, LAYER_SPEAKER),
        ));
        spawn_label(
            commands,
            &speaker.label,
            11.0,
            Color::srgba(0.62, 0.66, 0.76, 0.85),
            SpeakerLabel {
                channel: speaker.channel,
            },
        );
    }

    // ── Source badges: dark disc + colored ring + soft halo ──
    for source in &description.sources {
        spawn_one_source(commands, meshes, materials, source);
    }

    // ── Listener badge: dark disc + light ring (glyph drawn by gizmos) ──
    let listener_fill_mesh = meshes.add(Circle::new(BADGE_RADIUS + 0.04));
    let listener_ring_mesh = meshes.add(Annulus::new(BADGE_RADIUS - 0.02, BADGE_RADIUS + 0.04));
    let listener_world = atrium_to_world(description.listener.position);
    commands
        .spawn((
            SoundListener {
                id: "listener".into(),
                yaw_degrees: description.listener.yaw_degrees,
            },
            AtriumHeight(description.listener.position[2]),
            Mesh2d(listener_fill_mesh),
            MeshMaterial2d(materials.add(ColorMaterial::from_color(BADGE_FILL))),
            Transform::from_xyz(listener_world.x, listener_world.y, LAYER_LISTENER),
        ))
        .with_children(|badge| {
            badge.spawn((
                Mesh2d(listener_ring_mesh),
                MeshMaterial2d(
                    materials.add(ColorMaterial::from_color(Color::srgb(0.92, 0.95, 1.0))),
                ),
                Transform::from_xyz(0.0, 0.0, 0.1),
            ));
        });

    // "Listener" tag pill below the badge.
    commands
        .spawn((
            ListenerTag,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(-1000.0),
                top: Val::Px(-1000.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(3.0)),
                border_radius: BorderRadius::all(Val::Px(999.0)),
                ..default()
            },
            BackgroundColor(CARD_BG),
        ))
        .with_children(|pill| {
            pill.spawn((
                Text::new("Listener"),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(CARD_TEXT),
            ));
        });

    let ear_color = Color::srgba(0.92, 0.95, 1.0, 0.9);
    for (is_right, text) in [(false, "L"), (true, "R")] {
        spawn_label(commands, text, 12.0, ear_color, EarLabel { is_right });
    }

    // Themed background behind the map.
    commands.insert_resource(ClearColor(palette.background));
}

/// Spawn one source's badge (dark disc + colored ring + soft halo) and its info
/// card, keyed by `source.slot` (the audio-thread pool slot). Used by the
/// initial scene spawn and by live add-source.
pub(crate) fn spawn_one_source(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<ColorMaterial>,
    source: &SourceDescription,
) {
    let slot = source.slot;
    let rgb = parse_hex_color(&source.color);
    let color = Color::srgb(rgb[0], rgb[1], rgb[2]);
    let halo_color = Color::srgba(rgb[0], rgb[1], rgb[2], 0.18);
    let world = atrium_to_world(source.position);

    let id = if source.id.is_empty() {
        format!("source_{slot}")
    } else {
        source.id.clone()
    };
    let icon = IconKind::infer(&id, &source.name);

    let badge_fill_mesh = meshes.add(Circle::new(BADGE_RADIUS));
    let badge_ring_mesh = meshes.add(Annulus::new(BADGE_RADIUS - 0.05, BADGE_RADIUS));
    let badge_halo_mesh = meshes.add(Annulus::new(BADGE_RADIUS, BADGE_RADIUS + 0.14));
    let badge_fill_material = materials.add(ColorMaterial::from_color(BADGE_FILL));

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
            SoundSourceIndex(slot),
            SourceIcon(icon),
            AtriumHeight(source.position[2]),
            Mesh2d(badge_fill_mesh),
            MeshMaterial2d(badge_fill_material),
            Transform::from_xyz(world.x, world.y, LAYER_SOURCE),
        ))
        .with_children(|badge| {
            badge.spawn((
                Mesh2d(badge_ring_mesh),
                MeshMaterial2d(materials.add(ColorMaterial::from_color(color))),
                Transform::from_xyz(0.0, 0.0, 0.1),
            ));
            badge.spawn((
                Mesh2d(badge_halo_mesh),
                MeshMaterial2d(materials.add(ColorMaterial::from_color(halo_color))),
                Transform::from_xyz(0.0, 0.0, -0.1),
            ));
        });

    spawn_source_card(commands, slot, &source.name, color);
}

/// Info card for one source: "N Name" row + live "12.9 m  225°  -7.0 dB" row.
/// Positioned next to the badge each frame by `update_source_cards`.
fn spawn_source_card(commands: &mut Commands, index: usize, name: &str, color: Color) {
    commands
        .spawn((
            SourceCard { index },
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(-1000.0),
                top: Val::Px(-1000.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::axes(Val::Px(10.0), Val::Px(6.0)),
                row_gap: Val::Px(2.0),
                border_radius: BorderRadius::all(Val::Px(8.0)),
                ..default()
            },
            BackgroundColor(CARD_BG),
        ))
        .with_children(|card| {
            card.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(7.0),
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|title_row| {
                title_row.spawn((
                    Text::new(format!("{}", index + 1)),
                    TextFont {
                        font_size: 12.0,
                        ..default()
                    },
                    TextColor(CARD_METRICS_TEXT),
                ));
                title_row.spawn((
                    Text::new(name),
                    TextFont {
                        font_size: 13.0,
                        ..default()
                    },
                    TextColor(color),
                ));
            });
            card.spawn((
                SourceCardMetrics { index },
                Text::new("..."),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(CARD_TEXT),
            ));
        });
}

/// Spawn an absolutely-positioned UI text label (starts off-screen; billboard
/// systems reposition it each frame).
fn spawn_label(
    commands: &mut Commands,
    text: &str,
    font_size: f32,
    color: Color,
    marker: impl Component,
) {
    commands.spawn((
        marker,
        Text::new(text),
        TextFont {
            font_size,
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
