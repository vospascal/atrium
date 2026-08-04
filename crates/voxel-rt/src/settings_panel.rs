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
//! 7. **Studio** — asset and project panel.
//!
//! The leaf drawing functions for 4, 5, 7 and the movement readout still live in
//! [`crate::overlay`] next to the state they render; this module owns the window
//! and the composition, not a copy of them.

use voxel_color::{
    ColorSpaceOutcome, DisplayHeadroom, HeadroomChoice, OutputDepth, OutputSupport, TonemapCurve,
};
use voxel_environment::SunSettings;

use crate::overlay::{MovementReadout, WorldEditReadout};
use crate::studio_assets::StudioAssetPanelState;
use crate::variants::RenderQuality;

/// Everything the window reads or mutates, bundled.
///
/// A struct rather than fifteen parameters: the call already threads this many
/// values through [`crate::overlay::Overlay::render`], and repeating that list at
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
        crate::overlay::draw_movement_readout(ui, movement);
    });

    ui.collapsing("Presentation", |ui| {
        ui.checkbox(vsync_enabled, "VSync");
        ui.label(if *vsync_enabled {
            "VSync on: Frame loop normally tracks the display."
        } else {
            "VSync off: Compare GPU FPS; the frame loop can run ahead of the GPU."
        })
        .on_hover_text("Toggling this resets the performance panel's history.");
        crate::overlay::draw_output_depth(
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
        crate::overlay::draw_quality_section(ui, quality);
    });

    ui.collapsing("Sun", |ui| draw_sun_section(ui, sun_settings));

    ui.collapsing("Studio", |ui| {
        crate::overlay::draw_studio_assets_section(ui, studio_assets);
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
