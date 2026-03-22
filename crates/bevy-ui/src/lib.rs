//! Bevy 3D visualization for the Atrium spatial audio engine.
//!
//! Renders the environment, atrium, speakers, and sound sources in real time,
//! driven by telemetry from the audio thread via an rtrb ring buffer.

mod camera;
pub mod ecs;
mod hud;
mod input;
pub mod scene;
mod telemetry;

use bevy::dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin};

use bevy::prelude::*;
use bevy::window::WindowResolution;

pub use ecs::{
    SoundAtrium, SoundEnvironment, SoundListener, SoundSource, SoundSourceIndex, SoundSpeaker,
};
pub use scene::SceneDescription;
pub use telemetry::{CommandSender, TelemetryReceiver};

/// Main Atrium visualization plugin.
/// Requires `SceneDescription`, `TelemetryReceiver`, and `CommandSender` as
/// resources before `App::run()`.
pub struct AtriumPlugin;

impl Plugin for AtriumPlugin {
    fn build(&self, app: &mut App) {
        // Register reflect types for editor/inspector support
        app.register_type::<ecs::SoundSource>()
            .register_type::<ecs::SoundListener>()
            .register_type::<ecs::SoundSpeaker>()
            .register_type::<ecs::SoundEnvironment>()
            .register_type::<ecs::SoundAtrium>();

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
        .init_resource::<scene::save::SceneFilePath>()
        .add_message::<telemetry::TelemetryMessage>()
        // Phase 1: spawn scene entities
        .add_systems(Startup, scene::setup_scene)
        // Phase 2: systems that query spawned entities (run after flush)
        .add_systems(
            PostStartup,
            (camera::setup_camera, hud::setup_hud, init_orbit_speeds),
        )
        .add_systems(Update, telemetry::poll_telemetry)
        .add_systems(
            Update,
            (
                scene::update_listener,
                scene::update_sources,
                scene::update_source_lights,
                scene::update_gain_lines,
                scene::billboard_labels,
                scene::billboard_speaker_labels,
                scene::update_ear_labels,
                scene::draw_directivity_patterns,
                scene::draw_audible_rings,
                scene::draw_atrium_wireframe,
                scene::draw_listener_direction,
                scene::drag_sources,
                camera::orbit_camera,
            ),
        )
        .add_systems(
            Update,
            (
                hud::update_hud_sources,
                hud::update_hud_listener,
                hud::update_hud_meters,
                hud::update_hud_pipeline,
                input::handle_render_mode_buttons,
                input::handle_channel_mode_buttons,
                input::handle_mute_buttons,
                input::handle_pause_buttons,
                input::handle_atmosphere_buttons,
                input::handle_reset_button,
                // ECS → audio sync
                ecs::observers::sync_source_properties,
                ecs::observers::sync_speaker_positions,
                // Scene persistence
                scene::save::save_scene_on_keypress,
                // Input sync
                input::sync_render_mode_buttons,
                input::sync_channel_mode_buttons,
                input::sync_mute_buttons,
                input::sync_pause_buttons,
                input::sync_atmosphere_text,
            ),
        );
    }
}

fn init_orbit_speeds(
    mut commands: Commands,
    sources: Query<(&ecs::SoundSourceIndex, &ecs::SoundSource)>,
) {
    let mut sorted: Vec<_> = sources.iter().collect();
    sorted.sort_by_key(|(idx, _)| idx.0);
    let speeds = sorted
        .iter()
        .map(|(_, s)| if s.orbit_radius > 0.0 { 1.0 } else { 0.0 })
        .collect();
    commands.insert_resource(input::SourceOrbitSpeeds { speeds });
}

/// Launch the Bevy visualization. Blocks the calling thread (Bevy owns the event loop).
pub fn run(
    description: SceneDescription,
    telemetry_receiver: TelemetryReceiver,
    command_sender: CommandSender,
) {
    App::new()
        .insert_resource(description)
        .insert_resource(telemetry_receiver)
        .insert_resource(command_sender)
        .add_plugins(AtriumPlugin)
        .run();
}
