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
    ListenerTag, SourceCard, SourceCardLevel, SourceCardMetrics, SpeakerLabel, BADGE_RADIUS,
    LAYER_LISTENER, LAYER_SOURCE, LAYER_SPEAKER,
};
use crate::ecs::*;

/// Shared dark fill for source/listener badges — readable in every theme.
const BADGE_FILL: Color = Color::srgba(0.03, 0.055, 0.10, 0.92);

/// Info card background (matches the HUD panels, slightly deeper).
pub(crate) const CARD_BG: Color = Color::srgba(0.02, 0.04, 0.08, 0.78);
const CARD_TEXT: Color = Color::srgb(0.92, 0.95, 1.00);
const CARD_METRICS_TEXT: Color = Color::srgb(0.65, 0.73, 0.86);
const CARD_BORDER: Color = Color::srgba(0.48, 0.64, 0.78, 0.38);

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

    // Keep markers approximately the same screen size across compact indoor
    // rooms and wide outdoor maps; camera zoom still scales them naturally.
    let visual_scale = map_visual_scale(description);

    // ── Source badges: dark disc + colored ring + soft halo ──
    for source in &description.sources {
        spawn_one_source(commands, meshes, materials, source, visual_scale);
    }

    // ── Listener badge: larger dark disc, luminous rings, filled person glyph ──
    let listener_radius = BADGE_RADIUS + 0.12;
    let listener_fill_mesh = meshes.add(Circle::new(listener_radius));
    let listener_ring_mesh = meshes.add(Annulus::new(listener_radius - 0.05, listener_radius));
    let listener_glow_mesh =
        meshes.add(Annulus::new(listener_radius + 0.06, listener_radius + 0.09));
    let listener_head_mesh = meshes.add(Circle::new(0.09));
    let listener_body_mesh = meshes.add(Ellipse::new(0.17, 0.14));
    let listener_white = materials.add(ColorMaterial::from_color(Color::srgb(0.94, 0.97, 1.0)));
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
            Transform::from_xyz(listener_world.x, listener_world.y, LAYER_LISTENER)
                .with_scale(Vec3::splat(visual_scale)),
        ))
        .with_children(|badge| {
            badge.spawn((
                Mesh2d(listener_ring_mesh),
                MeshMaterial2d(listener_white.clone()),
                Transform::from_xyz(0.0, 0.0, 0.1),
            ));
            badge.spawn((
                Mesh2d(listener_glow_mesh),
                MeshMaterial2d(materials.add(ColorMaterial::from_color(Color::srgba(
                    0.27, 0.78, 0.98, 0.58,
                )))),
                Transform::from_xyz(0.0, 0.0, 0.05),
            ));
            badge.spawn((
                Mesh2d(listener_head_mesh),
                MeshMaterial2d(listener_white.clone()),
                Transform::from_xyz(0.0, 0.15, 0.2),
            ));
            badge.spawn((
                Mesh2d(listener_body_mesh),
                MeshMaterial2d(listener_white),
                Transform::from_xyz(0.0, -0.14, 0.2),
            ));
        });

    // "Listener" tag pill below the badge.
    commands
        .spawn((
            ListenerTag,
            UiTransform::IDENTITY,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(-1000.0),
                top: Val::Px(-1000.0),
                padding: UiRect::axes(Val::Px(10.0), Val::Px(3.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(999.0)),
                ..default()
            },
            BackgroundColor(CARD_BG),
            BorderColor::all(CARD_BORDER),
        ))
        .with_children(|pill| {
            pill.spawn((
                Text::new("Listener"),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(Color::srgb(0.27, 0.78, 0.98)),
            ));
        });

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
    visual_scale: f32,
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
                emitter_kind: source.emitter_kind.clone(),
                spl: source.spl,
                ref_distance: source.ref_distance,
                directivity: source.directivity.clone(),
                directivity_alpha: source.directivity_alpha,
                spread: source.spread,
                orbit_radius: source.orbit_radius,
                orbit_speed: source.orbit_speed,
                synth_kind: source.synth_kind.clone(),
            },
            SoundSourceIndex(slot),
            SourceIcon(icon),
            AtriumHeight(source.position[2]),
            Mesh2d(badge_fill_mesh),
            MeshMaterial2d(badge_fill_material),
            Transform::from_xyz(world.x, world.y, LAYER_SOURCE)
                .with_scale(Vec3::splat(visual_scale)),
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

pub(crate) fn map_visual_scale(description: &SceneDescription) -> f32 {
    (description
        .environment
        .width
        .max(description.environment.depth)
        / 60.0)
        .clamp(0.25, 1.0)
}

/// Compact map callout for one source: colored title, position row, and level row.
/// Positioned next to the badge each frame by `update_source_cards`.
fn spawn_source_card(commands: &mut Commands, index: usize, name: &str, color: Color) {
    commands
        .spawn((
            SourceCard { index },
            GlobalZIndex(3),
            UiTransform::IDENTITY,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(-1000.0),
                top: Val::Px(-1000.0),
                width: Val::Px(132.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::axes(Val::Px(9.0), Val::Px(7.0)),
                row_gap: Val::Px(4.0),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(12.0)),
                ..default()
            },
            BackgroundColor(CARD_BG),
            BorderColor::all(CARD_BORDER),
        ))
        .with_children(|card| {
            card.spawn((
                Text::new(name),
                TextFont {
                    font_size: 12.0,
                    ..default()
                },
                TextColor(color),
            ));
            card.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(7.0),
                ..default()
            })
            .with_children(|row| {
                spawn_pin_icon(row);
                row.spawn((
                    SourceCardMetrics { index },
                    Text::new("-- m  /  -- deg"),
                    TextFont {
                        font_size: 9.0,
                        ..default()
                    },
                    TextColor(CARD_METRICS_TEXT),
                ));
            });
            card.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(7.0),
                ..default()
            })
            .with_children(|row| {
                spawn_level_icon(row);
                row.spawn((
                    SourceCardLevel { index },
                    Text::new("-- dB"),
                    TextFont {
                        font_size: 9.0,
                        ..default()
                    },
                    TextColor(CARD_TEXT),
                ));
            });
        });
}

fn spawn_pin_icon(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn(Node {
            position_type: PositionType::Relative,
            width: Val::Px(14.0),
            height: Val::Px(14.0),
            ..default()
        })
        .with_children(|icon| {
            icon.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(3.0),
                    top: Val::Px(1.0),
                    width: Val::Px(8.0),
                    height: Val::Px(8.0),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius::all(Val::Px(999.0)),
                    ..default()
                },
                BorderColor::all(CARD_METRICS_TEXT),
            ));
            icon.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(6.5),
                    top: Val::Px(8.0),
                    width: Val::Px(1.0),
                    height: Val::Px(5.0),
                    ..default()
                },
                BackgroundColor(CARD_METRICS_TEXT),
            ));
        });
}

fn spawn_level_icon(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn(Node {
            width: Val::Px(14.0),
            height: Val::Px(14.0),
            flex_direction: FlexDirection::Row,
            align_items: AlignItems::FlexEnd,
            column_gap: Val::Px(2.0),
            ..default()
        })
        .with_children(|icon| {
            for height in [5.0, 9.0, 13.0] {
                icon.spawn((
                    Node {
                        width: Val::Px(2.0),
                        height: Val::Px(height),
                        border_radius: BorderRadius::all(Val::Px(1.0)),
                        ..default()
                    },
                    BackgroundColor(CARD_METRICS_TEXT),
                ));
            }
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
