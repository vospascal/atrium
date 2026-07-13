//! Gamepad button actions (PS4 / any gilrs-supported pad).
//!
//! Stick movement + trigger zoom live in [`crate::camera`]; this module handles
//! the discrete button presses: shoulder buttons cycle the render mode and the
//! South (Cross) button resets the scene. The bindings overlay toggle lives in
//! [`crate::bindings`] so keyboard + gamepad share one toggle path.
//!
//! Gamepads connect automatically via `GilrsPlugin` (part of `DefaultPlugins`),
//! spawning one entity with a `Gamepad` component each.

use atrium_core::commands::Command;
use atrium_core::speaker::RenderMode;
use bevy::prelude::*;

use crate::telemetry::LatestTelemetry;
use atrium_behavior::CommandSender;

/// Cycle the render mode with L1/R1 and reset the scene with South (Cross).
pub(crate) fn handle_gamepad_actions(
    gamepads: Query<&Gamepad>,
    mut command_sender: ResMut<CommandSender>,
    telemetry: Res<LatestTelemetry>,
) {
    for gamepad in &gamepads {
        // Shoulder buttons cycle render mode (R1 next, L1 previous).
        let next = gamepad.just_pressed(GamepadButton::RightTrigger);
        let prev = gamepad.just_pressed(GamepadButton::LeftTrigger);
        if next || prev {
            let current = telemetry.frame.render_mode.index();
            let count = RenderMode::ALL.len();
            let target = if next {
                (current + 1) % count
            } else {
                (current + count - 1) % count
            };
            command_sender.send(Command::SetRenderMode {
                mode: RenderMode::ALL[target],
            });
        }

        // South (Cross on PS4) resets the scene.
        if gamepad.just_pressed(GamepadButton::South) {
            command_sender.send(Command::ResetScene);
        }
    }
}
