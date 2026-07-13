//! Controls / key-bindings overlay.
//!
//! A modal help card listing every keyboard and gamepad binding, toggled with
//! `F1` (keyboard) or Circle / Start (gamepad). A small persistent hint at the
//! bottom of the screen tells the user how to open it.
//!
//! The overlay is spawned once at startup and never despawned on scene reload
//! (its markers aren't in the reload teardown filter), so it survives scene
//! swaps.

use bevy::prelude::*;

// ── Colors ──────────────────────────────────────────────────────────────────

const BACKDROP: Color = Color::srgba(0.0, 0.0, 0.0, 0.78);
const CARD_BG: Color = Color::srgb(0.08, 0.08, 0.10);
const TITLE: Color = Color::srgb(0.31, 0.76, 0.97);
const HEADER: Color = Color::srgb(0.95, 0.76, 0.30);
const KEY_COLOR: Color = Color::srgb(0.85, 0.85, 0.88);
const ACTION_COLOR: Color = Color::srgb(0.62, 0.62, 0.66);

// ── Marker + state ────────────────────────────────────────────────────────────

/// Root node of the toggleable modal overlay.
#[derive(Component)]
pub(crate) struct BindingsOverlay;

/// Whether the overlay is currently shown.
#[derive(Resource, Default)]
pub(crate) struct BindingsMenu {
    pub open: bool,
}

/// One (input, action) row.
type Binding = (&'static str, &'static str);

const KEYBOARD_BINDINGS: &[Binding] = &[
    ("W A S D / Arrows", "Move listener"),
    ("Q / E", "Turn left / right"),
    ("Scroll wheel", "Zoom"),
    ("Drag", "Move a source"),
    ("B", "Cycle biome"),
    ("N", "Toggle day / night"),
    ("Cmd + S", "Save scene"),
    ("F1", "Toggle this menu"),
];

const GAMEPAD_BINDINGS: &[Binding] = &[
    ("Left stick", "Move listener"),
    ("Right stick", "Turn"),
    ("L2 / R2", "Zoom out / in"),
    ("L1 / R1", "Prev / next render mode"),
    ("Cross (X)", "Reset scene"),
    ("Circle / Start", "Toggle this menu"),
];

// ── Setup ─────────────────────────────────────────────────────────────────────

/// Spawn the (hidden) modal overlay and the persistent open-hint.
pub(crate) fn setup_bindings_ui(mut commands: Commands) {
    // Modal overlay root (full-screen dimmer), hidden until toggled.
    commands
        .spawn((
            BindingsOverlay,
            Visibility::Hidden,
            Node {
                position_type: PositionType::Absolute,
                left: Val::Px(0.0),
                top: Val::Px(0.0),
                width: Val::Percent(100.0),
                height: Val::Percent(100.0),
                justify_content: JustifyContent::Center,
                align_items: AlignItems::Center,
                ..default()
            },
            BackgroundColor(BACKDROP),
            GlobalZIndex(100),
        ))
        .with_children(|overlay| {
            overlay
                .spawn((
                    Node {
                        flex_direction: FlexDirection::Column,
                        padding: UiRect::all(Val::Px(24.0)),
                        row_gap: Val::Px(16.0),
                        max_width: Val::Px(640.0),
                        border_radius: BorderRadius::all(Val::Px(8.0)),
                        ..default()
                    },
                    BackgroundColor(CARD_BG),
                ))
                .with_children(|card| {
                    card.spawn((
                        Text::new("Controls"),
                        TextFont {
                            font_size: 22.0,
                            ..default()
                        },
                        TextColor(TITLE),
                    ));

                    // Two columns side by side.
                    card.spawn(Node {
                        flex_direction: FlexDirection::Row,
                        column_gap: Val::Px(40.0),
                        ..default()
                    })
                    .with_children(|columns| {
                        spawn_binding_column(columns, "KEYBOARD & MOUSE", KEYBOARD_BINDINGS);
                        spawn_binding_column(columns, "GAMEPAD (PS4)", GAMEPAD_BINDINGS);
                    });

                    card.spawn((
                        Text::new("Press F1, Circle, or Start to close"),
                        TextFont {
                            font_size: 12.0,
                            ..default()
                        },
                        TextColor(ACTION_COLOR),
                    ));
                });
        });
}

fn spawn_binding_column(parent: &mut ChildSpawnerCommands, header: &str, bindings: &[Binding]) {
    parent
        .spawn(Node {
            flex_direction: FlexDirection::Column,
            row_gap: Val::Px(6.0),
            min_width: Val::Px(260.0),
            ..default()
        })
        .with_children(|column| {
            column.spawn((
                Text::new(header),
                TextFont {
                    font_size: 13.0,
                    ..default()
                },
                TextColor(HEADER),
                Node {
                    margin: UiRect::bottom(Val::Px(4.0)),
                    ..default()
                },
            ));

            for (input, action) in bindings {
                column
                    .spawn(Node {
                        flex_direction: FlexDirection::Row,
                        justify_content: JustifyContent::SpaceBetween,
                        column_gap: Val::Px(16.0),
                        ..default()
                    })
                    .with_children(|row| {
                        row.spawn((
                            Text::new(*input),
                            TextFont {
                                font_size: 13.0,
                                ..default()
                            },
                            TextColor(KEY_COLOR),
                        ));
                        row.spawn((
                            Text::new(*action),
                            TextFont {
                                font_size: 13.0,
                                ..default()
                            },
                            TextColor(ACTION_COLOR),
                        ));
                    });
            }
        });
}

// ── Toggle ──────────────────────────────────────────────────────────────────

/// Toggle the overlay on F1 (keyboard) or Circle / Start (gamepad), and keep the
/// overlay's visibility in sync with the state.
pub(crate) fn toggle_bindings_menu(
    keyboard: Res<ButtonInput<KeyCode>>,
    gamepads: Query<&Gamepad>,
    mut menu: ResMut<BindingsMenu>,
    mut overlay: Query<&mut Visibility, With<BindingsOverlay>>,
) {
    let mut toggle = keyboard.just_pressed(KeyCode::F1);
    for gamepad in &gamepads {
        if gamepad.just_pressed(GamepadButton::East) || gamepad.just_pressed(GamepadButton::Start) {
            toggle = true;
        }
    }

    if toggle {
        menu.open = !menu.open;
        let visibility = if menu.open {
            Visibility::Visible
        } else {
            Visibility::Hidden
        };
        for mut v in &mut overlay {
            *v = visibility;
        }
    }
}
