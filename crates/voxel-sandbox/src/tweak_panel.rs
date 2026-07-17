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

/// Live performance readout, toggled with `P` (or `VOXEL_PERF=1`).
#[derive(Resource, Default)]
pub struct PerfOverlay {
    pub visible: bool,
}

pub fn toggle_panel(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut tweaks: ResMut<ViewTweaks>,
    mut perf: ResMut<PerfOverlay>,
) {
    if keyboard.just_pressed(KeyCode::KeyV) {
        tweaks.panel_visible = !tweaks.panel_visible;
    }
    if keyboard.just_pressed(KeyCode::KeyP) {
        perf.visible = !perf.visible;
    }
}

/// The numbers currently shown in the overlay — refreshed a few times per
/// second, not every frame, so they hold still long enough to read.
#[derive(Default)]
pub struct PerfReadout {
    last_refresh_seconds: f32,
    fps: Option<f64>,
    frame_ms: Option<f64>,
    entities: Option<f64>,
}

/// FPS / frame-time / world-build stats in the top-right corner. Runs
/// after `view_tweak_panel`, so it ORs its pointer capture into
/// `pointer_over_panel` (dragging the overlay must not spin the camera).
pub fn perf_overlay(
    mut contexts: EguiContexts,
    perf: Res<PerfOverlay>,
    diagnostics: Res<bevy::diagnostic::DiagnosticsStore>,
    world_stats: Option<Res<crate::WorldStats>>,
    time: Res<Time>,
    mut readout: Local<PerfReadout>,
    mut tweaks: ResMut<ViewTweaks>,
) {
    if !perf.visible {
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };

    let now = time.elapsed_secs();
    if readout.fps.is_none() || now - readout.last_refresh_seconds >= 0.33 {
        readout.last_refresh_seconds = now;
        readout.fps = diagnostics
            .get(&bevy::diagnostic::FrameTimeDiagnosticsPlugin::FPS)
            .and_then(|diagnostic| diagnostic.smoothed());
        readout.frame_ms = diagnostics
            .get(&bevy::diagnostic::FrameTimeDiagnosticsPlugin::FRAME_TIME)
            .and_then(|diagnostic| diagnostic.smoothed());
        readout.entities = diagnostics
            .get(&bevy::diagnostic::EntityCountDiagnosticsPlugin::ENTITY_COUNT)
            .and_then(|diagnostic| diagnostic.value());
    }

    egui::Window::new("Performance")
        .anchor(egui::Align2::RIGHT_TOP, (-12.0, 12.0))
        .default_width(220.0)
        .resizable(false)
        .show(ctx, |ui| {
            match (readout.fps, readout.frame_ms) {
                (Some(fps), Some(frame_ms)) => {
                    ui.strong(format!("{fps:.0} fps   {frame_ms:.1} ms"));
                }
                _ => {
                    ui.strong("warming up…");
                }
            }
            if let Some(entities) = readout.entities {
                ui.label(format!("entities: {entities:.0}"));
            }

            if let Some(stats) = world_stats {
                ui.separator();
                ui.label(format!(
                    "world: {} chunks · {:.1} M verts",
                    stats.chunk_count,
                    stats.total_vertices as f32 / 1e6
                ));
                ui.label(format!(
                    "RLE: {:.1} M runs · {:.1} MB",
                    stats.rle_runs as f32 / 1e6,
                    stats.rle_bytes as f32 / 1e6
                ));
                ui.label(format!(
                    "gen {:.0} ms · mesh {:.0} ms",
                    stats.generation.as_secs_f32() * 1000.0,
                    stats.meshing.as_secs_f32() * 1000.0
                ));
            }
            ui.small("P hides this overlay");
        });

    tweaks.pointer_over_panel |= ctx.wants_pointer_input();
}

#[allow(clippy::too_many_arguments)]
pub fn view_tweak_panel(
    mut contexts: EguiContexts,
    mut tweaks: ResMut<ViewTweaks>,
    mut cycle: ResMut<DayNightCycle>,
    mut weather: ResMut<WeatherState>,
    mut fireflies: ResMut<FireflySettings>,
    mut season: ResMut<crate::Season>,
    mut regenerate: ResMut<crate::RegenerateRequest>,
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

            ui.separator();
            ui.label("Season");
            let season_response = ui.add(
                egui::Slider::new(&mut season.0, 0.0..=1.0)
                    .custom_formatter(|value, _| {
                        let name = if value < 0.20 {
                            "summer"
                        } else if value < 0.55 {
                            "late summer"
                        } else if value < 0.85 {
                            "autumn"
                        } else {
                            "deep autumn"
                        };
                        format!("{value:.2} {name}")
                    })
                    .text("foliage"),
            );
            if season_response.drag_stopped()
                || (season_response.changed() && !season_response.dragged())
            {
                regenerate.requested = true;
            }
            ui.small("release to regrow the island (takes a few seconds)");
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
