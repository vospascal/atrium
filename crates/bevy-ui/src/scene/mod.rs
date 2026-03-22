//! Scene setup, 3D visualization, and real-time updates.
//!
//! Reads `SceneDescription` to spawn ECS entities with audio components,
//! then updates positions each frame from telemetry.

pub mod export;
pub mod import;
pub mod save;
pub mod schema;

pub use schema::SceneDescription;

use std::f32::consts::PI;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use atrium_core::commands::Command;
use atrium_core::types::Vec3 as AtriumVec3;

use crate::camera::IsometricCamera;
use crate::ecs::*;
use crate::telemetry::{CommandSender, TelemetryMessage};

// ── Rendering-only markers (not scene data) ─────────────────────────────────

/// Marker for the gain line between a source and the listener.
#[derive(Component)]
pub struct GainLine {
    pub source_index: usize,
}

/// Marker for the point light child of a source (intensity driven by gain).
#[derive(Component)]
pub struct SourceLight {
    pub source_index: usize,
}

/// Marker for a screen-space label that tracks a source's 3D position.
#[derive(Component)]
pub(crate) struct SourceLabel {
    pub index: usize,
}

/// Marker for a screen-space label that tracks a speaker's 3D position.
#[derive(Component)]
pub(crate) struct SpeakerLabel {
    pub channel: usize,
}

/// Marker for the listener ear labels ("L" / "R").
#[derive(Component)]
pub(crate) struct EarLabel {
    /// true = right ear, false = left ear
    pub is_right: bool,
}

/// Marker for the listener's direction indicator cone.
#[derive(Component)]
pub(crate) struct ListenerDirectionCone;

// ── Coordinate mapping ───────────────────────────────────────────────────────
//
// Atrium: X = left/right, Y = front/back, Z = up/down
// Bevy:   X = right,      Y = up,         Z = back (right-handed, Y-up)
//
// Mapping: Bevy.X = Atrium.X, Bevy.Y = Atrium.Z, Bevy.Z = -Atrium.Y

pub fn atrium_to_bevy(position: [f32; 3]) -> Vec3 {
    Vec3::new(position[0], position[2], -position[1])
}

// ── Setup ────────────────────────────────────────────────────────────────────

pub fn setup_scene(
    mut commands: Commands,
    description: Res<SceneDescription>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut grass_materials: ResMut<Assets<crate::grass_material::GrassMaterial>>,
    asset_server: Res<AssetServer>,
) {
    import::spawn_scene(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut grass_materials,
        &asset_server,
        &description,
    );
}

// ── Per-frame updates ────────────────────────────────────────────────────────

/// Update listener mesh position from ListenerState.
pub fn update_listener(
    mut listener: Query<&mut Transform, With<SoundListener>>,
    state: Res<crate::camera::ListenerState>,
) {
    let Ok(mut transform) = listener.single_mut() else {
        return;
    };
    let target = atrium_to_bevy(state.position);
    transform.translation = transform.translation.lerp(target, 0.3);
}

/// Rotate the listener's direction cone to match the camera yaw.
pub(crate) fn update_listener_direction_cone(
    mut cones: Query<&mut Transform, With<ListenerDirectionCone>>,
    camera_query: Query<&Transform, (With<IsometricCamera>, Without<ListenerDirectionCone>)>,
) {
    let Ok(camera_transform) = camera_query.single() else {
        return;
    };
    let (camera_yaw, _, _) = camera_transform.rotation.to_euler(EulerRot::YXZ);

    for mut cone_transform in &mut cones {
        // Cone base rotation is -90° on X (points forward along -Z).
        // Apply camera yaw rotation on Y axis so it faces the listener's direction.
        cone_transform.rotation =
            Quat::from_rotation_y(camera_yaw) * Quat::from_rotation_x(-std::f32::consts::FRAC_PI_2);
    }
}

/// Update source positions from telemetry (skips sources being dragged).
pub(crate) fn update_sources(
    mut sources: Query<(&SoundSourceIndex, &mut Transform)>,
    mut messages: MessageReader<TelemetryMessage>,
    drag: Res<SourceDragState>,
) {
    let Some(msg) = messages.read().last() else {
        return;
    };
    let frame = &msg.frame;

    for (index, mut transform) in &mut sources {
        if is_source_dragging(&drag, index.0) {
            continue;
        }
        if index.0 < frame.source_count as usize {
            let source = &frame.sources[index.0];
            let target = atrium_to_bevy([source.x, source.y, source.z]);
            transform.translation = transform.translation.lerp(target, 0.3);
        }
    }
}

/// Update source point light intensity based on gain telemetry.
pub fn update_source_lights(
    mut lights: Query<(&SourceLight, &mut PointLight)>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    let Some(msg) = messages.read().last() else {
        return;
    };
    let frame = &msg.frame;

    for (marker, mut light) in &mut lights {
        if marker.source_index < frame.source_count as usize {
            let source = &frame.sources[marker.source_index];
            let gain = source.gain_total.clamp(0.0, 1.0);
            light.intensity = 1000.0 + gain * 15000.0;
        }
    }
}

/// Draw gain lines from sources to listener using gizmos.
pub fn update_gain_lines(
    mut gizmos: Gizmos,
    sources: Query<(&SoundSourceIndex, &SoundSource, &Transform)>,
    listener: Query<&Transform, With<SoundListener>>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    let Ok(listener_transform) = listener.single() else {
        return;
    };

    let latest = messages.read().last();

    for (index, source, source_transform) in &sources {
        let gain = latest
            .and_then(|msg| {
                if index.0 < msg.frame.source_count as usize {
                    Some(msg.frame.sources[index.0].gain_total)
                } else {
                    None
                }
            })
            .unwrap_or(0.2);

        let alpha = 0.1 + gain.clamp(0.0, 1.0) * 0.6;
        let color = Color::srgba(source.color[0], source.color[1], source.color[2], alpha);

        gizmos.line(
            source_transform.translation,
            listener_transform.translation,
            color,
        );
    }
}

/// Position screen-space labels above their corresponding 3D source positions.
pub(crate) fn billboard_labels(
    camera_query: Query<(&Camera, &GlobalTransform), With<IsometricCamera>>,
    sources: Query<(&SoundSourceIndex, &GlobalTransform)>,
    mut labels: Query<(&SourceLabel, &mut Node)>,
) {
    let Ok((camera, camera_global)) = camera_query.single() else {
        return;
    };

    for (label, mut node) in &mut labels {
        let Some((_, source_global)) = sources.iter().find(|(idx, _)| idx.0 == label.index) else {
            continue;
        };

        let world_pos = source_global.translation() + Vec3::Y * 0.5;
        if let Ok(viewport_pos) = camera.world_to_viewport(camera_global, world_pos) {
            node.left = Val::Px(viewport_pos.x - 30.0);
            node.top = Val::Px(viewport_pos.y - 20.0);
            node.display = Display::Flex;
        } else {
            node.display = Display::None;
        }
    }
}

/// Position screen-space labels above speakers.
pub(crate) fn billboard_speaker_labels(
    camera_query: Query<(&Camera, &GlobalTransform), With<IsometricCamera>>,
    speakers: Query<(&SoundSpeaker, &GlobalTransform)>,
    mut labels: Query<(&SpeakerLabel, &mut Node)>,
) {
    let Ok((camera, camera_global)) = camera_query.single() else {
        return;
    };

    for (label, mut node) in &mut labels {
        let Some((_, speaker_global)) = speakers.iter().find(|(s, _)| s.channel == label.channel)
        else {
            continue;
        };

        let world_pos = speaker_global.translation() + Vec3::Y * 0.3;
        if let Ok(viewport_pos) = camera.world_to_viewport(camera_global, world_pos) {
            node.left = Val::Px(viewport_pos.x - 10.0);
            node.top = Val::Px(viewport_pos.y - 16.0);
            node.display = Display::Flex;
        } else {
            node.display = Display::None;
        }
    }
}

/// Position "L" and "R" labels at the listener's ear positions (screen-space).
pub(crate) fn update_ear_labels(
    camera_query: Query<(&Camera, &GlobalTransform, &Transform), With<IsometricCamera>>,
    listener_query: Query<&Transform, (With<SoundListener>, Without<IsometricCamera>)>,
    mut labels: Query<(&EarLabel, &mut Node)>,
) {
    let Ok((camera, camera_global, camera_transform)) = camera_query.single() else {
        return;
    };
    let Ok(listener_transform) = listener_query.single() else {
        return;
    };

    let (camera_yaw, _, _) = camera_transform.rotation.to_euler(EulerRot::YXZ);
    let right = Vec3::new(camera_yaw.cos(), 0.0, -camera_yaw.sin());
    let ear_offset = 0.5;
    let ear_height = Vec3::Y * 0.4;

    let listener_pos = listener_transform.translation;

    for (ear, mut node) in &mut labels {
        let sign = if ear.is_right { 1.0 } else { -1.0 };
        let world_pos = listener_pos + right * sign * ear_offset + ear_height;

        if let Ok(viewport_pos) = camera.world_to_viewport(camera_global, world_pos) {
            node.left = Val::Px(viewport_pos.x - 6.0);
            node.top = Val::Px(viewport_pos.y - 8.0);
            node.display = Display::Flex;
        } else {
            node.display = Display::None;
        }
    }
}

/// Draw the listener's facing direction as a cone/arrow on the ground plane.
pub(crate) fn draw_listener_direction(
    mut gizmos: Gizmos,
    listener_query: Query<&Transform, With<SoundListener>>,
    camera_query: Query<&Transform, (With<IsometricCamera>, Without<SoundListener>)>,
) {
    let Ok(listener_transform) = listener_query.single() else {
        return;
    };
    let Ok(camera_transform) = camera_query.single() else {
        return;
    };
    let center = listener_transform.translation;
    let ground_y = 0.03;

    let (camera_yaw, _, _) = camera_transform.rotation.to_euler(EulerRot::YXZ);

    let cone_color = Color::srgba(0.2, 0.8, 0.4, 0.5);
    let cone_length = 1.5;
    let inner_half_angle = 15.0_f32.to_radians();
    let outer_half_angle = 45.0_f32.to_radians();

    let center_ground = Vec3::new(center.x, ground_y, center.z);

    let forward = Vec3::new(-camera_yaw.sin(), 0.0, -camera_yaw.cos());
    let forward_end = center_ground + forward * cone_length;

    gizmos.line(center_ground, forward_end, cone_color);

    for sign in [-1.0_f32, 1.0] {
        let angle = camera_yaw + sign * inner_half_angle;
        let dir = Vec3::new(-angle.sin(), 0.0, -angle.cos());
        gizmos.line(center_ground, center_ground + dir * cone_length, cone_color);
    }

    let outer_color = Color::srgba(0.2, 0.8, 0.4, 0.25);
    for sign in [-1.0_f32, 1.0] {
        let angle = camera_yaw + sign * outer_half_angle;
        let dir = Vec3::new(-angle.sin(), 0.0, -angle.cos());
        gizmos.line(
            center_ground,
            center_ground + dir * cone_length,
            outer_color,
        );
    }

    let arc_segments = 12;
    let mut prev = None;
    for step in 0..=arc_segments {
        let t = step as f32 / arc_segments as f32;
        let angle = camera_yaw + (-inner_half_angle + t * 2.0 * inner_half_angle);
        let dir = Vec3::new(-angle.sin(), 0.0, -angle.cos());
        let point = center_ground + dir * cone_length;
        if let Some(prev_point) = prev {
            gizmos.line(prev_point, point, cone_color);
        }
        prev = Some(point);
    }
}

/// Draw directivity patterns on the ground plane using gizmos.
pub(crate) fn draw_directivity_patterns(
    mut gizmos: Gizmos,
    sources: Query<(&SoundSourceIndex, &SoundSource, &Transform)>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    let latest = messages.read().last();
    const SEGMENTS: usize = 48;
    const PATTERN_RADIUS: f32 = 1.2;

    for (index, source, source_transform) in &sources {
        let color = Color::srgba(source.color[0], source.color[1], source.color[2], 0.6);

        let (orientation_x, orientation_y) = latest
            .and_then(|msg| {
                if index.0 < msg.frame.source_count as usize {
                    let s = &msg.frame.sources[index.0];
                    Some((s.orientation_x, s.orientation_y))
                } else {
                    None
                }
            })
            .unwrap_or((1.0, 0.0));

        let source_yaw = orientation_y.atan2(orientation_x);

        let center = source_transform.translation;
        let ground_y = 0.02;

        let mut prev_point = None;
        for step in 0..=SEGMENTS {
            let theta = (step as f32 / SEGMENTS as f32) * 2.0 * PI - PI;
            let gain = pattern_gain(&source.directivity, source.directivity_alpha, theta.abs());
            let radius = gain * PATTERN_RADIUS;

            let world_angle = -source_yaw + theta;
            let point = Vec3::new(
                center.x + radius * world_angle.sin(),
                ground_y,
                center.z - radius * world_angle.cos(),
            );

            if let Some(prev) = prev_point {
                gizmos.line(prev, point, color);
            }
            prev_point = Some(point);
        }

        if source.directivity != "omni" {
            let forward_gain = pattern_gain(&source.directivity, source.directivity_alpha, 0.0);
            let forward_end = Vec3::new(
                center.x + forward_gain * PATTERN_RADIUS * (-source_yaw).sin(),
                ground_y,
                center.z - forward_gain * PATTERN_RADIUS * (-source_yaw).cos(),
            );
            gizmos.line(Vec3::new(center.x, ground_y, center.z), forward_end, color);
        }
    }
}

/// Evaluate directivity gain at an angle.
fn pattern_gain(directivity: &str, alpha: f32, angle: f32) -> f32 {
    match directivity {
        "omni" => 1.0,
        "polar" => (alpha + (1.0 - alpha) * angle.cos()).max(0.0),
        _ => 1.0,
    }
}

/// Draw audible radius rings on the ground plane.
pub(crate) fn draw_audible_rings(mut gizmos: Gizmos, sources: Query<(&SoundSource, &Transform)>) {
    const SPL_THRESHOLD: f32 = 20.0;
    const SEGMENTS: usize = 64;
    const GROUND_Y: f32 = 0.01;

    for (source, source_transform) in &sources {
        let db_above = source.spl - SPL_THRESHOLD;
        if db_above <= 0.0 {
            continue;
        }

        let radius = source.ref_distance * 10.0_f32.powf(db_above / 20.0);
        let center = source_transform.translation;
        let color = Color::srgba(source.color[0], source.color[1], source.color[2], 0.15);

        let mut prev = None;
        for step in 0..=SEGMENTS {
            let angle = (step as f32 / SEGMENTS as f32) * 2.0 * PI;
            let point = Vec3::new(
                center.x + angle.cos() * radius,
                GROUND_Y,
                center.z + angle.sin() * radius,
            );
            if let Some(prev_point) = prev {
                gizmos.line(prev_point, point, color);
            }
            prev = Some(point);
        }
    }
}

// ── Source dragging ─────────────────────────────────────────────────────────

/// Tracks which source is being dragged (if any).
#[derive(Resource, Default)]
pub(crate) struct SourceDragState {
    pub dragging: Option<usize>,
}

const PICK_RADIUS: f32 = 40.0;

/// Left-click to pick a source, drag to move it on the ground plane.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drag_sources(
    camera_query: Query<(&Camera, &GlobalTransform), With<IsometricCamera>>,
    mut sources: Query<(&SoundSourceIndex, &mut Transform)>,
    mut drag: ResMut<SourceDragState>,
    mut command_sender: ResMut<CommandSender>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    windows: Query<&Window, With<PrimaryWindow>>,
) {
    let Ok((camera, camera_global)) = camera_query.single() else {
        return;
    };
    let Ok(window) = windows.single() else {
        return;
    };

    if mouse_buttons.just_released(MouseButton::Left) {
        drag.dragging = None;
        return;
    }

    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };

    if mouse_buttons.just_pressed(MouseButton::Left) {
        if mouse_buttons.pressed(MouseButton::Right) {
            return;
        }

        let mut best_dist = PICK_RADIUS;
        let mut best_index = None;

        for (index, transform) in &sources {
            let world_pos = transform.translation;
            if let Ok(screen_pos) = camera.world_to_viewport(camera_global, world_pos) {
                let dist = screen_pos.distance(cursor_pos);
                if dist < best_dist {
                    best_dist = dist;
                    best_index = Some(index.0);
                }
            }
        }

        drag.dragging = best_index;
    }

    if let Some(dragging_index) = drag.dragging {
        if !mouse_buttons.pressed(MouseButton::Left) {
            drag.dragging = None;
            return;
        }

        let Some(ray) = camera.viewport_to_world(camera_global, cursor_pos).ok() else {
            return;
        };

        if ray.direction.y.abs() < 1e-6 {
            return;
        }
        let t = -ray.origin.y / ray.direction.y;
        if t < 0.0 {
            return;
        }
        let ground_hit = ray.origin + ray.direction * t;

        for (index, mut transform) in &mut sources {
            if index.0 == dragging_index {
                transform.translation.x = ground_hit.x;
                transform.translation.z = ground_hit.z;

                let atrium_pos = AtriumVec3::new(
                    transform.translation.x,
                    -transform.translation.z,
                    transform.translation.y,
                );
                command_sender.send(Command::SetSourcePosition {
                    index: dragging_index as u16,
                    position: atrium_pos,
                });
                break;
            }
        }
    }
}

/// Returns true if a source is currently being dragged.
pub(crate) fn is_source_dragging(drag: &SourceDragState, index: usize) -> bool {
    drag.dragging == Some(index)
}
