//! Live tuning panels (egui), hidden by default — `V` toggles them.
//!
//! "View tuning": the camera values we keep second-guessing (FOV, eye
//! height, depth-of-field), found by feel and then baked into the defaults
//! here. "Time & weather": the day clock (0–23:59), moon phase, clouds,
//! fog, and precipitation — the same values biome presets will drive
//! dynamically later.

use bevy::prelude::*;
use bevy::render::view::ColorGrading;
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

/// Live-tunable underwater look, using a physically-based **atmospheric**
/// (per-channel Beer–Lambert) model — the way real underwater rendering works:
/// each colour channel is absorbed at its own rate over distance (red dies
/// fastest), and the water's own colour scatters in as things get further away.
/// That per-channel absorption is what a single-colour fog could never do, and
/// why the old version never looked right no matter the slider.
#[derive(Resource)]
pub struct UnderwaterTint {
    /// Whole-screen water tint the ENTIRE view is multiplied toward while
    /// submerged (KUDA's `color * waterColor`) — this is what stops near objects
    /// looking "too clear". Strength is how far toward it the view is pushed.
    pub screen_color: [f32; 3],
    pub screen_strength: f32,
    /// How far you can see underwater (world metres, ~5% contrast). Lower =
    /// murkier — the depth gradient on top of the screen tint.
    pub visibility: f32,
    /// Colour that *survives* absorption per channel — high channels reach far,
    /// low channels are absorbed fast. Red low → warm tones vanish first.
    pub extinction_color: [f32; 3],
    /// The water's own colour that scatters into view, filling in with distance
    /// (what the scene fades toward far away).
    pub inscattering_color: [f32; 3],
    /// Ambient brightness while submerged. Real fix for "black silhouettes":
    /// backlit underwater geometry only gets ambient, which is low by day, so it
    /// crushes to near-black against the bright surface. Lifting + water-tinting
    /// the ambient underwater makes solids read as submerged, not cut-outs.
    pub ambient_brightness: f32,
}

impl Default for UnderwaterTint {
    fn default() -> Self {
        // Physically honest clear water: near geometry stays ~natural (only a
        // faint cool ambient cast), and the blue-green builds with DISTANCE via
        // per-channel absorption + short visibility (you just can't see far
        // underwater — that's what fixes "too clear", not a flat filter). Push
        // `screen_strength` up for murky/silty water where particles tint even
        // close up. Tune by diving with the panel open.
        // Hand-tuned by the user against a Minecraft-shader reference (egui-linear
        // form of picker readouts: screen 85,110,125 · extinction 81,125,165 ·
        // in-scatter 81,120,140).
        Self {
            screen_color: [0.0908, 0.1559, 0.2051],
            screen_strength: 0.15,
            visibility: 8.5,
            extinction_color: [0.0823, 0.2051, 0.3763],
            inscattering_color: [0.0823, 0.1878, 0.2623],
            ambient_brightness: 4000.0,
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

/// FPS / frame-time / world-build stats in the top-right corner, plus
/// live GPU levers (MSAA, reflections, depth of field, bloom) to
/// attribute frame cost by toggling features while watching the ms. Runs
/// after `view_tweak_panel`, so it ORs its pointer capture into
/// `pointer_over_panel` (dragging the overlay must not spin the camera).
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn perf_overlay(
    mut commands: Commands,
    mut contexts: EguiContexts,
    perf: Res<PerfOverlay>,
    diagnostics: Res<bevy::diagnostic::DiagnosticsStore>,
    world_stats: Option<Res<crate::WorldStats>>,
    time: Res<Time>,
    mut readout: Local<PerfReadout>,
    mut tweaks: ResMut<ViewTweaks>,
    mut main_camera: Query<
        (
            Entity,
            &mut Msaa,
            Option<&mut bevy::post_process::dof::DepthOfField>,
            Option<&bevy::post_process::bloom::Bloom>,
            Option<&bevy::pbr::ScreenSpaceAmbientOcclusion>,
        ),
        With<bevy::core_pipeline::prepass::DepthPrepass>,
    >,
    mut reflection: ResMut<crate::water::ReflectionSettings>,
    mut grass: Query<&mut Visibility, With<crate::grass::GrassClump>>,
    mut grass_hidden: Local<bool>,
    mut sun: Query<&mut DirectionalLight, With<crate::day_night::SunLight>>,
    mut quality: ResMut<crate::RenderQuality>,
    mut shadow_map: ResMut<bevy::light::DirectionalLightShadowMap>,
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

            // GPU levers: flip a feature, watch the frame time move — the
            // difference is that feature's cost on THIS machine.
            if let Ok((camera_entity, mut msaa, mut depth_of_field, bloom, ssao)) =
                main_camera.single_mut()
            {
                ui.separator();
                ui.label("GPU levers (live)");

                // Quality levers: dial the fill-bound raymarches down and watch
                // the ms drop — dither hides the coarser sampling.
                ui.add(
                    egui::Slider::new(&mut quality.fog_steps, 2..=24).text("fog steps"),
                );
                ui.add(
                    egui::Slider::new(&mut quality.cloud_steps, 2..=16).text("cloud steps"),
                );
                ui.add(
                    egui::Slider::new(&mut quality.reflection_interval, 1..=6)
                        .text("reflect: every N frames"),
                );

                ui.horizontal(|ui| {
                    ui.label("MSAA");
                    for (label, samples) in [
                        ("off", Msaa::Off),
                        ("2×", Msaa::Sample2),
                        ("4×", Msaa::Sample4),
                    ] {
                        if ui.selectable_label(*msaa == samples, label).clicked() {
                            *msaa = samples;
                        }
                    }
                });

                // SSAO vs MSAA are mutually exclusive in Bevy (SSAO needs
                // Msaa::Off + a normal prepass). This toggle swaps the whole
                // mode so the two can be A/B'd: SSAO on → MSAA off; off → 2×.
                let mut ssao_enabled = ssao.is_some();
                if ui
                    .checkbox(&mut ssao_enabled, "SSAO (turns MSAA off)")
                    .changed()
                {
                    if ssao_enabled {
                        // #[require] auto-adds the DepthPrepass (already here)
                        // and NormalPrepass GTAO needs.
                        commands
                            .entity(camera_entity)
                            .insert(bevy::pbr::ScreenSpaceAmbientOcclusion::default());
                        *msaa = Msaa::Off;
                    } else {
                        commands
                            .entity(camera_entity)
                            .remove::<bevy::pbr::ScreenSpaceAmbientOcclusion>()
                            .remove::<bevy::core_pipeline::prepass::NormalPrepass>();
                        *msaa = Msaa::Sample2;
                    }
                }

                ui.checkbox(&mut reflection.enabled, "water reflections");
                if reflection.enabled {
                    if reflection.current_strength <= 0.0 {
                        ui.small(if reflection.water_on_screen {
                            "mirror parked (water too far)"
                        } else {
                            "mirror parked (no water on screen)"
                        });
                    } else {
                        ui.small(format!(
                            "mirror: {:.0}% res · strength {:.2} · water {:.0} m away",
                            reflection.current_tier * 100.0,
                            reflection.current_strength,
                            reflection.current_distance,
                        ));
                    }
                }

                let mut dof_enabled = depth_of_field.is_some();
                if ui.checkbox(&mut dof_enabled, "depth of field").changed() {
                    if dof_enabled {
                        commands.entity(camera_entity).insert(
                            bevy::post_process::dof::DepthOfField {
                                mode: bevy::post_process::dof::DepthOfFieldMode::Bokeh,
                                max_depth: 300.0,
                                ..default()
                            },
                        );
                    } else {
                        commands
                            .entity(camera_entity)
                            .remove::<bevy::post_process::dof::DepthOfField>();
                    }
                }
                if let Some(depth_of_field) = depth_of_field.as_mut() {
                    ui.horizontal(|ui| {
                        ui.label("   mode");
                        for (label, mode) in [
                            (
                                "bokeh (pretty)",
                                bevy::post_process::dof::DepthOfFieldMode::Bokeh,
                            ),
                            (
                                "gaussian (fast)",
                                bevy::post_process::dof::DepthOfFieldMode::Gaussian,
                            ),
                        ] {
                            if ui
                                .selectable_label(depth_of_field.mode == mode, label)
                                .clicked()
                            {
                                depth_of_field.mode = mode;
                            }
                        }
                    });
                }

                let mut bloom_enabled = bloom.is_some();
                if ui.checkbox(&mut bloom_enabled, "bloom").changed() {
                    if bloom_enabled {
                        commands.entity(camera_entity).insert(crate::scene_bloom());
                    } else {
                        commands
                            .entity(camera_entity)
                            .remove::<bevy::post_process::bloom::Bloom>();
                    }
                }

                // Grass on/off — flip it to read grass's exact frametime cost,
                // or to isolate whether an artifact is grass or the terrain.
                let mut grass_visible = !*grass_hidden;
                if ui.checkbox(&mut grass_visible, "grass").changed() {
                    *grass_hidden = !grass_visible;
                    let visibility = if grass_visible {
                        Visibility::Inherited
                    } else {
                        Visibility::Hidden
                    };
                    for mut grass_visibility in &mut grass {
                        *grass_visibility = visibility;
                    }
                }

                // Sun shadows on/off — flip it to test whether the diagonal
                // shimmer is shadow-map acne.
                if let Ok(mut sun_light) = sun.single_mut() {
                    ui.checkbox(&mut sun_light.shadows_enabled, "sun shadows");
                }
                // Shadow-map resolution: the cascade textures cost fill for every
                // caster in every view — dropping the size is a cheap win.
                ui.horizontal(|ui| {
                    ui.label("shadow res");
                    for size in [1024usize, 2048, 4096] {
                        if ui
                            .selectable_label(shadow_map.size == size, format!("{size}"))
                            .clicked()
                        {
                            shadow_map.size = size;
                        }
                    }
                });
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
    mut underwater: ResMut<UnderwaterTint>,
    mut surface: ResMut<crate::water::SurfaceTuning>,
    mut season: ResMut<crate::Season>,
    mut regenerate: ResMut<crate::RegenerateRequest>,
    view_mode: Res<ViewMode>,
    mut main_camera: Query<
        (
            Option<&mut bevy::post_process::bloom::Bloom>,
            &mut ColorGrading,
        ),
        With<bevy::core_pipeline::prepass::DepthPrepass>,
    >,
) {
    if !tweaks.panel_visible {
        tweaks.pointer_over_panel = false;
        return;
    }
    let Ok(ctx) = contexts.ctx_mut() else {
        return;
    };
    let camera_look = main_camera.single_mut().ok();

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
            ui.label("Look / grade (live)");
            if let Some((mut bloom, mut grading)) = camera_look {
                if let Some(bloom) = bloom.as_mut() {
                    ui.add(
                        egui::Slider::new(&mut bloom.intensity, 0.0..=0.6).text("bloom intensity"),
                    );
                }
                ui.add(
                    egui::Slider::new(&mut grading.global.exposure, -1.0..=1.0).text("exposure"),
                );
                ui.add(
                    egui::Slider::new(&mut grading.shadows.lift, -0.2..=0.2).text("shadow lift"),
                );
                ui.add(
                    egui::Slider::new(&mut grading.midtones.contrast, 0.5..=1.5)
                        .text("mid contrast"),
                );
                ui.add(
                    egui::Slider::new(&mut grading.global.post_saturation, 0.0..=2.0)
                        .text("saturation"),
                );
                if ui.button("reset grade").clicked() {
                    *grading = ColorGrading::default();
                }
            }
            ui.separator();
            ui.label("Underwater (live)");
            ui.horizontal(|ui| {
                ui.label("screen tint");
                ui.color_edit_button_rgb(&mut underwater.screen_color);
            });
            ui.add(
                egui::Slider::new(&mut underwater.screen_strength, 0.0..=0.95)
                    .text("tint strength"),
            );
            ui.add(
                egui::Slider::new(&mut underwater.visibility, 2.0..=40.0)
                    .logarithmic(true)
                    .text("visibility m"),
            );
            ui.horizontal(|ui| {
                ui.label("survives (extinction)");
                ui.color_edit_button_rgb(&mut underwater.extinction_color);
            });
            ui.horizontal(|ui| {
                ui.label("water color (in-scatter)");
                ui.color_edit_button_rgb(&mut underwater.inscattering_color);
            });
            ui.add(
                egui::Slider::new(&mut underwater.ambient_brightness, 0.0..=8000.0)
                    .text("submerged ambient (lift)"),
            );
            if ui.button("reset underwater").clicked() {
                *underwater = UnderwaterTint::default();
            }
            ui.small("dive (Space, in water) to preview · tint = whole-screen, fog = distance");
            ui.separator();
            ui.label("Water surface (live · from above)");
            ui.horizontal(|ui| {
                ui.label("water tint");
                ui.color_edit_button_rgb(&mut surface.tint);
            });
            ui.add(
                egui::Slider::new(&mut surface.reflectivity, 0.0..=2.0).text("reflectivity"),
            );
            ui.add(egui::Slider::new(&mut surface.depth, 0.2..=4.0).text("depth darkening"));
            ui.add(
                egui::Slider::new(&mut surface.underside_opacity, 0.0..=1.0)
                    .text("underside opacity (from below)"),
            );
            if ui.button("reset surface").clicked() {
                *surface = crate::water::SurfaceTuning::default();
            }
            ui.small("tint = water's own colour · reflectivity = sky/mirror amount");
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
            ui.add(
                egui::Slider::new(&mut cycle.sun_intensity, 0.0..=2.0)
                    .text("sun intensity (1.0 = default)"),
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
