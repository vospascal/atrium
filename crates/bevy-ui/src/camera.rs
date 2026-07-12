//! Top-down 2D camera and listener control.
//!
//! An orthographic `Camera2d` looks straight down at the ground plane
//! (a schematic "radar" view of the atrium). WASD moves the listener on the
//! plane, Q/E rotate the listener's facing, and the scroll wheel zooms.
//! The camera tracks the listener position; north is always up (no rotation).

use atrium_core::commands::Command;
use atrium_core::types::Vec3 as AtriumVec3;
use bevy::camera::ScalingMode;
use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;

use crate::ecs::SoundListener;
use crate::scene::{atrium_to_world, SceneDescription};
use crate::telemetry::CommandSender;

/// How the listener is drawn in the plane; its facing lives in `ListenerState`.
#[derive(Component)]
pub struct TopDownCamera;

/// Analog-stick / trigger deadzone — PS4 sticks rest slightly off-center and
/// triggers report tiny non-zero values at rest.
const STICK_DEADZONE: f32 = 0.15;

/// Apply a radial deadzone and rescale so travel starts at 0 just past the
/// deadzone (avoids a jump from 0 → deadzone when the stick first engages).
fn deadzone(value: f32) -> f32 {
    if value.abs() < STICK_DEADZONE {
        0.0
    } else {
        let sign = value.signum();
        sign * (value.abs() - STICK_DEADZONE) / (1.0 - STICK_DEADZONE)
    }
}

/// Tracks the listener's position and facing in Atrium coordinate space.
/// This is the single source of truth for the listener (unlike sources, which
/// are driven by telemetry). The HUD reads position/yaw from here.
#[derive(Resource)]
pub struct ListenerState {
    /// Listener position in Atrium coordinates [x, y, z].
    pub position: [f32; 3],
    /// Listener facing (yaw) in radians. 0 = +X, π/2 = +Y (front).
    pub yaw: f32,
}

#[derive(Resource)]
pub struct CameraSettings {
    pub ortho_scale: f32,
    pub min_scale: f32,
    pub max_scale: f32,
    pub zoom_speed: f32,
    /// Listener move speed (metres/second).
    pub move_speed: f32,
    /// Listener turn speed (radians/second).
    pub turn_speed: f32,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            ortho_scale: 1.0,
            min_scale: 0.3,
            max_scale: 5.0,
            zoom_speed: 0.15,
            move_speed: 3.0,
            turn_speed: 2.0,
        }
    }
}

/// PostStartup: spawn the 2D camera and insert control resources.
pub fn setup_camera(mut commands: Commands, description: Res<SceneDescription>) {
    // Show the whole environment plus a margin.
    let env = &description.environment;
    let viewport_height = env.width.max(env.depth) * 1.4;
    let settings = CameraSettings::default();

    let look_at = atrium_to_world(description.listener.position);

    commands.spawn((
        TopDownCamera,
        Camera2d,
        Projection::Orthographic(OrthographicProjection {
            scaling_mode: ScalingMode::FixedVertical { viewport_height },
            scale: settings.ortho_scale,
            ..OrthographicProjection::default_2d()
        }),
        // Keep camera z small: the default ortho far plane is 1000, so a large
        // camera z clips negative-z layers (the landscape decor at z -9..-6).
        Transform::from_xyz(look_at.x, look_at.y, 50.0),
    ));

    commands.insert_resource(ListenerState {
        position: description.listener.position,
        yaw: description.listener.yaw_degrees.to_radians(),
    });
    commands.insert_resource(settings);
}

/// Update: WASD / left stick moves the listener; Q/E / right stick rotate its
/// facing. Writes the listener entity transform + `ListenerState` and notifies
/// audio. Gamepad sticks are analog (partial deflection = slower), keyboard is
/// full-speed.
pub fn move_listener(
    mut listener: Single<&mut Transform, With<SoundListener>>,
    mut state: ResMut<ListenerState>,
    settings: Res<CameraSettings>,
    keyboard: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    time: Res<Time>,
    mut command_sender: ResMut<CommandSender>,
) {
    let dt = time.delta_secs();

    // Movement in world axes: W = +Y (front), S = -Y, A = -X, D = +X.
    let mut move_dir = Vec2::ZERO;
    if keyboard.pressed(KeyCode::KeyW) || keyboard.pressed(KeyCode::ArrowUp) {
        move_dir.y += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyS) || keyboard.pressed(KeyCode::ArrowDown) {
        move_dir.y -= 1.0;
    }
    if keyboard.pressed(KeyCode::KeyD) || keyboard.pressed(KeyCode::ArrowRight) {
        move_dir.x += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyA) || keyboard.pressed(KeyCode::ArrowLeft) {
        move_dir.x -= 1.0;
    }

    // Turning: Q = counter-clockwise, E = clockwise.
    let mut turn = 0.0;
    if keyboard.pressed(KeyCode::KeyQ) {
        turn += 1.0;
    }
    if keyboard.pressed(KeyCode::KeyE) {
        turn -= 1.0;
    }

    // Gamepad: left stick moves, right stick X turns. Additive with keyboard.
    for gamepad in &gamepads {
        move_dir.x += deadzone(gamepad.get(GamepadAxis::LeftStickX).unwrap_or(0.0));
        move_dir.y += deadzone(gamepad.get(GamepadAxis::LeftStickY).unwrap_or(0.0));
        turn -= deadzone(gamepad.get(GamepadAxis::RightStickX).unwrap_or(0.0));
    }

    let mut changed = false;

    if move_dir != Vec2::ZERO {
        // Clamp to unit length so diagonals/keyboard aren't faster, but keep
        // sub-unit analog magnitude from a partly-deflected stick.
        let len = move_dir.length();
        if len > 1.0 {
            move_dir /= len;
        }
        // No clamp on position — the listener roams freely (camera follows).
        let delta = move_dir * settings.move_speed * dt;
        state.position[0] += delta.x;
        state.position[1] += delta.y;
        changed = true;
    }
    let turn = turn.clamp(-1.0, 1.0);
    if turn != 0.0 {
        state.yaw += turn * settings.turn_speed * dt;
        changed = true;
    }

    if changed {
        let world = atrium_to_world(state.position);
        listener.translation.x = world.x;
        listener.translation.y = world.y;

        command_sender.send(Command::SetListenerPose {
            position: AtriumVec3::new(state.position[0], state.position[1], state.position[2]),
            yaw: state.yaw,
        });
    }
}

/// Update: scroll / gamepad triggers zoom, camera tracks the listener (north
/// stays up).
pub fn follow_and_zoom_camera(
    mut camera: Single<(&mut Transform, &mut Projection), With<TopDownCamera>>,
    mut settings: ResMut<CameraSettings>,
    state: Res<ListenerState>,
    mut scroll: MessageReader<MouseWheel>,
    gamepads: Query<&Gamepad>,
    time: Res<Time>,
    over_hud: Res<crate::hud::PointerOverHud>,
) {
    let (ref mut transform, ref mut projection) = *camera;

    // Always drain the wheel events; only apply zoom when the cursor isn't over
    // a HUD panel (there the wheel scrolls the panel instead).
    let mut step = 0.0;
    for event in scroll.read() {
        step += match event.unit {
            MouseScrollUnit::Line => event.y * settings.zoom_speed,
            MouseScrollUnit::Pixel => event.y * settings.zoom_speed * 0.01,
        };
    }
    if !over_hud.0 {
        settings.ortho_scale =
            (settings.ortho_scale - step).clamp(settings.min_scale, settings.max_scale);
    }

    // Gamepad triggers: R2 zooms in (smaller scale), L2 zooms out.
    let dt = time.delta_secs();
    for gamepad in &gamepads {
        let zoom_in = gamepad.get(GamepadButton::RightTrigger2).unwrap_or(0.0);
        let zoom_out = gamepad.get(GamepadButton::LeftTrigger2).unwrap_or(0.0);
        let zoom_delta = (zoom_out - zoom_in) * settings.zoom_speed * 12.0 * dt;
        if zoom_delta != 0.0 {
            settings.ortho_scale =
                (settings.ortho_scale + zoom_delta).clamp(settings.min_scale, settings.max_scale);
        }
    }

    if let Projection::Orthographic(ref mut ortho) = **projection {
        ortho.scale = settings.ortho_scale;
    }

    let look_at = atrium_to_world(state.position);
    transform.translation.x = look_at.x;
    transform.translation.y = look_at.y;
}
