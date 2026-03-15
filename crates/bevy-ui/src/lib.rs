//! Bevy 3D visualization for the Atrium spatial audio engine.
//!
//! Renders the environment, atrium, speakers, and sound sources in real time,
//! driven by telemetry from the audio thread via an rtrb ring buffer.

mod camera;
mod hud;
mod input;
pub mod scene;
mod telemetry;

use bevy::dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin};
use bevy::prelude::*;
use bevy::window::WindowResolution;

pub use scene::{SceneData, SourceData, SpeakerData};
pub use telemetry::{CommandSender, TelemetryReceiver};

/// Main Atrium visualization plugin.
/// Requires `SceneData` and `TelemetryReceiver` as resources before `App::run()`.
pub struct AtriumPlugin;

impl Plugin for AtriumPlugin {
    fn build(&self, app: &mut App) {
        app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Atrium".into(),
                resolution: WindowResolution::new(1280, 720),
                ..default()
            }),
            ..default()
        }))
        .add_plugins(FpsOverlayPlugin {
            config: FpsOverlayConfig {
                text_config: TextFont {
                    font_size: 14.0,
                    ..default()
                },
                text_color: Color::srgba(0.8, 0.8, 0.8, 0.6),
                enabled: true,
                ..default()
            },
        })
        .init_resource::<telemetry::LatestTelemetry>()
        .init_resource::<scene::SourceDragState>()
        .add_message::<telemetry::TelemetryMessage>()
        .add_systems(
            Startup,
            (
                scene::setup_scene,
                camera::setup_camera,
                hud::setup_hud,
                init_orbit_speeds,
            ),
        )
        .add_systems(
            Update,
            (
                telemetry::poll_telemetry,
                (
                    // Scene updates
                    (
                        scene::update_listener,
                        scene::update_sources,
                        scene::update_source_lights,
                        scene::update_gain_lines,
                        scene::billboard_labels,
                        scene::update_ear_labels,
                        scene::draw_directivity_patterns,
                        scene::draw_audible_rings,
                        scene::draw_listener_direction,
                        scene::drag_sources,
                    ),
                    // HUD updates
                    (
                        hud::update_hud_sources,
                        hud::update_hud_listener,
                        hud::update_hud_meters,
                        hud::update_hud_pipeline,
                    ),
                    // Input handlers
                    (
                        input::handle_render_mode_buttons,
                        input::handle_channel_mode_buttons,
                        input::handle_mute_buttons,
                        input::handle_pause_buttons,
                        input::handle_atmosphere_buttons,
                        input::handle_reset_button,
                    ),
                    // Input sync
                    (
                        input::sync_render_mode_buttons,
                        input::sync_channel_mode_buttons,
                        input::sync_mute_buttons,
                        input::sync_pause_buttons,
                        input::sync_atmosphere_text,
                    ),
                ),
                camera::orbit_camera,
            )
                .chain(),
        );
    }
}

fn init_orbit_speeds(mut commands: Commands, scene_data: Res<scene::SceneData>) {
    let speeds = scene_data
        .sources
        .iter()
        .map(|s| if s.orbit_radius > 0.0 { 1.0 } else { 0.0 })
        .collect();
    commands.insert_resource(input::SourceOrbitSpeeds { speeds });
}

/// Launch the Bevy visualization. Blocks the calling thread (Bevy owns the event loop).
pub fn run(
    scene_data: SceneData,
    telemetry_receiver: TelemetryReceiver,
    command_sender: CommandSender,
) {
    App::new()
        .insert_resource(scene_data)
        .insert_resource(telemetry_receiver)
        .insert_resource(command_sender)
        .add_plugins(AtriumPlugin)
        .run();
}
