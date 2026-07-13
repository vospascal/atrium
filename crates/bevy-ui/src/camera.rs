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
use bevy::window::PrimaryWindow;

use crate::ecs::{SoundListener, SoundSource};
use crate::scene::{atrium_to_world, SceneDescription};
use atrium_behavior::CommandSender;

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
    pub home_scale: f32,
    /// Minimum received relative level included by the fit-to-audible action.
    pub fit_audibility_db: f32,
    /// Listener move speed (metres/second).
    pub move_speed: f32,
    /// Listener turn speed (radians/second).
    pub turn_speed: f32,
}

/// User-controlled map offset. Dragging empty map space updates this while the
/// camera continues to track listener movement underneath it.
#[derive(Resource, Default)]
pub struct CameraPan {
    pub offset: Vec2,
    last_cursor: Option<Vec2>,
}

impl Default for CameraSettings {
    fn default() -> Self {
        Self {
            ortho_scale: 0.25,
            home_scale: 0.25,
            fit_audibility_db: -60.0,
            move_speed: 3.0,
            turn_speed: 2.0,
        }
    }
}

impl CameraSettings {
    pub const MIN_ZOOM_PERCENT: f32 = 50.0;
    pub const MAX_ZOOM_PERCENT: f32 = 250.0;
    pub const ZOOM_STEP_PERCENT: f32 = 10.0;

    pub fn zoom_percent(&self) -> f32 {
        100.0 * self.home_scale / self.ortho_scale
    }

    pub fn set_zoom_percent(&mut self, percentage: f32) {
        let snapped = (percentage / Self::ZOOM_STEP_PERCENT).round() * Self::ZOOM_STEP_PERCENT;
        let snapped = snapped.clamp(Self::MIN_ZOOM_PERCENT, Self::MAX_ZOOM_PERCENT);
        self.ortho_scale = self.home_scale * 100.0 / snapped;
    }

    pub fn step_zoom(&mut self, delta_percent: f32) {
        self.set_zoom_percent(self.zoom_percent() + delta_percent);
    }

    pub fn set_scale_snapped(&mut self, scale: f32) {
        let percentage = 100.0 * self.home_scale / scale.max(f32::EPSILON);
        self.set_zoom_percent(percentage);
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
        BoxShadowSamples(6),
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
    commands.insert_resource(CameraPan::default());
}

/// Grab empty map space with the primary mouse button to pan. Source markers
/// keep their existing drag behavior because a press that starts on a marker
/// is deliberately ignored here.
pub fn pan_camera(
    windows: Query<&Window, With<PrimaryWindow>>,
    camera: Query<(&Camera, &GlobalTransform), With<TopDownCamera>>,
    sources: Query<&GlobalTransform, With<SoundSource>>,
    mouse: Res<ButtonInput<MouseButton>>,
    over_hud: Res<crate::hud::PointerOverHud>,
    mut pan: ResMut<CameraPan>,
) {
    if mouse.just_released(MouseButton::Left) || !mouse.pressed(MouseButton::Left) {
        pan.last_cursor = None;
        return;
    }

    let Some(cursor) = windows.single().ok().and_then(Window::cursor_position) else {
        pan.last_cursor = None;
        return;
    };
    let Ok((camera, camera_global)) = camera.single() else {
        return;
    };

    if mouse.just_pressed(MouseButton::Left) {
        if over_hud.0 {
            return;
        }
        let over_source = sources.iter().any(|source| {
            camera
                .world_to_viewport(camera_global, source.translation())
                .is_ok_and(|viewport| viewport.distance(cursor) <= 40.0)
        });
        if !over_source {
            pan.last_cursor = Some(cursor);
        }
        return;
    }

    let Some(previous_cursor) = pan.last_cursor else {
        return;
    };
    let (Ok(previous_world), Ok(current_world)) = (
        camera.viewport_to_world_2d(camera_global, previous_cursor),
        camera.viewport_to_world_2d(camera_global, cursor),
    ) else {
        pan.last_cursor = Some(cursor);
        return;
    };
    pan.offset -= current_world - previous_world;
    pan.last_cursor = Some(cursor);
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
    mut scroll_accumulator: Local<f32>,
    gamepads: Query<&Gamepad>,
    over_hud: Res<crate::hud::PointerOverHud>,
    pan: Res<CameraPan>,
) {
    let (ref mut transform, ref mut projection) = *camera;

    // Always drain the wheel events; only apply zoom when the cursor isn't over
    // a HUD panel (there the wheel scrolls the panel instead).
    for event in scroll.read() {
        *scroll_accumulator += match event.unit {
            MouseScrollUnit::Line => event.y,
            MouseScrollUnit::Pixel => event.y / 100.0,
        };
    }
    if over_hud.0 {
        *scroll_accumulator = 0.0;
    } else if scroll_accumulator.abs() >= 1.0 {
        settings.step_zoom(scroll_accumulator.signum() * CameraSettings::ZOOM_STEP_PERCENT);
        // One deliberate gesture produces one zoom step; discard momentum so
        // a trackpad flick cannot race through several levels.
        *scroll_accumulator = 0.0;
    }

    // Gamepad triggers use the same discrete 10% steps.
    for gamepad in &gamepads {
        if gamepad.just_pressed(GamepadButton::RightTrigger2) {
            settings.step_zoom(CameraSettings::ZOOM_STEP_PERCENT);
        }
        if gamepad.just_pressed(GamepadButton::LeftTrigger2) {
            settings.step_zoom(-CameraSettings::ZOOM_STEP_PERCENT);
        }
    }

    if let Projection::Orthographic(ref mut ortho) = **projection {
        ortho.scale = settings.ortho_scale;
    }

    let look_at = atrium_to_world(state.position);
    transform.translation.x = look_at.x + pan.offset.x;
    transform.translation.y = look_at.y + pan.offset.y;
}

#[cfg(test)]
mod tests {
    use super::CameraSettings;

    #[test]
    fn zoom_uses_ten_percent_steps() {
        let mut settings = CameraSettings::default();
        settings.step_zoom(CameraSettings::ZOOM_STEP_PERCENT);
        assert_eq!(settings.zoom_percent(), 110.0);
        settings.step_zoom(-CameraSettings::ZOOM_STEP_PERCENT);
        assert_eq!(settings.zoom_percent(), 100.0);
    }

    #[test]
    fn zoom_is_clamped_to_fifty_and_two_hundred_fifty_percent() {
        let mut settings = CameraSettings::default();
        settings.set_zoom_percent(1_000.0);
        assert_eq!(settings.zoom_percent(), 250.0);
        settings.set_zoom_percent(-100.0);
        assert_eq!(settings.zoom_percent(), 50.0);
    }

    #[test]
    fn computed_fit_scale_snaps_to_a_valid_zoom_step() {
        let mut settings = CameraSettings::default();
        settings.set_scale_snapped(0.30);
        assert_eq!(settings.zoom_percent(), 80.0);
    }
}
