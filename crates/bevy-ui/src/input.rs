//! Button interaction handlers for the HUD control panel.
//!
//! Render mode, channel mode, source mute/pause, atmosphere, and reset.
//! All state is driven by engine telemetry — no local tracking needed.

use atrium_core::commands::Command;
use atrium_core::directivity::DirectivityPattern;
use atrium_core::speaker::{ChannelMode, RenderMode};
use bevy::prelude::*;

use crate::ecs::{SoundSource, SoundSourceIndex};
use crate::scene::landscape::{
    apply_theme, Biome, FloorSprite, LandscapeDecor, LandscapeTheme, TimeOfDay,
};
use crate::scene::reload::{
    PendingReload, PendingSave, PendingSceneEdit, ReloadTarget, SceneEditRequest,
};
use crate::scene::SceneDescription;
use crate::telemetry::{CommandSender, LatestTelemetry, TelemetryMessage};

// ── Colors ──────────────────────────────────────────────────────────────────

const BTN_INACTIVE: Color = Color::srgb(0.18, 0.18, 0.20);
const BTN_ACTIVE: Color = Color::srgb(0.31, 0.76, 0.97);
const BTN_DISABLED: Color = Color::srgb(0.12, 0.12, 0.13);
const BTN_TEXT: Color = Color::srgb(0.7, 0.7, 0.7);
const BTN_TEXT_ACTIVE: Color = Color::srgb(0.05, 0.05, 0.08);
const BTN_TEXT_DISABLED: Color = Color::srgb(0.35, 0.35, 0.35);
const BTN_MUTED: Color = Color::srgb(0.96, 0.26, 0.21);
const BTN_PAUSED: Color = Color::srgb(1.0, 0.60, 0.0);
const BTN_RESET: Color = Color::srgb(0.22, 0.22, 0.25);

// ── Marker components ───────────────────────────────────────────────────────

#[derive(Component)]
pub(crate) struct RenderModeButton {
    pub mode: RenderMode,
}

#[derive(Component)]
pub(crate) struct ChannelModeButton {
    pub mode: ChannelMode,
}

#[derive(Component)]
pub(crate) struct MuteButton {
    pub source_index: usize,
}

#[derive(Component)]
pub(crate) struct PauseButton {
    pub source_index: usize,
}

#[derive(Component)]
pub(crate) struct ResetSceneButton;

#[derive(Component)]
pub(crate) struct TempUpButton;

#[derive(Component)]
pub(crate) struct TempDownButton;

#[derive(Component)]
pub(crate) struct HumidityUpButton;

#[derive(Component)]
pub(crate) struct HumidityDownButton;

#[derive(Component)]
pub(crate) struct AtmosphereText;

#[derive(Component)]
pub(crate) struct BiomeButton {
    pub biome: Biome,
}

#[derive(Component)]
pub(crate) struct TimeButton {
    pub time: TimeOfDay,
}

#[derive(Component)]
pub(crate) struct ScenePickButton {
    pub path: String,
}

#[derive(Component)]
pub(crate) struct SaveSceneButton;

// ── Add / remove / edit source markers ───────────────────────────────────────

#[derive(Component)]
pub(crate) struct AddPresetButton {
    pub yaml_path: String,
}

#[derive(Component)]
pub(crate) struct AddBrowseButton;

#[derive(Component)]
pub(crate) struct RemoveSourceButton {
    pub slot: usize,
}

/// A per-source live property edit button (SPL / spread / directivity).
#[derive(Component)]
pub(crate) struct SourceEditButton {
    pub slot: usize,
    pub edit: SourceEdit,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum SourceEdit {
    SplUp,
    SplDown,
    SpreadUp,
    SpreadDown,
    CycleDirectivity,
}

/// Tracks original orbit speeds so we can restore them on unpause.
#[derive(Resource)]
pub(crate) struct SourceOrbitSpeeds {
    pub speeds: Vec<f32>,
}

// ── Button style ────────────────────────────────────────────────────────────

pub(crate) fn button_node() -> Node {
    Node {
        padding: UiRect::axes(Val::Px(10.0), Val::Px(5.0)),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        border_radius: BorderRadius::all(Val::Px(3.0)),
        ..default()
    }
}

fn small_button_node() -> Node {
    Node {
        padding: UiRect::axes(Val::Px(6.0), Val::Px(2.0)),
        justify_content: JustifyContent::Center,
        align_items: AlignItems::Center,
        border_radius: BorderRadius::all(Val::Px(3.0)),
        ..default()
    }
}

pub(crate) fn button_text(label: &str) -> (Text, TextFont, TextColor) {
    (
        Text::new(label),
        TextFont {
            font_size: 12.0,
            ..default()
        },
        TextColor(BTN_TEXT),
    )
}

fn small_button_text(label: &str) -> (Text, TextFont, TextColor) {
    (
        Text::new(label),
        TextFont {
            font_size: 10.0,
            ..default()
        },
        TextColor(BTN_TEXT),
    )
}

// ── Spawning: render mode / channel mode ────────────────────────────────────

pub(crate) fn spawn_render_mode_buttons(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(4.0),
            flex_wrap: FlexWrap::Wrap,
            row_gap: Val::Px(4.0),
            ..default()
        })
        .with_children(|row| {
            for mode in RenderMode::ALL {
                let label = match mode {
                    RenderMode::WorldLocked => "World",
                    RenderMode::Vbap => "VBAP",
                    RenderMode::Hrtf => "HRTF",
                    RenderMode::Dbap => "DBAP",
                    RenderMode::Ambisonics => "Ambi",
                };
                row.spawn((
                    RenderModeButton { mode },
                    Button,
                    button_node(),
                    BackgroundColor(BTN_INACTIVE),
                ))
                .with_children(|button| {
                    button.spawn(button_text(label));
                });
            }
        });
}

pub(crate) fn spawn_channel_mode_buttons(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(4.0),
            ..default()
        })
        .with_children(|row| {
            for (mode, label) in [
                (ChannelMode::Stereo, "Stereo"),
                (ChannelMode::Quad, "Quad"),
                (ChannelMode::Surround51, "5.1"),
            ] {
                row.spawn((
                    ChannelModeButton { mode },
                    Button,
                    button_node(),
                    BackgroundColor(BTN_INACTIVE),
                ))
                .with_children(|button| {
                    button.spawn(button_text(label));
                });
            }
        });
}

// ── Spawning: source mute/pause buttons ─────────────────────────────────────

pub(crate) fn spawn_source_buttons(
    parent: &mut ChildSpawnerCommands,
    index: usize,
    has_orbit: bool,
) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(4.0),
            margin: UiRect::left(Val::Px(14.0)),
            ..default()
        })
        .with_children(|row| {
            // Mute button
            row.spawn((
                MuteButton {
                    source_index: index,
                },
                Button,
                small_button_node(),
                BackgroundColor(BTN_INACTIVE),
            ))
            .with_children(|button| {
                button.spawn(small_button_text("mute"));
            });

            // Pause button (only for orbiting sources)
            if has_orbit {
                row.spawn((
                    PauseButton {
                        source_index: index,
                    },
                    Button,
                    small_button_node(),
                    BackgroundColor(BTN_INACTIVE),
                ))
                .with_children(|button| {
                    button.spawn(small_button_text("\u{25B6}"));
                });
            }

            // Remove button ("del" — Bevy's default font lacks a ✕ glyph).
            row.spawn((
                RemoveSourceButton { slot: index },
                Button,
                small_button_node(),
                BackgroundColor(BTN_RESET),
            ))
            .with_children(|button| {
                button.spawn(small_button_text("del"));
            });
        });

    // Edit row: SPL −/+, Spread −/+, and a directivity cycle button.
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(4.0),
            align_items: AlignItems::Center,
            margin: UiRect::new(Val::Px(14.0), Val::Px(0.0), Val::Px(2.0), Val::Px(2.0)),
            ..default()
        })
        .with_children(|row| {
            spawn_edit_group(row, index, "SPL", SourceEdit::SplDown, SourceEdit::SplUp);
            spawn_edit_group(
                row,
                index,
                "Spr",
                SourceEdit::SpreadDown,
                SourceEdit::SpreadUp,
            );
            row.spawn((
                SourceEditButton {
                    slot: index,
                    edit: SourceEdit::CycleDirectivity,
                },
                Button,
                small_button_node(),
                BackgroundColor(BTN_INACTIVE),
            ))
            .with_children(|button| {
                button.spawn(small_button_text("dir"));
            });
        });
}

/// A labelled −/+ pair for one editable property.
fn spawn_edit_group(
    parent: &mut ChildSpawnerCommands,
    slot: usize,
    label: &str,
    down: SourceEdit,
    up: SourceEdit,
) {
    parent.spawn((
        Text::new(label),
        TextFont {
            font_size: 10.0,
            ..default()
        },
        TextColor(BTN_TEXT),
    ));
    parent
        .spawn((
            SourceEditButton { slot, edit: down },
            Button,
            small_button_node(),
            BackgroundColor(BTN_INACTIVE),
        ))
        .with_children(|b| {
            b.spawn(small_button_text("-"));
        });
    parent
        .spawn((
            SourceEditButton { slot, edit: up },
            Button,
            small_button_node(),
            BackgroundColor(BTN_INACTIVE),
        ))
        .with_children(|b| {
            b.spawn(small_button_text("+"));
        });
}

// ── Spawning: atmosphere controls ───────────────────────────────────────────

pub(crate) fn spawn_atmosphere_controls(parent: &mut ChildSpawnerCommands) {
    // Temperature row: [−] 20°C [+]
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(4.0),
            ..default()
        })
        .with_children(|column| {
            // Temp row
            column
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(4.0),
                    align_items: AlignItems::Center,
                    ..default()
                })
                .with_children(|row| {
                    row.spawn((
                        Text::new("Temp"),
                        TextFont {
                            font_size: 11.0,
                            ..default()
                        },
                        TextColor(BTN_TEXT),
                        Node {
                            width: Val::Px(55.0),
                            ..default()
                        },
                    ));
                    row.spawn((
                        TempDownButton,
                        Button,
                        small_button_node(),
                        BackgroundColor(BTN_INACTIVE),
                    ))
                    .with_children(|b| {
                        b.spawn(small_button_text("\u{2212}"));
                    });
                    row.spawn((
                        TempUpButton,
                        Button,
                        small_button_node(),
                        BackgroundColor(BTN_INACTIVE),
                    ))
                    .with_children(|b| {
                        b.spawn(small_button_text("+"));
                    });
                });

            // Humidity row
            column
                .spawn(Node {
                    flex_direction: FlexDirection::Row,
                    column_gap: Val::Px(4.0),
                    align_items: AlignItems::Center,
                    ..default()
                })
                .with_children(|row| {
                    row.spawn((
                        Text::new("Humidity"),
                        TextFont {
                            font_size: 11.0,
                            ..default()
                        },
                        TextColor(BTN_TEXT),
                        Node {
                            width: Val::Px(55.0),
                            ..default()
                        },
                    ));
                    row.spawn((
                        HumidityDownButton,
                        Button,
                        small_button_node(),
                        BackgroundColor(BTN_INACTIVE),
                    ))
                    .with_children(|b| {
                        b.spawn(small_button_text("\u{2212}"));
                    });
                    row.spawn((
                        HumidityUpButton,
                        Button,
                        small_button_node(),
                        BackgroundColor(BTN_INACTIVE),
                    ))
                    .with_children(|b| {
                        b.spawn(small_button_text("+"));
                    });
                });

            // Atmosphere readout text
            column.spawn((
                AtmosphereText,
                Text::new("20 C  50%"),
                TextFont {
                    font_size: 10.0,
                    ..default()
                },
                TextColor(Color::srgb(0.5, 0.5, 0.5)),
            ));
        });
}

// ── Spawning: reset scene button ────────────────────────────────────────────

pub(crate) fn spawn_reset_button(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn((
            ResetSceneButton,
            Button,
            button_node(),
            BackgroundColor(BTN_RESET),
        ))
        .with_children(|button| {
            button.spawn(button_text("Reset Scene"));
        });
}

// ── Spawning: biome / time-of-day picker ─────────────────────────────────────

/// Landscape picker: a wrapped row of biome buttons + a Day/Night row.
/// Cosmetic only — mirrors the `B`/`N` keys, sends no audio command.
pub(crate) fn spawn_biome_controls(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(4.0),
            row_gap: Val::Px(4.0),
            flex_wrap: FlexWrap::Wrap,
            ..default()
        })
        .with_children(|row| {
            for biome in Biome::ALL {
                row.spawn((
                    BiomeButton { biome },
                    Button,
                    small_button_node(),
                    BackgroundColor(BTN_INACTIVE),
                ))
                .with_children(|button| {
                    button.spawn(small_button_text(biome.label()));
                });
            }
        });

    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(4.0),
            margin: UiRect::top(Val::Px(4.0)),
            ..default()
        })
        .with_children(|row| {
            for time in TimeOfDay::ALL {
                row.spawn((
                    TimeButton { time },
                    Button,
                    small_button_node(),
                    BackgroundColor(BTN_INACTIVE),
                ))
                .with_children(|button| {
                    button.spawn(small_button_text(time.label()));
                });
            }
        });
}

// ── Interaction systems ─────────────────────────────────────────────────────

pub(crate) fn handle_render_mode_buttons(
    buttons: Query<(&RenderModeButton, &Interaction), Changed<Interaction>>,
    mut command_sender: ResMut<CommandSender>,
) {
    for (button, interaction) in &buttons {
        if *interaction == Interaction::Pressed {
            command_sender.send(Command::SetRenderMode { mode: button.mode });
        }
    }
}

pub(crate) fn handle_channel_mode_buttons(
    buttons: Query<(&ChannelModeButton, &Interaction), Changed<Interaction>>,
    mut command_sender: ResMut<CommandSender>,
    telemetry: Res<LatestTelemetry>,
) {
    let valid = ChannelMode::valid_for(telemetry.frame.render_mode);

    for (button, interaction) in &buttons {
        if *interaction == Interaction::Pressed && valid.contains(&button.mode) {
            command_sender.send(Command::SetChannelMode { mode: button.mode });
        }
    }
}

pub(crate) fn handle_mute_buttons(
    buttons: Query<(&MuteButton, &Interaction), Changed<Interaction>>,
    mut command_sender: ResMut<CommandSender>,
    telemetry: Res<LatestTelemetry>,
) {
    let frame = &telemetry.frame;

    for (button, interaction) in &buttons {
        if *interaction == Interaction::Pressed {
            let currently_muted = if button.source_index < frame.source_count as usize {
                frame.sources[button.source_index].is_muted
            } else {
                false
            };
            command_sender.send(Command::SetSourceMuted {
                index: button.source_index as u16,
                muted: !currently_muted,
            });
        }
    }
}

pub(crate) fn handle_pause_buttons(
    buttons: Query<(&PauseButton, &Interaction), Changed<Interaction>>,
    mut command_sender: ResMut<CommandSender>,
    orbit_speeds: Res<SourceOrbitSpeeds>,
    telemetry: Res<LatestTelemetry>,
) {
    let frame = &telemetry.frame;

    for (button, interaction) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let index = button.source_index;
        let current_radius = if index < frame.source_count as usize {
            frame.sources[index].orbit_radius
        } else {
            0.0
        };

        // If orbit_radius > 0, source is orbiting → pause it
        // If orbit_radius == 0 and we have a stored speed → resume it
        if current_radius > 0.0 {
            // Store current speed and pause
            if index < orbit_speeds.speeds.len() {
                // Speed isn't in telemetry, but we stored the original from SceneData
                command_sender.send(Command::SetSourceOrbitSpeed {
                    index: index as u16,
                    speed: 0.0,
                });
            }
        } else if index < orbit_speeds.speeds.len() && orbit_speeds.speeds[index] > 0.0 {
            // Resume with original speed
            command_sender.send(Command::SetSourceOrbitSpeed {
                index: index as u16,
                speed: orbit_speeds.speeds[index],
            });
        }
    }
}

pub(crate) fn handle_atmosphere_buttons(
    temp_up: Query<&Interaction, (Changed<Interaction>, With<TempUpButton>)>,
    temp_down: Query<&Interaction, (Changed<Interaction>, With<TempDownButton>)>,
    humidity_up: Query<&Interaction, (Changed<Interaction>, With<HumidityUpButton>)>,
    humidity_down: Query<&Interaction, (Changed<Interaction>, With<HumidityDownButton>)>,
    mut command_sender: ResMut<CommandSender>,
    telemetry: Res<LatestTelemetry>,
) {
    let mut temperature = telemetry.frame.temperature_c;
    let mut humidity = telemetry.frame.humidity_pct;
    let mut changed = false;

    for interaction in &temp_up {
        if *interaction == Interaction::Pressed {
            temperature = (temperature + 5.0).min(45.0);
            changed = true;
        }
    }
    for interaction in &temp_down {
        if *interaction == Interaction::Pressed {
            temperature = (temperature - 5.0).max(-10.0);
            changed = true;
        }
    }
    for interaction in &humidity_up {
        if *interaction == Interaction::Pressed {
            humidity = (humidity + 10.0).min(100.0);
            changed = true;
        }
    }
    for interaction in &humidity_down {
        if *interaction == Interaction::Pressed {
            humidity = (humidity - 10.0).max(0.0);
            changed = true;
        }
    }

    if changed {
        command_sender.send(Command::SetAtmosphere {
            temperature_c: temperature,
            humidity_pct: humidity,
        });
    }
}

pub(crate) fn handle_reset_button(
    buttons: Query<&Interaction, (Changed<Interaction>, With<ResetSceneButton>)>,
    mut command_sender: ResMut<CommandSender>,
) {
    for interaction in &buttons {
        if *interaction == Interaction::Pressed {
            command_sender.send(Command::ResetScene);
        }
    }
}

// ── Sync systems ────────────────────────────────────────────────────────────

pub(crate) fn sync_render_mode_buttons(
    mut buttons: Query<(&RenderModeButton, &mut BackgroundColor, &Children)>,
    mut text_colors: Query<&mut TextColor>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    let Some(msg) = messages.read().last() else {
        return;
    };
    let active_mode = msg.frame.render_mode;

    for (button, mut background, children) in &mut buttons {
        let is_active = button.mode == active_mode;
        background.0 = if is_active { BTN_ACTIVE } else { BTN_INACTIVE };

        for child in children.iter() {
            if let Ok(mut text_color) = text_colors.get_mut(child) {
                text_color.0 = if is_active { BTN_TEXT_ACTIVE } else { BTN_TEXT };
            }
        }
    }
}

pub(crate) fn sync_channel_mode_buttons(
    mut buttons: Query<(&ChannelModeButton, &mut BackgroundColor, &Children)>,
    mut text_colors: Query<&mut TextColor>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    let Some(msg) = messages.read().last() else {
        return;
    };
    let active_channel = msg.frame.channel_mode;
    let valid = ChannelMode::valid_for(msg.frame.render_mode);

    for (button, mut background, children) in &mut buttons {
        let is_valid = valid.contains(&button.mode);
        let is_active = button.mode == active_channel;

        background.0 = if !is_valid {
            BTN_DISABLED
        } else if is_active {
            BTN_ACTIVE
        } else {
            BTN_INACTIVE
        };

        let text_color = if !is_valid {
            BTN_TEXT_DISABLED
        } else if is_active {
            BTN_TEXT_ACTIVE
        } else {
            BTN_TEXT
        };

        for child in children.iter() {
            if let Ok(mut tc) = text_colors.get_mut(child) {
                tc.0 = text_color;
            }
        }
    }
}

/// Sync mute button appearance: red when muted, default when not.
pub(crate) fn sync_mute_buttons(
    mut buttons: Query<(&MuteButton, &mut BackgroundColor, &Children)>,
    mut texts: Query<(&mut Text, &mut TextColor)>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    let Some(msg) = messages.read().last() else {
        return;
    };
    let frame = &msg.frame;

    for (button, mut background, children) in &mut buttons {
        let is_muted = if button.source_index < frame.source_count as usize {
            frame.sources[button.source_index].is_muted
        } else {
            false
        };

        background.0 = if is_muted { BTN_MUTED } else { BTN_INACTIVE };

        for child in children.iter() {
            if let Ok((mut text, mut color)) = texts.get_mut(child) {
                **text = if is_muted {
                    "muted".to_string()
                } else {
                    "mute".to_string()
                };
                color.0 = if is_muted { Color::WHITE } else { BTN_TEXT };
            }
        }
    }
}

/// Sync pause button appearance: orange ⏸ when playing, default ▶ when paused.
pub(crate) fn sync_pause_buttons(
    mut buttons: Query<(&PauseButton, &mut BackgroundColor, &Children)>,
    mut texts: Query<(&mut Text, &mut TextColor)>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    let Some(msg) = messages.read().last() else {
        return;
    };
    let frame = &msg.frame;

    for (button, mut background, children) in &mut buttons {
        let is_orbiting = if button.source_index < frame.source_count as usize {
            frame.sources[button.source_index].orbit_radius > 0.0
        } else {
            false
        };

        background.0 = if is_orbiting {
            BTN_PAUSED
        } else {
            BTN_INACTIVE
        };

        for child in children.iter() {
            if let Ok((mut text, mut color)) = texts.get_mut(child) {
                // ⏸ when playing (can pause), ▶ when paused (can resume)
                **text = if is_orbiting {
                    "\u{23F8}".to_string()
                } else {
                    "\u{25B6}".to_string()
                };
                color.0 = if is_orbiting {
                    BTN_TEXT_ACTIVE
                } else {
                    BTN_TEXT
                };
            }
        }
    }
}

/// Update atmosphere readout text.
pub(crate) fn sync_atmosphere_text(
    mut text: Query<&mut Text, With<AtmosphereText>>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    let Some(msg) = messages.read().last() else {
        return;
    };
    let temp = msg.frame.temperature_c;
    let humidity = msg.frame.humidity_pct;

    for mut t in &mut text {
        // "C" instead of "°C": Bevy's default font subset renders ° as tofu.
        **t = format!("{:.0} C  {:.0}%", temp, humidity);
    }
}

// ── Biome / time-of-day picker ───────────────────────────────────────────────

/// Apply biome / day-night button presses to `LandscapeTheme` and rebuild the
/// landscape (same path as the `B`/`N` keys via `apply_theme`).
#[allow(clippy::too_many_arguments)]
pub(crate) fn handle_biome_buttons(
    biome_buttons: Query<(&BiomeButton, &Interaction), Changed<Interaction>>,
    time_buttons: Query<(&TimeButton, &Interaction), Changed<Interaction>>,
    mut theme: ResMut<LandscapeTheme>,
    mut clear_color: ResMut<ClearColor>,
    mut commands: Commands,
    decor: Query<Entity, With<LandscapeDecor>>,
    mut floor: Query<&mut Sprite, With<FloorSprite>>,
    description: Res<SceneDescription>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<ColorMaterial>>,
) {
    let mut changed = false;
    for (button, interaction) in &biome_buttons {
        if *interaction == Interaction::Pressed {
            theme.biome = button.biome;
            changed = true;
        }
    }
    for (button, interaction) in &time_buttons {
        if *interaction == Interaction::Pressed {
            theme.time_of_day = button.time;
            changed = true;
        }
    }
    if !changed {
        return;
    }

    apply_theme(
        &mut commands,
        &mut clear_color,
        &decor,
        &mut floor,
        &description,
        &mut meshes,
        &mut materials,
        *theme,
    );
}

/// Highlight the active biome + time-of-day buttons from `LandscapeTheme`.
pub(crate) fn sync_biome_buttons(
    theme: Res<LandscapeTheme>,
    mut biome_buttons: Query<(&BiomeButton, &mut BackgroundColor, &Children), Without<TimeButton>>,
    mut time_buttons: Query<(&TimeButton, &mut BackgroundColor, &Children), Without<BiomeButton>>,
    mut text_colors: Query<&mut TextColor>,
) {
    let paint = |active: bool,
                 background: &mut BackgroundColor,
                 children: &Children,
                 text_colors: &mut Query<&mut TextColor>| {
        background.0 = if active { BTN_ACTIVE } else { BTN_INACTIVE };
        for child in children.iter() {
            if let Ok(mut text_color) = text_colors.get_mut(child) {
                text_color.0 = if active { BTN_TEXT_ACTIVE } else { BTN_TEXT };
            }
        }
    };

    for (button, mut background, children) in &mut biome_buttons {
        paint(
            button.biome == theme.biome,
            &mut background,
            children,
            &mut text_colors,
        );
    }
    for (button, mut background, children) in &mut time_buttons {
        paint(
            button.time == theme.time_of_day,
            &mut background,
            children,
            &mut text_colors,
        );
    }
}

// ── Scene picker ─────────────────────────────────────────────────────────────

/// A wrapped row of buttons, one per `scenes/*.yaml`, that load the scene at
/// runtime. Rebuilt on every HUD build, so saved scenes appear immediately.
pub(crate) fn spawn_scene_picker(parent: &mut ChildSpawnerCommands) {
    let mut scenes: Vec<(String, String)> = std::fs::read_dir("scenes")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                return None;
            }
            let label = path.file_stem()?.to_str()?.to_string();
            Some((label, path.to_str()?.to_string()))
        })
        .collect();
    scenes.sort();

    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(4.0),
            row_gap: Val::Px(4.0),
            flex_wrap: FlexWrap::Wrap,
            ..default()
        })
        .with_children(|row| {
            for (label, path) in scenes {
                row.spawn((
                    ScenePickButton { path },
                    Button,
                    small_button_node(),
                    BackgroundColor(BTN_INACTIVE),
                ))
                .with_children(|button| {
                    button.spawn(small_button_text(&label));
                });
            }
        });
}

/// Load the picked scene by requesting a reload (the driver rebuilds audio + UI).
pub(crate) fn handle_scene_pick_buttons(
    buttons: Query<(&ScenePickButton, &Interaction), Changed<Interaction>>,
    mut pending: ResMut<PendingReload>,
) {
    for (button, interaction) in &buttons {
        if *interaction == Interaction::Pressed {
            pending.0 = Some(ReloadTarget::ScenePath(button.path.clone().into()));
        }
    }
}

/// "Save" button: writes the current scene (with live positions) to a loadable
/// YAML the picker can reopen.
pub(crate) fn spawn_save_button(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn((
            SaveSceneButton,
            Button,
            small_button_node(),
            BackgroundColor(BTN_INACTIVE),
        ))
        .with_children(|button| {
            button.spawn(small_button_text("Save \u{2192} saved.yaml"));
        });
}

/// Request a save to `scenes/saved.yaml` (the driver serializes the live scene).
pub(crate) fn handle_save_button(
    buttons: Query<&Interaction, (Changed<Interaction>, With<SaveSceneButton>)>,
    mut pending: ResMut<PendingSave>,
) {
    for interaction in &buttons {
        if *interaction == Interaction::Pressed {
            pending.0 = Some(std::path::PathBuf::from("scenes/saved.yaml"));
        }
    }
}

// ── Add / remove / edit sources ──────────────────────────────────────────────

/// The ADD SOURCE section: one button per `sources/*.yaml` preset plus a
/// "Browse…" button that opens a native file dialog. Rebuilt on every HUD build.
pub(crate) fn spawn_add_source_controls(parent: &mut ChildSpawnerCommands) {
    let mut presets: Vec<(String, String)> = std::fs::read_dir("sources")
        .into_iter()
        .flatten()
        .flatten()
        .filter_map(|entry| {
            let path = entry.path();
            if path.extension().and_then(|e| e.to_str()) != Some("yaml") {
                return None;
            }
            let label = path.file_stem()?.to_str()?.to_string();
            Some((label, path.to_str()?.to_string()))
        })
        .collect();
    presets.sort();

    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            column_gap: Val::Px(4.0),
            row_gap: Val::Px(4.0),
            flex_wrap: FlexWrap::Wrap,
            ..default()
        })
        .with_children(|row| {
            for (label, yaml_path) in presets {
                row.spawn((
                    AddPresetButton { yaml_path },
                    Button,
                    small_button_node(),
                    BackgroundColor(BTN_INACTIVE),
                ))
                .with_children(|button| {
                    button.spawn(small_button_text(&label));
                });
            }
            row.spawn((
                AddBrowseButton,
                Button,
                small_button_node(),
                BackgroundColor(BTN_ACTIVE),
            ))
            .with_children(|button| {
                let (text, font, _) = small_button_text("Browse\u{2026}");
                button.spawn((text, font, TextColor(BTN_TEXT_ACTIVE)));
            });
        });
}

/// Preset / Browse buttons → queue a live add.
pub(crate) fn handle_add_source_buttons(
    presets: Query<(&AddPresetButton, &Interaction), Changed<Interaction>>,
    browse: Query<&Interaction, (Changed<Interaction>, With<AddBrowseButton>)>,
    mut pending: ResMut<PendingSceneEdit>,
) {
    for (button, interaction) in &presets {
        if *interaction == Interaction::Pressed && pending.0.is_none() {
            pending.0 = Some(SceneEditRequest::AddPreset(button.yaml_path.clone()));
        }
    }
    for interaction in &browse {
        if *interaction == Interaction::Pressed && pending.0.is_none() {
            pending.0 = Some(SceneEditRequest::AddBrowsed);
        }
    }
}

/// Remove (✕) buttons → queue a live remove.
pub(crate) fn handle_remove_source_buttons(
    buttons: Query<(&RemoveSourceButton, &Interaction), Changed<Interaction>>,
    mut pending: ResMut<PendingSceneEdit>,
) {
    for (button, interaction) in &buttons {
        if *interaction == Interaction::Pressed && pending.0.is_none() {
            pending.0 = Some(SceneEditRequest::Remove(button.slot as u16));
        }
    }
}

/// Per-source edit buttons: bump SPL / spread or cycle directivity live. Mutates
/// the `SoundSource` component (so export/Save captures the edit) and sends the
/// audio command. Spread rides the `sync_source_properties` observer; SPL and
/// directivity are sent explicitly (the observer doesn't cover them).
pub(crate) fn handle_source_edit_buttons(
    buttons: Query<(&SourceEditButton, &Interaction), Changed<Interaction>>,
    mut sources: Query<(&SoundSourceIndex, &mut SoundSource)>,
    mut command_sender: ResMut<CommandSender>,
) {
    for (button, interaction) in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        let Some((_, mut source)) = sources.iter_mut().find(|(idx, _)| idx.0 == button.slot) else {
            continue;
        };
        let slot = button.slot as u16;
        match button.edit {
            SourceEdit::SplUp | SourceEdit::SplDown => {
                let delta = if button.edit == SourceEdit::SplUp {
                    2.0
                } else {
                    -2.0
                };
                let spl = (source.spl + delta).clamp(0.0, 120.0);
                source.spl = spl;
                command_sender.send(Command::SetSourceSpl { index: slot, spl });
            }
            SourceEdit::SpreadUp | SourceEdit::SpreadDown => {
                let delta = if button.edit == SourceEdit::SpreadUp {
                    0.1
                } else {
                    -0.1
                };
                // Mutating the component triggers `sync_source_properties`, which
                // sends the SetSourceSpread command.
                source.spread = (source.spread + delta).clamp(0.0, 1.0);
            }
            SourceEdit::CycleDirectivity => {
                let (next, pattern) = cycle_directivity(&source.directivity);
                source.directivity = next.to_string();
                source.directivity_alpha = pattern.alpha();
                command_sender.send(Command::SetSourceDirectivity {
                    index: slot,
                    pattern,
                });
            }
        }
    }
}

/// omni → cardioid → supercardioid → omni.
fn cycle_directivity(current: &str) -> (&'static str, DirectivityPattern) {
    match current {
        "omni" => ("cardioid", DirectivityPattern::cardioid()),
        "cardioid" => ("supercardioid", DirectivityPattern::supercardioid()),
        _ => ("omni", DirectivityPattern::Omni),
    }
}
