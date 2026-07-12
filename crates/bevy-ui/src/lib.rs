//! Bevy visualization for the Atrium spatial audio engine.
//!
//! Renders a simple 2D top-down "radar" schematic of the atrium: speakers,
//! sound sources, and the listener, drawn with an orthographic `Camera2d`.
//! Driven by telemetry from the audio thread via an rtrb ring buffer; sends
//! control commands (move listener, drag sources, change mode) back.

mod bindings;
mod camera;
pub mod ecs;
mod gamepad;
mod hud;
mod input;
pub mod scene;
mod screenshot;
mod telemetry;

use bevy::dev_tools::fps_overlay::{FpsOverlayConfig, FpsOverlayPlugin, FrameTimeGraphConfig};
use bevy::prelude::*;
use bevy::window::WindowResolution;

pub use ecs::{
    SoundAtrium, SoundEnvironment, SoundListener, SoundSource, SoundSourceIndex, SoundSpeaker,
};
pub use scene::reload::{
    AddOrigin, AddSpec, AddedSource, AudioHandle, AudioHost, ReloadOutput, ReloadTarget, SceneHost,
};
pub use scene::SceneDescription;
pub use telemetry::{CommandSender, TelemetryReceiver};

/// Main Atrium visualization plugin.
/// Requires `SceneDescription`, `TelemetryReceiver`, and `CommandSender` as
/// resources before `App::run()`.
pub struct AtriumPlugin;

impl Plugin for AtriumPlugin {
    fn build(&self, app: &mut App) {
        // Register reflect types for editor/inspector support.
        app.register_type::<ecs::SoundSource>()
            .register_type::<ecs::SoundListener>()
            .register_type::<ecs::SoundSpeaker>()
            .register_type::<ecs::SoundEnvironment>()
            .register_type::<ecs::SoundAtrium>();

        app.add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Atrium".into(),
                resolution: WindowResolution::new(1400, 900),
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
                // The graph draws a solid red bar over the HUD — keep it off.
                frame_time_graph_config: FrameTimeGraphConfig {
                    enabled: false,
                    ..default()
                },
                ..default()
            },
        })
        .init_resource::<telemetry::LatestTelemetry>()
        .init_resource::<scene::SourceDragState>()
        .init_resource::<scene::save::SceneFilePath>()
        .init_resource::<scene::landscape::LandscapeTheme>()
        .init_resource::<hud::PointerOverHud>()
        .init_resource::<scene::reload::PendingReload>()
        .init_resource::<scene::reload::PendingSave>()
        .init_resource::<scene::reload::PendingSceneEdit>()
        .init_resource::<bindings::BindingsMenu>()
        .add_message::<telemetry::TelemetryMessage>()
        // Phase 1: spawn scene entities.
        .add_systems(Startup, scene::setup_scene)
        // Phase 2: systems that query spawned entities (run after flush).
        .add_systems(
            PostStartup,
            (
                camera::setup_camera,
                hud::setup_hud,
                bindings::setup_bindings_ui,
                init_orbit_speeds,
            ),
        )
        // Telemetry + camera/listener control.
        .add_systems(
            Update,
            (
                telemetry::poll_telemetry,
                hud::scroll_hud_panels,
                camera::move_listener,
                camera::follow_and_zoom_camera,
                scene::update_sources,
                scene::drag_sources,
            ),
        )
        // 2D gizmo overlays.
        .add_systems(
            Update,
            (
                scene::draw_room_bounds,
                scene::draw_source_links,
                scene::draw_source_ripples,
                scene::draw_source_icons,
                scene::draw_audible_rings,
                scene::draw_directivity_patterns,
                scene::draw_listener_rings,
                scene::draw_listener_direction,
                scene::update_speaker_visuals,
            ),
        )
        // Landscape backdrop.
        .add_systems(
            Update,
            (
                scene::landscape::sway_vegetation,
                scene::landscape::handle_theme_keys,
                input::handle_biome_buttons,
                input::sync_biome_buttons,
            ),
        )
        // Gamepad + controls overlay.
        .add_systems(
            Update,
            (
                gamepad::handle_gamepad_actions,
                bindings::toggle_bindings_menu,
            ),
        )
        // Scene management (picker + add/remove + reload driver; drivers run
        // last so UI teardown/rebuild sees a settled frame).
        .add_systems(
            Update,
            (
                input::handle_scene_pick_buttons,
                input::handle_save_button,
                input::handle_add_source_buttons,
                input::handle_remove_source_buttons,
                scene::reload::drain_retired_sources,
                scene::reload::drive_scene_edits,
                scene::reload::drive_scene_reload,
                scene::reload::drive_scene_save,
            )
                .chain(),
        )
        // Per-source live property edits (SPL / spread / directivity).
        .add_systems(Update, input::handle_source_edit_buttons);

        // Automated screenshot tour (visual verification, opt-in via env var).
        if let Ok(directory) = std::env::var("ATRIUM_SCREENSHOT_DIR") {
            app.insert_resource(screenshot::ScreenshotTour::new(directory))
                .add_systems(Update, screenshot::run_screenshot_tour);
        }

        app
            // Screen-space info cards + labels.
            .add_systems(
                Update,
                (
                    scene::update_source_cards,
                    scene::update_listener_tag,
                    scene::billboard_speaker_labels,
                    scene::update_ear_labels,
                    scene::retint_labels_on_theme_change,
                ),
            )
            // HUD + input controls.
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
                    ecs::observers::sync_source_properties,
                    ecs::observers::sync_speaker_positions,
                    scene::save::save_scene_on_keypress,
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
    // Slot-indexed (length MAX_SOURCES) so pause can address a source by its
    // (possibly sparse) pool slot rather than a dense list position.
    let mut speeds = vec![0.0; atrium_core::telemetry::MAX_SOURCES];
    for (idx, source) in &sources {
        if idx.0 < speeds.len() {
            speeds[idx.0] = if source.orbit_radius > 0.0 { 1.0 } else { 0.0 };
        }
    }
    commands.insert_resource(input::SourceOrbitSpeeds { speeds });
}

/// Launch the Bevy visualization. Blocks the calling thread (Bevy owns the event loop).
///
/// `initial_audio` is the live stream handle (kept as a NonSend resource, since
/// `cpal::Stream` is `!Send`); `host` rebuilds the audio scene + stream on
/// reload. Both are supplied by the `atrium` crate.
pub fn run(
    description: SceneDescription,
    telemetry_receiver: TelemetryReceiver,
    command_sender: CommandSender,
    initial_audio: Box<dyn AudioHandle>,
    host: Box<dyn SceneHost>,
) {
    App::new()
        .insert_resource(description)
        .insert_resource(telemetry_receiver)
        .insert_resource(command_sender)
        .insert_non_send_resource(AudioHost {
            current: initial_audio,
            host,
        })
        .add_plugins(AtriumPlugin)
        .run();
}
