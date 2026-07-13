//! HUD overlay for the top-down sound map.
//!
//! The map is the product's primary surface. Controls live in a compact top
//! bar and open one task-focused drawer at a time; detailed signal telemetry is
//! deliberately hidden until requested.

use bevy::input::mouse::{MouseScrollUnit, MouseWheel};
use bevy::prelude::*;
use bevy::ui::UiGlobalTransform;
use bevy::window::PrimaryWindow;

use atrium_behavior::CommandSender;
use atrium_core::commands::Command;

use crate::bindings::{BindingsMenu, BindingsOverlay};
use crate::camera::{CameraPan, CameraSettings};
use crate::ecs::{SoundListener, SoundSource, SoundSourceIndex};
use crate::input;
use crate::scene::schema::parse_hex_color;
use crate::scene::SceneDescription;
use crate::telemetry::{LatestTelemetry, TelemetryMessage};

// ── Colors ──────────────────────────────────────────────────────────────────

const PANEL_BG: Color = Color::srgba(0.018, 0.04, 0.075, 0.94);
const HEADER_BG: Color = Color::srgba(0.018, 0.04, 0.075, 0.78);
const BUTTON_BG: Color = Color::srgba(0.08, 0.13, 0.20, 0.84);
const TEXT_PRIMARY: Color = Color::srgb(0.90, 0.94, 0.98);
const TEXT_LABEL: Color = Color::srgb(0.57, 0.65, 0.76);
const TEXT_MUTED: Color = Color::srgb(0.95, 0.27, 0.21);
const ACCENT: Color = Color::srgb(0.27, 0.78, 0.98);
const METER_BG: Color = Color::srgb(0.07, 0.11, 0.16);
const METER_GREEN: Color = Color::srgb(0.30, 0.69, 0.31);
const METER_YELLOW: Color = Color::srgb(1.0, 0.76, 0.03);
const METER_RED: Color = Color::srgb(0.96, 0.26, 0.21);
const SEPARATOR: Color = Color::srgba(0.45, 0.58, 0.72, 0.22);

const FONT_SIZE: f32 = 13.0;
const FONT_SIZE_SMALL: f32 = 11.0;

// ── Marker components ───────────────────────────────────────────────────────

/// True when the cursor is over any HUD panel — so the mouse wheel scrolls the
/// panel instead of zooming the camera.
#[derive(Resource, Default)]
pub(crate) struct PointerOverHud(pub bool);

#[derive(Component)]
pub(crate) struct HudPanel;

/// The task drawer currently open from the top app bar.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HudDrawer {
    Scenes,
    Sources,
    AddSource,
    Mix,
    Environment,
    Signal,
}

#[derive(Resource)]
pub(crate) struct HudState {
    pub active_drawer: Option<HudDrawer>,
    pub master_gain: f32,
    pub scene_name: String,
}

impl Default for HudState {
    fn default() -> Self {
        Self {
            active_drawer: None,
            master_gain: 1.0,
            scene_name: "Default".to_string(),
        }
    }
}

#[derive(Component)]
pub(crate) struct HudDrawerPanel(pub HudDrawer);

#[derive(Component)]
pub(crate) struct HudMenuButton(pub HudDrawer);

#[derive(Component)]
pub(crate) struct HudCloseButton;

#[derive(Component)]
pub(crate) struct HudSceneSelector;

#[derive(Component)]
pub(crate) struct HudMenuUnderline(pub HudDrawer);

#[derive(Component)]
pub(crate) struct HudMenuIconPart(pub HudDrawer);

#[derive(Component)]
pub(crate) struct HudHelpButton;

#[derive(Component)]
pub(crate) struct SceneNameText;

#[derive(Component)]
pub(crate) struct MasterGainText;

#[derive(Component)]
pub(crate) struct MasterGainSegment(pub f32);

#[derive(Component)]
pub(crate) struct ZoomOutButton;

#[derive(Component)]
pub(crate) struct ZoomInButton;

#[derive(Component)]
pub(crate) struct ZoomResetButton;

#[derive(Component)]
pub(crate) struct ZoomText;

#[derive(Component)]
pub(crate) struct FitLevelDownButton;

#[derive(Component)]
pub(crate) struct FitLevelUpButton;

#[derive(Component)]
pub(crate) struct FitLevelText;

/// Marker for the right-hand Volume Pipeline panel root (so it can be despawned
/// on scene reload — it was previously untagged and would leak).
#[derive(Component)]
pub(crate) struct PipelinePanel;

/// Marker for any HUD panel that scrolls its overflow with the mouse wheel.
#[derive(Component)]
pub(crate) struct HudScrollable;

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

pub(crate) fn setup_hud(mut commands: Commands, description: Res<SceneDescription>) {
    build_hud_panels(&mut commands, &description);
}

/// Build the compact app bar and hidden task drawers from a
/// `SceneDescription`. Reads the description (not
/// ECS) so it can run at startup and on scene reload (where freshly-spawned
/// entities aren't yet queryable). Sources are already in index order.
pub(crate) fn build_hud_panels(commands: &mut Commands, description: &SceneDescription) {
    spawn_app_bar(commands, description);
    spawn_master_volume(commands);
    spawn_zoom_controls(commands);

    // Scene management is infrequent, so it lives behind one compact action.
    commands
        .spawn((
            HudPanel,
            HudDrawerPanel(HudDrawer::Scenes),
            GlobalZIndex(20),
            HudScrollable,
            drawer_node(),
            ScrollPosition::default(),
            BackgroundColor(PANEL_BG),
        ))
        .with_children(|panel| {
            spawn_drawer_header(panel, "Workspace", "Scenes");
            spawn_supporting_text(panel, "Load, save, or reset a spatial arrangement.");
            spawn_separator(panel);
            input::spawn_scene_picker(panel);
            input::spawn_save_button(panel);
            spawn_separator(panel);
            input::spawn_reset_button(panel);
        });

    // Source controls are contextual; the source cards remain visible on-map.
    commands
        .spawn((
            HudPanel,
            HudDrawerPanel(HudDrawer::Sources),
            GlobalZIndex(20),
            HudScrollable,
            drawer_node(),
            ScrollPosition::default(),
            BackgroundColor(PANEL_BG),
        ))
        .with_children(|panel| {
            spawn_drawer_header(panel, "Map objects", "Sources");
            spawn_supporting_text(
                panel,
                "Drag a marker to move it. Click a synth source for live parameters.",
            );
            spawn_separator(panel);
            for source in &description.sources {
                let color = parse_hex_color(&source.color);
                spawn_source_row(panel, source.slot, &source.name, color);
                input::spawn_source_buttons(panel, source.slot, source.orbit_radius > 0.0);
                spawn_separator(panel);
            }
        });

    commands
        .spawn((
            HudPanel,
            HudDrawerPanel(HudDrawer::AddSource),
            GlobalZIndex(20),
            HudScrollable,
            drawer_node(),
            ScrollPosition::default(),
            BackgroundColor(PANEL_BG),
        ))
        .with_children(|panel| {
            spawn_drawer_header(panel, "Sound library", "Add a source");
            spawn_supporting_text(
                panel,
                "Choose a preset or import audio. New sources appear near the listener.",
            );
            spawn_separator(panel);
            input::spawn_add_source_controls(panel);
        });

    commands
        .spawn((
            HudPanel,
            HudDrawerPanel(HudDrawer::Mix),
            GlobalZIndex(20),
            HudScrollable,
            drawer_node(),
            ScrollPosition::default(),
            BackgroundColor(PANEL_BG),
        ))
        .with_children(|panel| {
            spawn_drawer_header(panel, "Playback", "Spatial mix");
            spawn_supporting_text(
                panel,
                "Choose a renderer and speaker layout. Output meters stay here until needed.",
            );
            spawn_separator(panel);
            spawn_section_label(panel, "RENDERER");
            input::spawn_render_mode_buttons(panel);
            spawn_section_label(panel, "SPEAKER LAYOUT");
            input::spawn_channel_mode_buttons(panel);
            spawn_separator(panel);
            spawn_section_label(panel, "OUTPUT LEVELS");
            spawn_channel_meters(panel);
        });

    commands
        .spawn((
            HudPanel,
            HudDrawerPanel(HudDrawer::Environment),
            GlobalZIndex(20),
            HudScrollable,
            drawer_node(),
            ScrollPosition::default(),
            BackgroundColor(PANEL_BG),
        ))
        .with_children(|panel| {
            spawn_drawer_header(panel, "Appearance & acoustics", "Environment");
            spawn_supporting_text(panel, "Shape the ambience without crowding the sound map.");
            spawn_separator(panel);
            spawn_section_label(panel, "ATMOSPHERE");
            input::spawn_atmosphere_controls(panel);
            spawn_separator(panel);
            spawn_section_label(panel, "LANDSCAPE & LIGHT");
            input::spawn_biome_controls(panel);
            spawn_separator(panel);
            spawn_section_label(panel, "MAP VIEW");
            spawn_fit_level_control(panel);
            spawn_separator(panel);
            spawn_section_label(panel, "LISTENER POSITION");
            panel.spawn((
                ListenerPositionText,
                Text::new("position — / direction —"),
                TextFont {
                    font_size: FONT_SIZE,
                    ..default()
                },
                TextColor(TEXT_PRIMARY),
            ));
        });

    // Advanced diagnostics: deliberately hidden behind the Signal action.
    commands
        .spawn((
            HudPanel,
            PipelinePanel,
            HudDrawerPanel(HudDrawer::Signal),
            GlobalZIndex(20),
            HudScrollable,
            drawer_node(),
            ScrollPosition::default(),
            BackgroundColor(PANEL_BG),
        ))
        .with_children(|panel| {
            spawn_drawer_header(panel, "Diagnostics", "Signal path");
            spawn_supporting_text(
                panel,
                "Advanced gain stages and received level for each source.",
            );
            spawn_separator(panel);
            for source in &description.sources {
                let color = parse_hex_color(&source.color);
                spawn_pipeline_source(panel, source.slot, &source.name, color, source.spl);
            }
        });
}

fn drawer_node() -> Node {
    Node {
        position_type: PositionType::Absolute,
        right: Val::Px(18.0),
        top: Val::Px(96.0),
        bottom: Val::Px(18.0),
        width: Val::Px(324.0),
        display: Display::None,
        flex_direction: FlexDirection::Column,
        padding: UiRect::all(Val::Px(18.0)),
        row_gap: Val::Px(12.0),
        overflow: Overflow::scroll_y(),
        border_radius: BorderRadius::all(Val::Px(16.0)),
        ..default()
    }
}

fn spawn_app_bar(commands: &mut Commands, _description: &SceneDescription) {
    commands
        .spawn((
            HudPanel,
            GlobalZIndex(10),
            HudScrollable,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Percent(50.0),
                top: Val::Px(22.0),
                width: Val::Px(790.0),
                height: Val::Px(58.0),
                margin: UiRect::left(Val::Px(-395.0)),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                padding: UiRect::axes(Val::Px(16.0), Val::Px(7.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(16.0)),
                ..default()
            },
            ScrollPosition::default(),
            BackgroundColor(HEADER_BG),
            BorderColor::all(Color::srgba(0.42, 0.58, 0.70, 0.42)),
            BoxShadow::new(
                Color::srgba(0.0, 0.0, 0.0, 0.48),
                Val::Px(0.0),
                Val::Px(10.0),
                Val::Px(2.0),
                Val::Px(24.0),
            ),
        ))
        .with_children(|bar| {
            bar.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(10.0),
                ..default()
            })
            .with_children(|brand| {
                spawn_brand_mark(brand);
                brand.spawn((
                    Text::new("Atrium"),
                    TextFont {
                        font_size: 17.0,
                        ..default()
                    },
                    TextColor(TEXT_PRIMARY),
                ));
            });

            bar.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(6.0),
                ..default()
            })
            .with_children(|status| {
                status.spawn((
                    Node {
                        width: Val::Px(7.0),
                        height: Val::Px(7.0),
                        border_radius: BorderRadius::all(Val::Px(999.0)),
                        ..default()
                    },
                    BackgroundColor(METER_GREEN),
                ));
                status.spawn((
                    Text::new("LIVE"),
                    TextFont {
                        font_size: 10.0,
                        ..default()
                    },
                    TextColor(METER_GREEN),
                ));
            });

            spawn_toolbar_divider(bar);
            spawn_scene_selector(bar);
            spawn_toolbar_divider(bar);

            bar.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(2.0),
                ..default()
            })
            .with_children(|actions| {
                for (drawer, icon) in [
                    (HudDrawer::Sources, ToolbarIcon::Grid),
                    (HudDrawer::AddSource, ToolbarIcon::Add),
                    (HudDrawer::Mix, ToolbarIcon::Mix),
                    (HudDrawer::Environment, ToolbarIcon::World),
                    (HudDrawer::Signal, ToolbarIcon::Signal),
                ] {
                    spawn_icon_menu_button(actions, drawer, icon);
                }
                spawn_help_button(actions);
            });
        });
}

fn spawn_brand_mark(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn((
            Node {
                width: Val::Px(24.0),
                height: Val::Px(24.0),
                border: UiRect::all(Val::Px(1.0)),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(999.0)),
                ..default()
            },
            BorderColor::all(Color::srgba(0.90, 0.95, 1.0, 0.92)),
        ))
        .with_children(|mark| {
            mark.spawn((
                Node {
                    width: Val::Px(15.0),
                    height: Val::Px(8.0),
                    border: UiRect::all(Val::Px(1.0)),
                    border_radius: BorderRadius {
                        top_left: Val::Px(8.0),
                        top_right: Val::Px(1.0),
                        bottom_right: Val::Px(8.0),
                        bottom_left: Val::Px(1.0),
                    },
                    ..default()
                },
                BorderColor::all(ACCENT),
                UiTransform::from_rotation(Rot2::degrees(-42.0)),
            ))
            .with_children(|wave| {
                wave.spawn((
                    Node {
                        width: Val::Px(10.0),
                        height: Val::Px(1.0),
                        ..default()
                    },
                    BackgroundColor(ACCENT),
                ));
            });
        });
}

#[derive(Clone, Copy)]
enum ToolbarIcon {
    Grid,
    Add,
    Mix,
    World,
    Signal,
}

fn spawn_toolbar_divider(parent: &mut ChildSpawnerCommands) {
    parent.spawn((
        Node {
            width: Val::Px(1.0),
            height: Val::Px(26.0),
            margin: UiRect::horizontal(Val::Px(4.0)),
            ..default()
        },
        BackgroundColor(SEPARATOR),
    ));
}

fn spawn_scene_selector(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn((
            HudMenuButton(HudDrawer::Scenes),
            HudSceneSelector,
            Button,
            Node {
                width: Val::Px(138.0),
                height: Val::Px(36.0),
                padding: UiRect::horizontal(Val::Px(11.0)),
                flex_direction: FlexDirection::Row,
                justify_content: JustifyContent::SpaceBetween,
                align_items: AlignItems::Center,
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(8.0)),
                ..default()
            },
            BackgroundColor(Color::srgba(0.04, 0.08, 0.13, 0.86)),
            BorderColor::all(Color::srgba(0.42, 0.58, 0.70, 0.36)),
        ))
        .with_children(|selector| {
            selector.spawn((
                SceneNameText,
                Text::new("Default"),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(TEXT_PRIMARY),
            ));
            selector.spawn((
                Text::new("v"),
                TextFont {
                    font_size: 9.0,
                    ..default()
                },
                TextColor(TEXT_LABEL),
            ));
        });
}

fn spawn_icon_menu_button(parent: &mut ChildSpawnerCommands, drawer: HudDrawer, icon: ToolbarIcon) {
    parent
        .spawn((
            HudMenuButton(drawer),
            Button,
            Node {
                position_type: PositionType::Relative,
                width: Val::Px(40.0),
                height: Val::Px(42.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(8.0)),
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .with_children(|button| {
            spawn_toolbar_icon(button, drawer, icon);
            button.spawn((
                HudMenuUnderline(drawer),
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(8.0),
                    bottom: Val::Px(0.0),
                    width: Val::Px(24.0),
                    height: Val::Px(2.0),
                    display: Display::None,
                    border_radius: BorderRadius::all(Val::Px(999.0)),
                    ..default()
                },
                BackgroundColor(ACCENT),
            ));
        });
}

fn spawn_toolbar_icon(parent: &mut ChildSpawnerCommands, drawer: HudDrawer, icon: ToolbarIcon) {
    match icon {
        ToolbarIcon::Grid => {
            parent
                .spawn(Node {
                    width: Val::Px(18.0),
                    height: Val::Px(18.0),
                    flex_direction: FlexDirection::Row,
                    flex_wrap: FlexWrap::Wrap,
                    column_gap: Val::Px(2.0),
                    row_gap: Val::Px(2.0),
                    ..default()
                })
                .with_children(|grid| {
                    for _ in 0..4 {
                        grid.spawn((
                            HudMenuIconPart(drawer),
                            Node {
                                width: Val::Px(8.0),
                                height: Val::Px(8.0),
                                border: UiRect::all(Val::Px(1.0)),
                                ..default()
                            },
                            BorderColor::all(TEXT_PRIMARY),
                        ));
                    }
                });
        }
        ToolbarIcon::Add => {
            parent
                .spawn((
                    HudMenuIconPart(drawer),
                    Node {
                        width: Val::Px(20.0),
                        height: Val::Px(20.0),
                        border: UiRect::all(Val::Px(1.0)),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius::all(Val::Px(999.0)),
                        ..default()
                    },
                    BorderColor::all(TEXT_PRIMARY),
                ))
                .with_children(|circle| {
                    circle.spawn((
                        HudMenuIconPart(drawer),
                        Text::new("+"),
                        TextFont {
                            font_size: 13.0,
                            ..default()
                        },
                        TextColor(TEXT_PRIMARY),
                    ));
                });
        }
        ToolbarIcon::World => {
            parent
                .spawn((
                    HudMenuIconPart(drawer),
                    Node {
                        position_type: PositionType::Relative,
                        width: Val::Px(18.0),
                        height: Val::Px(12.0),
                        border: UiRect::all(Val::Px(1.0)),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius {
                            top_left: Val::Px(12.0),
                            top_right: Val::Px(0.0),
                            bottom_right: Val::Px(12.0),
                            bottom_left: Val::Px(0.0),
                        },
                        ..default()
                    },
                    BorderColor::all(TEXT_PRIMARY),
                    UiTransform::from_rotation(Rot2::degrees(-40.0)),
                ))
                .with_children(|leaf| {
                    leaf.spawn((
                        HudMenuIconPart(drawer),
                        Node {
                            width: Val::Px(13.0),
                            height: Val::Px(1.0),
                            ..default()
                        },
                        BackgroundColor(TEXT_PRIMARY),
                    ));
                    for (left, rotation) in [(5.0, -42.0), (9.0, 42.0)] {
                        leaf.spawn((
                            HudMenuIconPart(drawer),
                            Node {
                                position_type: PositionType::Absolute,
                                left: Val::Px(left),
                                top: Val::Px(5.5),
                                width: Val::Px(4.0),
                                height: Val::Px(1.0),
                                ..default()
                            },
                            BackgroundColor(TEXT_PRIMARY),
                            UiTransform::from_rotation(Rot2::degrees(rotation)),
                        ));
                    }
                });
        }
        ToolbarIcon::Mix => {
            parent
                .spawn(Node {
                    width: Val::Px(20.0),
                    height: Val::Px(20.0),
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    ..default()
                })
                .with_children(|mix| {
                    for top in [3.0, 9.0, 5.0] {
                        mix.spawn((
                            HudMenuIconPart(drawer),
                            Node {
                                position_type: PositionType::Relative,
                                width: Val::Px(2.0),
                                height: Val::Px(18.0),
                                ..default()
                            },
                            BackgroundColor(TEXT_PRIMARY),
                        ))
                        .with_children(|line| {
                            line.spawn((
                                HudMenuIconPart(drawer),
                                Node {
                                    position_type: PositionType::Absolute,
                                    left: Val::Px(-2.0),
                                    top: Val::Px(top),
                                    width: Val::Px(6.0),
                                    height: Val::Px(6.0),
                                    border_radius: BorderRadius::all(Val::Px(999.0)),
                                    ..default()
                                },
                                BackgroundColor(HEADER_BG),
                                Outline {
                                    width: Val::Px(1.0),
                                    offset: Val::Px(0.0),
                                    color: TEXT_PRIMARY,
                                },
                            ));
                        });
                    }
                });
        }
        ToolbarIcon::Signal => {
            parent
                .spawn(Node {
                    width: Val::Px(22.0),
                    height: Val::Px(20.0),
                    flex_direction: FlexDirection::Row,
                    justify_content: JustifyContent::SpaceBetween,
                    align_items: AlignItems::Center,
                    ..default()
                })
                .with_children(|wave| {
                    for height in [5.0, 11.0, 18.0, 10.0, 5.0] {
                        wave.spawn((
                            HudMenuIconPart(drawer),
                            Node {
                                width: Val::Px(2.0),
                                height: Val::Px(height),
                                border_radius: BorderRadius::all(Val::Px(999.0)),
                                ..default()
                            },
                            BackgroundColor(TEXT_PRIMARY),
                        ));
                    }
                });
        }
    }
}

fn spawn_help_button(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn((
            HudHelpButton,
            Button,
            Node {
                width: Val::Px(40.0),
                height: Val::Px(42.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .with_children(|button| {
            button
                .spawn((
                    Node {
                        width: Val::Px(20.0),
                        height: Val::Px(20.0),
                        border: UiRect::all(Val::Px(1.0)),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius::all(Val::Px(999.0)),
                        ..default()
                    },
                    BorderColor::all(TEXT_PRIMARY),
                ))
                .with_children(|circle| {
                    circle.spawn((
                        Text::new("?"),
                        TextFont {
                            font_size: 11.0,
                            ..default()
                        },
                        TextColor(TEXT_PRIMARY),
                    ));
                });
        });
}

fn spawn_master_volume(commands: &mut Commands) {
    commands
        .spawn((
            HudPanel,
            HudScrollable,
            GlobalZIndex(10),
            ScrollPosition::default(),
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(20.0),
                bottom: Val::Px(18.0),
                height: Val::Px(52.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(12.0),
                padding: UiRect::axes(Val::Px(14.0), Val::Px(8.0)),
                border: UiRect::all(Val::Px(1.0)),
                border_radius: BorderRadius::all(Val::Px(999.0)),
                ..default()
            },
            BackgroundColor(HEADER_BG),
            BorderColor::all(SEPARATOR),
        ))
        .with_children(|pill| {
            spawn_speaker_icon(pill);
            pill.spawn(Node {
                width: Val::Px(166.0),
                flex_direction: FlexDirection::Column,
                row_gap: Val::Px(5.0),
                ..default()
            })
            .with_children(|stack| {
                stack
                    .spawn(Node {
                        width: Val::Percent(100.0),
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        ..default()
                    })
                    .with_children(|row| {
                        row.spawn((
                            Text::new("Master Volume"),
                            TextFont {
                                font_size: 11.0,
                                ..default()
                            },
                            TextColor(TEXT_PRIMARY),
                        ));
                        row.spawn((
                            MasterGainText,
                            Text::new("0.0 dB"),
                            TextFont {
                                font_size: 10.0,
                                ..default()
                            },
                            TextColor(TEXT_LABEL),
                        ));
                    });
                stack
                    .spawn(Node {
                        width: Val::Percent(100.0),
                        height: Val::Px(5.0),
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(2.0),
                        ..default()
                    })
                    .with_children(|meter| {
                        for index in 0..14 {
                            meter.spawn((
                                MasterGainSegment((index + 1) as f32 / 14.0),
                                Button,
                                Node {
                                    width: Val::Px(10.0),
                                    height: Val::Px(4.0),
                                    border_radius: BorderRadius::all(Val::Px(1.0)),
                                    ..default()
                                },
                                BackgroundColor(METER_BG),
                            ));
                        }
                    });
            });
        });
}

fn spawn_speaker_icon(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn(Node {
            position_type: PositionType::Relative,
            width: Val::Px(28.0),
            height: Val::Px(24.0),
            ..default()
        })
        .with_children(|icon| {
            icon.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(1.0),
                    top: Val::Px(8.0),
                    width: Val::Px(6.0),
                    height: Val::Px(8.0),
                    border: UiRect::all(Val::Px(1.5)),
                    ..default()
                },
                BorderColor::all(TEXT_PRIMARY),
            ));
            icon.spawn((
                Node {
                    position_type: PositionType::Absolute,
                    left: Val::Px(7.0),
                    top: Val::Px(5.0),
                    width: Val::Px(7.0),
                    height: Val::Px(14.0),
                    border: UiRect::all(Val::Px(1.5)),
                    border_radius: BorderRadius {
                        top_left: Val::Px(0.0),
                        top_right: Val::Px(6.0),
                        bottom_right: Val::Px(6.0),
                        bottom_left: Val::Px(0.0),
                    },
                    ..default()
                },
                BorderColor::all(TEXT_PRIMARY),
            ));
            for (left, font_size) in [(14.0, 14.0), (18.0, 18.0)] {
                icon.spawn((
                    Text::new(")"),
                    TextFont {
                        font_size,
                        ..default()
                    },
                    TextColor(TEXT_PRIMARY),
                    Node {
                        position_type: PositionType::Absolute,
                        left: Val::Px(left),
                        top: Val::Px(if font_size < 16.0 { 4.0 } else { 1.0 }),
                        ..default()
                    },
                ));
            }
        });
}

fn spawn_zoom_controls(commands: &mut Commands) {
    commands
        .spawn((
            HudPanel,
            HudScrollable,
            GlobalZIndex(10),
            ScrollPosition::default(),
            Node {
                position_type: PositionType::Absolute,
                right: Val::Px(20.0),
                bottom: Val::Px(18.0),
                height: Val::Px(52.0),
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(10.0),
                ..default()
            },
        ))
        .with_children(|controls| {
            controls
                .spawn((
                    Node {
                        height: Val::Px(52.0),
                        flex_direction: FlexDirection::Row,
                        align_items: AlignItems::Center,
                        column_gap: Val::Px(6.0),
                        padding: UiRect::horizontal(Val::Px(12.0)),
                        border: UiRect::all(Val::Px(1.0)),
                        border_radius: BorderRadius::all(Val::Px(999.0)),
                        ..default()
                    },
                    BackgroundColor(HEADER_BG),
                    BorderColor::all(SEPARATOR),
                ))
                .with_children(|pill| {
                    spawn_magnifier_icon(pill);
                    spawn_zoom_text_button(pill, ZoomOutButton, "-");
                    pill.spawn((
                        ZoomText,
                        Text::new("100%"),
                        TextFont {
                            font_size: 11.0,
                            ..default()
                        },
                        TextColor(ACCENT),
                        Node {
                            min_width: Val::Px(44.0),
                            justify_content: JustifyContent::Center,
                            ..default()
                        },
                    ));
                    spawn_zoom_text_button(pill, ZoomInButton, "+");
                });

            controls
                .spawn((
                    ZoomResetButton,
                    Button,
                    Node {
                        width: Val::Px(52.0),
                        height: Val::Px(52.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border: UiRect::all(Val::Px(1.0)),
                        border_radius: BorderRadius::all(Val::Px(14.0)),
                        ..default()
                    },
                    BackgroundColor(HEADER_BG),
                    BorderColor::all(SEPARATOR),
                ))
                .with_children(spawn_fit_icon);
        });
}

fn spawn_zoom_text_button(parent: &mut ChildSpawnerCommands, marker: impl Component, label: &str) {
    parent
        .spawn((
            marker,
            Button,
            Node {
                width: Val::Px(28.0),
                height: Val::Px(36.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(999.0)),
                ..default()
            },
            BackgroundColor(Color::NONE),
        ))
        .with_children(|button| {
            button.spawn((
                Text::new(label),
                TextFont {
                    font_size: 16.0,
                    ..default()
                },
                TextColor(TEXT_PRIMARY),
            ));
        });
}

fn spawn_magnifier_icon(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn(Node {
            width: Val::Px(25.0),
            height: Val::Px(28.0),
            justify_content: JustifyContent::Center,
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|icon| {
            icon.spawn((
                Node {
                    position_type: PositionType::Relative,
                    width: Val::Px(13.0),
                    height: Val::Px(13.0),
                    border: UiRect::all(Val::Px(1.5)),
                    border_radius: BorderRadius::all(Val::Px(999.0)),
                    ..default()
                },
                BorderColor::all(TEXT_PRIMARY),
            ))
            .with_children(|lens| {
                lens.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        right: Val::Px(-6.0),
                        bottom: Val::Px(-3.0),
                        width: Val::Px(7.0),
                        height: Val::Px(1.5),
                        border_radius: BorderRadius::all(Val::Px(999.0)),
                        ..default()
                    },
                    BackgroundColor(TEXT_PRIMARY),
                    UiTransform::from_rotation(Rot2::degrees(45.0)),
                ));
            });
        });
}

fn spawn_fit_icon(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn(Node {
            position_type: PositionType::Relative,
            width: Val::Px(21.0),
            height: Val::Px(21.0),
            ..default()
        })
        .with_children(|icon| {
            for (left, top, right, bottom) in [
                (true, true, false, false),
                (false, true, true, false),
                (true, false, false, true),
                (false, false, true, true),
            ] {
                icon.spawn((
                    Node {
                        position_type: PositionType::Absolute,
                        left: left.then_some(Val::Px(0.0)).unwrap_or(Val::Auto),
                        top: top.then_some(Val::Px(0.0)).unwrap_or(Val::Auto),
                        right: right.then_some(Val::Px(0.0)).unwrap_or(Val::Auto),
                        bottom: bottom.then_some(Val::Px(0.0)).unwrap_or(Val::Auto),
                        width: Val::Px(7.0),
                        height: Val::Px(7.0),
                        border: UiRect {
                            left: Val::Px(if left { 1.5 } else { 0.0 }),
                            top: Val::Px(if top { 1.5 } else { 0.0 }),
                            right: Val::Px(if right { 1.5 } else { 0.0 }),
                            bottom: Val::Px(if bottom { 1.5 } else { 0.0 }),
                        },
                        ..default()
                    },
                    BorderColor::all(TEXT_PRIMARY),
                ));
            }
        });
}

fn spawn_drawer_header(parent: &mut ChildSpawnerCommands, eyebrow: &str, title: &str) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|header| {
            header
                .spawn(Node {
                    flex_direction: FlexDirection::Column,
                    row_gap: Val::Px(2.0),
                    ..default()
                })
                .with_children(|labels| {
                    labels.spawn((
                        Text::new(eyebrow.to_uppercase()),
                        TextFont {
                            font_size: 9.0,
                            ..default()
                        },
                        TextColor(ACCENT),
                    ));
                    labels.spawn((
                        Text::new(title),
                        TextFont {
                            font_size: 18.0,
                            ..default()
                        },
                        TextColor(TEXT_PRIMARY),
                    ));
                });

            header
                .spawn((
                    HudCloseButton,
                    Button,
                    Node {
                        width: Val::Px(30.0),
                        height: Val::Px(30.0),
                        justify_content: JustifyContent::Center,
                        align_items: AlignItems::Center,
                        border_radius: BorderRadius::all(Val::Px(999.0)),
                        ..default()
                    },
                    BackgroundColor(BUTTON_BG),
                ))
                .with_children(|button| {
                    button.spawn((
                        Text::new("x"),
                        TextFont {
                            font_size: 12.0,
                            ..default()
                        },
                        TextColor(TEXT_LABEL),
                    ));
                });
        });
}

fn spawn_supporting_text(parent: &mut ChildSpawnerCommands, copy: &str) {
    parent.spawn((
        Text::new(copy),
        TextFont {
            font_size: FONT_SIZE_SMALL,
            ..default()
        },
        TextColor(TEXT_LABEL),
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

fn spawn_fit_level_control(parent: &mut ChildSpawnerCommands) {
    parent
        .spawn(Node {
            width: Val::Percent(100.0),
            flex_direction: FlexDirection::Row,
            justify_content: JustifyContent::SpaceBetween,
            align_items: AlignItems::Center,
            ..default()
        })
        .with_children(|row| {
            row.spawn((
                Text::new("Fit audibility"),
                TextFont {
                    font_size: FONT_SIZE_SMALL,
                    ..default()
                },
                TextColor(TEXT_LABEL),
            ));
            row.spawn(Node {
                flex_direction: FlexDirection::Row,
                align_items: AlignItems::Center,
                column_gap: Val::Px(5.0),
                ..default()
            })
            .with_children(|controls| {
                spawn_fit_level_button(controls, FitLevelDownButton, "-");
                controls.spawn((
                    FitLevelText,
                    Text::new("-60 dB"),
                    TextFont {
                        font_size: 10.0,
                        ..default()
                    },
                    TextColor(ACCENT),
                    Node {
                        min_width: Val::Px(48.0),
                        justify_content: JustifyContent::Center,
                        ..default()
                    },
                ));
                spawn_fit_level_button(controls, FitLevelUpButton, "+");
            });
        });
}

fn spawn_fit_level_button(parent: &mut ChildSpawnerCommands, marker: impl Component, label: &str) {
    parent
        .spawn((
            marker,
            Button,
            Node {
                width: Val::Px(24.0),
                height: Val::Px(24.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                border_radius: BorderRadius::all(Val::Px(6.0)),
                ..default()
            },
            BackgroundColor(BUTTON_BG),
        ))
        .with_children(|button| {
            button.spawn((
                Text::new(label),
                TextFont {
                    font_size: 11.0,
                    ..default()
                },
                TextColor(TEXT_PRIMARY),
            ));
        });
}

/// Open/close drawers from the compact app bar. Only one drawer can be open.
pub(crate) fn handle_hud_menu_buttons(
    menu_buttons: Query<(&HudMenuButton, &Interaction), Changed<Interaction>>,
    close_buttons: Query<&Interaction, (Changed<Interaction>, With<HudCloseButton>)>,
    mut state: ResMut<HudState>,
) {
    for (button, interaction) in &menu_buttons {
        if *interaction == Interaction::Pressed {
            state.active_drawer = if state.active_drawer == Some(button.0) {
                None
            } else {
                Some(button.0)
            };
        }
    }

    for interaction in &close_buttons {
        if *interaction == Interaction::Pressed {
            state.active_drawer = None;
        }
    }
}

/// Apply drawer visibility and active navigation styling.
pub(crate) fn sync_hud_drawers(
    state: Res<HudState>,
    mut drawers: Query<(&HudDrawerPanel, &mut Node), Without<HudMenuUnderline>>,
    mut buttons: Query<
        (
            &HudMenuButton,
            Option<&HudSceneSelector>,
            &mut BackgroundColor,
        ),
        Without<HudMenuIconPart>,
    >,
    mut underlines: Query<(&HudMenuUnderline, &mut Node), Without<HudDrawerPanel>>,
    mut icon_borders: Query<(&HudMenuIconPart, &mut BorderColor)>,
    mut icon_backgrounds: Query<
        (&HudMenuIconPart, &mut BackgroundColor),
        (Without<HudMenuButton>, Without<BorderColor>),
    >,
    mut icon_outlines: Query<(&HudMenuIconPart, &mut Outline)>,
    mut icon_text: Query<(&HudMenuIconPart, &mut TextColor)>,
) {
    for (drawer, mut node) in &mut drawers {
        node.display = if state.active_drawer == Some(drawer.0) {
            Display::Flex
        } else {
            Display::None
        };
    }

    for (button, scene_selector, mut background) in &mut buttons {
        let active = state.active_drawer == Some(button.0);
        background.0 = if scene_selector.is_some() {
            if active {
                Color::srgba(0.08, 0.18, 0.25, 0.96)
            } else {
                Color::srgba(0.04, 0.08, 0.13, 0.86)
            }
        } else {
            Color::NONE
        };
    }

    for (underline, mut node) in &mut underlines {
        node.display = if state.active_drawer == Some(underline.0) {
            Display::Flex
        } else {
            Display::None
        };
    }

    for (part, mut border) in &mut icon_borders {
        border.set_all(if state.active_drawer == Some(part.0) {
            ACCENT
        } else {
            TEXT_PRIMARY
        });
    }
    for (part, mut background) in &mut icon_backgrounds {
        background.0 = if state.active_drawer == Some(part.0) {
            ACCENT
        } else {
            TEXT_PRIMARY
        };
    }
    for (part, mut outline) in &mut icon_outlines {
        outline.color = if state.active_drawer == Some(part.0) {
            ACCENT
        } else {
            TEXT_PRIMARY
        };
    }
    for (part, mut color) in &mut icon_text {
        color.0 = if state.active_drawer == Some(part.0) {
            ACCENT
        } else {
            TEXT_PRIMARY
        };
    }
}

pub(crate) fn sync_scene_name(
    state: Res<HudState>,
    mut text: Query<&mut Text, With<SceneNameText>>,
) {
    for mut label in &mut text {
        **label = state.scene_name.clone();
    }
}

pub(crate) fn handle_help_button(
    buttons: Query<&Interaction, (Changed<Interaction>, With<HudHelpButton>)>,
    mut menu: ResMut<BindingsMenu>,
    mut hud_state: ResMut<HudState>,
    mut overlay: Query<&mut Visibility, With<BindingsOverlay>>,
) {
    for interaction in &buttons {
        if *interaction != Interaction::Pressed {
            continue;
        }
        hud_state.active_drawer = None;
        menu.open = !menu.open;
        let visibility = if menu.open {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        for mut value in &mut overlay {
            *value = visibility;
        }
    }
}

pub(crate) fn handle_master_gain_buttons(
    segments: Query<(&MasterGainSegment, &Interaction), Changed<Interaction>>,
    mut state: ResMut<HudState>,
    mut command_sender: ResMut<CommandSender>,
) {
    let mut next_gain = None;
    for (segment, interaction) in &segments {
        if *interaction == Interaction::Pressed {
            next_gain = Some(segment.0);
        }
    }
    if let Some(gain) = next_gain {
        state.master_gain = gain;
        command_sender.send(Command::SetMasterGain {
            gain: state.master_gain,
        });
    }
}

pub(crate) fn sync_master_gain_ui(
    state: Res<HudState>,
    mut texts: Query<&mut Text, With<MasterGainText>>,
    mut segments: Query<(&MasterGainSegment, &mut BackgroundColor)>,
) {
    let db = if state.master_gain > 0.0001 {
        format!("{:+.1} dB", 20.0 * state.master_gain.log10())
    } else {
        "-inf dB".to_string()
    };
    for mut text in &mut texts {
        **text = db.clone();
    }
    for (segment, mut background) in &mut segments {
        background.0 = if segment.0 <= state.master_gain + 0.001 {
            ACCENT
        } else {
            METER_BG
        };
    }
}

pub(crate) fn handle_zoom_buttons(
    out: Query<&Interaction, (Changed<Interaction>, With<ZoomOutButton>)>,
    zoom_in: Query<&Interaction, (Changed<Interaction>, With<ZoomInButton>)>,
    reset: Query<&Interaction, (Changed<Interaction>, With<ZoomResetButton>)>,
    windows: Query<&Window, With<PrimaryWindow>>,
    sources: Query<(&SoundSourceIndex, &Transform), With<SoundSource>>,
    listener: Query<&Transform, With<SoundListener>>,
    telemetry: Res<LatestTelemetry>,
    description: Res<SceneDescription>,
    mut settings: ResMut<CameraSettings>,
    mut pan: ResMut<CameraPan>,
) {
    for interaction in &out {
        if *interaction == Interaction::Pressed {
            settings.step_zoom(-CameraSettings::ZOOM_STEP_PERCENT);
        }
    }
    for interaction in &zoom_in {
        if *interaction == Interaction::Pressed {
            settings.step_zoom(CameraSettings::ZOOM_STEP_PERCENT);
        }
    }
    for interaction in &reset {
        if *interaction != Interaction::Pressed {
            continue;
        }

        let Ok(listener) = listener.single() else {
            continue;
        };
        let listener_center = listener.translation.truncate();
        let listener_radius = (crate::scene::BADGE_RADIUS + 0.21) * listener.scale.x;
        let mut minimum = listener_center - Vec2::splat(listener_radius);
        let mut maximum = listener_center + Vec2::splat(listener_radius);

        for (index, source) in &sources {
            let Some(frame_source) = telemetry.frame.sources.get(index.0) else {
                continue;
            };
            if frame_source.is_muted || frame_source.gain_db < settings.fit_audibility_db {
                continue;
            }
            let center = source.translation.truncate();
            let radius = (crate::scene::BADGE_RADIUS + 0.18) * source.scale.x;
            minimum = minimum.min(center - Vec2::splat(radius));
            maximum = maximum.max(center + Vec2::splat(radius));
        }

        let center = (minimum + maximum) * 0.5;
        let bounds = (maximum - minimum) * 1.14;
        let aspect = windows
            .single()
            .map(|window| (window.width() / window.height().max(1.0)).max(0.1))
            .unwrap_or(1.6);
        let required_height = bounds.y.max(bounds.x / aspect).max(0.5);
        let base_height = description
            .environment
            .width
            .max(description.environment.depth)
            * 1.4;
        settings.set_scale_snapped(required_height / base_height.max(1.0));
        pan.offset = center - listener_center;
    }
}

pub(crate) fn handle_fit_level_buttons(
    down: Query<&Interaction, (Changed<Interaction>, With<FitLevelDownButton>)>,
    up: Query<&Interaction, (Changed<Interaction>, With<FitLevelUpButton>)>,
    mut settings: ResMut<CameraSettings>,
    mut text: Query<&mut Text, With<FitLevelText>>,
) {
    for interaction in &down {
        if *interaction == Interaction::Pressed {
            settings.fit_audibility_db = (settings.fit_audibility_db - 5.0).max(-96.0);
        }
    }
    for interaction in &up {
        if *interaction == Interaction::Pressed {
            settings.fit_audibility_db = (settings.fit_audibility_db + 5.0).min(0.0);
        }
    }
    for mut label in &mut text {
        **label = format!("{:.0} dB", settings.fit_audibility_db);
    }
}

pub(crate) fn sync_zoom_text(
    settings: Res<CameraSettings>,
    mut texts: Query<&mut Text, With<ZoomText>>,
) {
    let percentage = settings.zoom_percent().round();
    for mut text in &mut texts {
        **text = format!("{percentage:.0}%");
    }
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

fn spawn_source_row(parent: &mut ChildSpawnerCommands, index: usize, name: &str, color: [f32; 3]) {
    let source_color = Color::srgb(color[0], color[1], color[2]);

    parent
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(2.0),
            margin: UiRect::bottom(Val::Px(4.0)),
            ..default()
        })
        .with_children(|row| {
            row.spawn(Node {
                flex_direction: FlexDirection::Row,
                column_gap: Val::Px(6.0),
                align_items: AlignItems::Center,
                ..default()
            })
            .with_children(|name_row| {
                name_row.spawn((
                    Node {
                        width: Val::Px(8.0),
                        height: Val::Px(8.0),
                        ..default()
                    },
                    BackgroundColor(source_color),
                ));
                name_row.spawn((
                    Text::new(name),
                    TextFont {
                        font_size: FONT_SIZE,
                        ..default()
                    },
                    TextColor(source_color),
                ));
            });

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

// ── Pipeline stage colors ────────────────────────────────────────────────

const STAGE_COLOR_DIST: Color = Color::srgb(0.96, 0.26, 0.21);
const STAGE_COLOR_EMIT: Color = Color::srgb(0.61, 0.15, 0.69);
const STAGE_COLOR_HEAR: Color = Color::srgb(0.31, 0.76, 0.97);
const STAGE_COLOR_TOTAL: Color = Color::srgb(0.30, 0.69, 0.31);

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
    name: &str,
    color: [f32; 3],
    spl: f32,
) {
    let source_color = Color::srgb(color[0], color[1], color[2]);

    parent
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(3.0),
            margin: UiRect::bottom(Val::Px(6.0)),
            ..default()
        })
        .with_children(|col| {
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
                    Text::new(format!("{} ({:.0} dB SPL)", name, spl)),
                    TextFont {
                        font_size: FONT_SIZE,
                        ..default()
                    },
                    TextColor(source_color),
                ));
            });

            for stage in [
                PipelineStage::Distance,
                PipelineStage::Emission,
                PipelineStage::Hearing,
            ] {
                spawn_pipeline_bar(col, index, stage);
            }

            spawn_pipeline_total(col, index);

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

                        row.spawn((
                            Node {
                                width: Val::Px(160.0),
                                height: Val::Px(6.0),
                                ..default()
                            },
                            BackgroundColor(METER_BG),
                        ))
                        .with_children(|meter_bg| {
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
            "pos: ({:.1}, {:.1}, {:.1})  yaw: {:.0} deg",
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
fn gain_to_bar_fraction(gain: f32) -> f32 {
    if gain <= 0.0 {
        return 0.0;
    }
    let db = 20.0 * gain.log10();
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
    audio_sources: Query<(&SoundSourceIndex, &SoundSource)>,
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
        if marker.source_index < frame.source_count as usize {
            let telemetry = &frame.sources[marker.source_index];
            // Look up the reference SPL from the SoundSource component
            let reference_spl = audio_sources
                .iter()
                .find(|(idx, _)| idx.0 == marker.source_index)
                .map(|(_, s)| s.spl)
                .unwrap_or(80.0);

            if telemetry.gain_db.is_finite() {
                let received = reference_spl + telemetry.gain_db;
                **text = format!("{:.0} dB SPL", received);
            } else {
                **text = "-∞ dB SPL".to_string();
            }
        }
    }
}

/// Scroll HUD panels with the mouse wheel when the cursor is over them, and
/// publish whether the cursor is over a panel so the camera zoom can stand down.
pub(crate) fn scroll_hud_panels(
    windows: Query<&Window, With<PrimaryWindow>>,
    mut scroll: MessageReader<MouseWheel>,
    mut panels: Query<
        (
            &ComputedNode,
            &UiGlobalTransform,
            &Node,
            &mut ScrollPosition,
        ),
        With<HudScrollable>,
    >,
    mut over_hud: ResMut<PointerOverHud>,
) {
    let Some(cursor) = windows.single().ok().and_then(|w| w.cursor_position()) else {
        over_hud.0 = false;
        return;
    };

    over_hud.0 = panels.iter().any(|(computed, transform, node, _)| {
        node.display != Display::None && computed.contains_point(*transform, cursor)
    });

    let mut delta = 0.0;
    for event in scroll.read() {
        delta += match event.unit {
            MouseScrollUnit::Line => event.y * 20.0,
            MouseScrollUnit::Pixel => event.y,
        };
    }
    if delta == 0.0 {
        return;
    }

    for (computed, transform, node, mut position) in &mut panels {
        if node.display != Display::None && computed.contains_point(*transform, cursor) {
            // Scroll up (positive wheel) moves content down → decrease offset.
            position.0.y = (position.0.y - delta).max(0.0);
        }
    }
}
