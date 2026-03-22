//! Orbit camera with WASD listener movement, mouse orbit, fog, and depth of field.
//!
//! WASD moves the **listener** in Atrium coordinates, sending SetListenerPose
//! commands to the audio engine. The camera orbits around the listener.
//! Camera is clamped above the ground plane.

use std::f32::consts::FRAC_PI_2;
use std::ops::Range;

use atrium_core::commands::Command;
use atrium_core::types::Vec3 as AtriumVec3;
use bevy::core_pipeline::tonemapping::Tonemapping;
use bevy::input::mouse::{AccumulatedMouseMotion, MouseScrollUnit, MouseWheel};
use bevy::post_process::dof::{DepthOfField, DepthOfFieldMode};
use bevy::prelude::*;

use bevy::ui::IsDefaultUiCamera;

use crate::scene::{atrium_to_bevy, SceneDescription};
use crate::telemetry::CommandSender;

/// Minimum camera height above the ground plane.
const MIN_CAMERA_HEIGHT: f32 = 0.5;

#[derive(Component)]
pub struct OrbitCamera;

/// Tracks the listener's position in Atrium coordinate space.
/// Bevy.X = Atrium.X, Bevy.Y = Atrium.Z, Bevy.Z = -Atrium.Y
#[derive(Resource)]
pub struct ListenerState {
    /// Listener position in Atrium coordinates [x, y, z].
    pub position: [f32; 3],
    /// Listener yaw in radians.
    pub yaw: f32,
}

#[derive(Resource)]
pub struct CameraSettings {
    pub orbit_distance: f32,
    pub pitch_speed: f32,
    pub yaw_speed: f32,
    pub zoom_speed: f32,
    pub move_speed: f32,
    pub pitch_range: Range<f32>,
    pub min_distance: f32,
    pub max_distance: f32,
}

impl Default for CameraSettings {
    fn default() -> Self {
        let pitch_limit = FRAC_PI_2 - 0.01;
        Self {
            orbit_distance: 15.0,
            pitch_speed: 0.003,
            yaw_speed: 0.004,
            zoom_speed: 1.0,
            move_speed: 3.0,
            pitch_range: -pitch_limit..pitch_limit,
            min_distance: 2.0,
            max_distance: 80.0,
        }
    }
}

pub fn setup_camera(mut commands: Commands, description: Res<SceneDescription>) {
    let spawn = atrium_to_bevy(description.environment.spawn);

    let settings = CameraSettings {
        orbit_distance: description.atrium.width.max(description.atrium.depth) * 2.0,
        max_distance: description
            .environment
            .width
            .max(description.environment.depth),
        ..default()
    };

    // Fog density tuned so visibility drops to ~5% at max_distance
    let fog_density = 3.0 / settings.max_distance;

    commands.spawn((
        OrbitCamera,
        IsDefaultUiCamera,
        Camera3d::default(),
        Tonemapping::TonyMcMapface,
        Transform::from_xyz(spawn.x + 8.0, 10.0, spawn.z + 8.0).looking_at(spawn, Vec3::Y),
        DistanceFog {
            color: Color::srgb(0.08, 0.08, 0.10),
            falloff: FogFalloff::ExponentialSquared {
                density: fog_density,
            },
            ..default()
        },
        DepthOfField {
            mode: DepthOfFieldMode::Gaussian,
            focal_distance: settings.orbit_distance,
            aperture_f_stops: 1.0 / 4.0,
            max_depth: settings.max_distance,
            ..default()
        },
    ));

    commands.insert_resource(settings);
    commands.insert_resource(ListenerState {
        position: description.listener.position,
        yaw: description.listener.yaw_degrees.to_radians(),
    });
}

#[allow(clippy::too_many_arguments)]
pub fn orbit_camera(
    mut camera: Single<(&mut Transform, &mut DepthOfField), With<OrbitCamera>>,
    mut settings: ResMut<CameraSettings>,
    mut listener: ResMut<ListenerState>,
    mut command_sender: ResMut<CommandSender>,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    keyboard: Res<ButtonInput<KeyCode>>,
    time: Res<Time>,
    mut scroll_messages: MessageReader<MouseWheel>,
) {
    let (ref mut transform, ref mut dof) = *camera;

    // Right-click drag to orbit
    if mouse_buttons.pressed(MouseButton::Right) {
        let delta = mouse_motion.delta;
        let delta_pitch = delta.y * settings.pitch_speed;
        let delta_yaw = delta.x * settings.yaw_speed;

        let (yaw, pitch, _roll) = transform.rotation.to_euler(EulerRot::YXZ);
        let pitch =
            (pitch + delta_pitch).clamp(settings.pitch_range.start, settings.pitch_range.end);
        let yaw = yaw + delta_yaw;
        transform.rotation = Quat::from_euler(EulerRot::YXZ, yaw, pitch, 0.0);
    }

    // WASD — move listener relative to camera yaw (on the ground plane)
    let (camera_yaw, _, _) = transform.rotation.to_euler(EulerRot::YXZ);
    let bevy_forward = Vec3::new(-camera_yaw.sin(), 0.0, -camera_yaw.cos());
    let bevy_right = Vec3::new(camera_yaw.cos(), 0.0, -camera_yaw.sin());

    let atrium_yaw = FRAC_PI_2 + camera_yaw;
    let yaw_changed = (atrium_yaw - listener.yaw).abs() > 0.001;
    listener.yaw = atrium_yaw;

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
        settings.orbit_distance =
            (settings.orbit_distance - scroll).clamp(settings.min_distance, settings.max_distance);
    }

    dof.focal_distance = settings.orbit_distance;

    let orbit_target = atrium_to_bevy(listener.position);
    transform.translation = orbit_target - transform.forward() * settings.orbit_distance;
    transform.translation.y = transform.translation.y.max(MIN_CAMERA_HEIGHT);
}
