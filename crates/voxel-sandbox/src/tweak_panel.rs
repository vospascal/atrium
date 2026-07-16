//! Live view-tuning panel (egui).
//!
//! Sliders for the values we keep second-guessing — first-person FOV and
//! eye height, depth-of-field focus distance and width for both views —
//! so the sweet spot can be found by feel and then baked into the
//! defaults here afterwards.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::ViewMode;

#[derive(Resource)]
pub struct ViewTweaks {
    pub first_person_fov_degrees: f32,
    pub eye_height: f32,
    pub walk_focal_distance: f32,
    pub walk_aperture_f_stops: f32,
    pub orbit_fov_degrees: f32,
    pub orbit_aperture_f_stops: f32,
    /// Panel hidden by default; `V` toggles it.
    pub panel_visible: bool,
    /// True while the cursor is over the panel — camera ignores the mouse.
    pub pointer_over_panel: bool,
}

impl Default for ViewTweaks {
    fn default() -> Self {
        // Baked from hand-tuning (2026-07-16): wide-ish natural lens, eye a
        // touch lower, dreamy close focus with a gentle falloff.
        Self {
            first_person_fov_degrees: 65.0,
            eye_height: 1.60,
            walk_focal_distance: 1.75,
            walk_aperture_f_stops: 1.25,
            orbit_fov_degrees: 22.0,
            orbit_aperture_f_stops: 0.06,
            panel_visible: false,
            pointer_over_panel: false,
        }
    }
}

pub fn toggle_panel(keyboard: Res<ButtonInput<KeyCode>>, mut tweaks: ResMut<ViewTweaks>) {
    if keyboard.just_pressed(KeyCode::KeyV) {
        tweaks.panel_visible = !tweaks.panel_visible;
    }
}

pub fn view_tweak_panel(
    mut contexts: EguiContexts,
    mut tweaks: ResMut<ViewTweaks>,
    view_mode: Res<ViewMode>,
) {
    if !tweaks.panel_visible {
        tweaks.pointer_over_panel = false;
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    egui::Window::new("View tuning")
        .default_pos((12.0, 12.0))
        .default_width(280.0)
        .show(ctx, |ui| {
            match *view_mode {
                ViewMode::FirstPerson => {
                    ui.label("First person (active)");
                    ui.add(
                        egui::Slider::new(&mut tweaks.first_person_fov_degrees, 30.0..=100.0)
                            .text("FOV °"),
                    );
                    ui.add(
                        egui::Slider::new(&mut tweaks.eye_height, 0.6..=3.5).text("eye height m"),
                    );
                    ui.separator();
                    ui.label("Focus (depth of field)");
                    ui.add(
                        egui::Slider::new(&mut tweaks.walk_focal_distance, 1.0..=80.0)
                            .logarithmic(true)
                            .text("focus distance m"),
                    );
                    ui.add(
                        egui::Slider::new(&mut tweaks.walk_aperture_f_stops, 0.02..=8.0)
                            .logarithmic(true)
                            .text("focus width (f-stops)"),
                    );
                }
                ViewMode::Orbit => {
                    ui.label("Orbit (active)");
                    ui.add(
                        egui::Slider::new(&mut tweaks.orbit_fov_degrees, 8.0..=60.0).text("FOV °"),
                    );
                    ui.separator();
                    ui.label("Focus (depth of field)");
                    ui.add(
                        egui::Slider::new(&mut tweaks.orbit_aperture_f_stops, 0.01..=2.0)
                            .logarithmic(true)
                            .text("focus width (f-stops)"),
                    );
                }
            }
            ui.separator();
            ui.small("Tab switches view · smaller f-stops = blurrier");
        });

    tweaks.pointer_over_panel = ctx.wants_pointer_input();
}
