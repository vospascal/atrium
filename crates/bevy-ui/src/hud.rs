//! HUD overlay — left-side panel showing real-time telemetry.
//!
//! Mirrors the web UI's left panel: source list with gain/distance,
//! listener position, render mode, and channel peak meters.

use bevy::prelude::*;

use crate::input;
use crate::scene::SceneData;
use crate::telemetry::TelemetryMessage;

// ── Colors ──────────────────────────────────────────────────────────────────

const PANEL_BG: Color = Color::srgba(0.0, 0.0, 0.0, 0.7);
const TEXT_PRIMARY: Color = Color::srgb(0.8, 0.8, 0.8);
const TEXT_LABEL: Color = Color::srgb(0.6, 0.6, 0.6);
const TEXT_MUTED: Color = Color::srgb(0.95, 0.27, 0.21);
const ACCENT: Color = Color::srgb(0.31, 0.76, 0.97);
const METER_BG: Color = Color::srgb(0.15, 0.15, 0.15);
const METER_GREEN: Color = Color::srgb(0.30, 0.69, 0.31);
const METER_YELLOW: Color = Color::srgb(1.0, 0.76, 0.03);
const METER_RED: Color = Color::srgb(0.96, 0.26, 0.21);
const SEPARATOR: Color = Color::srgb(0.25, 0.25, 0.25);

const FONT_SIZE: f32 = 13.0;
const FONT_SIZE_TITLE: f32 = 16.0;
const FONT_SIZE_SMALL: f32 = 11.0;

// ── Marker components ───────────────────────────────────────────────────────

#[derive(Component)]
pub(crate) struct HudPanel;

/// Marker for a source row's dynamic text (gain/distance values).
#[derive(Component)]
pub(crate) struct SourceValueText {
    index: usize,
}

/// Marker for the listener position text.
#[derive(Component)]
pub(crate) struct ListenerPositionText;

/// Marker for a channel peak meter bar (the colored fill).
#[derive(Component)]
pub(crate) struct ChannelMeterBar {
    channel: usize,
}

/// Marker for a channel peak label.
#[derive(Component)]
pub(crate) struct ChannelPeakText {
    channel: usize,
}

/// Marker for a pipeline stage bar (the colored fill).
#[derive(Component)]
pub(crate) struct PipelineStageBar {
    source_index: usize,
    stage: PipelineStage,
}

/// Marker for a pipeline stage percentage text.
#[derive(Component)]
pub(crate) struct PipelineStageText {
    source_index: usize,
    stage: PipelineStage,
}

/// Marker for the total gain text at the bottom of a source's pipeline.
#[derive(Component)]
pub(crate) struct PipelineTotalText {
    source_index: usize,
}

/// Marker for the total gain bar.
#[derive(Component)]
pub(crate) struct PipelineTotalBar {
    source_index: usize,
}

#[derive(Clone, Copy, PartialEq, Eq)]
pub(crate) enum PipelineStage {
    Distance,
    Emission,
    Hearing,
}

/// Marker for the received SPL text at the bottom of a source's pipeline.
#[derive(Component)]
pub(crate) struct PipelineReceivedSplText {
    source_index: usize,
}

// ── Setup ───────────────────────────────────────────────────────────────────

pub(crate) fn setup_hud(mut commands: Commands, scene_data: Res<SceneData>) {
    // Root panel — top-left, semi-transparent
    commands
        .spawn((
            HudPanel,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(12.0),
                top: Val::Px(12.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(12.0)),
                row_gap: Val::Px(8.0),
                min_width: Val::Px(260.0),
                max_width: Val::Px(320.0),
                ..default()
            },
            BackgroundColor(PANEL_BG),
        ))
        .with_children(|panel| {
            // Title
            spawn_title(panel);

            // Separator
            spawn_separator(panel);

            // Render mode buttons
            spawn_section_label(panel, "RENDER MODE");
            input::spawn_render_mode_buttons(panel);

            spawn_separator(panel);

            // Channel mode buttons
            spawn_section_label(panel, "SPEAKERS");
            input::spawn_channel_mode_buttons(panel);

            spawn_separator(panel);

            // Listener
            spawn_section_label(panel, "LISTENER");
            panel.spawn((
                ListenerPositionText,
                Text::new("pos: — / yaw: —"),
                TextFont {
                    font_size: FONT_SIZE,
                    ..default()
                },
                TextColor(TEXT_PRIMARY),
            ));

            spawn_separator(panel);

            // Sources
            spawn_section_label(panel, "SOURCES");
            for (index, source) in scene_data.sources.iter().enumerate() {
                spawn_source_row(panel, index, source);
                input::spawn_source_buttons(panel, index, source.orbit_radius > 0.0);
            }

            spawn_separator(panel);

            // Atmosphere
            spawn_section_label(panel, "ATMOSPHERE");
            input::spawn_atmosphere_controls(panel);

            spawn_separator(panel);

            // Channel meters
            spawn_section_label(panel, "OUTPUT LEVELS");
            spawn_channel_meters(panel);

            spawn_separator(panel);

            // Reset
            input::spawn_reset_button(panel);
        });

    // ── Right panel: Volume Pipeline ────────────────────────────────────
    commands
        .spawn((
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(12.0),
                top: Val::Px(12.0),
                flex_direction: FlexDirection::Column,
                padding: UiRect::all(Val::Px(12.0)),
                row_gap: Val::Px(6.0),
                min_width: Val::Px(240.0),
                max_width: Val::Px(280.0),
                ..default()
            },
            BackgroundColor(PANEL_BG),
        ))
        .with_children(|panel| {
            // Title
            panel.spawn((
                Text::new("Volume Pipeline"),
                TextFont {
                    font_size: FONT_SIZE_TITLE,
                    ..default()
                },
                TextColor(ACCENT),
            ));

            spawn_separator(panel);

            for (index, source) in scene_data.sources.iter().enumerate() {
                spawn_pipeline_source(panel, index, source);
            }
        });
}

fn spawn_title(parent: &mut ChildSpawnerCommands) {
    parent.spawn((
        Text::new("Atrium"),
        TextFont {
            font_size: FONT_SIZE_TITLE,
            ..default()
        },
        TextColor(ACCENT),
    ));
}

fn spawn_section_label(parent: &mut ChildSpawnerCommands, label: &str) {
    parent.spawn((
        Text::new(label),
        TextFont {
            font_size: FONT_SIZE_SMALL,
            ..default()
        },
        TextColor(TEXT_LABEL),
    ));
}

fn spawn_separator(parent: &mut ChildSpawnerCommands) {
    parent.spawn((
        Node {
            width: Val::Percent(100.0),
            height: Val::Px(1.0),
            ..default()
        },
        BackgroundColor(SEPARATOR),
    ));
}

fn spawn_source_row(
    parent: &mut ChildSpawnerCommands,
    index: usize,
    source: &crate::scene::SourceData,
) {
    let source_color = Color::srgb(source.color[0], source.color[1], source.color[2]);

    parent
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            margin: UiRect::bottom(Val::Px(4.0)),
            ..default()
        })
        .with_children(|row| {
            // Source name with colored indicator
            row.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(6.0),
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|name_row| {
                // Color dot
                name_row.spawn((
                    Node {
                        width: Val::Px(8.0),
                        height: Val::Px(8.0),
                        ..default()
                    },
                    BackgroundColor(source_color),
                ));
                // Name
                name_row.spawn((
                    Text::new(&source.name),
                    TextFont {
                        font_size: FONT_SIZE,
                        ..default()
                    },
                    TextColor(source_color),
                ));
            });

            // Value text (updated each frame)
            row.spawn((
                SourceValueText { index },
                Text::new("dist: —  gain: —"),
                TextFont {
                    font_size: FONT_SIZE_SMALL,
                    ..default()
                },
                TextColor(TEXT_PRIMARY),
                Node {
                    margin: UiRect::left(Val::Px(14.0)),
                    ..default()
                },
            ));
        });
}

// ── Pipeline stage colors (matching the screenshot style) ────────────────

const STAGE_COLOR_DIST: Color = Color::srgb(0.96, 0.26, 0.21); // red
const STAGE_COLOR_EMIT: Color = Color::srgb(0.61, 0.15, 0.69); // purple
const STAGE_COLOR_HEAR: Color = Color::srgb(0.31, 0.76, 0.97); // blue
const STAGE_COLOR_TOTAL: Color = Color::srgb(0.30, 0.69, 0.31); // green

fn stage_color(stage: PipelineStage) -> Color {
    match stage {
        PipelineStage::Distance => STAGE_COLOR_DIST,
        PipelineStage::Emission => STAGE_COLOR_EMIT,
        PipelineStage::Hearing => STAGE_COLOR_HEAR,
    }
}

fn stage_label(stage: PipelineStage) -> &'static str {
    match stage {
        PipelineStage::Distance => "Distance",
        PipelineStage::Emission => "Emission",
        PipelineStage::Hearing => "Hearing",
    }
}

fn spawn_pipeline_source(
    parent: &mut ChildSpawnerCommands,
    index: usize,
    source: &crate::scene::SourceData,
) {
    let source_color = Color::srgb(source.color[0], source.color[1], source.color[2]);

    parent
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(3.0),
            margin: UiRect::bottom(Val::Px(6.0)),
            ..default()
        })
        .with_children(|col| {
            // Source name header
            col.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(6.0),
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|row| {
                row.spawn((
                    Node {
                        width: Val::Px(8.0),
                        height: Val::Px(8.0),
                        ..default()
                    },
                    BackgroundColor(source_color),
                ));
                row.spawn((
                    Text::new(format!("{} ({:.0} dB SPL)", source.name, source.spl)),
                    TextFont {
                        font_size: FONT_SIZE,
                        ..default()
                    },
                    TextColor(source_color),
                ));
            });

            // Pipeline stages
            for stage in [
                PipelineStage::Distance,
                PipelineStage::Emission,
                PipelineStage::Hearing,
            ] {
                spawn_pipeline_bar(col, index, stage);
            }

            // Total / final gain
            spawn_pipeline_total(col, index);

            // Received SPL
            col.spawn(Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                margin: UiRect::top(Val::Px(2.0)),
                ..default()
            })
            .with_children(|row| {
                row.spawn((
                    Text::new("Received"),
                    TextFont {
                        font_size: FONT_SIZE_SMALL,
                        ..default()
                    },
                    TextColor(ACCENT),
                ));
                row.spawn((
                    PipelineReceivedSplText {
                        source_index: index,
                    },
                    Text::new("— dB SPL"),
                    TextFont {
                        font_size: FONT_SIZE_SMALL,
                        ..default()
                    },
                    TextColor(ACCENT),
                ));
            });

            // Thin separator between sources
            col.spawn((
                Node {
                    width: Val::Percent(100.0),
                    height: Val::Px(1.0),
                    margin: UiRect::top(Val::Px(3.0)),
                    ..default()
                },
                BackgroundColor(SEPARATOR),
            ));
        });
}

fn spawn_pipeline_bar(
    parent: &mut ChildSpawnerCommands,
    source_index: usize,
    stage: PipelineStage,
) {
    let color = stage_color(stage);
    let bar_width = 140.0;

    parent
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(1.0),
            ..default()
        })
        .with_children(|col| {
            // Label + percentage row
            col.spawn(Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                ..default()
            })
            .with_children(|row| {
                row.spawn((
                    Text::new(stage_label(stage)),
                    TextFont {
                        font_size: FONT_SIZE_SMALL,
                        ..default()
                    },
                    TextColor(TEXT_LABEL),
                ));
                row.spawn((
                    PipelineStageText {
                        source_index,
                        stage,
                    },
                    Text::new("—"),
                    TextFont {
                        font_size: FONT_SIZE_SMALL,
                        ..default()
                    },
                    TextColor(TEXT_PRIMARY),
                ));
            });

            // Bar background + fill
            col.spawn((
                Node {
                    width: Val::Px(bar_width),
                    height: Val::Px(6.0),
                    ..default()
                },
                BackgroundColor(METER_BG),
            ))
            .with_children(|bg| {
                bg.spawn((
                    PipelineStageBar {
                        source_index,
                        stage,
                    },
                    Node {
                        width: Val::Px(bar_width),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    BackgroundColor(color),
                ));
            });
        });
}

fn spawn_pipeline_total(parent: &mut ChildSpawnerCommands, source_index: usize) {
    let bar_width = 140.0;

    parent
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(1.0),
            margin: UiRect::top(Val::Px(2.0)),
            ..default()
        })
        .with_children(|col| {
            col.spawn(Node {
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                ..default()
            })
            .with_children(|row| {
                row.spawn((
                    Text::new("Total"),
                    TextFont {
                        font_size: FONT_SIZE_SMALL,
                        ..default()
                    },
                    TextColor(TEXT_PRIMARY),
                ));
                row.spawn((
                    PipelineTotalText { source_index },
                    Text::new("—"),
                    TextFont {
                        font_size: FONT_SIZE_SMALL,
                        ..default()
                    },
                    TextColor(TEXT_PRIMARY),
                ));
            });

            col.spawn((
                Node {
                    width: Val::Px(bar_width),
                    height: Val::Px(6.0),
                    ..default()
                },
                BackgroundColor(METER_BG),
            ))
            .with_children(|bg| {
                bg.spawn((
                    PipelineTotalBar { source_index },
                    Node {
                        width: Val::Px(0.0),
                        height: Val::Percent(100.0),
                        ..default()
                    },
                    BackgroundColor(STAGE_COLOR_TOTAL),
                ));
            });
        });
}

fn spawn_channel_meters(parent: &mut ChildSpawnerCommands) {
    let channel_labels = ["L", "R", "C", "LFE", "RL", "RR", "SL", "SR"];

    parent
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(3.0),
            ..default()
        })
        .with_children(|meters| {
            for (channel, label) in channel_labels.iter().enumerate().take(6) {
                meters
                    .spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(6.0),
                        align_items: AlignItems::Center,
                        ..default()
                    })
                    .with_children(|row| {
                        // Channel label
                        row.spawn((
                            Text::new(*label),
                            TextFont {
                                font_size: FONT_SIZE_SMALL,
                                ..default()
                            },
                            TextColor(TEXT_LABEL),
                            Node {
                                width: Val::Px(24.0),
                                ..default()
                            },
                        ));

                        // Meter background
                        row.spawn((
                            Node {
                                width: Val::Px(160.0),
                                height: Val::Px(6.0),
                                ..default()
                            },
                            BackgroundColor(METER_BG),
                        ))
                        .with_children(|meter_bg| {
                            // Meter fill bar
                            meter_bg.spawn((
                                ChannelMeterBar { channel },
                                Node {
                                    width: Val::Px(0.0),
                                    height: Val::Percent(100.0),
                                    ..default()
                                },
                                BackgroundColor(METER_GREEN),
                            ));
                        });

                        // Peak dB text
                        row.spawn((
                            ChannelPeakText { channel },
                            Text::new("-∞"),
                            TextFont {
                                font_size: FONT_SIZE_SMALL,
                                ..default()
                            },
                            TextColor(TEXT_LABEL),
                        ));
                    });
            }
        });
}

// ── Per-frame updates ───────────────────────────────────────────────────────

/// Update source rows with latest telemetry values.
pub(crate) fn update_hud_sources(
    mut source_texts: Query<(&SourceValueText, &mut Text, &mut TextColor)>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    let Some(msg) = messages.read().last() else {
        return;
    };
    let frame = &msg.frame;

    for (marker, mut text, mut color) in &mut source_texts {
        if marker.index < frame.source_count as usize {
            let source = &frame.sources[marker.index];
            if source.is_muted {
                **text = format!("dist: {:.1}m  [MUTED]", source.distance,);
                color.0 = TEXT_MUTED;
            } else {
                **text = format!(
                    "dist: {:.1}m  gain: {:.2} ({:.0} dB)",
                    source.distance, source.gain_total, source.gain_db,
                );
                color.0 = TEXT_PRIMARY;
            }
        }
    }
}

/// Update listener position display.
pub(crate) fn update_hud_listener(
    mut text: Query<&mut Text, With<ListenerPositionText>>,
    listener: Res<crate::camera::ListenerState>,
) {
    let [x, y, z] = listener.position;
    for mut t in &mut text {
        **t = format!(
            "pos: ({:.1}, {:.1}, {:.1})  yaw: {:.0}°",
            x,
            y,
            z,
            listener.yaw.to_degrees(),
        );
    }
}

/// Update channel peak meters.
pub(crate) fn update_hud_meters(
    mut bars: Query<(&ChannelMeterBar, &mut Node, &mut BackgroundColor)>,
    mut labels: Query<(&ChannelPeakText, &mut Text)>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    let Some(msg) = messages.read().last() else {
        return;
    };
    let frame = &msg.frame;
    let meter_width = 160.0;

    for (marker, mut node, mut bg) in &mut bars {
        if marker.channel < frame.channel_count as usize {
            let peak = frame.channel_peaks[marker.channel].clamp(0.0, 1.0);
            node.width = Val::Px(peak * meter_width);

            // Color: green < -12dB, yellow < -3dB, red above
            bg.0 = if peak > 0.707 {
                METER_RED
            } else if peak > 0.25 {
                METER_YELLOW
            } else {
                METER_GREEN
            };
        }
    }

    for (marker, mut text) in &mut labels {
        if marker.channel < frame.channel_count as usize {
            let peak = frame.channel_peaks[marker.channel];
            if peak > 0.0001 {
                let db = 20.0 * peak.log10();
                **text = format!("{:.0}", db);
            } else {
                **text = "-∞".to_string();
            }
        }
    }
}

/// Map a linear gain to a 0..1 bar fraction using a dB scale.
/// Range: -40 dB (gain ~0.01) → 0.0,  +20 dB (gain 10.0) → 1.0.
/// gain=1.0 (0 dB) maps to 2/3 of the bar width.
fn gain_to_bar_fraction(gain: f32) -> f32 {
    if gain <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * gain.log10();
    // Map -40..+20 dB → 0..1
    ((db + 40.0) / 60.0).clamp(0.0, 1.0)
}

/// Format a linear gain as dB.
fn format_gain(gain: f32) -> String {
    if gain <= 0.0 {
        return "-∞ dB".to_string();
    }
    let db = 20.0 * gain.log10();
    let sign = if db >= 0.0 { "+" } else { "" };
    format!("{sign}{:.1} dB", db)
}

/// Update volume pipeline bars and labels from telemetry.
#[allow(clippy::too_many_arguments)]
pub(crate) fn update_hud_pipeline(
    mut stage_bars: Query<(&PipelineStageBar, &mut Node)>,
    mut stage_texts: Query<(&PipelineStageText, &mut Text)>,
    mut total_bars: Query<(&PipelineTotalBar, &mut Node), Without<PipelineStageBar>>,
    mut total_texts: Query<(&PipelineTotalText, &mut Text), Without<PipelineStageText>>,
    mut spl_texts: Query<
        (&PipelineReceivedSplText, &mut Text),
        (Without<PipelineStageText>, Without<PipelineTotalText>),
    >,
    scene_data: Res<SceneData>,
    mut messages: MessageReader<TelemetryMessage>,
) {
    let Some(msg) = messages.read().last() else {
        return;
    };
    let frame = &msg.frame;
    let bar_width = 140.0;

    for (marker, mut node) in &mut stage_bars {
        if marker.source_index < frame.source_count as usize {
            let source = &frame.sources[marker.source_index];
            let gain = match marker.stage {
                PipelineStage::Distance => source.gain_dist,
                PipelineStage::Emission => source.gain_emit,
                PipelineStage::Hearing => source.gain_hear,
            };
            node.width = Val::Px(gain_to_bar_fraction(gain) * bar_width);
        }
    }

    for (marker, mut text) in &mut stage_texts {
        if marker.source_index < frame.source_count as usize {
            let source = &frame.sources[marker.source_index];
            let gain = match marker.stage {
                PipelineStage::Distance => source.gain_dist,
                PipelineStage::Emission => source.gain_emit,
                PipelineStage::Hearing => source.gain_hear,
            };
            **text = format_gain(gain);
        }
    }

    for (marker, mut node) in &mut total_bars {
        if marker.source_index < frame.source_count as usize {
            let source = &frame.sources[marker.source_index];
            node.width = Val::Px(gain_to_bar_fraction(source.gain_total) * bar_width);
        }
    }

    for (marker, mut text) in &mut total_texts {
        if marker.source_index < frame.source_count as usize {
            let source = &frame.sources[marker.source_index];
            **text = format_gain(source.gain_total);
        }
    }

    for (marker, mut text) in &mut spl_texts {
        if marker.source_index < frame.source_count as usize
            && marker.source_index < scene_data.sources.len()
        {
            let telemetry = &frame.sources[marker.source_index];
            let reference_spl = scene_data.sources[marker.source_index].spl;
            // received_spl = reference_spl + gain_db
            // gain_db already encodes: -20·log₁₀(dist/ref_dist) + 20·log₁₀(emit) + 20·log₁₀(hear)
            if telemetry.gain_db.is_finite() {
                let received = reference_spl + telemetry.gain_db;
                **text = format!("{:.0} dB SPL", received);
            } else {
                **text = "-∞ dB SPL".to_string();
            }
        }
    }
}
