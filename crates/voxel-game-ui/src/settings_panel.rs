//! The settings and debug window — press **O**.
//!
//! # Why this exists
//!
//! Everything in here used to live in one always-on `egui::Area` pinned to the
//! top-left corner: resolutions, edit counters, movement state, vsync, output
//! depth, every quality lever and the whole sun section. It covered a large part
//! of the viewport permanently, which is exactly what you cannot afford when the
//! thing being judged is a *look*. A renderer whose debug UI hides the render is
//! measuring the wrong thing.
//!
//! So the always-on surface is now one compact line, and this window holds the
//! rest. It is the second of the toggled windows, after [`crate::performance_panel`]
//! on **P**, and the pattern is deliberate: new readouts and controls go in a
//! window, never back into the permanent corner.
//!
//! # Sections
//!
//! Diagnostics first, then controls — read before you change:
//!
//! 1. **Display** — window, physical and render resolutions (the Retina trap).
//! 2. **World (E2)** — edit counters and world-thread state.
//! 3. **Movement (E2b)** — which model drives the view and what the body is doing.
//! 4. **Presentation** — vsync and the whole output-depth / tonemap path.
//! 5. **Quality** — the lever registry and its presets.
//! 6. **Sun** — the day/night cycle and light scaling.
//! 7. **Weather** — the cloud deck: named conditions, then its dials.
//! 8. **Studio** — asset and project panel.
//!
//! The leaf drawing functions for 4, 5, 7 and the movement readout live here, with the two
//! readout structs they render ([`MovementReadout`], [`WorldEditReadout`]).
//!
//! They used to sit in `voxel`'s overlay, which made the two modules mutually dependent —
//! `overlay` needed `SettingsContext` to open the window, and this module needed four `draw_*`
//! helpers and both readout types back. `scripts/dep-cycles.py` reported it as a cycle. Nothing
//! in `overlay` actually called those helpers: it only defined them. So they were simply in the
//! wrong file, and no third "shared widgets" module is warranted — the dependency runs one way
//! now, `overlay` -> `settings_panel`.

use voxel_color::{
    ColorSpaceOutcome, DisplayHeadroom, HeadroomChoice, OutputDepth, OutputSupport, TonemapCurve,
};
use voxel_core::weather::WeatherKind;
use voxel_environment::SunSettings;

use voxel_rt::ao::AoMode;
use voxel_rt::character::Submersion;
use voxel_rt::shadows::ShadowMode;
use voxel_rt::sky_weather::SkyWeather;
use voxel_rt::studio_assets::StudioAssetPanelState;
use voxel_rt::variants::{
    levers_of, Lever, LeverId, LeverRange, LeverSubsystem, LeverValue, QualityPreset,
    RenderQuality, QUALITY_PRESETS, VOXELS_PER_METER,
};
use voxel_rt::water::WaterMode;
use voxel_rt::world_edit::ClearanceUpdateMode;
use voxel_rt::world_host::WorldEditStats;

/// Everything the window reads or mutates, bundled.
///
/// A struct rather than fifteen parameters: the call already threads this many
/// values through `voxel`'s `Overlay::render`, and repeating that list at
/// every hop is how a signature becomes unreadable. Same reasoning as bundling the
/// foliage parameters — a group that always travels together should travel as one
/// thing.
pub struct SettingsContext<'frame> {
    /// Logical window size in points, and the scale factor behind it.
    pub logical_size: (f64, f64),
    /// Physical swapchain size in pixels. On macOS this can be 4x the logical
    /// area — the Retina trap worth keeping visible.
    pub physical_size: (u32, u32),
    /// Ray-traced storage-texture size, which the render scale also changes.
    pub render_resolution: (u32, u32),
    pub scale_factor: f64,
    pub world_edit: &'frame WorldEditReadout,
    pub movement: &'frame MovementReadout,
    pub vsync_enabled: &'frame mut bool,
    pub output_depth: &'frame mut OutputDepth,
    pub output_support: OutputSupport,
    /// Diagnostics only — the panel reports these, never acts on them.
    pub output_color_space: ColorSpaceOutcome,
    pub output_headroom: DisplayHeadroom,
    pub headroom_backend: &'static str,
    pub headroom_choice: &'frame mut HeadroomChoice,
    pub tonemap_curve: &'frame mut TonemapCurve,
    pub content_peak: &'frame mut f32,
    pub exposure: &'frame mut f32,
    pub sun_settings: &'frame mut SunSettings,
    /// The cloud deck and the weather driving it. Next to the sun because they are one sky:
    /// coverage changes what the sun delivers, and judging either alone is misleading.
    pub sky_weather: &'frame mut SkyWeather,
    pub quality: &'frame mut RenderQuality,
    pub studio_assets: &'frame mut StudioAssetPanelState,
}

pub struct SettingsPanel {
    /// Toggled by **O**. Starts hidden: the viewport is the point.
    pub visible: bool,
}

impl Default for SettingsPanel {
    fn default() -> Self {
        Self::new()
    }
}

impl SettingsPanel {
    pub fn new() -> Self {
        Self { visible: false }
    }

    pub fn toggle(&mut self) {
        self.visible = !self.visible;
    }

    /// Draw the window when visible. Scrollable and resizable, because the
    /// quality section alone is taller than a laptop screen.
    pub fn draw(&mut self, context: &egui::Context, settings: SettingsContext<'_>) {
        if !self.visible {
            return;
        }
        // `Window::open` borrows the flag mutably, which would conflict with the
        // body — so the flag round-trips through a local.
        let mut visible = self.visible;
        egui::Window::new("Settings & Debug  (O)")
            .open(&mut visible)
            .default_width(420.0)
            .default_height(560.0)
            .resizable(true)
            .vscroll(true)
            .show(context, |ui| draw_body(ui, settings));
        self.visible = visible;
    }
}

fn draw_body(ui: &mut egui::Ui, settings: SettingsContext<'_>) {
    let SettingsContext {
        logical_size,
        physical_size,
        render_resolution,
        scale_factor,
        world_edit,
        movement,
        vsync_enabled,
        output_depth,
        output_support,
        output_color_space,
        output_headroom,
        headroom_backend,
        headroom_choice,
        tonemap_curve,
        content_peak,
        exposure,
        sun_settings,
        sky_weather,
        quality,
        studio_assets,
    } = settings;

    // Collapsed by default where the content is long, so opening the window does
    // not immediately reproduce the wall of text it was built to replace.
    ui.collapsing("Display", |ui| {
        ui.label(format!(
            "window {:.0} x {:.0} @ {scale_factor:.2}x",
            logical_size.0, logical_size.1
        ));
        ui.label(format!(
            "physical {} x {}",
            physical_size.0, physical_size.1
        ))
        .on_hover_text(
            "The swapchain is PHYSICAL pixels. On macOS at scale factor 2.0 \
                 that is four times the logical window area — the Retina trap.",
        );
        ui.label(format!(
            "render {} x {}",
            render_resolution.0, render_resolution.1
        ))
        .on_hover_text(
            "The ray-traced storage texture, which the render-scale lever changes \
             independently of the window.",
        );
    });

    ui.collapsing("World (E2)", |ui| {
        ui.label(format!(
            "edits {} ({} voxels, {} ignored) | apply {:.0} us | delta {} B",
            world_edit.stats.edits_applied,
            world_edit.stats.voxels_written,
            world_edit.stats.edits_ignored,
            world_edit.stats.last_apply_micros,
            world_edit.stats.last_upload_bytes,
        ))
        .on_hover_text(
            "E2: voxel edits applied by the world authority — one per click, and \
             one delta per one-metre world edit. \
             `apply` is the CPU cost of patching the brickmap and every derived \
             structure; `delta` is what the last edit uploaded to the GPU. On the \
             world thread neither is paid inside a frame.",
        );
        ui.label(format!(
            "world thread: {} | {} in flight | {:.1} KB uploaded",
            if world_edit.threaded { "on" } else { "off" },
            world_edit.in_flight,
            world_edit.stats.total_upload_bytes as f32 / 1024.0,
        ));
    });

    ui.collapsing("Movement (E2b)", |ui| {
        draw_movement_readout(ui, movement);
    });

    ui.collapsing("Presentation", |ui| {
        ui.checkbox(vsync_enabled, "VSync");
        ui.label(if *vsync_enabled {
            "VSync on: Frame loop normally tracks the display."
        } else {
            "VSync off: Compare GPU FPS; the frame loop can run ahead of the GPU."
        })
        .on_hover_text("Toggling this resets the performance panel's history.");
        draw_output_depth(
            ui,
            output_depth,
            output_support,
            output_color_space,
            output_headroom,
            headroom_backend,
            headroom_choice,
            tonemap_curve,
            content_peak,
            exposure,
        );
    });

    ui.collapsing("Quality", |ui| {
        draw_quality_section(ui, quality);
    });

    ui.collapsing("Sun", |ui| draw_sun_section(ui, sun_settings));

    ui.collapsing("Weather", |ui| draw_weather_section(ui, sky_weather));

    ui.collapsing("Studio", |ui| {
        draw_studio_assets_section(ui, studio_assets);
        ui.label("Material authoring is defined by nodes in Graph Studio.");
    });
}

/// The sun and ambient controls. Moved here verbatim from the always-on area —
/// same knobs, same hover text, just no longer permanently on screen.
fn draw_sun_section(ui: &mut egui::Ui, sun_settings: &mut SunSettings) {
    ui.checkbox(&mut sun_settings.day_night_enabled, "day/night sky");
    if sun_settings.day_night_enabled {
        ui.horizontal(|ui| {
            ui.checkbox(&mut sun_settings.cycle_running, "run clock");
            ui.label(sun_settings.clock_label());
        });
        ui.add(egui::Slider::new(&mut sun_settings.day_phase, 0.0..=1.0).text("time of day"));
        ui.add(
            egui::Slider::new(&mut sun_settings.day_length_seconds, 30.0..=1_200.0)
                .logarithmic(true)
                .text("seconds per day"),
        );
        ui.add(egui::Slider::new(&mut sun_settings.moon_phase, 0.0..=1.0).text("moon phase"));
        ui.add(
            egui::Slider::new(&mut sun_settings.azimuth_degrees, 0.0..=360.0).text("noon azimuth"),
        );
        ui.add(
            egui::Slider::new(&mut sun_settings.elevation_degrees, 2.0..=90.0)
                .text("noon elevation"),
        );
    } else {
        ui.add(egui::Slider::new(&mut sun_settings.azimuth_degrees, 0.0..=360.0).text("azimuth"));
        ui.add(
            egui::Slider::new(&mut sun_settings.elevation_degrees, 2.0..=90.0).text("elevation"),
        );
    }
    ui.add(
        egui::Slider::new(&mut sun_settings.intensity_scale, 0.0..=2.0)
            .text("sun intensity")
            .max_decimals(2),
    )
    .on_hover_text(
        "Scales the sun, 1.0 being the shipped look. ZERO IS \
         NIGHT: the sun contributes nothing and only ambient, GI \
         and emitters are left.\n\n\
         Added because an emitter cannot be judged against a \
         light you cannot turn down — a glowing material and the \
         light it casts were both washed out by a hardcoded 2.2 \
         of daylight. Turn this and the ambient below to zero to \
         see what a material actually emits.",
    );
    ui.add(
        egui::Slider::new(&mut sun_settings.ambient_scale, 0.0..=2.0)
            .text("ambient")
            .max_decimals(2),
    )
    .on_hover_text(
        "Scales the hemisphere ambient floor. Needed alongside \
         the sun: at sun zero the ambient alone still reads every \
         surface, so an emitter's own contribution stays \
         invisible until this comes down too.",
    );
}

/// The cloud deck: the four named conditions, then the dials.
///
/// Conditions first because that is how you *look* at clouds — pick a sky, watch it arrive,
/// and only then reach for a slider. The dials below are split into two groups on purpose:
/// **weather** values are overwritten every frame by the condition, so editing them is a
/// preview that a transition will undo; **look** and **cost** values are authored and survive
/// any weather change.
fn draw_weather_section(ui: &mut egui::Ui, sky: &mut SkyWeather) {
    ui.horizontal(|ui| {
        ui.label("condition:");
        for (kind, label) in [
            (WeatherKind::Clear, "clear"),
            (WeatherKind::Scattered, "scattered"),
            (WeatherKind::Overcast, "overcast"),
            (WeatherKind::Storm, "storm"),
        ] {
            let selected = sky.state.target == kind;
            if ui.selectable_label(selected, label).clicked() {
                sky.set_target(kind);
            }
        }
    });
    ui.horizontal(|ui| {
        if sky.state.transitioning() {
            ui.label("transitioning…");
        } else {
            ui.label("settled");
        }
        ui.label(format!(
            "wind {:.1} m/s toward {:.0}°",
            sky.deck.wind[0].hypot(sky.deck.wind[1]),
            sky.wind_direction_degrees
        ));
    });
    let mut wind_speed = sky.wind_speed_meters_per_second();
    if ui
        .add(
            egui::Slider::new(&mut wind_speed, 0.0..=30.0)
                .text("wind speed")
                .suffix(" m/s"),
        )
        .on_hover_text(
            "Cloud advection speed in metres per second. The weather condition remains the base model, so storms naturally retain stronger wind than clear skies.",
        )
        .changed()
    {
        sky.set_wind_speed_meters_per_second(wind_speed);
    }
    ui.add(
        egui::Slider::new(&mut sky.wind_direction_degrees, -180.0..=180.0)
            .text("wind direction")
            .suffix("°"),
    )
    .on_hover_text(
        "Direction the clouds move toward: 0° is +X, 90° is +Z. The same direction is used by the weather-driven deck.",
    );
    ui.add(
        egui::Slider::new(&mut sky.state.transition_seconds, 1.0..=600.0)
            .logarithmic(true)
            .text("seconds per change"),
    )
    .on_hover_text(
        "Turn this down to a few seconds to compare conditions quickly. \
         The shipped 120 s is what reads as weather rather than as a switch.",
    );

    ui.separator();
    ui.checkbox(&mut sky.deck.enabled, "clouds")
        .on_hover_text("Off skips the march entirely and every cloud term returns its identity, so the image is the pre-cloud one.");

    ui.checkbox(&mut sky.manual, "hand-dial the deck")
        .on_hover_text(
            "The weather rewrites the five values below EVERY frame — the wind's slow channel \
             breathes coverage continuously — so without this they are dead controls. Picking \
             a condition above turns this back off. The deck still drifts either way.",
        );
    ui.add_enabled_ui(sky.manual, |ui| {
        ui.add(egui::Slider::new(&mut sky.deck.coverage, 0.0..=1.0).text("coverage"))
            .on_hover_text(
                "Erodes the deck from the edges inward rather than fading it, which is how a \
             clearing sky behaves. Also dims the sun and bends the ambient curve.",
            );
        ui.add(egui::Slider::new(&mut sky.deck.cloud_type, 0.0..=1.0).text("cloud type"))
            .on_hover_text(
                "0 stratus (flat slab), 0.5 cumulus (billowing), 1 cumulonimbus (towering).",
            );
        ui.add(egui::Slider::new(&mut sky.deck.bottom_world, 40.0..=800.0).text("deck base"));
        ui.add(
            egui::Slider::new(&mut sky.deck.thickness_world, 20.0..=800.0).text("deck thickness"),
        );
        ui.add(
            egui::Slider::new(&mut sky.deck.extinction, 0.005..=0.4)
                .logarithmic(true)
                .text("extinction sigma_t"),
        );
    });

    ui.separator();
    ui.label("look (authored — survives a weather change):");
    ui.add(egui::Slider::new(&mut sky.deck.weather_variation, 0.0..=1.0).text("weather variation"))
        .on_hover_text(
            "How much coverage varies ACROSS the sky. At zero the whole sky gets one coverage \
             number and the deck reads as a single continuous mass — Nubis calls coverage \
             \"a FUNCTION of our weather system\", i.e. a 2D map, not a scalar. Turn it up for \
             distinct cloud groups with real sky between them.",
        );
    ui.add(egui::Slider::new(&mut sky.deck.density_scale, 0.2..=4.0).text("density scale"))
        .on_hover_text(
            "Scales the shaped density so cores SATURATE. The noise supplies the shape, this \
             supplies the substance. Below ~1 the deck returns to looking like mist, because the \
             density chain's natural peak is only about 0.37.",
        );
    ui.add(egui::Slider::new(&mut sky.deck.detail_strength, 0.0..=1.5).text("erosion"))
        .on_hover_text(
            "High-frequency Worley carving. Up frays the deck; down leaves rounded blobs.",
        );
    ui.add(egui::Slider::new(&mut sky.deck.powder_strength, 0.0..=1.0).text("powder rim"))
        .on_hover_text(
            "Beer-Powder. Beer's law alone makes cloud EDGES dark, because thin cloud transmits \
             nearly everything; this restores the bright rim. Non-physical and aesthetic.",
        );
    ui.add(egui::Slider::new(&mut sky.deck.forward_scatter, 0.0..=0.95).text("forward scatter"))
        .on_hover_text("Henyey-Greenstein forward lobe: the silver lining looking toward the sun.");
    ui.add(egui::Slider::new(&mut sky.deck.back_scatter, -0.95..=0.0).text("back scatter"))
        .on_hover_text(
            "The weaker back lobe. The PAIR is what makes cloud read as cloud rather than fog.",
        );
    ui.add(egui::Slider::new(&mut sky.deck.ambient_density, 0.0..=1.5).text("ambient occlusion"))
        .on_hover_text(
            "Extinction on the three upward sky-occlusion taps. Zero makes shadowed cloud flat; \
             high makes undersides heavy.",
        );
    ui.add(egui::Slider::new(&mut sky.deck.albedo, 0.0..=1.0).text("albedo sigma_s/sigma_t"))
        .on_hover_text(
            "Cloud sits at ~0.999. Drop it toward 0.3 and the deck renders as SMOKE — this is \
             the field that keeps the two from sharing one coefficient.",
        );

    ui.separator();
    ui.label("cost:");
    ui.add(egui::Slider::new(&mut sky.deck.primary_steps, 4..=128).text("primary steps"))
        .on_hover_text(
            "View-ray samples, distributed logarithmically so near cloud gets more. THE dominant \
         cost, and the first lever to reach for.",
        );
    ui.add(egui::Slider::new(&mut sky.deck.light_steps, 1..=16).text("light taps"))
        .on_hover_text(
            "Cone taps toward the sun per in-cloud sample. Multiplies the primary count, so \
             this is the second-order cost — and it early-outs on an exact transmittance threshold.",
        );
}

/// What the overlay shows about movement (E2b). Flat and pre-read so the panel
/// never borrows the controller itself.
pub struct MovementReadout {
    /// S0 — the studio orbit radius in meters when the material studio is driving
    /// the view, `None` for the two world modes. Takes precedence over the fields
    /// below, none of which mean anything in the studio: there is no ground to be
    /// grounded on and nowhere to walk.
    pub studio_orbit_distance_meters: Option<f32>,
    /// False = the fly camera, true = the character body.
    pub walking: bool,
    /// The mouse-wheel-tuned base speed of whichever mode is active, m/s.
    pub speed_meters_per_second: f32,
    /// Walk mode: resting on solid ground.
    pub grounded: bool,
    /// Walk mode: how wet the body is.
    pub submersion: Submersion,
    /// Walk mode: the eye is under a liquid (E6's underwater flag).
    pub head_submerged: bool,
    /// E6 — whether the ACTIVE eye (fly camera or body head) sits in a liquid, so
    /// the shading pass took its underwater path. Read from the world with
    /// [`voxel_rt::water::eye_is_submerged`], which is why it is true in fly mode as
    /// well: E6 owns "the view is underwater", E2b's `head_submerged` owns "the
    /// body's head is wet", and they agree wherever both apply.
    pub eye_submerged: bool,
    /// Walk mode: CPU cost of the last movement + collision step, microseconds.
    pub step_micros: f32,
}

/// What the overlay shows about the world authority (E2).
pub struct WorldEditReadout {
    /// Whether edits are applied on the world thread (variant B) or inline.
    pub threaded: bool,
    /// Requests handed to the world thread that have not come back yet.
    pub in_flight: usize,
    pub stats: WorldEditStats,
}

/// E2b — the movement readout: which model is driving the view, the key that
/// switches it, and (in walk mode) the body's state plus what its collision step
/// costs the frame thread.
pub(crate) fn draw_movement_readout(ui: &mut egui::Ui, movement: &MovementReadout) {
    // S0 — the studio has its own single line and none of the body state below it.
    if let Some(distance_meters) = movement.studio_orbit_distance_meters {
        ui.label(format!(
            "mode: MATERIAL STUDIO | orbit {distance_meters:.2} m"
        ))
        .on_hover_text(
            "S0: one voxel on a plate, judged in isolation. Mouse turns the subject, \
             the wheel pulls in and out. Launched with `--studio`; no world is \
             generated at all, and the movement modes are off because there is \
             nowhere to walk.",
        );
        return;
    }
    let mode_line = if movement.walking {
        format!(
            "mode: WALK (F = fly) | {:.1} m/s",
            movement.speed_meters_per_second
        )
    } else {
        format!(
            "mode: FLY (F = walk) | {:.1} m/s",
            movement.speed_meters_per_second
        )
    };
    ui.label(mode_line).on_hover_text(
        "E2b: F toggles the fly camera and the walking body. Mouse-look, the \
         mouse-wheel speed knob and click-to-dig/place work in both. Walk mode \
         snaps the body to the ground under the camera on entry; fly mode keeps \
         the eye where the body's head was.",
    );
    if movement.eye_submerged {
        ui.label("view: UNDERWATER").on_hover_text(
            "E6: the primary rays start inside a liquid, so the frame is rendered \
             from inside the medium — extinction accumulates from the eye and \
             looking up gives Snell's window (the sky compressed into a 48.6-degree \
             cone, a mirror outside it). Tested on the eye's own voxel, so it holds \
             for a fly camera under the surface as well as for a swimming body.",
        );
    }
    if movement.walking {
        ui.label(format!(
            "body: {} | {}{} | {:.0} us",
            if movement.grounded {
                "grounded"
            } else {
                "airborne"
            },
            movement.submersion.label(),
            if movement.head_submerged {
                ", head under"
            } else {
                ""
            },
            movement.step_micros,
        ))
        .on_hover_text(
            "The body is a 0.6 x 1.8 m box swept against the voxel grid, resolved \
             per axis, with a 0.375 m (3-voxel) auto-step. The microseconds are \
             the whole movement + collision step on the frame thread.",
        );
    }
}

/// Persistent Studio controls. This only raises a request; the platform layer
/// owns filesystem I/O, loading, and all renderer consequences.
pub(crate) fn draw_studio_assets_section(ui: &mut egui::Ui, state: &mut StudioAssetPanelState) {
    ui.collapsing("Project", |ui| {
        ui.horizontal(|ui| {
            ui.label("folder");
            ui.add(
                egui::TextEdit::singleline(&mut state.project_path)
                    .desired_width(190.0)
                    .hint_text("studio-project"),
            );
        });
        ui.horizontal(|ui| {
            ui.label("quality name");
            ui.add(
                egui::TextEdit::singleline(&mut state.quality_name)
                    .desired_width(150.0)
                    .hint_text("Active quality"),
            );
        });
        ui.horizontal(|ui| {
            if ui
                .button("Save project")
                .on_hover_text(
                    "Saves every live material row, the current quality settings, and the \
                     project manifest. Generated GPU data and pipelines are not saved.",
                )
                .clicked()
            {
                state.save_requested = true;
            }
            if ui
                .button("Load project")
                .on_hover_text(
                    "Loads saved materials and the active quality recipe. Invalid files leave \
                     the live renderer on its last valid state.",
                )
                .clicked()
            {
                state.load_requested = true;
            }
        });
        ui.checkbox(&mut state.autosave_enabled, "Autosave after 2 seconds idle")
            .on_hover_text(
                "OFF by default, and saving does not turn it on. Writes the same portable project \
                 assets after authored material or quality values stop changing.\n\n\
                 Leave it off if anything else edits the project files — a script, an editor, \
                 another tool. Autosave writes from memory, so it will overwrite whatever \
                 arrived on disk since the project was loaded.",
            );
        if state.recovery_available {
            ui.separator();
            ui.label("An interrupted save was found.");
            ui.horizontal(|ui| {
                if ui
                    .button("Restore recovery")
                    .on_hover_text(
                        "Restores the complete pre-commit snapshot, then saves it normally.",
                    )
                    .clicked()
                {
                    state.restore_recovery_requested = true;
                }
                if ui
                    .button("Discard recovery")
                    .on_hover_text(
                        "Deletes only the interrupted-save journal; normal project assets remain.",
                    )
                    .clicked()
                {
                    state.discard_recovery_requested = true;
                }
            });
        }
        if !state.status.is_empty() {
            ui.label(&state.status);
        }
    });
}

#[allow(clippy::too_many_arguments)]
/// Output bit depth, beside VSync because both are DISPLAY properties rather than
/// quality tiers — see the `voxel-color` crate.
///
/// The only lever in the overlay that can be UNAVAILABLE. Every quality lever is
/// always valid; ten-bit output depends on the adapter, the surface's advertised
/// formats and the display. So the control disables itself and says which half is
/// missing, rather than offering a checkbox that silently does nothing.
// Eight arguments for one panel, and bundling them would be worse: three are `&mut`
// selections the caller owns and three are diagnostics that must not be mutable, so a
// struct would either lose that distinction or need two structs to keep it.
pub(crate) fn draw_output_depth(
    ui: &mut egui::Ui,
    depth: &mut OutputDepth,
    support: OutputSupport,
    color_space: ColorSpaceOutcome,
    headroom: DisplayHeadroom,
    headroom_backend: &'static str,
    headroom_choice: &mut HeadroomChoice,
    tonemap_curve: &mut TonemapCurve,
    content_peak: &mut f32,
    exposure: &mut f32,
) {
    ui.horizontal(|ui| {
        ui.label("Output:");
        for option in OutputDepth::ALL {
            // Ask about EACH option, not about 10-bit. The three modes have different
            // vetoes: 8-bit is always available, 10-bit needs both a surface format and
            // a device feature, HDR float needs both a float surface and a presentation
            // contract the platform can establish. Gating them all on one predicate
            // would disable a mode the device can actually do.
            ui.add_enabled_ui(support.supports(option), |ui| {
                ui.selectable_value(depth, option, option.label());
            });
        }
    });
    ui.label(
        "Switching reconfigures the surface, reallocates the frame texture and \
         rebuilds three pipelines. Not a per-frame knob.",
    );
    if !support.supports(OutputDepth::TenBit) {
        ui.label(format!("  10-bit: {}", support.ten_bit_diagnosis()));
    }
    if !support.supports(OutputDepth::HdrFloat) {
        ui.label(format!("  HDR float: {}", support.hdr_diagnosis()));
    }
    // SHOWN ALWAYS, not only on failure. An untagged surface is displayed
    // pass-through, so a missing tag is a wrong picture that nothing else reports —
    // which is exactly how it went unnoticed until HDR made it obvious. See
    // `voxel_color::color_space`.
    ui.label(format!("  colour space: {}", color_space.diagnosis()));
    // The number AND where it came from. "1.6x" alone cannot be told apart from a guess,
    // and the old hard-coded 4.0 was exactly a guess nobody could see — so the source is
    // shown beside it always, not only when it is missing.
    ui.label(format!(
        "  headroom: {:.2}x ({:.0} cd/m², {})",
        headroom.ratio(),
        headroom.peak_nits(),
        headroom.source().diagnosis(),
    ));
    ui.horizontal(|ui| {
        ui.label("Headroom:");
        for option in HeadroomChoice::PRESETS {
            ui.selectable_value(headroom_choice, option, option.label());
        }
    });
    ui.label(format!("  backend: {headroom_backend}"));
    if *depth == OutputDepth::HdrFloat && !headroom.has_headroom() {
        ui.label(
            "  no headroom: HDR curves are confined to SDR range. Reinhard+HDR exactly \
             matches SDR; raise display brightness, or pin a ratio above.",
        );
    }

    // The tonemap is here rather than in the Quality section on purpose: it is not a
    // performance tier, it is the other half of what the depth toggle changes. Switching
    // to HDR float used to swap this curve invisibly, which is why the whole room got
    // brighter and nothing said so.
    ui.horizontal(|ui| {
        ui.label("Tonemap:");
        for option in TonemapCurve::ALL {
            ui.selectable_value(tonemap_curve, option, option.label());
        }
    });
    ui.label(format!("  {}", tonemap_curve.description()));
    if *depth == OutputDepth::HdrFloat && !tonemap_curve.can_exceed_white() {
        ui.label("  this curve cannot exceed white, so HDR float has nothing extra to show.");
    }
    // ABOVE the tonemap row, because it runs first and because reading them in pipeline
    // order is what makes the separation obvious.
    ui.horizontal(|ui| {
        ui.label("Exposure:");
        ui.add(
            egui::Slider::new(exposure, 0.0..=8.0)
                .logarithmic(true)
                .show_value(true),
        );
    });
    ui.label(
        "  scales the scene BEFORE the tonemap. 1.0 is the look before exposure existed. \
         Without it the curve doubles as exposure, which is why changing curves used to \
         change brightness.",
    );

    // Shown only for the curve that reads it — a live-looking control that does nothing is
    // worse than no control.
    if tonemap_curve.uses_content_peak() {
        ui.horizontal(|ui| {
            ui.label("Scene peak:");
            for option in voxel_color::tonemap::CONTENT_PEAK_PRESETS {
                ui.selectable_value(content_peak, option, format!("{option:.0}x"));
            }
        });
        ui.label(format!(
            "  assumes the brightest pixel is {:.0} cd/m². NOT measured — the EETF needs a \
             content peak and nothing reports one.",
            voxel_color::nits(*content_peak),
        ));
    }
}

/// The E1c Quality section: preset selector on top, then every registry lever
/// grouped by subsystem. Selecting a preset overwrites the knobs; touching any
/// knob switches the tag to [`QualityPreset::Custom`] — detected by comparing
/// the knobs before and after the UI ran, so no widget can forget to do it.
pub(crate) fn draw_quality_section(ui: &mut egui::Ui, quality: &mut RenderQuality) {
    let knobs_before = *quality;
    let mut preset_selected = false;

    ui.collapsing("Quality", |ui| {
        ui.horizontal_wrapped(|ui| {
            ui.label("preset");
            for spec in QUALITY_PRESETS {
                if ui
                    .radio(quality.preset == spec.preset, spec.label)
                    .on_hover_text(spec.summary)
                    .clicked()
                {
                    quality.apply_preset(spec.preset);
                    preset_selected = true;
                }
            }
        });
        for subsystem in LeverSubsystem::ALL {
            ui.collapsing(subsystem.label(), |ui| {
                for lever in levers_of(subsystem) {
                    ui.add_enabled_ui(lever_is_relevant(quality, lever.id), |ui| {
                        draw_lever(ui, quality, lever);
                    });
                }
            });
        }
    });

    if !preset_selected && quality.knobs_differ(&knobs_before) {
        quality.preset = QualityPreset::Custom;
    }
}

/// Whether a lever currently does anything — the ray knobs are meaningless for
/// the analytic estimators, the penumbra scale for hard shadows, the fade range
/// with the fade off. Greyed out instead of hidden so the panel layout is
/// stable and the verdict stays readable. Presentation only: the value itself
/// is preserved either way.
fn lever_is_relevant(quality: &RenderQuality, lever_id: LeverId) -> bool {
    let ambient_occlusion = &quality.ambient_occlusion;
    match lever_id {
        LeverId::AoStrength => ambient_occlusion.mode != AoMode::Off,
        LeverId::AoRayCount
        | LeverId::AoMaxDistance
        | LeverId::AoDirectionMode
        | LeverId::AoDistanceFalloff
        | LeverId::AoBrickEarlyOut
        | LeverId::AoSunAwareRayBudget => ambient_occlusion.mode == AoMode::RayTraced,
        LeverId::AoDistanceFade => ambient_occlusion.mode != AoMode::Off,
        LeverId::AoFadeStart | LeverId::AoFadeEnd => {
            ambient_occlusion.mode != AoMode::Off && ambient_occlusion.distance_fade
        }
        LeverId::ShadowPenumbraScale => quality.shadows.mode == ShadowMode::SoftDistanceField,
        // Every CAGI knob is meaningless without the light volume (E4).
        LeverId::GiResolution
        | LeverId::GiRule
        | LeverId::GiSkyTest
        | LeverId::GiSunCache
        | LeverId::GiSampleMode
        | LeverId::GiIterationsPerFrame
        | LeverId::GiStrength
        | LeverId::GiAmbientFloor
        | LeverId::GiSunBounce => quality.global_illumination.enabled,
        // E6: every water knob is meaningless while water is drawn opaque, and the
        // two ray budgets only mean something for a mode that traces rays.
        LeverId::WaterAbsorption | LeverId::WaterScattering | LeverId::WaterSunThroughLiquid => {
            quality.water.mode != WaterMode::Opaque
        }
        LeverId::WaterRayCutoff => {
            quality.water.mode.traces_reflection() || quality.water.mode.traces_refraction()
        }
        // E6 step 3: both of these describe what happens after a ray mirrors off the
        // underside of the surface, and the shipped transparent interface never
        // mirrors.
        LeverId::WaterBounces | LeverId::WaterTirFallback => {
            (quality.water.mode.traces_reflection() || quality.water.mode.traces_refraction())
                && quality.water.bounce_levers_have_an_effect()
        }
        LeverId::WaterUnderwaterInterface => quality.water.mode != WaterMode::Opaque,
        // E7: turbidity and the bounce light both need water that is not drawn opaque, and
        // caustics additionally need the sun to reach through the liquid at all — they scale
        // that sun term and there is nothing to scale without it.
        LeverId::WaterVisibilityDepth | LeverId::WaterBounceLight => {
            quality.water.mode != WaterMode::Opaque
        }
        // The milkiness split has nothing to split while turbidity is off.
        LeverId::WaterTurbidityScattering => {
            quality.water.mode != WaterMode::Opaque && quality.water.visibility_depth_blocks > 0.0
        }
        LeverId::WaterCaustics => {
            quality.water.mode != WaterMode::Opaque
                && quality.water.sun_through_liquid
                && quality.water.waves
        }
        // E2: the box radius only means something for the bounded strategy, and
        // re-flooding needs a light volume to flood.
        LeverId::EditClearanceRadius => {
            quality.world_edit.clearance_update == ClearanceUpdateMode::LocalBox
        }
        LeverId::EditGiReflood => quality.global_illumination.enabled,
        _ => true,
    }
}

/// One lever's control, shaped by its value kind and [`LeverRange`], with its
/// measured verdict as hover text (per-option verdicts for mode levers).
fn draw_lever(ui: &mut egui::Ui, quality: &mut RenderQuality, lever: &Lever) {
    match lever.id.read(quality) {
        LeverValue::Flag(current) => {
            let mut flag = current;
            if ui
                .checkbox(&mut flag, lever.label)
                .on_hover_text(lever.verdict)
                .changed()
            {
                lever.id.apply(quality, LeverValue::Flag(flag));
            }
        }
        LeverValue::Mode(current) => {
            ui.horizontal_wrapped(|ui| {
                ui.label(lever.label).on_hover_text(lever.verdict);
                for option in lever.mode_options {
                    if ui
                        .radio(current == option.value, option.label)
                        .on_hover_text(option.verdict)
                        .clicked()
                    {
                        lever.id.apply(quality, LeverValue::Mode(option.value));
                    }
                }
            });
        }
        LeverValue::Count(current) => {
            draw_rung_row(ui, quality, lever, current, LeverValue::Count);
        }
        LeverValue::VoxelDistance(current) => match lever.range {
            LeverRange::Rungs(_) => {
                draw_rung_row(ui, quality, lever, current, LeverValue::VoxelDistance);
            }
            LeverRange::Meters { minimum, maximum } => {
                let mut meters = current as f32 / VOXELS_PER_METER;
                if ui
                    .add(egui::Slider::new(&mut meters, minimum..=maximum).text(lever.label))
                    .on_hover_text(lever.verdict)
                    .changed()
                {
                    let voxels = (meters * VOXELS_PER_METER).round() as u32;
                    lever.id.apply(quality, LeverValue::VoxelDistance(voxels));
                }
            }
            _ => panic!("{:?} needs Rungs or Meters bounds", lever.id),
        },
        LeverValue::Scalar(current) => {
            let LeverRange::Continuous {
                minimum,
                maximum,
                logarithmic,
            } = lever.range
            else {
                panic!("{:?} needs Continuous bounds", lever.id);
            };
            let mut value = current;
            if ui
                .add(
                    egui::Slider::new(&mut value, minimum..=maximum)
                        .logarithmic(logarithmic)
                        .text(lever.label),
                )
                .on_hover_text(lever.verdict)
                .changed()
            {
                lever.id.apply(quality, LeverValue::Scalar(value));
            }
        }
    }
}

/// A radio row over a lever's fixed integer rungs.
fn draw_rung_row(
    ui: &mut egui::Ui,
    quality: &mut RenderQuality,
    lever: &Lever,
    current: u32,
    wrap: fn(u32) -> LeverValue,
) {
    let LeverRange::Rungs(rungs) = lever.range else {
        panic!("{:?} needs Rungs bounds", lever.id);
    };
    ui.horizontal_wrapped(|ui| {
        ui.label(lever.label).on_hover_text(lever.verdict);
        for rung in rungs {
            if ui
                .radio(current == *rung, rung.to_string())
                .on_hover_text(lever.verdict)
                .clicked()
            {
                lever.id.apply(quality, wrap(*rung));
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_window_starts_hidden_and_o_toggles_it() {
        let mut panel = SettingsPanel::new();
        assert!(
            !panel.visible,
            "the viewport is the point — this must not cost screen space unasked"
        );
        panel.toggle();
        assert!(panel.visible);
        panel.toggle();
        assert!(!panel.visible);
    }
}
