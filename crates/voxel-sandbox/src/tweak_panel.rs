//! Live tuning panels (egui), hidden by default — `V` toggles them.
//!
//! "View tuning": the camera values we keep second-guessing (FOV, eye
//! height, depth-of-field), found by feel and then baked into the defaults
//! here. "Time & weather": the day clock (0–23:59), moon phase, clouds,
//! fog, and precipitation — the same values biome presets will drive
//! dynamically later.

use bevy::prelude::*;
use bevy_egui::{egui, EguiContexts};

use crate::day_night::DayNightCycle;
use crate::fireflies::FireflySettings;
use crate::weather::{Precipitation, WeatherState};
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
        // touch lower, close focus point. Both apertures re-tuned to f/2.2
        // after the sky landed — a crisper look that keeps clouds, stars,
        // and rain readable instead of melting them into bokeh.
        Self {
            first_person_fov_degrees: 65.0,
            eye_height: 1.60,
            walk_focal_distance: 1.75,
            walk_aperture_f_stops: 2.2,
            orbit_fov_degrees: 22.0,
            orbit_aperture_f_stops: 2.2,
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
    mut cycle: ResMut<DayNightCycle>,
    mut weather: ResMut<WeatherState>,
    mut fireflies: ResMut<FireflySettings>,
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
                        egui::Slider::new(&mut tweaks.orbit_aperture_f_stops, 0.01..=8.0)
                            .logarithmic(true)
                            .text("focus width (f-stops)"),
                    );
                }
            }
            ui.separator();
            ui.small("Tab switches view · smaller f-stops = blurrier");
        });

    egui::Window::new("Time & weather")
        .default_pos((12.0, 320.0))
        .default_width(280.0)
        .show(ctx, |ui| {
            ui.label("Time");
            // 10-minute steps: 00:00 .. 23:50.
            const TEN_MINUTES_HOURS: f64 = 1.0 / 6.0;
            let mut hours = cycle.time_fraction * 24.0;
            if ui
                .add(
                    egui::Slider::new(&mut hours, 0.0..=(24.0 - TEN_MINUTES_HOURS as f32))
                        .step_by(TEN_MINUTES_HOURS)
                        .custom_formatter(|value, _| {
                            format!(
                                "{:02}:{:02}",
                                value as u32,
                                ((value.fract() * 60.0 / 10.0).round() * 10.0) as u32 % 60
                            )
                        })
                        .text("time of day"),
                )
                .changed()
            {
                cycle.time_fraction = hours / 24.0;
            }
            ui.checkbox(&mut cycle.run_clock, "run clock (hold N to fast-forward)");
            ui.add(
                egui::Slider::new(&mut cycle.moon_phase, 0.0..=1.0).text("moon phase (0.5 = full)"),
            );

            ui.separator();
            ui.label("Clouds");
            ui.add(egui::Slider::new(&mut weather.target.cloud_coverage, 0.0..=1.0).text("cover"));
            ui.add(
                egui::Slider::new(&mut weather.target.cloud_type, 0.0..=2.0)
                    .custom_formatter(|value, _| {
                        let name = if value < 0.5 {
                            "stratus"
                        } else if value < 1.5 {
                            "cumulus"
                        } else {
                            "cirrus"
                        };
                        format!("{value:.2} {name}")
                    })
                    .text("type"),
            );
            ui.add(egui::Slider::new(&mut weather.target.wind_speed, 0.0..=40.0).text("wind m/s"));
            ui.add(
                egui::Slider::new(&mut weather.target.wind_direction_degrees, 0.0..=360.0)
                    .text("wind direction °"),
            );

            ui.separator();
            ui.label("Air");
            ui.add(egui::Slider::new(&mut weather.target.fog, 0.0..=1.0).text("fog"));

            ui.separator();
            ui.label("Precipitation");
            egui::ComboBox::from_label("type ")
                .selected_text(match weather.precipitation {
                    Precipitation::None => "none",
                    Precipitation::Rain => "rain",
                    Precipitation::Snow => "snow",
                })
                .show_ui(ui, |ui| {
                    ui.selectable_value(&mut weather.precipitation, Precipitation::None, "none");
                    ui.selectable_value(&mut weather.precipitation, Precipitation::Rain, "rain");
                    ui.selectable_value(&mut weather.precipitation, Precipitation::Snow, "snow");
                });
            ui.add(
                egui::Slider::new(&mut weather.target.precipitation_intensity, 0.0..=1.0)
                    .text("intensity"),
            );
            ui.small("weather eases in over a few seconds");
        });

    egui::Window::new("Fireflies")
        .default_pos((12.0, 640.0))
        .default_width(280.0)
        .show(ctx, |ui| {
            ui.add(egui::Slider::new(&mut fireflies.amount, 0..=150).text("amount per swarm"));
            ui.add(egui::Slider::new(&mut fireflies.width, 0.5..=8.0).text("width m"));
            ui.add(egui::Slider::new(&mut fireflies.height, 0.2..=8.0).text("height m"));
            ui.add(
                egui::Slider::new(&mut fireflies.size, 0.01..=0.15)
                    .logarithmic(true)
                    .text("size m"),
            );
            ui.add(
                egui::Slider::new(&mut fireflies.glow, 1.0..=60.0)
                    .logarithmic(true)
                    .text("emit power"),
            );
            ui.add(egui::Slider::new(&mut fireflies.blink_speed, 0.1..=3.0).text("blink speed"));
            ui.add(
                egui::Slider::new(&mut fireflies.light_intensity, 0.0..=8_000.0)
                    .text("ground light lm"),
            );
            ui.horizontal(|ui| {
                ui.label("color");
                ui.color_edit_button_rgb(&mut fireflies.color);
            });
            ui.small("F spawns a swarm · Shift+F clears · night only");
        });

    tweaks.pointer_over_panel = ctx.wants_pointer_input();
}
