//! Scene setup and real-time source updates.
//!
//! Reads `SceneData` to build the initial 3D scene (room wireframe, speakers,
//! sources), then updates source positions each frame from telemetry messages.

use std::f32::consts::PI;

use bevy::prelude::*;
use bevy::window::PrimaryWindow;

use atrium_core::commands::Command;
use atrium_core::types::Vec3 as AtriumVec3;

use crate::camera::OrbitCamera;
use crate::telemetry::{CommandSender, TelemetryMessage};

// ── Scene data (passed in from main binary) ──────────────────────────────────

/// Static scene description, built from the YAML config before Bevy launches.
#[derive(Resource, Clone, Debug)]
pub struct SceneData {
    /// Environment dimensions (the virtual acoustic space).
    pub environment_width: f32,
    pub environment_depth: f32,
    pub environment_height: f32,
    /// Atrium dimensions (the physical speaker room).
    pub atrium_width: f32,
    pub atrium_depth: f32,
    pub atrium_height: f32,
    /// Spawn point — where the atrium center sits in the environment.
    pub spawn: [f32; 3],
    /// Speaker world positions.
    pub speakers: Vec<SpeakerData>,
    /// Source initial state.
    pub sources: Vec<SourceData>,
    /// Listener initial position (world coordinates).
    pub listener_position: [f32; 3],
    pub listener_yaw: f32,
}

#[derive(Clone, Debug)]
pub struct SpeakerData {
    pub label: String,
    pub position: [f32; 3],
}

#[derive(Clone, Debug)]
pub struct SourceData {
    pub name: String,
    pub color: [f32; 3],
    pub position: [f32; 3],
    pub orbit_radius: f32,
    /// Source reference SPL (dB).
    pub spl: f32,
    /// Reference distance for distance attenuation (meters).
    pub ref_distance: f32,
    /// Directivity type: "omni" or "polar".
    pub directivity: String,
    /// Alpha parameter for polar patterns (1.0 = omni, 0.5 = cardioid).
    pub directivity_alpha: f32,
    /// MDAP spread [0.0, 1.0].
    pub spread: f32,
}

// ── ECS markers ──────────────────────────────────────────────────────────────

/// Marker for source entities (index into the sources array).
#[derive(Component)]
pub struct AudioSourceMarker {
    pub index: usize,
}

/// Marker for the gain line between a source and the listener.
#[derive(Component)]
pub struct GainLine {
    pub source_index: usize,
}

/// Marker for the listener entity.
#[derive(Component)]
pub struct ListenerMarker;

/// Marker for speaker entities.
#[derive(Component)]
pub struct SpeakerMarker;

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

/// Marker for the listener ear labels ("L" / "R").
#[derive(Component)]
pub(crate) struct EarLabel {
    /// true = right ear, false = left ear
    pub is_right: bool,
}

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
    scene_data: Res<SceneData>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    // Ground plane (environment floor)
    let ground_size = scene_data
        .environment_width
        .max(scene_data.environment_depth);
    commands.spawn((
        Mesh3d(meshes.add(Plane3d::default().mesh().size(ground_size, ground_size))),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgb(0.15, 0.18, 0.12),
            perceptual_roughness: 0.95,
            ..default()
        })),
        Transform::from_translation(atrium_to_bevy(scene_data.spawn)),
    ));

    // Atrium wireframe (the physical speaker room) — drawn with gizmos in update,
    // but we spawn a subtle semi-transparent floor to show the room boundary
    let atrium_center = atrium_to_bevy(scene_data.spawn);
    commands.spawn((
        Mesh3d(
            meshes.add(
                Plane3d::default()
                    .mesh()
                    .size(scene_data.atrium_width, scene_data.atrium_depth),
            ),
        ),
        MeshMaterial3d(materials.add(StandardMaterial {
            base_color: Color::srgba(0.3, 0.4, 0.5, 0.15),
            alpha_mode: AlphaMode::Blend,
            ..default()
        })),
        Transform::from_translation(atrium_center + Vec3::Y * 0.01), // slight offset to avoid z-fighting
    ));

    // Speakers — small cubes
    let speaker_mesh = meshes.add(Cuboid::new(0.15, 0.15, 0.15));
    let speaker_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.6, 0.6, 0.7),
        ..default()
    });
    for speaker in &scene_data.speakers {
        commands.spawn((
            SpeakerMarker,
            Mesh3d(speaker_mesh.clone()),
            MeshMaterial3d(speaker_material.clone()),
            Transform::from_translation(atrium_to_bevy(speaker.position)),
        ));
    }

    // Sources — colored spheres with point lights
    let source_mesh = meshes.add(Sphere::new(0.2));
    for (index, source) in scene_data.sources.iter().enumerate() {
        let color = Color::srgb(source.color[0], source.color[1], source.color[2]);
        let material = materials.add(StandardMaterial {
            base_color: color,
            emissive: LinearRgba::new(
                source.color[0] * 0.5,
                source.color[1] * 0.5,
                source.color[2] * 0.5,
                1.0,
            ),
            ..default()
        });
        commands
            .spawn((
                AudioSourceMarker { index },
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

        // Screen-space label that tracks the source's 3D position
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

    // Listener — small distinct shape
    let listener_mesh = meshes.add(Capsule3d::new(0.08, 0.3));
    let listener_material = materials.add(StandardMaterial {
        base_color: Color::srgb(0.2, 0.8, 0.4),
        emissive: LinearRgba::new(0.1, 0.4, 0.2, 1.0),
        ..default()
    });
    commands.spawn((
        ListenerMarker,
        Mesh3d(listener_mesh),
        MeshMaterial3d(listener_material),
        Transform::from_translation(atrium_to_bevy(scene_data.listener_position)),
    ));

    // Listener ear labels ("L" and "R")
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

    // Lighting
    commands.spawn((
        DirectionalLight {
            illuminance: 2000.0,
            shadows_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::XYZ, -0.8, 0.3, 0.0)),
    ));

    // Global ambient light
    commands.spawn(AmbientLight {
        color: Color::srgb(0.6, 0.65, 0.8),
        brightness: 200.0,
        affects_lightmapped_meshes: true,
    });
}

// ── Per-frame updates ────────────────────────────────────────────────────────

/// Update listener mesh position from ListenerState.
pub fn update_listener(
    mut listener: Query<&mut Transform, With<ListenerMarker>>,
    state: Res<crate::camera::ListenerState>,
) {
    let Ok(mut transform) = listener.single_mut() else {
        return;
    };
    let target = atrium_to_bevy(state.position);
    transform.translation = transform.translation.lerp(target, 0.3);
}

/// Update source positions from telemetry (skips sources being dragged).
pub(crate) fn update_sources(
    mut sources: Query<(&AudioSourceMarker, &mut Transform)>,
    mut messages: MessageReader<TelemetryMessage>,
    drag: Res<SourceDragState>,
) {
    // Only use the latest message
    let Some(msg) = messages.read().last() else {
        return;
    };
    let frame = &msg.frame;

    for (marker, mut transform) in &mut sources {
        // Skip telemetry updates for the source being dragged
        if is_source_dragging(&drag, marker.index) {
            continue;
        }
        if marker.index < frame.source_count as usize {
            let source = &frame.sources[marker.index];
            let target = atrium_to_bevy([source.x, source.y, source.z]);
            // Smooth interpolation for nice movement
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
            // Scale light intensity with gain: louder = brighter glow
            let gain = source.gain_total.clamp(0.0, 1.0);
            light.intensity = 1000.0 + gain * 15000.0;
        }
    }
}

/// Draw gain lines from sources to listener using gizmos.
pub fn update_gain_lines(
    mut gizmos: Gizmos,
    sources: Query<(&AudioSourceMarker, &Transform)>,
    listener: Query<&Transform, With<ListenerMarker>>,
    scene_data: Res<SceneData>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    let Ok(listener_transform) = listener.single() else {
        return;
    };

    // Read latest telemetry for gain values
    let latest = messages.read().last();

    for (marker, source_transform) in &sources {
        let gain = latest
            .and_then(|msg| {
                if marker.index < msg.frame.source_count as usize {
                    Some(msg.frame.sources[marker.index].gain_total)
                } else {
                    None
                }
            })
            .unwrap_or(0.2);

        let alpha = 0.1 + gain.clamp(0.0, 1.0) * 0.6;
        let source_data = &scene_data.sources[marker.index];
        let color = Color::srgba(
            source_data.color[0],
            source_data.color[1],
            source_data.color[2],
            alpha,
        );

        gizmos.line(
            source_transform.translation,
            listener_transform.translation,
            color,
        );
    }

    // Draw atrium wireframe
    let center = atrium_to_bevy(scene_data.spawn);
    let half_w = scene_data.atrium_width / 2.0;
    let half_d = scene_data.atrium_depth / 2.0;
    let height = scene_data.atrium_height;

    let wireframe_color = Color::srgba(0.4, 0.5, 0.6, 0.3);

    // Floor rectangle
    let corners_floor = [
        center + Vec3::new(-half_w, 0.0, -half_d),
        center + Vec3::new(half_w, 0.0, -half_d),
        center + Vec3::new(half_w, 0.0, half_d),
        center + Vec3::new(-half_w, 0.0, half_d),
    ];
    for i in 0..4 {
        gizmos.line(
            corners_floor[i],
            corners_floor[(i + 1) % 4],
            wireframe_color,
        );
    }

    // Ceiling rectangle
    let corners_ceiling = corners_floor.map(|c| c + Vec3::Y * height);
    for i in 0..4 {
        gizmos.line(
            corners_ceiling[i],
            corners_ceiling[(i + 1) % 4],
            wireframe_color,
        );
    }

    // Vertical edges
    for i in 0..4 {
        gizmos.line(corners_floor[i], corners_ceiling[i], wireframe_color);
    }
}

/// Position screen-space labels above their corresponding 3D source positions.
pub(crate) fn billboard_labels(
    camera_query: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    sources: Query<(&AudioSourceMarker, &GlobalTransform)>,
    mut labels: Query<(&SourceLabel, &mut Node)>,
) {
    let Ok((camera, camera_global)) = camera_query.single() else {
        return;
    };

    for (label, mut node) in &mut labels {
        let Some((_, source_global)) = sources.iter().find(|(m, _)| m.index == label.index) else {
            continue;
        };

        // Project source position (offset up above the sphere) to screen space
        let world_pos = source_global.translation() + Vec3::Y * 0.5;
        if let Ok(viewport_pos) = camera.world_to_viewport(camera_global, world_pos) {
            node.left = Val::Px(viewport_pos.x - 30.0);
            node.top = Val::Px(viewport_pos.y - 20.0);
            node.display = Display::Flex;
        } else {
            // Behind camera — hide off-screen
            node.display = Display::None;
        }
    }
}

/// Position "L" and "R" labels at the listener's ear positions (screen-space).
pub(crate) fn update_ear_labels(
    camera_query: Query<(&Camera, &GlobalTransform, &Transform), With<OrbitCamera>>,
    listener_query: Query<&Transform, (With<ListenerMarker>, Without<OrbitCamera>)>,
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
    let ear_offset = 0.5; // distance from center to ear position
    let ear_height = Vec3::Y * 0.4; // slightly above listener center

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
    listener_query: Query<&Transform, With<ListenerMarker>>,
    camera_query: Query<&Transform, (With<OrbitCamera>, Without<ListenerMarker>)>,
) {
    let Ok(listener_transform) = listener_query.single() else {
        return;
    };
    let Ok(camera_transform) = camera_query.single() else {
        return;
    };
    let center = listener_transform.translation;
    let ground_y = 0.03;

    // Use camera yaw directly — the cone should point where the camera looks
    let (camera_yaw, _, _) = camera_transform.rotation.to_euler(EulerRot::YXZ);

    let cone_color = Color::srgba(0.2, 0.8, 0.4, 0.5);
    let cone_length = 1.5;
    let inner_half_angle = 15.0_f32.to_radians();
    let outer_half_angle = 45.0_f32.to_radians();

    let center_ground = Vec3::new(center.x, ground_y, center.z);

    // Camera forward projected onto the ground plane (Bevy Y-up, yaw around Y)
    let forward = Vec3::new(-camera_yaw.sin(), 0.0, -camera_yaw.cos());
    let forward_end = center_ground + forward * cone_length;

    // Center line
    gizmos.line(center_ground, forward_end, cone_color);

    // Inner cone edges
    for sign in [-1.0_f32, 1.0] {
        let angle = camera_yaw + sign * inner_half_angle;
        let dir = Vec3::new(-angle.sin(), 0.0, -angle.cos());
        gizmos.line(center_ground, center_ground + dir * cone_length, cone_color);
    }

    // Outer cone edges (dimmer)
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

    // Arc at the end of the inner cone
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
    sources: Query<(&AudioSourceMarker, &Transform)>,
    scene_data: Res<SceneData>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    let latest = messages.read().last();
    const SEGMENTS: usize = 48;
    const PATTERN_RADIUS: f32 = 1.2;

    for (marker, source_transform) in &sources {
        let source_data = &scene_data.sources[marker.index];
        let color = Color::srgba(
            source_data.color[0],
            source_data.color[1],
            source_data.color[2],
            0.6,
        );

        // Get orientation from telemetry (or default forward)
        let (orientation_x, orientation_y) = latest
            .and_then(|msg| {
                if marker.index < msg.frame.source_count as usize {
                    let s = &msg.frame.sources[marker.index];
                    Some((s.orientation_x, s.orientation_y))
                } else {
                    None
                }
            })
            .unwrap_or((1.0, 0.0));

        // Source yaw from orientation: engine +X/+Y → Bevy ground plane
        // Engine orientation (ox, oy) → Bevy rotation around Y
        let source_yaw = orientation_y.atan2(orientation_x);

        let center = source_transform.translation;
        // Draw on the ground plane just above it
        let ground_y = 0.02;

        let mut prev_point = None;
        for step in 0..=SEGMENTS {
            let theta = (step as f32 / SEGMENTS as f32) * 2.0 * PI - PI;
            let gain = pattern_gain(
                &source_data.directivity,
                source_data.directivity_alpha,
                theta.abs(),
            );
            let radius = gain * PATTERN_RADIUS;

            // Polar to Cartesian on the Bevy XZ ground plane, rotated by source yaw
            // In Atrium coords: forward = +Y, but we convert to Bevy: Z = -Y
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

        // Draw a forward direction line (thicker visual cue)
        if source_data.directivity != "omni" {
            let forward_gain =
                pattern_gain(&source_data.directivity, source_data.directivity_alpha, 0.0);
            let forward_end = Vec3::new(
                center.x + forward_gain * PATTERN_RADIUS * (-source_yaw).sin(),
                ground_y,
                center.z - forward_gain * PATTERN_RADIUS * (-source_yaw).cos(),
            );
            gizmos.line(Vec3::new(center.x, ground_y, center.z), forward_end, color);
        }
    }
}

/// Evaluate directivity gain at an angle (mirrors Rust DirectivityPattern::gain_at_angle).
fn pattern_gain(directivity: &str, alpha: f32, angle: f32) -> f32 {
    match directivity {
        "omni" => 1.0,
        "polar" => (alpha + (1.0 - alpha) * angle.cos()).max(0.0),
        _ => 1.0,
    }
}

/// Draw audible radius rings (20 dB SPL hearing floor) on the ground plane.
pub(crate) fn draw_audible_rings(
    mut gizmos: Gizmos,
    sources: Query<(&AudioSourceMarker, &Transform)>,
    scene_data: Res<SceneData>,
) {
    const SPL_THRESHOLD: f32 = 20.0; // dB SPL hearing floor
    const SEGMENTS: usize = 64;
    const GROUND_Y: f32 = 0.01;

    for (marker, source_transform) in &sources {
        let source_data = &scene_data.sources[marker.index];
        let db_above = source_data.spl - SPL_THRESHOLD;
        if db_above <= 0.0 {
            continue;
        }

        // Audible radius: ref_dist * 10^(db_above / 20)
        let radius = source_data.ref_distance * 10.0_f32.powf(db_above / 20.0);
        let center = source_transform.translation;
        let color = Color::srgba(
            source_data.color[0],
            source_data.color[1],
            source_data.color[2],
            0.15,
        );

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
    /// Index of the source currently being dragged.
    pub dragging: Option<usize>,
}

/// Pick radius for selecting a source (screen-space distance in pixels).
const PICK_RADIUS: f32 = 40.0;

/// Left-click to pick a source, drag to move it on the ground plane.
#[allow(clippy::too_many_arguments)]
pub(crate) fn drag_sources(
    camera_query: Query<(&Camera, &GlobalTransform), With<OrbitCamera>>,
    mut sources: Query<(&AudioSourceMarker, &mut Transform)>,
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

    // On left-click release, stop dragging
    if mouse_buttons.just_released(MouseButton::Left) {
        drag.dragging = None;
        return;
    }

    let Some(cursor_pos) = window.cursor_position() else {
        return;
    };

    // On left-click press (not right-click which is orbit), pick nearest source
    if mouse_buttons.just_pressed(MouseButton::Left) {
        // Don't start drag if right button is also held (orbiting)
        if mouse_buttons.pressed(MouseButton::Right) {
            return;
        }

        let mut best_dist = PICK_RADIUS;
        let mut best_index = None;

        for (marker, transform) in &sources {
            let world_pos = transform.translation;
            if let Ok(screen_pos) = camera.world_to_viewport(camera_global, world_pos) {
                let dist = screen_pos.distance(cursor_pos);
                if dist < best_dist {
                    best_dist = dist;
                    best_index = Some(marker.index);
                }
            }
        }

        drag.dragging = best_index;
    }

    // While dragging, raycast cursor onto the ground plane (Y=0) and move source
    if let Some(dragging_index) = drag.dragging {
        if !mouse_buttons.pressed(MouseButton::Left) {
            drag.dragging = None;
            return;
        }

        // Cast ray from cursor through camera onto the Y=0 ground plane
        let Some(ray) = camera.viewport_to_world(camera_global, cursor_pos).ok() else {
            return;
        };

        // Intersect with Y=0 plane: ray.origin.y + t * ray.direction.y = 0
        if ray.direction.y.abs() < 1e-6 {
            return; // Ray is parallel to ground
        }
        let t = -ray.origin.y / ray.direction.y;
        if t < 0.0 {
            return; // Intersection is behind camera
        }
        let ground_hit = ray.origin + ray.direction * t;

        // Update the source's Bevy transform
        for (marker, mut transform) in &mut sources {
            if marker.index == dragging_index {
                transform.translation.x = ground_hit.x;
                transform.translation.z = ground_hit.z;
                // Keep original height (Y in Bevy = Z in Atrium)

                // Convert Bevy position back to Atrium coordinates and send command
                // Atrium.X = Bevy.X, Atrium.Y = -Bevy.Z, Atrium.Z = Bevy.Y
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

/// Returns true if a source is currently being dragged (used to suppress telemetry position updates).
pub(crate) fn is_source_dragging(drag: &SourceDragState, index: usize) -> bool {
    drag.dragging == Some(index)
}
