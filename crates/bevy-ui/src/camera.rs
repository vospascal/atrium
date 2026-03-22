//! Isometric camera with orthographic projection and yaw rotation.
//!
//! The camera looks down at a fixed pitch (~35°) and can be rotated
//! horizontally via right-click drag. WASD moves the listener,
//! scroll wheel zooms, and the camera tracks the listener position.

use atrium_core::commands::Command;
use atrium_core::types::Vec3 as AtriumVec3;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::ui::IsDefaultUiCamera;

use crate::scene::{atrium_to_bevy, SceneDescription};
use crate::telemetry::CommandSender;
use crate::weather::WeatherState;

/// Fixed camera pitch angle from horizontal (radians). ~35° gives a nice ¾ view.
const CAMERA_PITCH: f32 = 0.61;

/// Distance from the look-at point to the camera along the view direction.
/// Kept small to avoid fog darkening (ortho doesn't need large distance).
const CAMERA_DISTANCE: f32 = 15.0;

/// Mouse sensitivity for yaw rotation (radians per pixel of drag).
const YAW_SENSITIVITY: f32 = 0.005;

#[derive(Component)]
pub struct IsometricCamera;

/// Tracks the listener's position in Atrium coordinate space.
#[derive(Resource)]
pub struct ListenerState {
    /// Listener position in Atrium coordinates [x, y, z].
    pub position: [f32; 3],
    /// Listener yaw in radians.
    pub yaw: f32,
}

#[derive(Resource)]
pub struct CameraSettings {
    pub ortho_scale: f32,
    pub min_scale: f32,
    pub max_scale: f32,
    pub zoom_speed: f32,
    pub move_speed: f32,
    /// Visible world height when ortho_scale = 1.0.
    pub viewport_height: f32,
    /// Camera yaw in radians (0 = north-facing, rotated by right-click drag).
    pub camera_yaw: f32,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            ortho_scale: 1.0,
            min_scale: 0.3,
            max_scale: 5.0,
            zoom_speed: 0.15,
            move_speed: 3.0,
            viewport_height: 10.0,
            camera_yaw: 0.0,
        }
    }
}

/// Compute camera offset from the look-at target for a given yaw.
fn camera_offset(yaw: f32) -> Vec3 {
    let rotation = Quat::from_euler(EulerRot::YXZ, yaw, -CAMERA_PITCH, 0.0);
    rotation * Vec3::new(0.0, 0.0, CAMERA_DISTANCE)
}

/// Movement directions on the ground plane for a given camera yaw.
fn movement_directions(yaw: f32) -> (Vec3, Vec3) {
    let forward = Vec3::new(-yaw.sin(), 0.0, -yaw.cos());
    let right = Vec3::new(yaw.cos(), 0.0, -yaw.sin());
    (forward, right)
}

pub fn setup_camera(
    mut commands: Commands,
    description: Res<SceneDescription>,
    mut weather: ResMut<WeatherState>,
) {
    let viewport_height = description.atrium.width.max(description.atrium.depth) * 1.5;
    let settings = CameraSettings {
        ortho_scale: 1.0,
        min_scale: 0.3,
        max_scale: 5.0,
        viewport_height,
        ..default()
    };

    weather.base_fog_density = 0.002;

    let look_at = atrium_to_bevy(description.listener.position);
    let camera_pos = look_at + camera_offset(settings.camera_yaw);

    commands.spawn((
        IsometricCamera,
        IsDefaultUiCamera,
        Camera3d::default(),
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: bevy::camera::ScalingMode::FixedVertical {
                viewport_height: settings.viewport_height,
            },
            scale: settings.ortho_scale,
            near: -200.0,
            far: 200.0,
            ..OrthographicProjection::default_3d()
        }),
        Tonemapping::TonyMcMapface,
        Transform::from_translation(camera_pos).looking_at(look_at, Vec3::Y),
        AmbientLight {
            color: Color::srgb(0.8, 0.85, 0.95),
            brightness: 1000.0,
            ..default()
        },
    ));

    commands.insert_resource(settings);

    let listener_yaw = description.listener.yaw_degrees.to_radians();
    commands.insert_resource(ListenerState {
        position: description.listener.position,
        yaw: listener_yaw,
    });
}

#[allow(clippy::too_many_arguments)]
pub fn update_isometric_camera(
    mut camera: Single<(&mut Transform, &mut Projection), With<IsometricCamera>>,
    mut settings: ResMut<CameraSettings>,
    mut listener: ResMut<ListenerState>,
    mut command_sender: ResMut<CommandSender>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut scroll_messages: MessageReader<MouseWheel>,
) {
    let (ref mut transform, ref mut projection) = *camera;

    // Right-click drag to rotate camera yaw
    if mouse_buttons.pressed(MouseButton::Right) {
        let delta = mouse_motion.delta;
        settings.camera_yaw += delta.x * YAW_SENSITIVITY;
    }

    // Sync listener yaw to camera yaw (Atrium coordinates)
    let atrium_yaw = std::f32::consts::FRAC_PI_2 + settings.camera_yaw;
    let yaw_changed = (atrium_yaw - listener.yaw).abs() > 0.001;
    listener.yaw = atrium_yaw;

    // WASD — move listener relative to current camera orientation
    let (bevy_forward, bevy_right) = movement_directions(settings.camera_yaw);

    let mut move_dir = Vec3::ZERO;
    if keyboard.pressed(KeyCode::KeyW) {
        move_dir += bevy_forward;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        move_dir -= bevy_forward;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        move_dir += bevy_right;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        move_dir -= bevy_right;
    }

    let moved = move_dir != Vec3::ZERO;
    if moved {
        let bevy_delta = move_dir.normalize() * settings.move_speed * time.delta_secs();
        listener.position[0] += bevy_delta.x;
        listener.position[1] -= bevy_delta.z;
        listener.position[2] += bevy_delta.y;
    }

    if moved || yaw_changed {
        command_sender.send(Command::SetListenerPose {
            position: AtriumVec3::new(
                listener.position[0],
                listener.position[1],
                listener.position[2],
            ),
            yaw: listener.yaw,
        });
    }

    // Scroll to zoom
    for event in scroll_messages.read() {
        let scroll = match event.unit {
            MouseScrollUnit::Line => event.y * settings.zoom_speed,
            MouseScrollUnit::Pixel => event.y * settings.zoom_speed * 0.01,
        };
        settings.ortho_scale =
            (settings.ortho_scale - scroll).clamp(settings.min_scale, settings.max_scale);
    }

    // Apply zoom
    if let Projection::Orthographic(ref mut ortho) = **projection {
        ortho.scale = settings.ortho_scale;
    }

    // Camera tracks listener position with current yaw
    let look_at = atrium_to_bevy(listener.position);
    let camera_pos = look_at + camera_offset(settings.camera_yaw);
    **transform = Transform::from_translation(camera_pos).looking_at(look_at, Vec3::Y);
}
