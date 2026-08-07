//! egui overlay: stats panel (window/render sizes, moving-average frame-loop
//! FPS, GPU-only FPS, per-pass GPU times), the vsync lever, a collapsible sun-position
//! section, the E1c **Quality** section — preset selector plus every lever
//! grouped by subsystem, each carrying its measured verdict as hover text so
//! "why is this off?" is answerable in-app — and a **Debug tools** section for
//! the actions that change the WORLD rather than how it is drawn. Drawn on top
//! of the rendered frame in its own render pass (LoadOp::Load).
//!
//! The Quality section is generated from [`voxel_rt::variants::REGISTRY`]: the
//! widget shape comes from each lever's [`LeverRange`], the hover text from its
//! verdict, and reads/writes go through [`LeverId::read`] / [`LeverId::apply`].
//! Adding a lever row therefore adds its control here automatically.
//!
//! Seam: the overlay only mutates the state it is handed (`vsync_enabled`,
//! [`SunSettings`], [`RenderQuality`]) — reconfiguring the surface, resizing the
//! storage texture, writing the lighting uniform, and switching the pipeline on
//! a compile-time lever change all stay in the platform layer.

use winit::event::WindowEvent;
use winit::window::Window;

use crate::material_edit::{MaterialDebugView, MaterialPanelState, WORLD_HOTBAR_BLOCKS};
use voxel_color::{
    ColorSpaceOutcome, DisplayHeadroom, HeadroomChoice, OutputDepth, OutputSupport, TonemapCurve,
};
use voxel_environment::SunSettings;
use voxel_game_ui::performance_panel::PerformancePanel;
use voxel_game_ui::settings_panel::{
    MovementReadout, SettingsContext, SettingsPanel, WorldEditReadout,
};
use voxel_material::material::{self, MATERIAL_COUNT};
use voxel_material_graph::lowering::GraphEditorState;
use voxel_rt::material_table::MaterialTable;
use voxel_rt::profiling::FrameTimings;
use voxel_rt::studio::StudioPose;
use voxel_rt::studio_assets::StudioAssetPanelState;
use voxel_rt::variants::RenderQuality;
use voxel_studio::{draw_graph_drawer, graph_drawer_height};

/// Read-only per-frame display data for the stats panel.
pub struct OverlayFrameData {
    /// Storage-texture (ray-traced) resolution, pixels.
    pub render_resolution: (u32, u32),
    /// E2 — the edit pipeline's live numbers, so the "zero frame hitches" gate is
    /// judgeable in-app and not only in the harness.
    pub world_edit: WorldEditReadout,
    /// E2b — which movement model is driving the view, and what the body is
    /// doing.
    pub movement: MovementReadout,
    /// The editable face currently under the crosshair, projected to the overlay
    /// so the player gets a precise placement preview before editing it.
    pub target: Option<TargetHighlightReadout>,
    /// S2 — `Some` only in the material studio, where the example-scene bar lives.
    pub studio: Option<StudioPoseReadout>,
}

/// S2 — what the studio's example-scene bar has to mark as current.
///
/// `active` is `None` while an imported `.vox` subject is on screen: the subject
/// overrides every built-in pose, so highlighting one would claim you are looking
/// at a shape you are not.
pub struct StudioPoseReadout {
    pub active: Option<StudioPose>,
    /// Whether the ground plate is currently part of the scene — the eye's state.
    pub plate_shown: bool,
    /// What the ground plate is made of — the floor picker's state. Not the row being
    /// edited: that is `MaterialPanelState::selected`, and Graph Studio owns it.
    pub plate_material: u8,
}

/// Read-only target data prepared by the platform layer. Keeping screen-space
/// corners here lets the overlay draw a world-aligned face without owning a
/// camera or touching the world lock.
pub struct TargetHighlightReadout {
    pub material: u8,
    pub voxel: [i32; 3],
    pub distance_meters: f32,
    pub screen_corners: Option<[[f32; 2]; 4]>,
}

pub struct Overlay {
    context: egui::Context,
    winit_state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    /// The surface format the egui pipeline was built against. egui builds its OWN
    /// render pipeline, so it is a consumer of the output format like the blit is —
    /// see [`Overlay::set_surface_format`].
    surface_format: wgpu::TextureFormat,
    /// Press P. Owns every perf statistic and all of its history — the overlay
    /// itself keeps no timing state, so there is one definition of "how fast".
    performance: PerformancePanel,
    /// Press O. Holds everything that used to be permanently on screen.
    settings: SettingsPanel,
}

impl Overlay {
    pub fn new(
        window: &Window,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
    ) -> Self {
        let context = egui::Context::default();
        let winit_state = egui_winit::State::new(
            context.clone(),
            egui::ViewportId::ROOT,
            window,
            None,
            None,
            None,
        );
        let renderer = egui_wgpu::Renderer::new(
            device,
            surface_format,
            egui_wgpu::RendererOptions::default(),
        );

        Self {
            context,
            winit_state,
            renderer,
            surface_format,
            performance: PerformancePanel::new(),
            settings: SettingsPanel::new(),
        }
    }

    /// Feed a window event to egui; returns true when egui consumed it (e.g.
    /// a click on the overlay panel) so the platform layer can skip its own
    /// handling.
    pub fn handle_window_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        let consumed = self.winit_state.on_window_event(window, event).consumed;
        // `on_window_event` only reports whether this particular event was
        // consumed. The graph canvas is an interactive region even when the
        // event is a wheel or a middle-button gesture, so also consult egui's
        // current hit-test state to keep viewport input from leaking through.
        consumed
            || self.context.egui_wants_pointer_input()
            || self.context.egui_wants_keyboard_input()
    }

    pub fn wants_keyboard_input(&self) -> bool {
        self.context.egui_wants_keyboard_input()
    }

    pub fn wants_pointer_input(&self) -> bool {
        self.context.egui_wants_pointer_input()
    }

    /// Fold one frame's measurements into the performance panel. Call once per
    /// frame, at the END of the frame, so the spans drained here are the ones
    /// this frame actually recorded — including present.
    pub fn record_frame(
        &mut self,
        span_recorder: &atrium_profile::cpu::SpanRecorder,
        gpu_timings: Option<FrameTimings>,
    ) {
        self.performance.record_frame(span_recorder, gpu_timings);
    }

    /// The performance panel's byte gauges, for the platform layer to report the
    /// sizes it owns once per frame.
    pub fn performance_memory(&self) -> &atrium_profile::memory::MemoryLedger {
        self.performance.memory()
    }

    /// P — show or hide the performance window.
    pub fn toggle_performance_panel(&mut self) {
        self.performance.toggle();
    }

    /// O — show or hide the settings and debug window.
    pub fn toggle_settings_panel(&mut self) {
        self.settings.toggle();
    }

    /// Drop perf history after a deliberate discontinuity (vsync toggle, output
    /// format change) so the transient is not read as a regression.
    pub fn reset_performance_history(&mut self) {
        self.performance.reset();
    }

    /// Rebuild egui for a new surface format.
    ///
    /// egui-wgpu bakes the colour attachment format into its pipeline at
    /// construction, so an output-depth change otherwise leaves it targeting the old
    /// format and `set_pipeline` fails with *"Render pipeline targets are incompatible
    /// with render pass"*. That makes egui a consumer of
    /// [`voxel_color::OutputFormat`] exactly as the blit is — the difference
    /// being that we do not own its pipeline, so the renderer is replaced wholesale.
    ///
    /// **The CONTEXT is rebuilt too, and it has to be.** `egui::Context` remembers
    /// which textures it has already handed to a renderer, so a fresh `Renderer`
    /// paired with the old `Context` receives a PARTIAL font-atlas update for a
    /// texture it never allocated — *"Tried to update a texture that has not been
    /// allocated yet"*, inside a non-unwinding callback, so it aborts rather than
    /// panics cleanly. `Context::set_fonts` is not a way out: it compares definitions
    /// and does nothing when they are unchanged.
    ///
    /// The cost is UI state — collapsed sections, scroll offsets, window positions all
    /// reset. Accepted because this fires only on an output-depth change, which is
    /// already reconfiguring the surface and rebuilding three pipelines. The performance
    /// history is carried across deliberately, so the graph does not blank.
    pub fn set_surface_format(
        &mut self,
        window: &Window,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
    ) {
        if surface_format == self.surface_format {
            return;
        }
        // Carry the whole performance panel across the rebuild — history,
        // visibility and all. Rebuilding it would blank several seconds of
        // history exactly when an output-depth change is the thing being
        // measured, and would close the window the user had open.
        let performance = std::mem::take(&mut self.performance);
        let settings = std::mem::take(&mut self.settings);
        *self = Overlay::new(window, device, surface_format);
        self.performance = performance;
        self.settings = settings;
    }

    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        window: &Window,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        target_view: &wgpu::TextureView,
        frame_data: &OverlayFrameData,
        timestamp_writes: Option<wgpu::RenderPassTimestampWrites<'_>>,
        vsync_enabled: &mut bool,
        output_depth: &mut OutputDepth,
        output_support: OutputSupport,
        // Diagnostics only — the overlay reports these, never acts on them.
        output_color_space: ColorSpaceOutcome,
        output_headroom: DisplayHeadroom,
        headroom_backend: &'static str,
        headroom_choice: &mut HeadroomChoice,
        tonemap_curve: &mut TonemapCurve,
        content_peak: &mut f32,
        exposure: &mut f32,
        sun_settings: &mut SunSettings,
        sky_weather: &mut voxel_rt::sky_weather::SkyWeather,
        quality: &mut RenderQuality,
        material_table: &mut MaterialTable,
        material_panel: &mut MaterialPanelState,
        studio_assets: &mut StudioAssetPanelState,
        graph_editor: &mut GraphEditorState,
    ) {
        // The one number that stays permanently on screen. Everything else moved
        // into the P window: per-span cost is something you go looking for, and
        // the always-on panel was already too large.
        let frames_per_second = self.performance.frames_per_second();

        // Surface the Retina trap: on macOS the swapchain is PHYSICAL pixels,
        // which can be 4x the logical window area at scale factor 2.0.
        let physical_size = window.inner_size();
        let scale_factor = window.scale_factor();
        let logical_size = physical_size.to_logical::<f64>(scale_factor);

        let raw_input = self.winit_state.take_egui_input(window);
        // `egui::Context` is an `Arc` handle, so cloning it is cheap — and it is
        // what lets the closure below borrow `self.performance` mutably while the
        // context drives the UI pass.
        let context = self.context.clone();
        let performance = &mut self.performance;
        let settings = &mut self.settings;
        let full_output = context.run_ui(raw_input, |root_ui| {
            performance.draw(root_ui.ctx());
            draw_graph_drawer(root_ui, graph_editor, material_table);
            draw_target_highlight(root_ui, frame_data.target.as_ref(), material_table);
            if let Some(studio) = &frame_data.studio {
                draw_studio_pose_bar(root_ui, studio, material_panel, material_table);
            } else {
                draw_material_debug_bar(root_ui, material_panel, material_table);
            }
            // Not in the studio: it is a PLACEMENT bar, and the studio does not
            // place — `ControlMode::StudioOrbit::allows_world_edits` is false, so
            // "Right-click: place selected" was advertising a click that does
            // nothing while covering the bottom of the subject. The studio picks its
            // material from the row being edited (see
            // `follow_selected_row_in_the_studio`), not from a hotbar.
            if frame_data.studio.is_none() {
                let drawer_height = graph_drawer_height(root_ui, graph_editor);
                draw_block_hotbar(root_ui, material_table, material_panel, drawer_height);
            }
            // The ONLY permanently visible readout: one line. Everything that used
            // to live here — resolutions, edit counters, movement, vsync, output
            // depth, quality levers, the sun section — moved into the O window,
            // because a debug UI that covers the render is measuring the wrong
            // thing.
            egui::Area::new(egui::Id::new("hud"))
                .anchor(egui::Align2::LEFT_TOP, egui::vec2(8.0, 8.0))
                .show(root_ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(format!(
                            "{}  ·  {} x {}  ·  P perf  ·  O settings",
                            match frames_per_second {
                                Some(value) => format!("{value:.0} FPS"),
                                None => "-- FPS".to_string(),
                            },
                            frame_data.render_resolution.0,
                            frame_data.render_resolution.1,
                        ))
                        .on_hover_text(
                            "Frame rate from the MEDIAN frame interval, not a mean: \
                             a mean is dominated by the fastest frames and once read \
                             '1200 FPS' while the display was visibly hitching.\n\n\
                             P opens spans, pacing, waves and memory. O opens settings \
                             and the rest of the diagnostics.",
                        );
                    });
                });
            settings.draw(
                root_ui.ctx(),
                SettingsContext {
                    logical_size: (logical_size.width, logical_size.height),
                    physical_size: (physical_size.width, physical_size.height),
                    render_resolution: frame_data.render_resolution,
                    scale_factor,
                    world_edit: &frame_data.world_edit,
                    movement: &frame_data.movement,
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
                },
            );
        });
        self.winit_state
            .handle_platform_output(window, full_output.platform_output);

        let clipped_primitives = self
            .context
            .tessellate(full_output.shapes, full_output.pixels_per_point);
        let screen_descriptor = egui_wgpu::ScreenDescriptor {
            size_in_pixels: [physical_size.width, physical_size.height],
            pixels_per_point: full_output.pixels_per_point,
        };

        for (texture_id, image_delta) in &full_output.textures_delta.set {
            self.renderer
                .update_texture(device, queue, *texture_id, image_delta);
        }
        self.renderer.update_buffers(
            device,
            queue,
            encoder,
            &clipped_primitives,
            &screen_descriptor,
        );

        {
            let mut render_pass = encoder
                .begin_render_pass(&wgpu::RenderPassDescriptor {
                    label: Some("egui overlay pass"),
                    color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                        view: target_view,
                        depth_slice: None,
                        resolve_target: None,
                        ops: wgpu::Operations {
                            load: wgpu::LoadOp::Load,
                            store: wgpu::StoreOp::Store,
                        },
                    })],
                    depth_stencil_attachment: None,
                    timestamp_writes,
                    occlusion_query_set: None,
                    multiview_mask: None,
                })
                .forget_lifetime();
            self.renderer
                .render(&mut render_pass, &clipped_primitives, &screen_descriptor);
        }

        for texture_id in &full_output.textures_delta.free {
            self.renderer.free_texture(texture_id);
        }
    }
}

/// Draw a bright outline around the precise face the edit ray will affect.
/// The target comes from the same CPU DDA cast as placement/removal, so the
/// outline and the next click cannot disagree about which block is selected.
fn draw_target_highlight(
    ui: &mut egui::Ui,
    target: Option<&TargetHighlightReadout>,
    material_table: &MaterialTable,
) {
    let Some(target) = target else {
        return;
    };
    let Some(corners) = target.screen_corners else {
        return;
    };
    let points = corners.map(|point| egui::pos2(point[0], point[1]));
    let painter = ui.ctx().layer_painter(egui::LayerId::new(
        egui::Order::Foreground,
        egui::Id::new("voxel_target_highlight"),
    ));
    let stroke = egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 226, 92));
    for (from, to) in [(0, 1), (1, 3), (3, 2), (2, 0)] {
        painter.line_segment([points[from], points[to]], stroke);
    }
    let name = material_table
        .row(target.material)
        .map_or("unknown", |row| row.name);
    let label_position = points
        .iter()
        .copied()
        .reduce(|nearest, point| if point.y < nearest.y { point } else { nearest })
        .unwrap_or(points[0]);
    painter.text(
        label_position + egui::vec2(4.0, -4.0),
        egui::Align2::LEFT_BOTTOM,
        format!(
            "{name} · {}, {}, {} · {:.1} m",
            target.voxel[0], target.voxel[1], target.voxel[2], target.distance_meters
        ),
        egui::TextStyle::Small.resolve(ui.style()),
        egui::Color32::from_rgb(255, 239, 167),
    );
}

/// The yellow the overlay already uses for "this is the current one" — target
/// highlight and hotbar selection both mark with it, so the pose bar does too.
const OVERLAY_ACCENT: egui::Color32 = egui::Color32::from_rgb(255, 226, 92);

/// S2 — the studio's example scenes as four one-click icons, top right.
///
/// Drawn shapes rather than words: what you want to know before clicking is which
/// SHAPE the material will be shown on, and four labels is four labels to read
/// every time. Writes a one-shot request instead of the pose itself, because
/// servicing it rebuilds the world and that is the platform layer's job.
///
/// The eye takes the ground plate out of the scene, which is the one part of the
/// studio you sometimes need gone: it is what the subject casts its shadow onto and
/// bounces light off, so a material judged against white snow is not the same
/// material judged against sky.
fn draw_studio_pose_bar(
    ui: &mut egui::Ui,
    studio: &StudioPoseReadout,
    material_panel: &mut MaterialPanelState,
    material_table: &MaterialTable,
) {
    egui::Area::new(egui::Id::new("studio_pose_bar"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    if studio_eye_icon(ui, studio.plate_shown)
                        .on_hover_text(if studio.plate_shown {
                            "Hide the ground plate under the subject"
                        } else {
                            "Show the ground plate under the subject"
                        })
                        .clicked()
                    {
                        material_panel.studio_plate_requested = Some(!studio.plate_shown);
                    }
                    for pose in StudioPose::EXAMPLES {
                        let response = studio_pose_icon(ui, pose, studio.active == Some(pose));
                        if response.on_hover_text(pose.label()).clicked() {
                            material_panel.studio_pose_requested = Some(pose);
                        }
                    }
                    draw_studio_base_picker(ui, studio, material_panel, material_table);
                    draw_material_debug_menu(ui, material_panel, material_table);
                });
            });
        });
}

/// The same temporary material inspection controls are available over the world
/// renderer as over the studio. Keeping this state in the platform panel means a
/// user can isolate a grass layer on the cube, then walk into the world with the
/// exact same channel view still active.
fn draw_material_debug_bar(
    ui: &mut egui::Ui,
    material_panel: &mut MaterialPanelState,
    material_table: &MaterialTable,
) {
    egui::Area::new(egui::Id::new("material_debug_bar"))
        .anchor(egui::Align2::RIGHT_TOP, egui::vec2(-8.0, 8.0))
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                draw_material_debug_menu(ui, material_panel, material_table);
            });
        });
}

/// Temporary renderer inspection controls. They deliberately do not edit the
/// graph or the material table: the mask is a per-frame shader override, which
/// makes "all layers", "no layers", and "only this layer" useful comparisons.
fn draw_material_debug_menu(
    ui: &mut egui::Ui,
    material_panel: &mut MaterialPanelState,
    material_table: &MaterialTable,
) {
    let active_layers = material_table
        .row(material_panel.selected)
        .map_or(0, |row| row.patterns.active_count())
        .min(4);
    let debug_active =
        material_panel.debug_layers_enabled || material_panel.debug_view != MaterialDebugView::Lit;
    let button_label = if debug_active { "debug •" } else { "debug" };

    ui.menu_button(button_label, |menu| {
        menu.checkbox(
            &mut material_panel.debug_layers_enabled,
            "Enable layer override",
        )
        .on_hover_text(
            "Temporarily controls which pattern layers reach the renderer. The saved graph is unchanged.",
        );
        menu.separator();
        menu.label("Preview channel");
        for view in MaterialDebugView::ALL {
            menu.selectable_value(&mut material_panel.debug_view, view, view.label());
        }
        menu.separator();
        menu.label(format!("Pattern layers ({active_layers})"));
        menu.horizontal(|row| {
            if row.button("all").clicked() {
                material_panel.debug_layer_mask = if active_layers == 0 {
                    0
                } else {
                    (1u8 << active_layers) - 1
                };
            }
            if row.button("none").clicked() {
                material_panel.debug_layer_mask = 0;
            }
        });
        for slot in 0..active_layers {
            let bit = 1u8 << slot;
            let mut enabled = (material_panel.debug_layer_mask & bit) != 0;
            row_layer_toggle(
                menu,
                slot,
                &mut enabled,
                &mut material_panel.debug_layer_mask,
            );
        }
        menu.small("Use one checked layer for an isolated preview.");
    });
}

fn row_layer_toggle(menu: &mut egui::Ui, slot: usize, enabled: &mut bool, mask: &mut u8) {
    if menu
        .checkbox(enabled, format!("Layer {}", slot + 1))
        .changed()
    {
        let bit = 1u8 << slot;
        if *enabled {
            *mask |= bit;
        } else {
            *mask &= !bit;
        }
    }
}

/// What the base plate under the subject is made of.
///
/// Called "base", not "floor": [`StudioPose::Floor`] is a floor too, and that one is
/// the SUBJECT — the 9x9 slab of the row you are editing. This picker is the
/// backdrop it stands on, casts onto, and bounces light off.
///
/// Deliberately NOT a subject picker: the block being previewed is the row being
/// edited, and Graph Studio's material slot is where that is chosen (it writes
/// `MaterialPanelState::selected`, which the studio subject follows).
fn draw_studio_base_picker(
    ui: &mut egui::Ui,
    studio: &StudioPoseReadout,
    material_panel: &mut MaterialPanelState,
    material_table: &MaterialTable,
) {
    let plate_name = material_table
        .row(studio.plate_material)
        .map_or("unknown", |row| row.name);
    // Greyed out with the plate hidden: picking a base you cannot see would look
    // like the box doing nothing.
    ui.add_enabled_ui(studio.plate_shown, |ui| {
        ui.label("base");
        let (swatch_rect, _) = ui.allocate_exact_size(egui::vec2(14.0, 44.0), egui::Sense::hover());
        ui.painter().rect_filled(
            egui::Rect::from_center_size(swatch_rect.center(), egui::vec2(14.0, 28.0)),
            3.0,
            material_swatch(material_table, studio.plate_material),
        );
        let mut chosen = studio.plate_material;
        egui::ComboBox::from_id_salt("studio_plate_material")
            .selected_text(format!("{plate_name} ▾"))
            .width(112.0)
            .show_ui(ui, |ui| {
                for id in 1..MATERIAL_COUNT as u8 {
                    let name = material_table.row(id).map_or("unknown", |row| row.name);
                    ui.selectable_value(&mut chosen, id, format!("{id:>2}  {name}"));
                }
            })
            .response
            .on_hover_text(
                "Which row the base plate is built from. Same table as everything \
                 else, so editing that row changes the base too — exactly as it does \
                 in the world.",
            );
        if chosen != studio.plate_material {
            material_panel.studio_plate_material_requested = Some(chosen);
        }
    });
}

/// The plate's show/hide eye. Narrower than a pose icon: it is a toggle for the
/// scene, not a fifth example to pick.
fn studio_eye_icon(ui: &mut egui::Ui, shown: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(28.0, 44.0), egui::Sense::click());
    let painter = ui.painter_at(rect);
    if response.hovered() {
        painter.rect_filled(rect, 4.0, egui::Color32::from_gray(58));
    }
    let ink = if shown {
        egui::Color32::from_gray(220)
    } else {
        egui::Color32::from_gray(140)
    };
    let stroke = egui::Stroke::new(1.4, ink);
    let center = rect.center();
    painter.add(egui::Shape::ellipse_stroke(
        center,
        egui::vec2(9.0, 5.5),
        stroke,
    ));
    painter.circle_filled(center, 2.6, ink);
    if !shown {
        // The struck-through eye is the state that has to read at a glance: it is
        // what answers "why is my subject floating in the sky?".
        painter.line_segment(
            [
                center + egui::vec2(-8.0, 6.0),
                center + egui::vec2(8.0, -6.0),
            ],
            egui::Stroke::new(1.6, ink),
        );
    }
    response
}

/// One example-scene button: a small isometric drawing of the pose's geometry.
fn studio_pose_icon(ui: &mut egui::Ui, pose: StudioPose, selected: bool) -> egui::Response {
    let (rect, response) = ui.allocate_exact_size(egui::vec2(44.0, 44.0), egui::Sense::click());
    let painter = ui.painter_at(rect);
    let background = match (selected, response.hovered()) {
        (true, _) => egui::Color32::from_rgb(58, 52, 26),
        (false, true) => egui::Color32::from_gray(58),
        (false, false) => egui::Color32::from_gray(38),
    };
    painter.rect_filled(rect, 4.0, background);
    painter.rect_stroke(
        rect,
        4.0,
        if selected {
            egui::Stroke::new(2.0, OVERLAY_ACCENT)
        } else {
            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(70))
        },
        egui::StrokeKind::Inside,
    );

    // Everything below is in normalised icon space, so the drawing scales with the
    // button rather than being a table of pixel offsets.
    let art = rect.shrink(8.0);
    let at =
        |x: f32, y: f32| egui::pos2(art.left() + x * art.width(), art.top() + y * art.height());
    let lit = if selected {
        OVERLAY_ACCENT
    } else {
        egui::Color32::from_gray(238)
    };
    let front = if selected {
        OVERLAY_ACCENT.gamma_multiply(0.62)
    } else {
        egui::Color32::from_gray(168)
    };
    let side = if selected {
        OVERLAY_ACCENT.gamma_multiply(0.4)
    } else {
        egui::Color32::from_gray(112)
    };
    let seam = egui::Stroke::new(1.0, egui::Color32::from_black_alpha(120));
    let ground = egui::Stroke::new(1.0, egui::Color32::from_white_alpha(60));

    match pose {
        // A ground plane in perspective: near edge wide and low, far edge narrow
        // and high, with the grid that makes it read as a surface and not a shape.
        StudioPose::Floor => {
            let far_left = 0.26;
            let far_right = 0.74;
            let far_y = 0.30;
            let near_y = 0.80;
            painter.add(egui::Shape::convex_polygon(
                vec![
                    at(far_left, far_y),
                    at(far_right, far_y),
                    at(1.0, near_y),
                    at(0.0, near_y),
                ],
                lit.gamma_multiply(0.8),
                seam,
            ));
            for step in 1..3 {
                let t = step as f32 / 3.0;
                let y = far_y + (near_y - far_y) * t;
                let left = far_left * (1.0 - t);
                let right = far_right + (1.0 - far_right) * t;
                painter.line_segment([at(left, y), at(right, y)], seam);
            }
            for step in 1..3 {
                let t = step as f32 / 3.0;
                painter.line_segment(
                    [
                        at(far_left + (far_right - far_left) * t, far_y),
                        at(t, near_y),
                    ],
                    seam,
                );
            }
        }
        // The three solid poses share one drawing: a front face divided into its
        // one-metre voxels, plus a top and a right face so the depth is visible —
        // which is the only thing that separates the 1-deep wall from the cube.
        pose => {
            let [columns, rows, layers] = pose.extent();
            let (left, right, top, bottom, depth) = match pose {
                StudioPose::Single => (0.30, 0.60, 0.40, 0.70, 0.22),
                StudioPose::Cube => (0.08, 0.68, 0.24, 0.84, 0.24),
                _ => (0.04, 0.78, 0.16, 0.90, 0.09),
            };
            let lift = depth * 0.78;
            let shift = |x: f32, y: f32| at(x + depth, y - lift);
            painter.add(egui::Shape::convex_polygon(
                vec![
                    at(left, top),
                    at(right, top),
                    shift(right, top),
                    shift(left, top),
                ],
                lit,
                seam,
            ));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    at(right, top),
                    shift(right, top),
                    shift(right, bottom),
                    at(right, bottom),
                ],
                side,
                seam,
            ));
            painter.add(egui::Shape::convex_polygon(
                vec![
                    at(left, top),
                    at(right, top),
                    at(right, bottom),
                    at(left, bottom),
                ],
                front,
                seam,
            ));
            for column in 1..columns {
                let x = left + (right - left) * column as f32 / columns as f32;
                painter.line_segment([at(x, top), at(x, bottom)], seam);
            }
            for row in 1..rows {
                let y = top + (bottom - top) * row as f32 / rows as f32;
                painter.line_segment([at(left, y), at(right, y)], seam);
            }
            // Depth divisions on the top face, so a 3-deep cube is not drawn as a
            // 1-deep slab with a taller lid.
            for layer in 1..layers {
                let t = layer as f32 / layers as f32;
                painter.line_segment(
                    [
                        at(left + depth * t, top - lift * t),
                        at(right + depth * t, top - lift * t),
                    ],
                    seam,
                );
            }
            painter.line_segment([at(0.0, bottom + 0.06), at(1.0, bottom + 0.06)], ground);
        }
    }
    response
}

fn material_swatch(table: &MaterialTable, id: u8) -> egui::Color32 {
    let color = table.row(id).map_or([0.2, 0.2, 0.2], |row| row.albedo);
    egui::Color32::from_rgb(
        (color[0].clamp(0.0, 1.0) * 255.0) as u8,
        (color[1].clamp(0.0, 1.0) * 255.0) as u8,
        (color[2].clamp(0.0, 1.0) * 255.0) as u8,
    )
}

/// The in-world palette: a short bar for common test materials plus a complete
/// picker for the rest. It deliberately shares `MaterialPanelState::selected`
/// with the editor, so bar selection, keyboard selection, eyedropper picks, and
/// placement are one state rather than four subtly different choices.
fn draw_block_hotbar(
    ui: &mut egui::Ui,
    material_table: &MaterialTable,
    material_panel: &mut MaterialPanelState,
    graph_drawer_height: f32,
) {
    egui::Area::new(egui::Id::new("world_block_hotbar"))
        // This is the bottom of the RENDERED viewport, not of the whole
        // window. Graph Studio owns the bottom panel and may resize it, so an
        // absolute "bottom of window" overlay would inevitably cover its
        // nodes; reserve the drawer's exact current height first.
        .anchor(
            egui::Align2::CENTER_BOTTOM,
            egui::vec2(0.0, -(graph_drawer_height + 8.0)),
        )
        .show(ui.ctx(), |ui| {
            egui::Frame::popup(ui.style()).show(ui, |ui| {
                ui.horizontal(|ui| {
                    for (index, voxel) in WORLD_HOTBAR_BLOCKS.iter().enumerate() {
                        let id = material::material_id(*voxel);
                        let name = material_table.row(id).map_or("unknown", |row| row.name);
                        let swatch = material_swatch(material_table, id);
                        let luma = 0.2126 * f32::from(swatch.r())
                            + 0.7152 * f32::from(swatch.g())
                            + 0.0722 * f32::from(swatch.b());
                        let text = if luma > 145.0 {
                            egui::Color32::BLACK
                        } else {
                            egui::Color32::WHITE
                        };
                        let button = egui::Button::new(
                            egui::RichText::new(format!("{}\n{name}", index + 1)).color(text),
                        )
                        .fill(swatch.gamma_multiply(0.82))
                        .stroke(if material_panel.selected == id {
                            egui::Stroke::new(3.0, egui::Color32::from_rgb(255, 226, 92))
                        } else {
                            egui::Stroke::new(1.0, egui::Color32::from_white_alpha(110))
                        });
                        if ui.add_sized(egui::vec2(64.0, 42.0), button).clicked() {
                            material_panel.selected = id;
                        }
                    }
                    let selected_name = material_table
                        .row(material_panel.selected)
                        .map_or("more", |row| row.name);
                    egui::ComboBox::from_id_salt("world_hotbar_more_materials")
                        .selected_text(format!("{selected_name} ▾"))
                        .width(104.0)
                        .show_ui(ui, |ui| {
                            for id in 1..MATERIAL_COUNT as u8 {
                                let name = material_table.row(id).map_or("unknown", |row| row.name);
                                ui.selectable_value(
                                    &mut material_panel.selected,
                                    id,
                                    format!("{id:>2}  {name}"),
                                );
                            }
                        });
                });
                ui.label("Right-click: place selected · Left-click: remove · Middle-click: pick");
            });
        });
}
