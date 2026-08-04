//! egui overlay: stats panel (window/render sizes, moving-average frame-loop
//! FPS, GPU-only FPS, per-pass GPU times), the vsync lever, a collapsible sun-position
//! section, the E1c **Quality** section — preset selector plus every lever
//! grouped by subsystem, each carrying its measured verdict as hover text so
//! "why is this off?" is answerable in-app — and a **Debug tools** section for
//! the actions that change the WORLD rather than how it is drawn. Drawn on top
//! of the rendered frame in its own render pass (LoadOp::Load).
//!
//! The Quality section is generated from [`crate::variants::REGISTRY`]: the
//! widget shape comes from each lever's [`LeverRange`], the hover text from its
//! verdict, and reads/writes go through [`LeverId::read`] / [`LeverId::apply`].
//! Adding a lever row therefore adds its control here automatically.
//!
//! Seam: the overlay only mutates the state it is handed (`vsync_enabled`,
//! [`SunSettings`], [`RenderQuality`]) — reconfiguring the surface, resizing the
//! storage texture, writing the lighting uniform, and switching the pipeline on
//! a compile-time lever change all stay in the platform layer.

use std::collections::{BTreeSet, VecDeque};

use egui::{Event, Key, MouseWheelUnit};
use winit::event::WindowEvent;
use winit::window::Window;

use crate::ao::AoMode;
use crate::character::Submersion;
use crate::frame_timing::FrameTimings;
use crate::lighting::SunSettings;
use crate::material::{self, MATERIAL_COUNT};
use crate::material_edit::{MaterialPanelState, WORLD_HOTBAR_BLOCKS};
use crate::material_graph::{ConnectorDrag, GraphEditorState};
use crate::material_table::MaterialTable;
use crate::shadows::ShadowMode;
use crate::studio_assets::StudioAssetPanelState;
use crate::variants::{
    levers_of, Lever, LeverId, LeverRange, LeverSubsystem, LeverValue, QualityPreset,
    RenderQuality, QUALITY_PRESETS, VOXELS_PER_METER,
};
use crate::water::WaterMode;
use crate::world_edit::ClearanceUpdateMode;
use crate::world_host::WorldEditStats;
use voxel_color::{
    ColorSpaceOutcome, DisplayHeadroom, HeadroomChoice, OutputDepth, OutputSupport, TonemapCurve,
};
use voxel_graph::{
    node_reachability, Cardinality, ConnectionError, FieldDeclarationStatic, FieldTarget,
    GraphAsset, GraphCommand, InputPin, LinkId, NodeCategory, NodeDeclaration, NodeId, NodePreview,
    NodeRecord, NodeRegistry, NodeTypeId, NumericRange, OutputPin, PropertyValue,
    SocketDeclarationStatic, SocketKey, SocketType,
};

const FRAME_TIME_SAMPLE_COUNT: usize = 120;
/// Timestamp readback lands a few frames late, so a moderate window keeps the
/// GPU number readable without hiding a sustained regression.
const GPU_FRAME_TIME_SAMPLE_COUNT: usize = 60;
const FPS_GRAPH_MAX_WIDTH: f32 = 240.0;
const FPS_GRAPH_HEIGHT: f32 = 60.0;
const GRAPH_NODE_CONTROL_ZOOM: f32 = 0.95;

const FRAME_LOOP_GRAPH_COLOR: egui::Color32 = egui::Color32::from_rgb(117, 180, 255);
const GPU_GRAPH_COLOR: egui::Color32 = egui::Color32::from_rgb(110, 220, 155);

fn push_rolling_sample(samples: &mut VecDeque<f32>, capacity: usize, sample: f32) {
    if samples.len() == capacity {
        samples.pop_front();
    }
    samples.push_back(sample);
}

fn rolling_average(samples: &VecDeque<f32>) -> Option<f32> {
    (!samples.is_empty()).then(|| samples.iter().sum::<f32>() / samples.len() as f32)
}

/// A compact history chart without a separate plotting dependency. The source
/// samples remain frame times, so changing the graph never changes the moving
/// averages shown beside it.
fn draw_fps_history(
    ui: &mut egui::Ui,
    frame_time_samples: &VecDeque<f32>,
    gpu_frame_time_samples: &VecDeque<f32>,
) {
    let frame_loop_fps: Vec<f32> = frame_time_samples
        .iter()
        .filter(|seconds| **seconds > 0.0)
        .map(|seconds| 1.0 / seconds)
        .collect();
    let gpu_fps: Vec<f32> = gpu_frame_time_samples
        .iter()
        .filter(|milliseconds| **milliseconds > 0.0)
        .map(|milliseconds| 1_000.0 / milliseconds)
        .collect();
    let peak_fps = frame_loop_fps
        .iter()
        .chain(gpu_fps.iter())
        .copied()
        .fold(60.0_f32, f32::max);
    // Hold a readable 30-FPS grid step for the whole history window rather
    // than rescaling every sample as a line wiggles.
    let graph_ceiling_fps = (peak_fps / 30.0).ceil() * 30.0;

    ui.horizontal(|ui| {
        ui.colored_label(FRAME_LOOP_GRAPH_COLOR, "— frame loop");
        ui.colored_label(GPU_GRAPH_COLOR, "— GPU work");
        ui.label(format!("history · 0–{graph_ceiling_fps:.0} FPS"));
    });
    let (response, painter) = ui.allocate_painter(
        egui::vec2(
            ui.available_width().min(FPS_GRAPH_MAX_WIDTH),
            FPS_GRAPH_HEIGHT,
        ),
        egui::Sense::hover(),
    );
    let rect = response.rect;
    let plot = rect.shrink2(egui::vec2(4.0, 4.0));
    painter.rect_filled(plot, 3.0, ui.visuals().faint_bg_color);

    for fraction in [0.0, 0.5, 1.0] {
        let y = egui::lerp(plot.bottom()..=plot.top(), fraction);
        painter.line_segment(
            [egui::pos2(plot.left(), y), egui::pos2(plot.right(), y)],
            egui::Stroke::new(1.0, ui.visuals().widgets.noninteractive.bg_stroke.color),
        );
    }

    draw_fps_history_line(
        &painter,
        plot,
        &frame_loop_fps,
        graph_ceiling_fps,
        FRAME_LOOP_GRAPH_COLOR,
    );
    draw_fps_history_line(&painter, plot, &gpu_fps, graph_ceiling_fps, GPU_GRAPH_COLOR);
}

fn draw_fps_history_line(
    painter: &egui::Painter,
    plot: egui::Rect,
    samples: &[f32],
    ceiling_fps: f32,
    color: egui::Color32,
) {
    if samples.len() < 2 {
        return;
    }
    let last_index = (samples.len() - 1) as f32;
    let points = samples
        .iter()
        .enumerate()
        .map(|(index, fps)| {
            let x = egui::remap(index as f32, 0.0..=last_index, plot.left()..=plot.right());
            let y = egui::remap_clamp(*fps, 0.0..=ceiling_fps, plot.bottom()..=plot.top());
            egui::pos2(x, y)
        })
        .collect();
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.5, color)));
}

/// Read-only per-frame display data for the stats panel.
pub struct OverlayFrameData {
    /// Storage-texture (ray-traced) resolution, pixels.
    pub render_resolution: (u32, u32),
    /// Latest completed GPU pass timings; `None` when the device has no
    /// timestamp-query support (the panel says so instead of showing numbers).
    pub gpu_timings: Option<FrameTimings>,
    /// E2 — the edit pipeline's live numbers, so the "zero frame hitches" gate is
    /// judgeable in-app and not only in the harness.
    pub world_edit: WorldEditReadout,
    /// E2b — which movement model is driving the view, and what the body is
    /// doing.
    pub movement: MovementReadout,
    /// The editable face currently under the crosshair, projected to the overlay
    /// so the player gets a precise placement preview before editing it.
    pub target: Option<TargetHighlightReadout>,
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
    /// [`crate::water::eye_is_submerged`], which is why it is true in fly mode as
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

pub struct Overlay {
    context: egui::Context,
    winit_state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    /// The surface format the egui pipeline was built against. egui builds its OWN
    /// render pipeline, so it is a consumer of the output format like the blit is —
    /// see [`Overlay::set_surface_format`].
    surface_format: wgpu::TextureFormat,
    frame_time_samples: VecDeque<f32>,
    /// Completed GPU-frame timings only — never frame-loop timing. A rolling
    /// average makes the asynchronous timestamp readback useful at a glance.
    gpu_frame_time_samples: VecDeque<f32>,
    /// `collect` returns its most recent result on frames where no new map has
    /// landed. Keep it from entering the smoothing window more than once.
    last_gpu_frame_sample_sequence: Option<u64>,
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
            frame_time_samples: VecDeque::with_capacity(FRAME_TIME_SAMPLE_COUNT),
            gpu_frame_time_samples: VecDeque::with_capacity(GPU_FRAME_TIME_SAMPLE_COUNT),
            last_gpu_frame_sample_sequence: None,
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

    pub fn record_frame_time(&mut self, frame_time_seconds: f32) {
        push_rolling_sample(
            &mut self.frame_time_samples,
            FRAME_TIME_SAMPLE_COUNT,
            frame_time_seconds,
        );
    }

    fn record_gpu_frame_time(&mut self, timings: Option<FrameTimings>) {
        let Some(timings) = timings else {
            return;
        };
        let Some(gpu_frame_time_milliseconds) = timings.frame_milliseconds() else {
            return;
        };
        if self.last_gpu_frame_sample_sequence == Some(timings.sample_sequence) {
            return;
        }
        self.last_gpu_frame_sample_sequence = Some(timings.sample_sequence);
        push_rolling_sample(
            &mut self.gpu_frame_time_samples,
            GPU_FRAME_TIME_SAMPLE_COUNT,
            gpu_frame_time_milliseconds,
        );
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
    /// already reconfiguring the surface and rebuilding three pipelines. The frame-time
    /// history is carried across deliberately, so the FPS graph does not blank.
    pub fn set_surface_format(
        &mut self,
        window: &Window,
        device: &wgpu::Device,
        surface_format: wgpu::TextureFormat,
    ) {
        if surface_format == self.surface_format {
            return;
        }
        let frame_time_samples = std::mem::take(&mut self.frame_time_samples);
        let gpu_frame_time_samples = std::mem::take(&mut self.gpu_frame_time_samples);
        let last_gpu_frame_sample_sequence = self.last_gpu_frame_sample_sequence;
        *self = Overlay::new(window, device, surface_format);
        self.frame_time_samples = frame_time_samples;
        self.gpu_frame_time_samples = gpu_frame_time_samples;
        self.last_gpu_frame_sample_sequence = last_gpu_frame_sample_sequence;
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
        quality: &mut RenderQuality,
        material_table: &mut MaterialTable,
        material_panel: &mut MaterialPanelState,
        studio_assets: &mut StudioAssetPanelState,
        graph_editor: &mut GraphEditorState,
    ) {
        self.record_gpu_frame_time(frame_data.gpu_timings);
        let average_frame_time_seconds = rolling_average(&self.frame_time_samples).unwrap_or(0.0);
        let frame_time_milliseconds = average_frame_time_seconds * 1000.0;
        // This measures how quickly the application enters its frame function,
        // which often tracks the display while FIFO is active. It is *not*
        // OS-confirmed scanout timing: presentation is asynchronous. GPU FPS
        // below is the separate answer to "how fast can the renderer do this
        // frame's work?".
        let frame_loop_frames_per_second = if average_frame_time_seconds > 0.0 {
            1.0 / average_frame_time_seconds
        } else {
            0.0
        };
        let gpu_frames_per_second = rolling_average(&self.gpu_frame_time_samples)
            .filter(|milliseconds| *milliseconds > 0.0)
            .map(|milliseconds| 1_000.0 / milliseconds);

        // Surface the Retina trap: on macOS the swapchain is PHYSICAL pixels,
        // which can be 4x the logical window area at scale factor 2.0.
        let physical_size = window.inner_size();
        let scale_factor = window.scale_factor();
        let logical_size = physical_size.to_logical::<f64>(scale_factor);

        let raw_input = self.winit_state.take_egui_input(window);
        let full_output = self.context.run_ui(raw_input, |root_ui| {
            draw_graph_drawer(root_ui, graph_editor, material_table);
            draw_target_highlight(root_ui, frame_data.target.as_ref(), material_table);
            let drawer_height = graph_drawer_height(root_ui, graph_editor);
            draw_block_hotbar(
                root_ui,
                material_table,
                material_panel,
                drawer_height,
            );
            egui::Area::new(egui::Id::new("fps_overlay"))
                .anchor(egui::Align2::LEFT_TOP, egui::vec2(8.0, 8.0))
                .show(root_ui.ctx(), |ui| {
                    egui::Frame::popup(ui.style()).show(ui, |ui| {
                        ui.label(format!(
                            "window {:.0} x {:.0} @ {scale_factor:.2}x",
                            logical_size.width, logical_size.height
                        ));
                        ui.label(format!(
                            "physical {} x {}",
                            physical_size.width, physical_size.height
                        ));
                        ui.label(format!(
                            "render {} x {}",
                            frame_data.render_resolution.0, frame_data.render_resolution.1
                        ));
                        ui.label(format!(
                            "frame loop {frame_time_milliseconds:.2} ms  |  {frame_loop_frames_per_second:.0} FPS"
                        ));
                        match gpu_frames_per_second {
                            Some(frames_per_second) => {
                                ui.label(format!(
                                    "GPU work (60-sample avg)  |  {frames_per_second:.0} FPS"
                                ))
                                    .on_hover_text(
                                        "The measured DDA + CAGI + blit/UI GPU work, excluding \
                                         swapchain acquisition and presentation. This shows renderer \
                                         throughput even when macOS paces a window to its display.",
                                    );
                            }
                            None => {
                                ui.label(if frame_data.gpu_timings.is_some() {
                                    "GPU work  |  waiting for timestamp data"
                                } else {
                                    "GPU work  |  timestamp queries unavailable"
                                });
                            }
                        }
                        draw_fps_history(
                            ui,
                            &self.frame_time_samples,
                            &self.gpu_frame_time_samples,
                        );
                        match &frame_data.gpu_timings {
                            Some(timings) => {
                                ui.label(format!(
                                    "DDA pass: {}",
                                    format_pass_milliseconds(timings.dda_milliseconds())
                                ));
                                ui.label(format!(
                                    "CAGI: {}",
                                    format_pass_milliseconds(timings.cagi_milliseconds())
                                ));
                                ui.label(format!(
                                    "blit+ui: {}",
                                    format_pass_milliseconds(timings.post_milliseconds())
                                ));
                            }
                            None => {
                                ui.label("GPU pass timers unavailable");
                            }
                        }
                        let world_edit = &frame_data.world_edit;
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
                        draw_movement_readout(ui, &frame_data.movement);
                        ui.checkbox(vsync_enabled, "VSync");
                        ui.label(if *vsync_enabled {
                            "VSync on: Frame loop normally tracks the display."
                        } else {
                            "VSync off: Compare GPU FPS; the frame loop can run ahead of the GPU."
                        });
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
                        draw_studio_assets_section(ui, studio_assets);
                        draw_quality_section(ui, quality);
                        ui.label("Material authoring is defined by nodes in Graph Studio.");
                        ui.collapsing("Sun", |ui| {
                            ui.checkbox(&mut sun_settings.day_night_enabled, "day/night sky");
                            if sun_settings.day_night_enabled {
                                ui.horizontal(|ui| {
                                    ui.checkbox(&mut sun_settings.cycle_running, "run clock");
                                    ui.label(sun_settings.clock_label());
                                });
                                ui.add(
                                    egui::Slider::new(&mut sun_settings.day_phase, 0.0..=1.0)
                                        .text("time of day"),
                                );
                                ui.add(
                                    egui::Slider::new(
                                        &mut sun_settings.day_length_seconds,
                                        30.0..=1_200.0,
                                    )
                                    .logarithmic(true)
                                    .text("seconds per day"),
                                );
                                ui.add(
                                    egui::Slider::new(&mut sun_settings.moon_phase, 0.0..=1.0)
                                        .text("moon phase"),
                                );
                                ui.add(
                                    egui::Slider::new(
                                        &mut sun_settings.azimuth_degrees,
                                        0.0..=360.0,
                                    )
                                    .text("noon azimuth"),
                                );
                                ui.add(
                                    egui::Slider::new(
                                        &mut sun_settings.elevation_degrees,
                                        2.0..=90.0,
                                    )
                                    .text("noon elevation"),
                                );
                            } else {
                                ui.add(
                                    egui::Slider::new(
                                        &mut sun_settings.azimuth_degrees,
                                        0.0..=360.0,
                                    )
                                    .text("azimuth"),
                                );
                                ui.add(
                                    egui::Slider::new(
                                        &mut sun_settings.elevation_degrees,
                                        2.0..=90.0,
                                    )
                                    .text("elevation"),
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
                        });
                    });
                });
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

/// Keep overlay controls aligned with the same panel geometry that reserves the
/// Graph Studio drawer. `draw_graph_drawer` clamps expanded panels to 80% of
/// the window, so the palette must use that clamped value too.
fn graph_drawer_height(ui: &egui::Ui, state: &GraphEditorState) -> f32 {
    if state.visible {
        let maximum = (ui.ctx().viewport_rect().height() * 0.8).max(320.0);
        state.drawer_height.clamp(120.0, maximum)
    } else {
        34.0
    }
}

/// E2b — the movement readout: which model is driving the view, the key that
/// switches it, and (in walk mode) the body's state plus what its collision step
/// costs the frame thread.
fn draw_movement_readout(ui: &mut egui::Ui, movement: &MovementReadout) {
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
fn draw_studio_assets_section(ui: &mut egui::Ui, state: &mut StudioAssetPanelState) {
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

/// Blender-style Graph Studio split editor. Unlike an anchored `Area`, a
/// `TopBottomPanel` reserves layout space, so the node editor and the rendered
/// viewport are separate regions rather than one floating overlay covering the
/// other.
fn draw_graph_drawer(
    ui: &mut egui::Ui,
    state: &mut GraphEditorState,
    material_table: &MaterialTable,
) {
    let max_height = (ui.ctx().viewport_rect().height() * 0.8).max(320.0);
    let mut expanded = state.visible;
    let mut toggle_requested = false;
    // The v2 ids intentionally discard the oversized panel state created by
    // the first drawer implementation. The compact default should apply on
    // the next launch instead of being masked by egui's persisted PanelState.
    let collapsed_panel = egui::Panel::bottom(egui::Id::new("graph_studio_collapsed_v3"))
        .resizable(false)
        .exact_size(34.0);
    let expanded_panel = egui::Panel::bottom(egui::Id::new("graph_studio_expanded_v3"))
        .resizable(false)
        .exact_size(state.drawer_height.clamp(120.0, max_height));
    let _panel = egui::Panel::show_switched(
        ui,
        &mut expanded,
        collapsed_panel,
        expanded_panel,
        |ui, is_expanded| {
            ui.horizontal(|ui| {
                let toggle_label = if is_expanded {
                    "▾ Graph Studio"
                } else {
                    "▸ Graph Studio"
                };
                if ui.button(toggle_label).clicked() {
                    toggle_requested = true;
                }
                ui.label(format!("Material {:02}", state.material_slot));
                if !is_expanded {
                    ui.label("click to open node editor");
                }
                ui.separator();
                ui.label(&state.status);
            });
            if is_expanded {
                let handle_rect = egui::Rect::from_min_size(
                    ui.cursor().min,
                    egui::vec2(ui.available_width(), 8.0),
                );
                let handle_response = ui.interact(
                    handle_rect,
                    egui::Id::new("graph_studio_resize_handle_v3"),
                    egui::Sense::click_and_drag(),
                );
                let pointer_y = ui.input(|input| input.pointer.hover_pos().map(|pos| pos.y));
                if handle_response.drag_started() {
                    state.drawer_resize_last_y = pointer_y;
                }
                if handle_response.dragged() {
                    if let (Some(previous_y), Some(current_y)) =
                        (state.drawer_resize_last_y, pointer_y)
                    {
                        let delta = current_y - previous_y;
                        state.drawer_height =
                            (state.drawer_height - delta).clamp(120.0, max_height);
                        state.drawer_resize_last_y = Some(current_y);
                        ui.ctx().request_repaint();
                    }
                }
                if handle_response.drag_stopped() {
                    state.drawer_resize_last_y = None;
                }
                ui.painter().rect_filled(
                    handle_rect,
                    0.0,
                    if handle_response.hovered() || handle_response.dragged() {
                        egui::Color32::from_rgb(145, 190, 235)
                    } else {
                        egui::Color32::from_rgb(75, 84, 100)
                    },
                );
                ui.advance_cursor_after_rect(handle_rect);
                ui.separator();
                draw_graph_section_contents(ui, state, material_table);
            }
        },
    );
    if toggle_requested {
        expanded = !expanded;
    }
    state.visible = expanded;
}

/// First Graph Studio canvas. It intentionally uses egui primitives rather
/// than making a widget library the owner of graph semantics: every edit still
/// goes through `GraphCommand` and `GraphHistory`.
fn draw_graph_section_contents(
    ui: &mut egui::Ui,
    state: &mut GraphEditorState,
    material_table: &MaterialTable,
) {
    let registry = crate::graph::CATALOGUE;
    // Scope the dwell to Graph Studio: sockets sit a few pixels apart, so
    // egui's default instant tooltip strobes while a connector is dragged
    // across a node. The delay is read from the GLOBAL style when a tooltip is
    // decided, so it is set for the canvas and restored when it is done.
    let previous_tooltip_delay = ui.ctx().global_style().interaction.tooltip_delay;
    ui.ctx().all_styles_mut(|style| {
        style.interaction.tooltip_delay = GRAPH_TOOLTIP_DELAY_SECONDS;
    });
    let previous_material_slot = state.material_slot;
    ui.horizontal_wrapped(|ui| {
        if ui.button("Undo").clicked() {
            state.undo(&registry);
        }
        if ui.button("Redo").clicked() {
            state.redo(&registry);
        }
        if ui.button("Open").clicked() {
            state.open_requested = true;
        }
        let graph_valid = !state
            .diagnostics
            .iter()
            .any(|diagnostic| diagnostic.severity == voxel_graph::DiagnosticSeverity::Error);
        if ui
            .add_enabled(graph_valid, egui::Button::new("Save"))
            .on_disabled_hover_text("Resolve graph errors before saving")
            .clicked()
        {
            state.save_requested = true;
        }
        if ui.button("Duplicate").clicked() {
            state.duplicate_requested = true;
        }
        if ui.button("Copy").clicked() {
            state.copy_selected();
        }
        if ui
            .add_enabled(state.can_paste(), egui::Button::new("Paste"))
            .clicked()
        {
            state.paste_clipboard(&registry);
        }
        if ui.button("Reset").clicked() {
            state.reset_requested = true;
        }
        if ui.button("Frame all").clicked() {
            state.frame_all_requested = true;
        }
        if ui
            .add_enabled(
                !state.selected_nodes.is_empty(),
                egui::Button::new("Frame selected"),
            )
            .clicked()
        {
            state.frame_selection_requested = true;
        }
        ui.separator();
        ui.label("material");
        egui::ComboBox::from_id_salt("graph-material-slot")
            .selected_text(graph_material_label(material_table, state.material_slot))
            .show_ui(ui, |ui| {
                for slot in 0..material_table.rows().len() as u8 {
                    ui.selectable_value(
                        &mut state.material_slot,
                        slot,
                        graph_material_label(material_table, slot),
                    );
                }
            });
    });
    if state.material_slot != previous_material_slot {
        state.material_select_requested = Some(state.material_slot);
        state.status = format!(
            "Selected {} — loading graph",
            graph_material_label(material_table, state.material_slot)
        );
    }
    ui.horizontal(|ui| {
        ui.label("add");
        ui.add(
            egui::TextEdit::singleline(&mut state.search)
                .desired_width(150.0)
                .hint_text("search nodes"),
        );
        let visible_nodes = state.visible_node_types(&registry);
        // The palette reads as a list of NODES, not of registry keys: the title
        // leads, the stable id stays as dim secondary text for the times you
        // need it, and the authored description is one hover away.
        let selected_title = registry
            .find(&NodeTypeId(state.node_type.clone()))
            .map_or_else(|| state.node_type.clone(), |node| node.title.to_string());
        egui::ComboBox::from_id_salt("graph-node-type")
            .selected_text(selected_title)
            .show_ui(ui, |ui| {
                for category in NodeCategory::ALL {
                    let category_nodes = visible_nodes
                        .iter()
                        .copied()
                        .filter(|node| node.category == *category);
                    let mut category_nodes = category_nodes.peekable();
                    if category_nodes.peek().is_none() {
                        continue;
                    }
                    ui.separator();
                    ui.label(egui::RichText::new(category.label()).strong());
                    for node in category_nodes {
                        let row_text = graph_palette_row_text(ui, node.title, node.id);
                        ui.selectable_value(&mut state.node_type, node.id.to_string(), row_text)
                            .on_hover_text(node.description);
                    }
                }
            });
        let selected_node_type = NodeTypeId(state.node_type.clone());
        let can_add_selected = state
            .graph
            .can_add_node_type(&registry, &selected_node_type);
        if ui
            .add_enabled(can_add_selected, egui::Button::new("Add node"))
            .on_disabled_hover_text("This graph already contains the maximum number of this node")
            .clicked()
        {
            state.add_node(NodeTypeId(state.node_type.clone()), &registry);
        }
        if let Some(layer) = registry.declarations().iter().find(|declaration| {
            declaration.kinds.contains(&state.graph.kind)
                && declaration.operation
                    == crate::graph::NodeOperation::Material(
                        crate::graph::MaterialNodeOperation::PatternLayer,
                    )
                    .tag()
        }) {
            let can_add_layer = state
                .graph
                .can_add_node_type(&registry, &NodeTypeId(layer.id.into()));
            if ui
                .add_enabled(
                    can_add_layer,
                    egui::Button::new(format!("Add {}", layer.title)),
                )
                .clicked()
            {
                state.add_node(NodeTypeId(layer.id.to_string()), &registry);
            }
        }
    });

    // Let the canvas consume the panel's remaining height. A fixed canvas
    // height would become the panel's implicit minimum and make the top edge
    // appear stuck when the user tries to resize the drawer smaller.
    let canvas_height = ui.available_height().max(1.0);
    let canvas_size = egui::vec2(ui.available_width(), canvas_height);
    // Register the canvas as a click-only input boundary. It deliberately does
    // not own a drag gesture, so node headers and middle-button navigation keep
    // their custom behavior, but it does tell the platform that the pointer is
    // inside Graph Studio and must not drive the rendered viewport.
    let (canvas_rect, canvas_response) = ui.allocate_exact_size(canvas_size, egui::Sense::click());
    let painter = ui.painter_at(canvas_rect);
    painter.rect_filled(canvas_rect, 4.0, egui::Color32::from_rgb(24, 27, 34));
    painter.rect_stroke(
        canvas_rect,
        4.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(70, 76, 88)),
        egui::StrokeKind::Outside,
    );
    // `contains_pointer`, not `hovered`: node bodies and sockets are interactive
    // widgets stacked on top of this rectangle, and egui marks only the topmost
    // interactive hit as hovered. Panning and zooming must keep working with
    // the pointer over a node, so ask the question that overlap does not answer
    // away.
    let canvas_hovered = canvas_response.contains_pointer();
    let (
        pointer_position,
        middle_down,
        scroll_delta,
        zoom_delta,
        translation_delta,
        has_multi_touch,
        has_point_wheel,
        shift_down,
    ) = ui.input(|input| {
        let has_point_wheel = input.raw.events.iter().any(|event| {
            matches!(
                event,
                Event::MouseWheel {
                    unit: MouseWheelUnit::Point,
                    ..
                }
            )
        });
        (
            input.pointer.hover_pos(),
            input.pointer.middle_down(),
            input.smooth_scroll_delta(),
            input.zoom_delta(),
            input.translation_delta(),
            input.multi_touch().is_some(),
            has_point_wheel,
            input.modifiers.shift,
        )
    });
    let (delete_pressed, copy_pressed, paste_pressed, command_modifier) = ui.input(|input| {
        (
            input.key_pressed(Key::Backspace) || input.key_pressed(Key::Delete),
            input.key_pressed(Key::C),
            input.key_pressed(Key::V),
            input.modifiers.command || input.modifiers.ctrl,
        )
    });
    if !ui.ctx().egui_wants_keyboard_input() && command_modifier {
        if copy_pressed {
            state.copy_selected();
        }
        if paste_pressed {
            state.paste_clipboard(&registry);
        }
    }
    if canvas_hovered
        && delete_pressed
        && !ui.ctx().egui_wants_keyboard_input()
        && (!state.selected_nodes.is_empty() || state.selected_node.is_some())
    {
        let mut nodes = state.selected_nodes.iter().cloned().collect::<Vec<_>>();
        if nodes.is_empty() {
            nodes.push(state.selected_node.clone().expect("checked above"));
        }
        state.remove_nodes(nodes.clone(), &registry);
        state.selected_nodes.clear();
        state.selected_node = None;
        state.status = format!(
            "{} node{} deleted",
            nodes.len(),
            if nodes.len() == 1 { "" } else { "s" }
        );
    }

    // Middle-button panning is latched at the point where the gesture starts.
    // Relying on `canvas_response.hovered()` for every frame made the grab feel
    // sticky when the cursor crossed a node, separator, or the canvas edge.
    if !middle_down {
        state.canvas_middle_pan_active = false;
        state.canvas_middle_pan_last_pointer = None;
    } else if !state.canvas_middle_pan_active && canvas_hovered {
        state.canvas_middle_pan_active = true;
        state.canvas_middle_pan_last_pointer =
            pointer_position.map(|pointer| [pointer.x, pointer.y]);
    }

    let mut navigation_changed = false;
    if state.canvas_middle_pan_active {
        if let Some(pointer) = pointer_position {
            let current = [pointer.x, pointer.y];
            if let Some(previous) = state.canvas_middle_pan_last_pointer {
                let delta = [current[0] - previous[0], current[1] - previous[1]];
                if delta[0] != 0.0 || delta[1] != 0.0 {
                    state.pan[0] += delta[0];
                    state.pan[1] += delta[1];
                    navigation_changed = true;
                }
            }
            state.canvas_middle_pan_last_pointer = Some(current);
        }
    }

    if canvas_hovered {
        // A two-finger trackpad gesture arrives either as egui multi-touch or
        // as a point-unit wheel event. Both represent content translation. A
        // shifted wheel is also a useful explicit pan fallback on a mouse.
        let trackpad_pan = if has_multi_touch {
            translation_delta
        } else if has_point_wheel || shift_down {
            scroll_delta
        } else {
            egui::Vec2::ZERO
        };
        if trackpad_pan != egui::Vec2::ZERO {
            state.pan[0] += trackpad_pan.x;
            state.pan[1] += trackpad_pan.y;
            navigation_changed = true;
        }

        // Pinch/ctrl-scroll zoom and ordinary mouse-wheel zoom are anchored at
        // the pointer, so the graph under the cursor stays in place. Point-unit
        // trackpad scrolling is excluded here because it is used for panning.
        let mut zoom_factor = zoom_delta;
        if !has_multi_touch && !has_point_wheel && !shift_down {
            // egui's wheel delta follows content movement: positive Y means
            // scrolling down, which should zoom out in a node editor.
            zoom_factor *= (-scroll_delta.y * 0.0015).exp();
        }
        if zoom_factor.is_finite() && (zoom_factor - 1.0).abs() > f32::EPSILON {
            let old_zoom = state.zoom;
            let new_zoom = (old_zoom * zoom_factor).clamp(0.45, 2.5);
            if (new_zoom - old_zoom).abs() > f32::EPSILON {
                let pointer = pointer_position.unwrap_or(canvas_rect.center());
                let pointer_in_canvas = pointer - canvas_rect.min;
                let graph_at_pointer =
                    (pointer_in_canvas - egui::vec2(state.pan[0], state.pan[1])) / old_zoom;
                let new_pan = pointer_in_canvas - graph_at_pointer * new_zoom;
                state.zoom = new_zoom;
                state.pan = [new_pan.x, new_pan.y];
                navigation_changed = true;
            }
        }
    }
    if navigation_changed {
        ui.ctx().request_repaint();
    }

    let pan = state.pan;
    let zoom = state.zoom;
    let to_screen = |position: [f32; 2]| {
        egui::pos2(
            canvas_rect.left() + pan[0] + position[0] * zoom,
            canvas_rect.top() + pan[1] + position[1] * zoom,
        )
    };

    // Blender's node editor uses a quiet, regular grid rather than a framed
    // list. Major lines every five cells make panning and spacing legible.
    let grid_step = 24.0 * zoom;
    if grid_step > 1.0 {
        let x_offset = pan[0].rem_euclid(grid_step);
        let y_offset = pan[1].rem_euclid(grid_step);
        let mut x = canvas_rect.left() + x_offset;
        let mut column = 0;
        while x <= canvas_rect.right() {
            let major = column % 5 == 0;
            painter.line_segment(
                [
                    egui::pos2(x, canvas_rect.top()),
                    egui::pos2(x, canvas_rect.bottom()),
                ],
                egui::Stroke::new(
                    if major { 1.0 } else { 0.5 },
                    if major {
                        egui::Color32::from_rgb(45, 49, 57)
                    } else {
                        egui::Color32::from_rgb(33, 37, 44)
                    },
                ),
            );
            x += grid_step;
            column += 1;
        }
        let mut y = canvas_rect.top() + y_offset;
        let mut row = 0;
        while y <= canvas_rect.bottom() {
            let major = row % 5 == 0;
            painter.line_segment(
                [
                    egui::pos2(canvas_rect.left(), y),
                    egui::pos2(canvas_rect.right(), y),
                ],
                egui::Stroke::new(
                    if major { 1.0 } else { 0.5 },
                    if major {
                        egui::Color32::from_rgb(45, 49, 57)
                    } else {
                        egui::Color32::from_rgb(33, 37, 44)
                    },
                ),
            );
            y += grid_step;
            row += 1;
        }
    }

    let mut visuals = Vec::new();
    for node_id in state.graph.nodes.keys().cloned() {
        let Some(record) = state.graph.nodes.get(&node_id).cloned() else {
            continue;
        };
        let Some(declaration) = registry.find(&record.node_type) else {
            continue;
        };
        let position = *state
            .graph
            .layout
            .positions
            .get(&node_id)
            .unwrap_or(&[0.0, 0.0]);
        let socket_rows = declaration.inputs.len().max(declaration.outputs.len());
        let property_rows = declaration
            .fields
            .iter()
            .filter(|field| field.target == FieldTarget::Property)
            .count();
        let collapsed = state.collapsed_nodes.contains(&node_id);
        let rows = if collapsed {
            socket_rows.max(1)
        } else {
            socket_rows.saturating_add(property_rows).max(1)
        };
        let preview_height = if collapsed {
            0.0
        } else {
            graph_node_preview_height(&record, declaration)
        };
        let node_size = egui::vec2(
            204.0 * zoom,
            (28.0 + 8.0 + preview_height + rows as f32 * 20.0 + 8.0) * zoom,
        );
        visuals.push(GraphNodeVisual {
            id: node_id,
            record,
            declaration,
            preview_height,
            rect: egui::Rect::from_min_size(to_screen(position), node_size),
        });
    }

    if (state.frame_all_requested || state.frame_selection_requested) && !visuals.is_empty() {
        let frame_ids = if state.frame_selection_requested && !state.selected_nodes.is_empty() {
            state.selected_nodes.iter().collect::<Vec<_>>()
        } else {
            visuals.iter().map(|visual| &visual.id).collect::<Vec<_>>()
        };
        let mut min = egui::pos2(f32::INFINITY, f32::INFINITY);
        let mut max = egui::pos2(f32::NEG_INFINITY, f32::NEG_INFINITY);
        for visual in visuals
            .iter()
            .filter(|visual| frame_ids.contains(&&visual.id))
        {
            let position = state
                .graph
                .layout
                .positions
                .get(&visual.id)
                .copied()
                .unwrap_or([0.0, 0.0]);
            min.x = min.x.min(position[0]);
            min.y = min.y.min(position[1]);
            max.x = max.x.max(position[0] + visual.rect.width() / zoom);
            max.y = max.y.max(position[1] + visual.rect.height() / zoom);
        }
        let width = (max.x - min.x).max(1.0);
        let height = (max.y - min.y).max(1.0);
        state.zoom = ((canvas_rect.width() - 40.0) / width)
            .min((canvas_rect.height() - 40.0) / height)
            .clamp(0.45, 2.5);
        let center = egui::pos2((min.x + max.x) * 0.5, (min.y + max.y) * 0.5);
        let canvas_center = canvas_rect.center() - canvas_rect.min;
        state.pan = [
            canvas_center.x - center.x * state.zoom,
            canvas_center.y - center.y * state.zoom,
        ];
        state.frame_all_requested = false;
        state.frame_selection_requested = false;
        ui.ctx().request_repaint();
    }

    // Draw links first, so the nodes sit above the wires just like Blender.
    // Moving a saturated socket temporarily hides its old wire. The command is
    // only committed on release, so Escape still restores the original graph.
    let detached_link_ids = state
        .connector_drag
        .as_ref()
        .map(|drag| graph_connector_detached_link_ids(drag, &visuals, &state.graph))
        .unwrap_or_default();
    for (link_id, link) in &state.graph.links {
        if detached_link_ids.contains(link_id) {
            continue;
        }
        let Some(source) = visuals.iter().find(|node| node.id == link.from.node) else {
            continue;
        };
        let Some(destination) = visuals.iter().find(|node| node.id == link.to.node) else {
            continue;
        };
        let Some(source_index) = source
            .declaration
            .outputs
            .iter()
            .position(|socket| socket.key == link.from.socket.0)
        else {
            continue;
        };
        let Some(destination_index) = destination
            .declaration
            .inputs
            .iter()
            .position(|socket| socket.key == link.to.socket.0)
        else {
            continue;
        };
        let from = graph_socket_position(
            source.rect,
            false,
            source_index,
            zoom,
            source.preview_height,
        );
        let to = graph_socket_position(
            destination.rect,
            true,
            destination_index,
            zoom,
            destination.preview_height,
        );
        let socket_type = source.declaration.outputs[source_index].value_type;
        draw_graph_wire(&painter, from, to, graph_socket_color(socket_type));
    }

    // While a connector is being dragged, keep a live wire under the pointer.
    // Only a socket with spare capacity can become an add-node operation. A
    // saturated socket is moving its existing wire and therefore has no `+`.
    if state.connector_menu_position.is_none() {
        if let (Some(drag), Some(pointer)) = (&state.connector_drag, pointer_position) {
            if let Some(origin) = graph_connector_origin(drag, &visuals, zoom) {
                let source_socket = graph_connector_source_socket(drag, &visuals);
                let color = source_socket
                    .map(|socket| graph_socket_color(socket.value_type))
                    .unwrap_or(egui::Color32::from_gray(210));
                match drag {
                    ConnectorDrag::FromOutput(_) => {
                        draw_graph_wire(&painter, origin, pointer, color)
                    }
                    ConnectorDrag::FromInput(_) => {
                        draw_graph_wire(&painter, pointer, origin, color)
                    }
                }
                painter.circle_stroke(pointer, 9.0 * zoom.max(0.75), egui::Stroke::new(1.5, color));
                if detached_link_ids.is_empty() {
                    painter.text(
                        pointer + egui::vec2(11.0, -13.0),
                        egui::Align2::LEFT_TOP,
                        "+",
                        egui::FontId::proportional(20.0),
                        egui::Color32::WHITE,
                    );
                }
            }
        }
    }

    // The node bodies are custom-painted rectangles rather than egui widgets.
    // Track their drag gesture explicitly so the interaction remains reliable
    // even when a node overlaps a socket or the pointer leaves the canvas.
    let pointer_position = ui.input(|input| input.pointer.hover_pos());
    let primary_pressed = ui.input(|input| input.pointer.primary_pressed());
    let primary_down = ui.input(|input| input.pointer.primary_down());
    let primary_released = ui.input(|input| input.pointer.primary_released());
    let selection_modifier =
        ui.input(|input| input.modifiers.shift || input.modifiers.command || input.modifiers.ctrl);
    if state.dragging_node.is_none() && state.box_select_start.is_none() && primary_pressed {
        if let Some(pointer_position) = pointer_position {
            if let Some(visual) = visuals
                .iter()
                .rev()
                .find(|visual| visual.rect.contains(pointer_position))
            {
                let was_selected = state.selected_nodes.contains(&visual.id);
                if selection_modifier {
                    if was_selected {
                        state.selected_nodes.remove(&visual.id);
                    } else {
                        state.selected_nodes.insert(visual.id.clone());
                    }
                } else if !was_selected {
                    state.selected_nodes.clear();
                    state.selected_nodes.insert(visual.id.clone());
                }
                state.selected_node = if state.selected_nodes.contains(&visual.id) {
                    Some(visual.id.clone())
                } else {
                    state.selected_nodes.iter().next().cloned()
                };
                if graph_visual_header_hit(visual, pointer_position, zoom) {
                    if pointer_position.x <= visual.rect.left() + 28.0 * zoom {
                        if !state.collapsed_nodes.insert(visual.id.clone()) {
                            state.collapsed_nodes.remove(&visual.id);
                        }
                        state.status = if state.collapsed_nodes.contains(&visual.id) {
                            "Node collapsed".to_string()
                        } else {
                            "Node expanded".to_string()
                        };
                    } else if !state.selected_nodes.is_empty() {
                        state.dragging_node = Some(visual.id.clone());
                        state.drag_pointer_start = Some([pointer_position.x, pointer_position.y]);
                        state.drag_start_positions = state
                            .selected_nodes
                            .iter()
                            .map(|id| {
                                (
                                    id.clone(),
                                    *state.graph.layout.positions.get(id).unwrap_or(&[0.0, 0.0]),
                                )
                            })
                            .collect();
                        state.status = format!("Dragging {}", visual.declaration.title);
                    }
                }
            } else if canvas_hovered && state.connector_drag.is_none() {
                state.box_select_start = Some([pointer_position.x, pointer_position.y]);
                state.box_select_current = Some([pointer_position.x, pointer_position.y]);
                if !selection_modifier {
                    state.selected_nodes.clear();
                    state.selected_node = None;
                }
            }
        }
    }
    if let Some(start) = state.box_select_start {
        if primary_down {
            if let Some(pointer) = pointer_position {
                state.box_select_current = Some([pointer.x, pointer.y]);
                ui.ctx().request_repaint();
            }
        }
        if primary_released || !primary_down {
            let end = state.box_select_current.unwrap_or(start);
            let selection_rect = egui::Rect::from_two_pos(
                egui::pos2(start[0], start[1]),
                egui::pos2(end[0], end[1]),
            );
            let mut boxed = Vec::new();
            for visual in &visuals {
                if selection_rect.intersects(visual.rect) {
                    boxed.push(visual.id.clone());
                }
            }
            if selection_modifier {
                state.selected_nodes.extend(boxed);
            } else {
                state.selected_nodes = boxed.into_iter().collect();
            }
            state.selected_node = state.selected_nodes.iter().next().cloned();
            state.box_select_start = None;
            state.box_select_current = None;
        }
    }
    if let Some(dragging_node) = state.dragging_node.clone() {
        if primary_down {
            if let (Some(pointer_position), Some(pointer_start)) =
                (pointer_position, state.drag_pointer_start)
            {
                if state.drag_start_positions.contains_key(&dragging_node) {
                    let delta = [
                        (pointer_position.x - pointer_start[0]) / zoom,
                        (pointer_position.y - pointer_start[1]) / zoom,
                    ];
                    for (id, start) in state.drag_start_positions.clone() {
                        state
                            .graph
                            .layout
                            .positions
                            .insert(id, [start[0] + delta[0], start[1] + delta[1]]);
                    }
                    ui.ctx().request_repaint();
                }
            }
        }
        if primary_released || !primary_down {
            if !state.drag_start_positions.is_empty() {
                let starts = std::mem::take(&mut state.drag_start_positions);
                let positions = starts
                    .iter()
                    .map(|(id, start)| {
                        let end = *state.graph.layout.positions.get(id).unwrap_or(start);
                        state.graph.layout.positions.insert(id.clone(), *start);
                        (id.clone(), end)
                    })
                    .collect();
                state.apply(GraphCommand::MoveNodes { positions }, &registry);
            }
            state.dragging_node = None;
            state.drag_pointer_start = None;
        }
    }

    let hovered_socket =
        pointer_position.and_then(|pointer| graph_socket_hit_at_pointer(&visuals, pointer, zoom));
    // Derived once per frame and shared by every tooltip and context menu: a
    // node wired to nothing is visually identical to a working one, so this is
    // the only thing that can tell them apart.
    let reachable = node_reachability(&state.graph, &registry);
    for visual in &visuals {
        let node_id = &visual.id;
        let rect = visual.rect;
        let header_height = 28.0 * zoom;
        let row_height = 20.0 * zoom;
        let socket_rows = visual
            .declaration
            .inputs
            .len()
            .max(visual.declaration.outputs.len());
        let selected = state.selected_nodes.contains(node_id);
        let collapsed = state.collapsed_nodes.contains(node_id);
        painter.rect_filled(rect, 6.0, egui::Color32::from_rgb(40, 42, 47));
        let header_rect = egui::Rect::from_min_max(
            rect.left_top(),
            egui::pos2(rect.right(), rect.top() + header_height),
        );
        painter.rect_filled(
            header_rect,
            6.0,
            graph_node_header_color(visual.declaration),
        );
        painter.rect_stroke(
            rect,
            6.0,
            egui::Stroke::new(
                if selected { 1.5 } else { 1.0 },
                if selected {
                    egui::Color32::from_rgb(220, 235, 255)
                } else {
                    egui::Color32::from_rgb(18, 19, 22)
                },
            ),
            egui::StrokeKind::Outside,
        );
        painter.text(
            rect.left_top() + egui::vec2(9.0 * zoom, 6.0 * zoom),
            egui::Align2::LEFT_TOP,
            format!(
                "{} {}",
                if collapsed { "▸" } else { "▾" },
                visual.declaration.title
            ),
            egui::FontId::proportional(12.0 * zoom.max(0.75)),
            egui::Color32::WHITE,
        );
        painter.circle_filled(
            rect.right_top() + egui::vec2(-12.0 * zoom, 14.0 * zoom),
            4.0 * zoom.max(0.75),
            egui::Color32::from_rgb(220, 220, 220),
        );

        // Registered BEFORE the sockets so the socket hit areas, which sit on
        // this same rectangle's edges, stay on top of it. The custom drag and
        // selection gestures read raw pointer input and are unaffected.
        let node_response = ui.interact(
            rect,
            egui::Id::new(("graph-node-body", node_id.0.as_str())),
            egui::Sense::click(),
        );
        node_response.clone().on_hover_ui(|hover_ui| {
            let tooltip = graph_node_tooltip(
                &state.graph,
                &reachable,
                node_id,
                visual.declaration,
                &visual.record,
            );
            graph_tooltip_body(hover_ui, &tooltip);
        });
        node_response.context_menu(|menu_ui| {
            menu_ui.set_min_width(240.0);
            menu_ui.label(
                egui::RichText::new(format!(
                    "{} · {}",
                    visual.declaration.title,
                    visual.declaration.category.label()
                ))
                .strong(),
            );
            graph_tooltip_prose(menu_ui, visual.declaration.description, None);
            let reaches_output = reachable.contains(node_id);
            menu_ui.colored_label(
                if reaches_output {
                    GRAPH_TOOLTIP_GOOD_COLOR
                } else {
                    GRAPH_TOOLTIP_BAD_COLOR
                },
                if reaches_output {
                    "Reaches Material Output"
                } else {
                    "Does not reach Material Output"
                },
            );
            menu_ui.separator();
            if menu_ui
                .button(if collapsed { "Expand" } else { "Collapse" })
                .clicked()
            {
                if !state.collapsed_nodes.insert(node_id.clone()) {
                    state.collapsed_nodes.remove(node_id);
                }
                menu_ui.close();
            }
            if menu_ui.button("Duplicate").clicked() {
                state.selected_nodes.clear();
                state.selected_nodes.insert(node_id.clone());
                state.selected_node = Some(node_id.clone());
                state.duplicate_requested = true;
                menu_ui.close();
            }
            if menu_ui.button("Delete").clicked() {
                state.remove_nodes(vec![node_id.clone()], &registry);
                state.selected_nodes.remove(node_id);
                state.selected_node = state.selected_nodes.iter().next().cloned();
                state.status = format!("{} deleted", visual.declaration.title);
                menu_ui.close();
            }
            let attached_links = state
                .graph
                .links
                .iter()
                .filter(|(_, link)| link.from.node == *node_id || link.to.node == *node_id)
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            if menu_ui
                .add_enabled(
                    !attached_links.is_empty(),
                    egui::Button::new(format!("Disconnect all ({})", attached_links.len())),
                )
                .clicked()
            {
                state.apply(
                    GraphCommand::Transaction {
                        commands: attached_links
                            .into_iter()
                            .map(|id| GraphCommand::Disconnect { id })
                            .collect(),
                    },
                    &registry,
                );
                state.status = format!("{} disconnected", visual.declaration.title);
                menu_ui.close();
            }
            if menu_ui.button("Frame this").clicked() {
                state.selected_nodes.clear();
                state.selected_nodes.insert(node_id.clone());
                state.selected_node = Some(node_id.clone());
                state.frame_selection_requested = true;
                menu_ui.close();
            }
        });

        if !collapsed {
            if let Some(preview_type) = graph_node_preview_type(&visual.record, visual.declaration)
            {
                let preview_rect = egui::Rect::from_min_size(
                    egui::pos2(
                        rect.left() + 8.0 * zoom,
                        rect.top() + header_height + 4.0 * zoom,
                    ),
                    egui::vec2(
                        rect.width() - 16.0 * zoom,
                        visual.preview_height * zoom - 8.0 * zoom,
                    ),
                );
                draw_graph_node_preview(
                    &painter,
                    preview_rect,
                    &visual.record,
                    visual.declaration,
                    preview_type,
                );
            }
        }

        for (index, socket) in visual.declaration.inputs.iter().enumerate() {
            let point = graph_socket_position(rect, true, index, zoom, visual.preview_height);
            let socket_rect = egui::Rect::from_center_size(
                point,
                egui::vec2(18.0 * zoom.max(0.75), 18.0 * zoom.max(0.75)),
            );
            let color = graph_socket_color(socket.value_type);
            let link_count = state
                .graph
                .links
                .values()
                .filter(|link| link.to.node == *node_id && link.to.socket.0 == socket.key)
                .count();
            draw_graph_socket(
                &painter,
                point,
                color,
                socket.cardinality,
                true,
                link_count,
                zoom,
            );
            if hovered_socket
                .as_ref()
                .is_some_and(|hit| hit.node == *node_id && hit.input && hit.socket.0 == socket.key)
            {
                let compatible = state.connector_drag.as_ref().is_some_and(|drag| {
                    graph_connector_can_link(
                        drag,
                        &GraphSocketHit {
                            node: node_id.clone(),
                            socket: SocketKey(socket.key.into()),
                            input: true,
                        },
                        &state.graph,
                        &registry,
                    )
                });
                painter.circle_stroke(
                    point,
                    9.0 * zoom.max(0.75),
                    egui::Stroke::new(
                        2.0,
                        if compatible {
                            egui::Color32::from_rgb(100, 240, 130)
                        } else {
                            egui::Color32::from_rgb(240, 100, 100)
                        },
                    ),
                );
            }
            let socket_response = ui.interact(
                socket_rect,
                egui::Id::new(("graph-input", node_id.0.as_str(), socket.key)),
                egui::Sense::click_and_drag(),
            );
            socket_response.clone().on_hover_ui(|hover_ui| {
                let tooltip = graph_socket_tooltip(
                    &state.graph,
                    &registry,
                    &reachable,
                    state.connector_drag.as_ref(),
                    node_id,
                    visual.declaration,
                    &visual.record,
                    *socket,
                    true,
                    link_count,
                );
                graph_tooltip_body(hover_ui, &tooltip);
            });
            graph_socket_context_menu(
                &socket_response,
                state,
                &registry,
                node_id,
                visual.declaration,
                &visual.record,
                *socket,
                true,
                point,
            );
            painter.text(
                point + egui::vec2(10.0 * zoom.max(0.75), -7.0 * zoom.max(0.75)),
                egui::Align2::LEFT_TOP,
                socket.key,
                egui::FontId::proportional(10.0 * zoom.max(0.75)),
                egui::Color32::from_gray(215),
            );
            if socket_response.drag_started() {
                state.connector_drag = Some(ConnectorDrag::FromInput(InputPin {
                    node: node_id.clone(),
                    socket: SocketKey(socket.key.into()),
                }));
                state.connector_menu_position = None;
                state.connector_menu_filter.clear();
                state.pending_output = None;
                state.status = if socket.cardinality.accepts_additional(link_count) {
                    format!("Dragging into `{}` — release to add a node", socket.key)
                } else {
                    format!(
                        "Dragging into `{}` — linking a new source replaces its current connection",
                        socket.key
                    )
                };
            } else if socket_response.clicked() && state.connector_drag.is_none() {
                if let Some(from) = state.pending_output.take() {
                    state.apply(
                        GraphCommand::Connect {
                            id: LinkId::new(),
                            from,
                            to: InputPin {
                                node: node_id.clone(),
                                socket: SocketKey(socket.key.into()),
                            },
                        },
                        &registry,
                    );
                }
            }
        }
        for (index, socket) in visual.declaration.outputs.iter().enumerate() {
            let point = graph_socket_position(rect, false, index, zoom, visual.preview_height);
            let socket_rect = egui::Rect::from_center_size(
                point,
                egui::vec2(18.0 * zoom.max(0.75), 18.0 * zoom.max(0.75)),
            );
            let color = graph_socket_color(socket.value_type);
            let link_count = state
                .graph
                .links
                .values()
                .filter(|link| link.from.node == *node_id && link.from.socket.0 == socket.key)
                .count();
            draw_graph_socket(
                &painter,
                point,
                color,
                socket.cardinality,
                false,
                link_count,
                zoom,
            );
            if hovered_socket
                .as_ref()
                .is_some_and(|hit| hit.node == *node_id && !hit.input && hit.socket.0 == socket.key)
            {
                painter.circle_stroke(
                    point,
                    9.0 * zoom.max(0.75),
                    egui::Stroke::new(2.0, egui::Color32::from_rgb(100, 240, 130)),
                );
            }
            if state
                .pending_output
                .as_ref()
                .is_some_and(|pending| pending.node == *node_id && pending.socket.0 == socket.key)
            {
                painter.circle_stroke(
                    point,
                    8.0 * zoom.max(0.75),
                    egui::Stroke::new(2.0, egui::Color32::WHITE),
                );
            }
            let socket_response = ui.interact(
                socket_rect,
                egui::Id::new(("graph-output", node_id.0.as_str(), socket.key)),
                egui::Sense::click_and_drag(),
            );
            socket_response.clone().on_hover_ui(|hover_ui| {
                let tooltip = graph_socket_tooltip(
                    &state.graph,
                    &registry,
                    &reachable,
                    state.connector_drag.as_ref(),
                    node_id,
                    visual.declaration,
                    &visual.record,
                    *socket,
                    false,
                    link_count,
                );
                graph_tooltip_body(hover_ui, &tooltip);
            });
            graph_socket_context_menu(
                &socket_response,
                state,
                &registry,
                node_id,
                visual.declaration,
                &visual.record,
                *socket,
                false,
                point,
            );
            painter.text(
                point - egui::vec2(10.0 * zoom.max(0.75), 7.0 * zoom.max(0.75)),
                egui::Align2::RIGHT_TOP,
                socket.key,
                egui::FontId::proportional(10.0 * zoom.max(0.75)),
                egui::Color32::from_gray(215),
            );
            if socket_response.drag_started() {
                state.connector_drag = Some(ConnectorDrag::FromOutput(OutputPin {
                    node: node_id.clone(),
                    socket: SocketKey(socket.key.into()),
                }));
                state.connector_menu_position = None;
                state.connector_menu_filter.clear();
                state.pending_output = None;
                state.status = if socket.cardinality.accepts_additional(link_count) {
                    format!("Dragging `{}` — release to add a node", socket.key)
                } else {
                    format!(
                        "Dragging `{}` — linking a new destination replaces its current connection",
                        socket.key
                    )
                };
            } else if socket_response.clicked() && state.connector_drag.is_none() {
                state.pending_output = Some(OutputPin {
                    node: node_id.clone(),
                    socket: SocketKey(socket.key.into()),
                });
                state.status = format!("Output `{}` selected — click an input", socket.key);
            }
        }

        if !collapsed {
            for (index, field) in visual
                .declaration
                .fields
                .iter()
                .filter(|field| field.target == FieldTarget::Property)
                .enumerate()
            {
                let row = socket_rows + index;
                let y = rect.top()
                    + header_height
                    + (8.0 + visual.preview_height) * zoom
                    + row as f32 * row_height;
                let property_rect = egui::Rect::from_min_size(
                    egui::pos2(rect.left() + 8.0 * zoom, y + 1.0 * zoom),
                    egui::vec2(
                        rect.width() - 16.0 * zoom,
                        (row_height - 2.0 * zoom).max(1.0),
                    ),
                );
                painter.rect_filled(property_rect, 2.0, egui::Color32::from_rgb(53, 56, 63));
                let mut value = visual
                    .record
                    .properties
                    .get(field.key)
                    .cloned()
                    .unwrap_or_else(|| field.default.value());
                let changed = if zoom < GRAPH_NODE_CONTROL_ZOOM {
                    let response = ui.interact(
                        property_rect,
                        egui::Id::new((
                            "graph-node-property-summary",
                            node_id.0.as_str(),
                            field.key,
                        )),
                        egui::Sense::hover(),
                    );
                    response.on_hover_ui(|hover_ui| {
                        graph_tooltip_body(hover_ui, &graph_field_tooltip(field, &value));
                        hover_ui.colored_label(
                            GRAPH_TOOLTIP_KEY_COLOR,
                            "Zoom to 100% to edit this property",
                        );
                    });
                    draw_graph_property_summary(&painter, property_rect, field, &value, zoom);
                    false
                } else {
                    ui.push_id(
                        ("graph-node-property", node_id.0.as_str(), field.key),
                        |ui| {
                            ui.scope_builder(
                                egui::UiBuilder::new()
                                    .max_rect(property_rect)
                                    .layout(egui::Layout::left_to_right(egui::Align::Center)),
                                |ui| {
                                    ui.add_enabled_ui(!field.read_only, |ui| {
                                        draw_graph_property(ui, field, &mut value, zoom)
                                    })
                                    .inner
                                },
                            )
                            .inner
                        },
                    )
                    .inner
                };
                if changed {
                    state.apply(
                        GraphCommand::SetProperty {
                            node: node_id.clone(),
                            property: field.key.to_string(),
                            value,
                        },
                        &registry,
                    );
                }
            }
        }
    }

    if let (Some(start), Some(end)) = (state.box_select_start, state.box_select_current) {
        let selection_rect =
            egui::Rect::from_two_pos(egui::pos2(start[0], start[1]), egui::pos2(end[0], end[1]));
        painter.rect_filled(
            selection_rect,
            2.0,
            egui::Color32::from_rgba_unmultiplied(80, 140, 220, 35),
        );
        painter.rect_stroke(
            selection_rect,
            2.0,
            egui::Stroke::new(1.0, egui::Color32::from_rgb(120, 180, 255)),
            egui::StrokeKind::Inside,
        );
    }

    // A connector drag either completes on a compatible socket, removes a
    // moved saturated connection, or becomes the filtered add-node menu when
    // released over empty space with capacity still available.
    if primary_released && state.connector_drag.is_some() && state.connector_menu_position.is_none()
    {
        let drag = state.connector_drag.clone().expect("checked above");
        let detached_link_ids = graph_connector_detached_link_ids(&drag, &visuals, &state.graph);
        if let Some(pointer) = pointer_position {
            if let Some(hit) = graph_socket_hit_at_pointer(&visuals, pointer, zoom) {
                if graph_connector_can_link(&drag, &hit, &state.graph, &registry) {
                    let (from, to) = graph_connector_link(&drag, hit);
                    state.apply(
                        GraphCommand::Connect {
                            id: LinkId::new(),
                            from,
                            to,
                        },
                        &registry,
                    );
                    state.connector_drag = None;
                    state.pending_output = None;
                    state.status = "Connector linked".to_string();
                } else if !detached_link_ids.is_empty() {
                    state.apply(
                        GraphCommand::Transaction {
                            commands: detached_link_ids
                                .into_iter()
                                .map(|id| GraphCommand::Disconnect { id })
                                .collect(),
                        },
                        &registry,
                    );
                    state.connector_drag = None;
                    state.pending_output = None;
                    state.status = "Connection removed".to_string();
                } else {
                    state.connector_menu_position = Some([pointer.x, pointer.y]);
                    state.connector_menu_filter.clear();
                    state.status = "No compatible socket here — choose a node to add".to_string();
                }
            } else if !detached_link_ids.is_empty() {
                state.apply(
                    GraphCommand::Transaction {
                        commands: detached_link_ids
                            .into_iter()
                            .map(|id| GraphCommand::Disconnect { id })
                            .collect(),
                    },
                    &registry,
                );
                state.connector_drag = None;
                state.pending_output = None;
                state.status = "Connection removed".to_string();
            } else {
                state.connector_menu_position = Some([pointer.x, pointer.y]);
                state.connector_menu_filter.clear();
                state.status = "Choose a compatible node to add".to_string();
            }
        } else {
            state.connector_drag = None;
        }
    }

    draw_graph_connector_menu(ui, state, &registry, &visuals, canvas_rect, zoom);
    draw_graph_canvas_context_menu(&canvas_response, state, &registry, canvas_rect);

    ui.label(format!(
        "zoom {:.0}% | {} nodes | {} links",
        state.zoom * 100.0,
        state.graph.nodes.len(),
        state.graph.links.len()
    ));
    if let Some(node_id) = state.selected_node.clone() {
        draw_graph_inspector(ui, state, &registry, &node_id);
    }
    for diagnostic in &state.diagnostics {
        ui.colored_label(
            if diagnostic.severity == voxel_graph::DiagnosticSeverity::Error {
                egui::Color32::from_rgb(255, 125, 125)
            } else {
                egui::Color32::from_rgb(255, 210, 100)
            },
            format!("{}: {}", diagnostic.code, diagnostic.message),
        );
    }

    ui.ctx().all_styles_mut(|style| {
        style.interaction.tooltip_delay = previous_tooltip_delay;
    });
}

/// The empty-canvas right-click menu: add a node where the click landed, paste,
/// or frame the whole graph. The node list is the same filtered palette the
/// toolbar uses, so there is one answer to "what can I add here?".
fn draw_graph_canvas_context_menu(
    canvas_response: &egui::Response,
    state: &mut GraphEditorState,
    registry: &NodeRegistry,
    canvas_rect: egui::Rect,
) {
    // The menu outlives the click frame, so remember where the click landed
    // instead of following the pointer around while the menu is open.
    let anchor_id = egui::Id::new("graph_canvas_context_menu_anchor");
    if canvas_response.secondary_clicked() {
        if let Some(position) = canvas_response.interact_pointer_pos() {
            canvas_response
                .ctx
                .data_mut(|data| data.insert_temp(anchor_id, [position.x, position.y]));
        }
    }
    canvas_response.context_menu(|menu_ui| {
        menu_ui.set_min_width(280.0);
        let anchor = menu_ui
            .data(|data| data.get_temp::<[f32; 2]>(anchor_id))
            .map_or_else(|| canvas_rect.center(), |at| egui::pos2(at[0], at[1]));
        let graph_position = [
            (anchor.x - canvas_rect.left() - state.pan[0]) / state.zoom,
            (anchor.y - canvas_rect.top() - state.pan[1]) / state.zoom,
        ];

        menu_ui.label(egui::RichText::new("Add node").strong());
        menu_ui.horizontal(|menu_ui| {
            menu_ui.label("⌕");
            menu_ui.add(
                egui::TextEdit::singleline(&mut state.search)
                    .desired_width(220.0)
                    .hint_text("search nodes"),
            );
        });
        let visible_nodes = state.visible_node_types(registry);
        let mut chosen = None;
        egui::ScrollArea::vertical()
            .max_height(240.0)
            .show(menu_ui, |menu_ui| {
                if visible_nodes.is_empty() {
                    menu_ui.colored_label(GRAPH_TOOLTIP_KEY_COLOR, "No nodes match this search");
                }
                for node in &visible_nodes {
                    let row_text = graph_palette_row_text(menu_ui, node.title, node.id);
                    if menu_ui
                        .selectable_label(false, row_text)
                        .on_hover_text(node.description)
                        .clicked()
                    {
                        chosen = Some(NodeTypeId(node.id.to_string()));
                    }
                }
            });
        if let Some(node_type) = chosen {
            state.add_node_at(node_type, graph_position, registry);
            state.status = "Node added".to_string();
            menu_ui.close();
        }

        menu_ui.separator();
        if menu_ui
            .add_enabled(state.can_paste(), egui::Button::new("Paste"))
            .clicked()
        {
            state.paste_clipboard(registry);
            menu_ui.close();
        }
        if menu_ui.button("Frame all").clicked() {
            state.frame_all_requested = true;
            menu_ui.close();
        }
    });
}

struct GraphNodeVisual {
    id: NodeId,
    record: NodeRecord,
    declaration: &'static NodeDeclaration,
    preview_height: f32,
    rect: egui::Rect,
}

#[derive(Clone)]
struct GraphSocketHit {
    node: NodeId,
    socket: SocketKey,
    input: bool,
}

#[derive(Clone)]
struct GraphConnectorCandidate {
    node_type: NodeTypeId,
    node_title: String,
    socket: SocketKey,
}

fn graph_socket_hit_at_pointer(
    visuals: &[GraphNodeVisual],
    pointer: egui::Pos2,
    zoom: f32,
) -> Option<GraphSocketHit> {
    let hit_radius = 10.0 * zoom.max(0.75);
    for visual in visuals.iter().rev() {
        for (index, socket) in visual.declaration.inputs.iter().enumerate() {
            let point =
                graph_socket_position(visual.rect, true, index, zoom, visual.preview_height);
            if point.distance(pointer) <= hit_radius {
                return Some(GraphSocketHit {
                    node: visual.id.clone(),
                    socket: SocketKey(socket.key.into()),
                    input: true,
                });
            }
        }
        for (index, socket) in visual.declaration.outputs.iter().enumerate() {
            let point =
                graph_socket_position(visual.rect, false, index, zoom, visual.preview_height);
            if point.distance(pointer) <= hit_radius {
                return Some(GraphSocketHit {
                    node: visual.id.clone(),
                    socket: SocketKey(socket.key.into()),
                    input: false,
                });
            }
        }
    }
    None
}

fn graph_connector_source_socket(
    drag: &ConnectorDrag,
    visuals: &[GraphNodeVisual],
) -> Option<voxel_graph::SocketDeclarationStatic> {
    let (node, socket, input) = match drag {
        ConnectorDrag::FromOutput(pin) => (&pin.node, &pin.socket, false),
        ConnectorDrag::FromInput(pin) => (&pin.node, &pin.socket, true),
    };
    visuals
        .iter()
        .find(|visual| visual.id == *node)
        .and_then(|visual| {
            if input {
                visual
                    .declaration
                    .inputs
                    .iter()
                    .find(|candidate| candidate.key == socket.0)
                    .copied()
            } else {
                visual
                    .declaration
                    .outputs
                    .iter()
                    .find(|candidate| candidate.key == socket.0)
                    .copied()
            }
        })
}

/// A full socket does not start a second connection: it moves its existing
/// one. Keep this as a presentation-only projection until the drop succeeds,
/// so cancelling the gesture does not mutate the graph.
fn graph_connector_detached_link_ids(
    drag: &ConnectorDrag,
    visuals: &[GraphNodeVisual],
    graph: &voxel_graph::GraphAsset,
) -> BTreeSet<LinkId> {
    let Some(socket) = graph_connector_source_socket(drag, visuals) else {
        return BTreeSet::new();
    };
    let links = graph
        .links
        .iter()
        .filter(|(_, link)| match drag {
            ConnectorDrag::FromOutput(pin) => {
                link.from.node == pin.node && link.from.socket == pin.socket
            }
            ConnectorDrag::FromInput(pin) => {
                link.to.node == pin.node && link.to.socket == pin.socket
            }
        })
        .map(|(id, _)| id.clone())
        .collect::<BTreeSet<_>>();
    if socket.cardinality.accepts_additional(links.len()) {
        BTreeSet::new()
    } else {
        links
    }
}

fn graph_connector_origin(
    drag: &ConnectorDrag,
    visuals: &[GraphNodeVisual],
    zoom: f32,
) -> Option<egui::Pos2> {
    let (node, socket, input) = match drag {
        ConnectorDrag::FromOutput(pin) => (&pin.node, &pin.socket, false),
        ConnectorDrag::FromInput(pin) => (&pin.node, &pin.socket, true),
    };
    let visual = visuals.iter().find(|visual| visual.id == *node)?;
    let sockets = if input {
        visual.declaration.inputs
    } else {
        visual.declaration.outputs
    };
    let index = sockets
        .iter()
        .position(|candidate| candidate.key == socket.0)?;
    Some(graph_socket_position(
        visual.rect,
        input,
        index,
        zoom,
        visual.preview_height,
    ))
}

fn graph_connector_can_link(
    drag: &ConnectorDrag,
    hit: &GraphSocketHit,
    graph: &voxel_graph::GraphAsset,
    registry: &NodeRegistry,
) -> bool {
    let pins = match drag {
        ConnectorDrag::FromOutput(from) if hit.input => Some((
            from.clone(),
            InputPin {
                node: hit.node.clone(),
                socket: hit.socket.clone(),
            },
        )),
        ConnectorDrag::FromInput(to) if !hit.input => Some((
            OutputPin {
                node: hit.node.clone(),
                socket: hit.socket.clone(),
            },
            to.clone(),
        )),
        _ => None,
    };
    pins.is_some_and(|(from, to)| graph.connection_plan(registry, &from, &to).is_ok())
}

fn graph_connector_link(drag: &ConnectorDrag, hit: GraphSocketHit) -> (OutputPin, InputPin) {
    match drag {
        ConnectorDrag::FromOutput(from) => (
            from.clone(),
            InputPin {
                node: hit.node,
                socket: hit.socket,
            },
        ),
        ConnectorDrag::FromInput(to) => (
            OutputPin {
                node: hit.node,
                socket: hit.socket,
            },
            to.clone(),
        ),
    }
}

fn graph_connector_candidates(
    drag: &ConnectorDrag,
    source: voxel_graph::SocketDeclarationStatic,
    filter: &str,
    graph: &voxel_graph::GraphAsset,
    registry: &NodeRegistry,
) -> Vec<GraphConnectorCandidate> {
    let query = filter.trim().to_ascii_lowercase();
    let wants_input = matches!(drag, ConnectorDrag::FromOutput(_));
    registry
        .declarations()
        .iter()
        .filter(|declaration| {
            declaration.kinds.contains(&graph.kind)
                && graph.can_add_node_type(registry, &NodeTypeId(declaration.id.into()))
        })
        .flat_map(|declaration| {
            let sockets = if wants_input {
                declaration.inputs
            } else {
                declaration.outputs
            };
            let query = query.clone();
            sockets.iter().filter_map(move |socket| {
                let compatible = if wants_input {
                    source.can_feed(*socket)
                } else {
                    socket.can_feed(source)
                };
                if !compatible {
                    return None;
                }
                let title = declaration.title.to_string();
                let searchable =
                    format!("{} {} {}", declaration.id, title, socket.key).to_ascii_lowercase();
                if !query.is_empty() && !searchable.contains(&query) {
                    return None;
                }
                Some(GraphConnectorCandidate {
                    node_type: NodeTypeId(declaration.id.to_string()),
                    node_title: title,
                    socket: SocketKey(socket.key.to_string()),
                })
            })
        })
        .collect()
}

fn draw_graph_connector_menu(
    ui: &mut egui::Ui,
    state: &mut GraphEditorState,
    registry: &NodeRegistry,
    visuals: &[GraphNodeVisual],
    canvas_rect: egui::Rect,
    zoom: f32,
) {
    let Some(menu_position) = state.connector_menu_position else {
        return;
    };
    let Some(drag) = state.connector_drag.clone() else {
        state.connector_menu_position = None;
        return;
    };
    let Some(source_socket) = graph_connector_source_socket(&drag, visuals) else {
        return;
    };
    let candidates = graph_connector_candidates(
        &drag,
        source_socket,
        &state.connector_menu_filter,
        &state.graph,
        registry,
    );
    let mut selected = None;
    let menu_pos = egui::pos2(menu_position[0], menu_position[1]);
    egui::Area::new(egui::Id::new("graph_connector_add_menu"))
        .order(egui::Order::Foreground)
        .fixed_pos(menu_pos)
        .show(ui.ctx(), |menu_ui| {
            egui::Frame::popup(menu_ui.style()).show(menu_ui, |menu_ui| {
                menu_ui.set_min_width(300.0);
                menu_ui.horizontal(|menu_ui| {
                    menu_ui.label("⌕");
                    menu_ui.add(
                        egui::TextEdit::singleline(&mut state.connector_menu_filter)
                            .desired_width(260.0)
                            .hint_text("search compatible nodes"),
                    );
                });
                menu_ui.separator();
                egui::ScrollArea::vertical()
                    .max_height(260.0)
                    .show(menu_ui, |menu_ui| {
                        if candidates.is_empty() {
                            menu_ui.colored_label(
                                egui::Color32::from_gray(160),
                                "No compatible nodes",
                            );
                        }
                        for candidate in &candidates {
                            let label =
                                format!("{}  >  {}", candidate.node_title, candidate.socket.0);
                            if menu_ui
                                .selectable_label(false, label)
                                .on_hover_text(format!(
                                    "Add {} and connect its `{}` socket",
                                    candidate.node_title, candidate.socket.0
                                ))
                                .clicked()
                            {
                                selected = Some(candidate.clone());
                            }
                        }
                    });
            });
        });

    if ui.input(|input| input.key_pressed(egui::Key::Escape)) {
        state.connector_drag = None;
        state.connector_menu_position = None;
        state.connector_menu_filter.clear();
        state.status = "Connector insert cancelled".to_string();
        return;
    }

    let Some(candidate) = selected else {
        return;
    };
    let graph_position = {
        let pointer_in_canvas = menu_pos - canvas_rect.min;
        let x_offset = match drag {
            ConnectorDrag::FromOutput(_) => 12.0,
            ConnectorDrag::FromInput(_) => 216.0,
        };
        [
            pointer_in_canvas.x / zoom - x_offset,
            pointer_in_canvas.y / zoom - 38.0,
        ]
    };
    let node_id = NodeId::new();
    let (from, to) = match drag {
        ConnectorDrag::FromOutput(output) => (
            output,
            InputPin {
                node: node_id.clone(),
                socket: candidate.socket,
            },
        ),
        ConnectorDrag::FromInput(input) => (
            OutputPin {
                node: node_id.clone(),
                socket: candidate.socket,
            },
            input,
        ),
    };
    if !state.apply(
        GraphCommand::Transaction {
            commands: vec![
                GraphCommand::AddNode {
                    id: node_id.clone(),
                    node_type: candidate.node_type,
                    position: graph_position,
                },
                GraphCommand::Connect {
                    id: LinkId::new(),
                    from,
                    to,
                },
            ],
        },
        registry,
    ) {
        return;
    }
    state.selected_node = Some(node_id.clone());
    state.selected_nodes.clear();
    state.selected_nodes.insert(node_id);
    state.connector_drag = None;
    state.connector_menu_position = None;
    state.connector_menu_filter.clear();
    state.status = "Node added and connector linked".to_string();
}

fn draw_graph_socket(
    painter: &egui::Painter,
    point: egui::Pos2,
    color: egui::Color32,
    cardinality: Cardinality,
    input: bool,
    link_count: usize,
    zoom: f32,
) {
    let scale = zoom.max(0.75);
    let accepts_more = cardinality.accepts_additional(link_count);
    let fill = if accepts_more {
        egui::Color32::from_rgb(40, 42, 47)
    } else {
        color
    };
    if input && cardinality.allows_many() {
        let rect = egui::Rect::from_center_size(point, egui::vec2(14.0 * scale, 9.0 * scale));
        painter.rect_filled(rect, 4.5 * scale, fill);
        painter.rect_stroke(
            rect,
            4.5 * scale,
            egui::Stroke::new(1.5, color),
            egui::StrokeKind::Inside,
        );
    } else {
        painter.circle_filled(point, 5.0 * scale, fill);
        painter.circle_stroke(point, 5.0 * scale, egui::Stroke::new(1.5, color));
    }
}

fn graph_socket_capacity_label(cardinality: Cardinality, link_count: usize) -> String {
    match cardinality.maximum {
        Some(maximum) => {
            let state = if cardinality.accepts_additional(link_count) {
                "open"
            } else {
                "full; a new link replaces the current one"
            };
            format!("{link_count}/{maximum} links ({state})")
        }
        None => format!("{link_count} links (open)"),
    }
}

// ---------------------------------------------------------------------------
// Blender-style graph tooltips
//
// One renderer serves sockets, node headers, palette rows and property widgets
// so the anatomy cannot drift between them: authored prose on top, a dim
// aligned monospace key/value block underneath, then a type-specific visual.
// Everything below the prose is DERIVED from the live graph — no second place
// to author "what does this do right now".
// ---------------------------------------------------------------------------

/// Blender wraps tooltip prose at a wide, fixed column so a sentence reads as a
/// sentence instead of a stack of two-word lines.
const GRAPH_TOOLTIP_WIDTH: f32 = 380.0;
/// Sockets sit a few pixels apart, so an instant tooltip strobes while a
/// connector is dragged across a node. Dwell first, then explain.
const GRAPH_TOOLTIP_DELAY_SECONDS: f32 = 0.5;
const GRAPH_TOOLTIP_KEY_COLOR: egui::Color32 = egui::Color32::from_rgb(134, 141, 154);
const GRAPH_TOOLTIP_VALUE_COLOR: egui::Color32 = egui::Color32::from_rgb(198, 204, 214);
const GRAPH_TOOLTIP_ACCENT_COLOR: egui::Color32 = egui::Color32::from_rgb(120, 190, 255);
const GRAPH_TOOLTIP_GOOD_COLOR: egui::Color32 = egui::Color32::from_rgb(120, 214, 140);
const GRAPH_TOOLTIP_BAD_COLOR: egui::Color32 = egui::Color32::from_rgb(240, 132, 132);

/// One aligned `Key: value` line of a tooltip's monospace block.
struct GraphTooltipRow {
    key: String,
    value: String,
    value_color: Option<egui::Color32>,
    /// A wire-colour dot drawn beside the value. Used on the `Type:` row so
    /// hovering teaches the socket colour code instead of leaving it to be
    /// memorised from the canvas.
    dot: Option<egui::Color32>,
}

impl GraphTooltipRow {
    fn new(key: impl Into<String>, value: impl Into<String>) -> Self {
        Self {
            key: key.into(),
            value: value.into(),
            value_color: None,
            dot: None,
        }
    }

    fn colored(key: impl Into<String>, value: impl Into<String>, color: egui::Color32) -> Self {
        Self {
            value_color: Some(color),
            ..Self::new(key, value)
        }
    }

    fn with_dot(mut self, dot: egui::Color32) -> Self {
        self.dot = Some(dot);
        self
    }
}

/// The type-specific body drawn under the key/value block.
enum GraphTooltipVisual {
    None,
    Color([f32; 4]),
    Scalar {
        value: f32,
        soft_range: Option<NumericRange>,
        hard_range: Option<NumericRange>,
    },
    Vector3([f32; 3]),
    Boolean(bool),
    /// The node's own live preview, reusing the canvas renderer so a tooltip
    /// can never disagree with what the node itself is drawing.
    NodePreview {
        record: NodeRecord,
        declaration: &'static NodeDeclaration,
        socket_type: SocketType,
    },
}

/// The assembled content of one tooltip, built by the per-subject builders
/// below and rendered by [`graph_tooltip_body`].
struct GraphTooltip {
    title: Option<String>,
    prose: String,
    /// Appended to the prose in the accent colour — the live value inlined into
    /// the sentence, exactly as Blender does for enum properties.
    prose_accent: Option<String>,
    /// A second, dimmer sentence: the selected enum choice's own description.
    detail: Option<String>,
    rows: Vec<GraphTooltipRow>,
    visual: GraphTooltipVisual,
}

impl GraphTooltip {
    fn new(prose: impl Into<String>) -> Self {
        Self {
            title: None,
            prose: prose.into(),
            prose_accent: None,
            detail: None,
            rows: Vec::new(),
            visual: GraphTooltipVisual::None,
        }
    }
}

/// The one shared tooltip renderer. Anatomy, in order: prose, the dim
/// monospace key/value block, then the type-specific visual.
fn graph_tooltip_body(ui: &mut egui::Ui, tooltip: &GraphTooltip) {
    ui.set_max_width(GRAPH_TOOLTIP_WIDTH);
    if let Some(title) = &tooltip.title {
        ui.label(egui::RichText::new(title).strong());
    }
    if !tooltip.prose.is_empty() || tooltip.prose_accent.is_some() {
        graph_tooltip_prose(ui, &tooltip.prose, tooltip.prose_accent.as_deref());
    }
    if let Some(detail) = &tooltip.detail {
        ui.label(egui::RichText::new(detail).color(GRAPH_TOOLTIP_KEY_COLOR));
    }
    if !tooltip.rows.is_empty() {
        ui.add_space(5.0);
        egui::Grid::new("graph-tooltip-rows")
            .num_columns(2)
            .spacing(egui::vec2(10.0, 2.0))
            .show(ui, |ui| {
                for row in &tooltip.rows {
                    ui.label(
                        egui::RichText::new(format!("{}:", row.key))
                            .monospace()
                            .color(GRAPH_TOOLTIP_KEY_COLOR),
                    );
                    ui.horizontal(|ui| {
                        if let Some(dot) = row.dot {
                            let (dot_rect, _) = ui
                                .allocate_exact_size(egui::vec2(11.0, 11.0), egui::Sense::hover());
                            ui.painter().circle_filled(dot_rect.center(), 4.5, dot);
                            ui.painter().circle_stroke(
                                dot_rect.center(),
                                4.5,
                                egui::Stroke::new(1.0, egui::Color32::from_black_alpha(120)),
                            );
                        }
                        ui.label(
                            egui::RichText::new(&row.value)
                                .monospace()
                                .color(row.value_color.unwrap_or(GRAPH_TOOLTIP_VALUE_COLOR)),
                        );
                    });
                    ui.end_row();
                }
            });
    }
    graph_tooltip_visual(ui, &tooltip.visual);
}

/// The prose line, with the live value inlined in the accent colour. A layout
/// job rather than two labels so the accent wraps inside the sentence.
fn graph_tooltip_prose(ui: &mut egui::Ui, prose: &str, accent: Option<&str>) {
    let font = egui::TextStyle::Body.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.wrap.max_width = GRAPH_TOOLTIP_WIDTH;
    if !prose.is_empty() {
        job.append(
            prose,
            0.0,
            egui::TextFormat {
                font_id: font.clone(),
                color: ui.visuals().text_color(),
                ..Default::default()
            },
        );
    }
    if let Some(accent) = accent {
        job.append(
            &format!("{}{accent}", if prose.is_empty() { "" } else { " " }),
            0.0,
            egui::TextFormat {
                font_id: font,
                color: GRAPH_TOOLTIP_ACCENT_COLOR,
                ..Default::default()
            },
        );
    }
    ui.label(job);
}

fn graph_tooltip_visual(ui: &mut egui::Ui, visual: &GraphTooltipVisual) {
    match visual {
        GraphTooltipVisual::None => {}
        GraphTooltipVisual::Color(color) => graph_tooltip_color_body(ui, *color),
        GraphTooltipVisual::Scalar {
            value,
            soft_range,
            hard_range,
        } => graph_tooltip_scalar_body(ui, *value, *soft_range, *hard_range),
        GraphTooltipVisual::Vector3(components) => graph_tooltip_vector_body(ui, *components),
        GraphTooltipVisual::Boolean(state) => graph_tooltip_boolean_body(ui, *state),
        GraphTooltipVisual::NodePreview {
            record,
            declaration,
            socket_type,
        } => {
            let height = graph_node_preview_height(record, declaration).max(46.0);
            ui.add_space(5.0);
            let (rect, _) = ui.allocate_exact_size(
                egui::vec2(ui.available_width().max(120.0), height),
                egui::Sense::hover(),
            );
            draw_graph_node_preview(ui.painter(), rect, record, declaration, *socket_type);
        }
    }
}

/// The Blender colour tooltip: one large filled swatch over a checkerboard so
/// alpha is visible, then the numeric readouts.
fn graph_tooltip_color_body(ui: &mut egui::Ui, color: [f32; 4]) {
    ui.add_space(5.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(120.0), 40.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    let checker = 8.0;
    let mut row = 0;
    let mut y = rect.top();
    while y < rect.bottom() {
        let mut column = 0;
        let mut x = rect.left();
        while x < rect.right() {
            let cell = egui::Rect::from_min_max(
                egui::pos2(x, y),
                egui::pos2(
                    (x + checker).min(rect.right()),
                    (y + checker).min(rect.bottom()),
                ),
            );
            painter.rect_filled(
                cell,
                0.0,
                if (row + column) % 2 == 0 {
                    egui::Color32::from_gray(90)
                } else {
                    egui::Color32::from_gray(60)
                },
            );
            x += checker;
            column += 1;
        }
        y += checker;
        row += 1;
    }
    painter.rect_filled(rect, 3.0, graph_color32(color));
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(140)),
        egui::StrokeKind::Inside,
    );

    let [red, green, blue, alpha] = color;
    let [hue, saturation, value] = rgb_to_hsv(red, green, blue);
    ui.add_space(4.0);
    egui::Grid::new("graph-tooltip-color-rows")
        .num_columns(2)
        .spacing(egui::vec2(10.0, 2.0))
        .show(ui, |ui| {
            for (key, text) in [
                ("Display RGB", format!("{red:.3}  {green:.3}  {blue:.3}")),
                ("HSV", format!("{hue:.3}  {saturation:.3}  {value:.3}")),
                ("Alpha", format!("{alpha:.3}")),
                (
                    "Hex",
                    format!(
                        "#{:02X}{:02X}{:02X}{:02X}",
                        graph_color_byte(red),
                        graph_color_byte(green),
                        graph_color_byte(blue),
                        graph_color_byte(alpha),
                    ),
                ),
            ] {
                ui.label(
                    egui::RichText::new(format!("{key}:"))
                        .monospace()
                        .color(GRAPH_TOOLTIP_KEY_COLOR),
                );
                ui.label(
                    egui::RichText::new(text)
                        .monospace()
                        .color(GRAPH_TOOLTIP_VALUE_COLOR),
                );
                ui.end_row();
            }
        });
}

/// Where the value sits in its soft range, with the hard-range ends marked so
/// "how far can I push this?" is answerable without opening the registry.
fn graph_tooltip_scalar_body(
    ui: &mut egui::Ui,
    value: f32,
    soft_range: Option<NumericRange>,
    hard_range: Option<NumericRange>,
) {
    let soft = soft_range
        .or(hard_range)
        .unwrap_or(NumericRange::new(0.0, 1.0));
    let low = soft.min.min(value);
    let high = soft.max.max(value);
    let span = (high - low).abs().max(f32::EPSILON);

    ui.add_space(5.0);
    let (rect, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width().max(120.0), 14.0),
        egui::Sense::hover(),
    );
    let painter = ui.painter();
    painter.rect_filled(rect, 3.0, egui::Color32::from_gray(48));
    let soft_rect = egui::Rect::from_min_max(
        egui::pos2(
            egui::lerp(rect.left()..=rect.right(), (soft.min - low) / span),
            rect.top(),
        ),
        egui::pos2(
            egui::lerp(rect.left()..=rect.right(), (soft.max - low) / span),
            rect.bottom(),
        ),
    );
    painter.rect_filled(soft_rect, 3.0, egui::Color32::from_gray(66));
    let marker_x = egui::lerp(rect.left()..=rect.right(), (value - low) / span);
    painter.rect_filled(
        egui::Rect::from_min_max(rect.left_top(), egui::pos2(marker_x, rect.bottom())),
        3.0,
        GRAPH_TOOLTIP_ACCENT_COLOR.gamma_multiply(0.55),
    );
    painter.line_segment(
        [
            egui::pos2(marker_x, rect.top()),
            egui::pos2(marker_x, rect.bottom()),
        ],
        egui::Stroke::new(2.0, GRAPH_TOOLTIP_ACCENT_COLOR),
    );
    if let Some(hard) = hard_range {
        for end in [hard.min, hard.max] {
            if end < low || end > high {
                continue;
            }
            let x = egui::lerp(rect.left()..=rect.right(), (end - low) / span);
            painter.line_segment(
                [egui::pos2(x, rect.top()), egui::pos2(x, rect.bottom())],
                egui::Stroke::new(1.5, GRAPH_TOOLTIP_BAD_COLOR),
            );
        }
    }
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, egui::Color32::from_black_alpha(120)),
        egui::StrokeKind::Inside,
    );

    let mut caption = format!("soft {:.3} … {:.3}", soft.min, soft.max);
    if let Some(hard) = hard_range {
        caption.push_str(&format!("   ·   hard {:.3} … {:.3}", hard.min, hard.max));
    }
    ui.label(
        egui::RichText::new(caption)
            .monospace()
            .small()
            .color(GRAPH_TOOLTIP_KEY_COLOR),
    );
}

fn graph_tooltip_vector_body(ui: &mut egui::Ui, components: [f32; 3]) {
    ui.add_space(5.0);
    ui.horizontal(|ui| {
        let (gizmo_rect, _) = ui.allocate_exact_size(egui::vec2(40.0, 40.0), egui::Sense::hover());
        let painter = ui.painter();
        let center = gizmo_rect.center();
        let radius = gizmo_rect.height() * 0.44;
        painter.circle_filled(center, radius, egui::Color32::from_gray(44));
        painter.circle_stroke(
            center,
            radius,
            egui::Stroke::new(1.0, egui::Color32::from_gray(80)),
        );
        let length = (components[0].powi(2) + components[1].powi(2) + components[2].powi(2)).sqrt();
        if length > f32::EPSILON {
            // A plain XY projection with Z shading the tip: enough to read
            // "which way does this point" at a glance.
            let direction = egui::vec2(components[0] / length, -components[1] / length);
            let tip = center + direction * radius * 0.9;
            painter.line_segment(
                [center, tip],
                egui::Stroke::new(2.0, GRAPH_TOOLTIP_ACCENT_COLOR),
            );
            let depth = ((components[2] / length) * 0.5 + 0.5).clamp(0.0, 1.0);
            painter.circle_filled(
                tip,
                2.0 + 2.5 * depth,
                egui::Color32::from_rgb(255, 210, 120),
            );
        }
        ui.vertical(|ui| {
            for (axis, component, color) in [
                ("X", components[0], egui::Color32::from_rgb(226, 108, 108)),
                ("Y", components[1], egui::Color32::from_rgb(120, 210, 130)),
                ("Z", components[2], egui::Color32::from_rgb(120, 160, 240)),
            ] {
                ui.horizontal(|ui| {
                    ui.label(egui::RichText::new(axis).monospace().color(color));
                    ui.label(
                        egui::RichText::new(format!("{component:>9.3}"))
                            .monospace()
                            .color(GRAPH_TOOLTIP_VALUE_COLOR),
                    );
                });
            }
        });
    });
}

fn graph_tooltip_boolean_body(ui: &mut egui::Ui, state: bool) {
    ui.add_space(5.0);
    let (rect, _) = ui.allocate_exact_size(egui::vec2(54.0, 18.0), egui::Sense::hover());
    let painter = ui.painter();
    let color = if state {
        GRAPH_TOOLTIP_GOOD_COLOR
    } else {
        egui::Color32::from_gray(78)
    };
    painter.rect_filled(rect, 9.0, color.gamma_multiply(0.35));
    painter.rect_stroke(
        rect,
        9.0,
        egui::Stroke::new(1.0, color),
        egui::StrokeKind::Inside,
    );
    let knob_x = if state {
        rect.right() - 9.0
    } else {
        rect.left() + 9.0
    };
    painter.circle_filled(egui::pos2(knob_x, rect.center().y), 6.0, color);
    painter.text(
        egui::pos2(
            if state {
                rect.left() + 7.0
            } else {
                rect.right() - 7.0
            },
            rect.center().y,
        ),
        if state {
            egui::Align2::LEFT_CENTER
        } else {
            egui::Align2::RIGHT_CENTER
        },
        if state { "on" } else { "off" },
        egui::FontId::monospace(10.0),
        GRAPH_TOOLTIP_VALUE_COLOR,
    );
}

fn graph_color_byte(channel: f32) -> u8 {
    (channel.clamp(0.0, 1.0) * 255.0).round() as u8
}

fn graph_color32(color: [f32; 4]) -> egui::Color32 {
    egui::Color32::from_rgba_unmultiplied(
        graph_color_byte(color[0]),
        graph_color_byte(color[1]),
        graph_color_byte(color[2]),
        graph_color_byte(color[3]),
    )
}

fn rgb_to_hsv(red: f32, green: f32, blue: f32) -> [f32; 3] {
    let maximum = red.max(green).max(blue);
    let minimum = red.min(green).min(blue);
    let delta = maximum - minimum;
    let hue = if delta <= f32::EPSILON {
        0.0
    } else if (maximum - red).abs() < f32::EPSILON {
        ((green - blue) / delta).rem_euclid(6.0) / 6.0
    } else if (maximum - green).abs() < f32::EPSILON {
        ((blue - red) / delta + 2.0) / 6.0
    } else {
        ((red - green) / delta + 4.0) / 6.0
    };
    let saturation = if maximum <= f32::EPSILON {
        0.0
    } else {
        delta / maximum
    };
    [hue, saturation, maximum]
}

/// The effective value of a property or socket default, formatted for the
/// monospace block — never `{:?}`.
fn graph_value_text(value: &PropertyValue) -> String {
    match value {
        PropertyValue::Scalar(value) => format!("{value:.3}"),
        PropertyValue::Integer(value) => value.to_string(),
        PropertyValue::Vector3([x, y, z]) => format!("{x:.3}, {y:.3}, {z:.3}"),
        PropertyValue::Color([red, green, blue, alpha]) => {
            format!("{red:.3}, {green:.3}, {blue:.3}, {alpha:.3}")
        }
        PropertyValue::Boolean(value) => (if *value { "On" } else { "Off" }).to_string(),
        PropertyValue::Text(value) => value.clone(),
        PropertyValue::Asset(value) => value.0.clone(),
    }
}

fn graph_tooltip_visual_for_value(
    value: &PropertyValue,
    field: Option<FieldDeclarationStatic>,
) -> GraphTooltipVisual {
    match value {
        PropertyValue::Color(color) => GraphTooltipVisual::Color(*color),
        PropertyValue::Scalar(scalar) => GraphTooltipVisual::Scalar {
            value: *scalar,
            soft_range: field.and_then(|field| field.soft_range),
            hard_range: field.and_then(|field| field.hard_range),
        },
        PropertyValue::Integer(integer) => GraphTooltipVisual::Scalar {
            value: *integer as f32,
            soft_range: field.and_then(|field| field.soft_range),
            hard_range: field.and_then(|field| field.hard_range),
        },
        PropertyValue::Vector3(components) => GraphTooltipVisual::Vector3(*components),
        PropertyValue::Boolean(state) => GraphTooltipVisual::Boolean(*state),
        PropertyValue::Text(_) | PropertyValue::Asset(_) => GraphTooltipVisual::None,
    }
}

/// `Type:` never prints a debug name — the registry owns the display strings.
fn graph_socket_type_row(socket: SocketDeclarationStatic) -> GraphTooltipRow {
    GraphTooltipRow::new(
        "Type",
        format!(
            "{} · {}",
            socket.value_type.display_name(),
            socket.rate.display_name()
        ),
    )
    .with_dot(graph_socket_color(socket.value_type))
}

/// The node and socket feeding an input, named the way the canvas names them.
fn graph_input_driver(
    graph: &GraphAsset,
    registry: &NodeRegistry,
    node_id: &NodeId,
    socket_key: &str,
) -> Option<String> {
    graph
        .links
        .values()
        .find(|link| link.to.node == *node_id && link.to.socket.0 == socket_key)
        .map(|link| {
            let title = graph
                .nodes
                .get(&link.from.node)
                .and_then(|record| registry.find(&record.node_type))
                .map_or("unknown node", |declaration| declaration.title);
            format!("{title} · {}", link.from.socket.0)
        })
}

/// Everything an output feeds. A plain sentence when it feeds nothing, because
/// an empty list reads as "the tooltip is broken".
fn graph_output_consumers(
    graph: &GraphAsset,
    registry: &NodeRegistry,
    node_id: &NodeId,
    socket_key: &str,
) -> Vec<String> {
    graph
        .links
        .values()
        .filter(|link| link.from.node == *node_id && link.from.socket.0 == socket_key)
        .map(|link| {
            let title = graph
                .nodes
                .get(&link.to.node)
                .and_then(|record| registry.find(&record.node_type))
                .map_or("unknown node", |declaration| declaration.title);
            format!("{title} · {}", link.to.socket.0)
        })
        .collect()
}

/// The highest-value line in the whole tooltip: a node wired to nothing looks
/// identical to a working one on the canvas.
fn graph_effect_row(reachable: &BTreeSet<NodeId>, node_id: &NodeId) -> GraphTooltipRow {
    if reachable.contains(node_id) {
        GraphTooltipRow::colored(
            "Effect",
            "reaches Material Output",
            GRAPH_TOOLTIP_GOOD_COLOR,
        )
    } else {
        GraphTooltipRow::colored(
            "Effect",
            "none — does not reach Material Output",
            GRAPH_TOOLTIP_BAD_COLOR,
        )
    }
}

/// Phrase a rejected connection from the planner's own error, so the reason a
/// drop will not take is visible BEFORE the drop rather than after it fails.
fn graph_connection_error_text(error: &ConnectionError) -> String {
    match error {
        ConnectionError::TypeMismatch { from, to } => format!(
            "no — {} cannot feed {}",
            from.display_name(),
            to.display_name()
        ),
        ConnectionError::RateMismatch { from, to } => format!(
            "no — a {} value cannot drive a {} input",
            from.display_name(),
            to.display_name()
        ),
        ConnectionError::InputAtCapacity(_) => {
            "no — this input already holds every link it allows".to_string()
        }
        ConnectionError::OutputAtCapacity(_) => {
            "no — the dragged output already holds every link it allows".to_string()
        }
        ConnectionError::Cycle => "no — this would feed the node back into itself".to_string(),
        ConnectionError::MissingNode(_)
        | ConnectionError::UnknownInput(_)
        | ConnectionError::UnknownOutput(_) => "no — that socket no longer exists".to_string(),
    }
}

/// Whether the connector currently in flight would land on THIS socket.
fn graph_connect_gate_row(
    drag: &ConnectorDrag,
    graph: &GraphAsset,
    registry: &NodeRegistry,
    node_id: &NodeId,
    socket_key: &str,
    is_input: bool,
) -> GraphTooltipRow {
    let socket = SocketKey(socket_key.to_string());
    let pins = match drag {
        ConnectorDrag::FromOutput(from) if is_input => Some((
            from.clone(),
            InputPin {
                node: node_id.clone(),
                socket,
            },
        )),
        ConnectorDrag::FromInput(to) if !is_input => Some((
            OutputPin {
                node: node_id.clone(),
                socket,
            },
            to.clone(),
        )),
        _ => None,
    };
    let Some((from, to)) = pins else {
        return GraphTooltipRow::colored(
            "Connect",
            match drag {
                ConnectorDrag::FromOutput(_) => "no — an output can only feed an input",
                ConnectorDrag::FromInput(_) => "no — an input can only be fed by an output",
            },
            GRAPH_TOOLTIP_BAD_COLOR,
        );
    };
    match graph.connection_plan(registry, &from, &to) {
        Ok(plan) if plan.replaced.is_empty() => {
            GraphTooltipRow::colored("Connect", "yes — release to link", GRAPH_TOOLTIP_GOOD_COLOR)
        }
        Ok(_) => GraphTooltipRow::colored(
            "Connect",
            "yes — replaces the link already here",
            GRAPH_TOOLTIP_GOOD_COLOR,
        ),
        Err(error) => GraphTooltipRow::colored(
            "Connect",
            graph_connection_error_text(&error),
            GRAPH_TOOLTIP_BAD_COLOR,
        ),
    }
}

/// A source node with no inputs computes nothing: its output IS its authored
/// field, so the tooltip can show a real value. Anything with inputs is
/// computed per sample and has no single honest number to print.
fn graph_output_static_value(
    record: &NodeRecord,
    declaration: &NodeDeclaration,
    socket: SocketDeclarationStatic,
) -> Option<(PropertyValue, FieldDeclarationStatic)> {
    if !declaration.inputs.is_empty() {
        return None;
    }
    declaration
        .fields
        .iter()
        .find(|field| field.default.value().socket_type() == socket.value_type)
        .map(|field| {
            let value = record
                .properties
                .get(field.key)
                .or_else(|| {
                    record
                        .socket_defaults
                        .get(&SocketKey(field.key.to_string()))
                })
                .cloned()
                .unwrap_or_else(|| field.default.value());
            (value, *field)
        })
}

#[allow(clippy::too_many_arguments)]
fn graph_socket_tooltip(
    graph: &GraphAsset,
    registry: &NodeRegistry,
    reachable: &BTreeSet<NodeId>,
    connector_drag: Option<&ConnectorDrag>,
    node_id: &NodeId,
    declaration: &'static NodeDeclaration,
    record: &NodeRecord,
    socket: SocketDeclarationStatic,
    is_input: bool,
    link_count: usize,
) -> GraphTooltip {
    let mut tooltip = GraphTooltip::new(socket.description);
    tooltip.title = Some(format!(
        "{} · {}",
        socket.label,
        if is_input { "input" } else { "output" }
    ));

    let field = declaration.field(
        if is_input {
            FieldTarget::InputSocket
        } else {
            FieldTarget::Property
        },
        socket.key,
    );
    // The field that describes the value shown, so the scalar bar can mark the
    // right soft and hard ends rather than falling back to 0…1.
    let mut value_field = field;
    let mut effective_value = None;
    if is_input {
        match graph_input_driver(graph, registry, node_id, socket.key) {
            Some(driver) => tooltip.rows.push(GraphTooltipRow::colored(
                "Driven by",
                driver,
                GRAPH_TOOLTIP_ACCENT_COLOR,
            )),
            None => {
                let value = record
                    .socket_defaults
                    .get(&SocketKey(socket.key.to_string()))
                    .cloned()
                    .or_else(|| field.map(|field| field.default.value()));
                tooltip.rows.push(GraphTooltipRow::new(
                    "Value",
                    value
                        .as_ref()
                        .map_or_else(|| "unset".to_string(), graph_value_text),
                ));
                effective_value = value;
            }
        }
    } else if let Some((value, source_field)) =
        graph_output_static_value(record, declaration, socket)
    {
        tooltip
            .rows
            .push(GraphTooltipRow::new("Value", graph_value_text(&value)));
        value_field = Some(source_field);
        effective_value = Some(value);
    }

    tooltip.rows.push(graph_socket_type_row(socket));
    tooltip.rows.push(GraphTooltipRow::new(
        "Links",
        format!(
            "{} · {}",
            graph_socket_capacity_label(socket.cardinality, link_count),
            socket.cardinality.description()
        ),
    ));

    if !is_input {
        let consumers = graph_output_consumers(graph, registry, node_id, socket.key);
        tooltip.rows.push(if consumers.is_empty() {
            GraphTooltipRow::colored(
                "Feeds",
                "nothing — this output is not connected",
                GRAPH_TOOLTIP_BAD_COLOR,
            )
        } else {
            GraphTooltipRow::new("Feeds", consumers.join(", "))
        });
    }

    tooltip.rows.push(graph_effect_row(reachable, node_id));
    if let Some(drag) = connector_drag {
        tooltip.rows.push(graph_connect_gate_row(
            drag, graph, registry, node_id, socket.key, is_input,
        ));
    }

    // A node that draws a preview of this socket's own type teaches more than
    // any generated swatch, so reuse the canvas renderer whenever it applies.
    let preview_type = graph_node_preview_type(record, declaration);
    tooltip.visual = if preview_type == Some(socket.value_type) {
        GraphTooltipVisual::NodePreview {
            record: record.clone(),
            declaration,
            socket_type: socket.value_type,
        }
    } else if let Some(value) = &effective_value {
        graph_tooltip_visual_for_value(value, value_field)
    } else {
        GraphTooltipVisual::None
    };
    tooltip
}

fn graph_node_tooltip(
    graph: &GraphAsset,
    reachable: &BTreeSet<NodeId>,
    node_id: &NodeId,
    declaration: &'static NodeDeclaration,
    record: &NodeRecord,
) -> GraphTooltip {
    let mut tooltip = GraphTooltip::new(declaration.description);
    tooltip.title = Some(declaration.title.to_string());
    let connected_inputs = declaration
        .inputs
        .iter()
        .filter(|socket| {
            graph
                .links
                .values()
                .any(|link| link.to.node == *node_id && link.to.socket.0 == socket.key)
        })
        .count();
    let connected_outputs = declaration
        .outputs
        .iter()
        .filter(|socket| {
            graph
                .links
                .values()
                .any(|link| link.from.node == *node_id && link.from.socket.0 == socket.key)
        })
        .count();
    tooltip.rows.push(GraphTooltipRow::new(
        "Category",
        declaration.category.label(),
    ));
    tooltip
        .rows
        .push(GraphTooltipRow::new("Node type", declaration.id));
    tooltip.rows.push(GraphTooltipRow::new(
        "Inputs",
        format!("{connected_inputs}/{} connected", declaration.inputs.len()),
    ));
    tooltip.rows.push(GraphTooltipRow::new(
        "Outputs",
        format!(
            "{connected_outputs}/{} connected",
            declaration.outputs.len()
        ),
    ));
    tooltip.rows.push(graph_effect_row(reachable, node_id));
    if let Some(socket_type) = graph_node_preview_type(record, declaration) {
        tooltip.visual = GraphTooltipVisual::NodePreview {
            record: record.clone(),
            declaration,
            socket_type,
        };
    }
    tooltip
}

/// The enum pattern from Blender: the description with the CURRENT value
/// inlined in the accent colour, then the selected choice's own description.
fn graph_field_tooltip(field: &FieldDeclarationStatic, value: &PropertyValue) -> GraphTooltip {
    let mut tooltip = GraphTooltip::new(field.description);
    tooltip.title = Some(field.label.to_string());
    let choice = match value {
        PropertyValue::Text(text) => field.choice(text.as_str()),
        _ => None,
    };
    tooltip.prose_accent = Some(match choice {
        Some(choice) => choice.label.to_string(),
        None => graph_value_text(value),
    });
    if let Some(choice) = choice {
        tooltip.detail = Some(choice.description.to_string());
    }
    tooltip
        .rows
        .push(GraphTooltipRow::new("Value", graph_value_text(value)));
    let socket_type = value.socket_type();
    tooltip.rows.push(
        GraphTooltipRow::new(
            "Type",
            if field.choices.is_empty() {
                socket_type.display_name().to_string()
            } else {
                format!(
                    "{} · {} options",
                    socket_type.display_name(),
                    field.choices.len()
                )
            },
        )
        .with_dot(graph_socket_color(socket_type)),
    );
    if let Some(range) = field.soft_range {
        tooltip.rows.push(GraphTooltipRow::new(
            "Soft range",
            format!("{:.3} … {:.3}", range.min, range.max),
        ));
    }
    if let Some(range) = field.hard_range {
        tooltip.rows.push(GraphTooltipRow::new(
            "Hard range",
            format!("{:.3} … {:.3}", range.min, range.max),
        ));
    }
    if field.read_only {
        tooltip.rows.push(GraphTooltipRow::colored(
            "Editable",
            "no — this value is derived",
            GRAPH_TOOLTIP_BAD_COLOR,
        ));
    }
    tooltip.visual = graph_tooltip_visual_for_value(value, Some(*field));
    tooltip
}

/// The socket right-click menu: what this socket is, what it currently carries,
/// and the three things you can do to it from here. "Add compatible node"
/// deliberately reuses the connector-drag menu rather than growing a second
/// filtered node picker — the filter is already exactly "what can legally
/// connect to this socket".
#[allow(clippy::too_many_arguments)]
fn graph_socket_context_menu(
    response: &egui::Response,
    state: &mut GraphEditorState,
    registry: &NodeRegistry,
    node_id: &NodeId,
    declaration: &'static NodeDeclaration,
    record: &NodeRecord,
    socket: SocketDeclarationStatic,
    is_input: bool,
    socket_point: egui::Pos2,
) {
    response.context_menu(|menu_ui| {
        menu_ui.set_min_width(240.0);
        menu_ui.label(
            egui::RichText::new(format!(
                "{} · {}",
                socket.label,
                if is_input { "input" } else { "output" }
            ))
            .strong(),
        );
        graph_tooltip_prose(menu_ui, socket.description, None);
        let driver = is_input
            .then(|| graph_input_driver(&state.graph, registry, node_id, socket.key))
            .flatten();
        match &driver {
            Some(driver) => {
                menu_ui.colored_label(GRAPH_TOOLTIP_ACCENT_COLOR, format!("Driven by {driver}"));
            }
            None => {
                let value = record
                    .socket_defaults
                    .get(&SocketKey(socket.key.to_string()))
                    .cloned()
                    .or_else(|| {
                        (!is_input)
                            .then(|| graph_output_static_value(record, declaration, socket))
                            .flatten()
                            .map(|(value, _)| value)
                    });
                if let Some(value) = value {
                    menu_ui.colored_label(
                        GRAPH_TOOLTIP_VALUE_COLOR,
                        format!("Value {}", graph_value_text(&value)),
                    );
                }
            }
        }
        menu_ui.separator();

        let attached_links = state
            .graph
            .links
            .iter()
            .filter(|(_, link)| {
                if is_input {
                    link.to.node == *node_id && link.to.socket.0 == socket.key
                } else {
                    link.from.node == *node_id && link.from.socket.0 == socket.key
                }
            })
            .map(|(id, _)| id.clone())
            .collect::<Vec<_>>();
        if menu_ui
            .add_enabled(
                !attached_links.is_empty(),
                egui::Button::new(format!("Disconnect ({})", attached_links.len())),
            )
            .clicked()
        {
            state.apply(
                GraphCommand::Transaction {
                    commands: attached_links
                        .into_iter()
                        .map(|id| GraphCommand::Disconnect { id })
                        .collect(),
                },
                registry,
            );
            state.status = format!("`{}` disconnected", socket.key);
            menu_ui.close();
        }

        let field_default = declaration
            .field(FieldTarget::InputSocket, socket.key)
            .map(|field| field.default.value());
        if menu_ui
            .add_enabled(
                is_input && field_default.is_some(),
                egui::Button::new("Reset to default"),
            )
            .clicked()
        {
            if let Some(value) = field_default {
                state.apply(
                    GraphCommand::SetSocketDefault {
                        node: node_id.clone(),
                        socket: SocketKey(socket.key.to_string()),
                        value,
                    },
                    registry,
                );
                state.status = format!("`{}` reset to its default", socket.key);
            }
            menu_ui.close();
        }

        if menu_ui.button("Add compatible node…").clicked() {
            state.connector_drag = Some(if is_input {
                ConnectorDrag::FromInput(InputPin {
                    node: node_id.clone(),
                    socket: SocketKey(socket.key.to_string()),
                })
            } else {
                ConnectorDrag::FromOutput(OutputPin {
                    node: node_id.clone(),
                    socket: SocketKey(socket.key.to_string()),
                })
            });
            state.connector_menu_filter.clear();
            state.connector_menu_position = Some([socket_point.x, socket_point.y]);
            state.pending_output = None;
            state.status = format!("Choose a node to connect to `{}`", socket.key);
            menu_ui.close();
        }
    });
}

/// A palette row: the node title in normal text, its stable registry id dim
/// behind it. One layout job so both halves live in a single selectable row.
fn graph_palette_row_text(ui: &egui::Ui, title: &str, node_type_id: &str) -> egui::text::LayoutJob {
    let font = egui::TextStyle::Button.resolve(ui.style());
    let mut job = egui::text::LayoutJob::default();
    job.append(
        title,
        0.0,
        egui::TextFormat {
            font_id: font.clone(),
            color: ui.visuals().text_color(),
            ..Default::default()
        },
    );
    job.append(
        node_type_id,
        10.0,
        egui::TextFormat {
            font_id: font,
            color: GRAPH_TOOLTIP_KEY_COLOR,
            ..Default::default()
        },
    );
    job
}

fn graph_socket_position(
    rect: egui::Rect,
    input: bool,
    index: usize,
    zoom: f32,
    preview_height: f32,
) -> egui::Pos2 {
    let row_height = 20.0 * zoom;
    let y = rect.top()
        + (28.0 + 8.0 + preview_height) * zoom
        + index as f32 * row_height
        + row_height * 0.5;
    if input {
        egui::pos2(rect.left(), y)
    } else {
        egui::pos2(rect.right(), y)
    }
}

fn graph_visual_header_hit(visual: &GraphNodeVisual, pointer: egui::Pos2, zoom: f32) -> bool {
    let header = egui::Rect::from_min_max(
        visual.rect.left_top(),
        egui::pos2(visual.rect.right(), visual.rect.top() + 28.0 * zoom),
    );
    header.contains(pointer) && pointer.x < header.right() - 24.0 * zoom
}

fn graph_material_label(material_table: &MaterialTable, slot: u8) -> String {
    let Some(material) = material_table.row(slot) else {
        return format!("{slot:02}  <none>");
    };
    let mut label = format!("{slot:02}  {}", material.name);
    if material.face_roles.is_some() {
        label.push_str("  · faces");
    }
    let layers = material.patterns.active_count();
    if layers > 0 {
        label.push_str(&format!(
            "  · {layers} layer{}",
            if layers == 1 { "" } else { "s" }
        ));
    }
    label
}

fn graph_node_preview_type(
    _record: &NodeRecord,
    declaration: &NodeDeclaration,
) -> Option<SocketType> {
    match declaration.preview {
        NodePreview::None => None,
        NodePreview::ColorWheel | NodePreview::MaterialSphere | NodePreview::ColorRamp => {
            Some(SocketType::Color)
        }
        NodePreview::Noise => Some(SocketType::Scalar),
        NodePreview::Value => declaration.outputs.first().map(|socket| socket.value_type),
    }
}

fn graph_node_preview_height(_record: &NodeRecord, declaration: &NodeDeclaration) -> f32 {
    match declaration.preview {
        NodePreview::None => 0.0,
        NodePreview::ColorWheel => 112.0,
        NodePreview::MaterialSphere => 64.0,
        NodePreview::Noise | NodePreview::ColorRamp => 76.0,
        NodePreview::Value => 46.0,
    }
}

fn draw_graph_node_preview(
    painter: &egui::Painter,
    rect: egui::Rect,
    record: &NodeRecord,
    declaration: &NodeDeclaration,
    socket_type: SocketType,
) {
    painter.rect_filled(rect, 3.0, egui::Color32::from_rgb(23, 25, 29));
    painter.rect_stroke(
        rect,
        3.0,
        egui::Stroke::new(1.0, egui::Color32::from_rgb(12, 13, 15)),
        egui::StrokeKind::Inside,
    );
    if declaration.preview == NodePreview::ColorWheel {
        draw_graph_color_wheel(painter, rect, graph_record_color(record));
        return;
    }
    if declaration.preview == NodePreview::MaterialSphere {
        let center = rect.center() + egui::vec2(-rect.width() * 0.04, rect.height() * 0.05);
        let radius = rect.height().min(rect.width()) * 0.36;
        for step in (1..=12).rev() {
            let t = step as f32 / 12.0;
            let offset = egui::vec2(-radius * 0.22 * (1.0 - t), -radius * 0.28 * (1.0 - t));
            let shade = (80.0 + 130.0 * t) as u8;
            painter.circle_filled(
                center + offset,
                radius * t,
                egui::Color32::from_rgb(shade, shade, shade),
            );
        }
        painter.circle_filled(
            center + egui::vec2(-radius * 0.22, -radius * 0.28),
            radius * 0.12,
            egui::Color32::from_white_alpha(210),
        );
        return;
    }
    if declaration.preview == NodePreview::Noise {
        draw_graph_noise_preview(painter, rect, record);
        return;
    }
    if declaration.preview == NodePreview::ColorRamp {
        draw_graph_color_ramp_preview(painter, rect, record);
        return;
    }
    match socket_type {
        SocketType::Color => {
            let color = graph_record_color(record);
            painter.rect_filled(
                rect.shrink2(egui::vec2(3.0, 3.0)),
                2.0,
                egui::Color32::from_rgba_unmultiplied(
                    (color[0].clamp(0.0, 1.0) * 255.0) as u8,
                    (color[1].clamp(0.0, 1.0) * 255.0) as u8,
                    (color[2].clamp(0.0, 1.0) * 255.0) as u8,
                    (color[3].clamp(0.0, 1.0) * 255.0) as u8,
                ),
            );
        }
        SocketType::Scalar => {
            let value = graph_record_scalar(record).clamp(0.0, 1.0);
            let segments = 16;
            for index in 0..segments {
                let left = rect.left() + rect.width() * index as f32 / segments as f32;
                let right = rect.left() + rect.width() * (index + 1) as f32 / segments as f32;
                let shade = (255.0 * index as f32 / (segments - 1) as f32) as u8;
                painter.rect_filled(
                    egui::Rect::from_min_max(
                        egui::pos2(left, rect.top()),
                        egui::pos2(right, rect.bottom()),
                    ),
                    0.0,
                    egui::Color32::from_gray(shade),
                );
            }
            let marker_x = rect.left() + rect.width() * value;
            painter.line_segment(
                [
                    egui::pos2(marker_x, rect.top()),
                    egui::pos2(marker_x, rect.bottom()),
                ],
                egui::Stroke::new(2.0, egui::Color32::from_rgb(255, 180, 65)),
            );
        }
        SocketType::Vector3 => {
            let colors = [
                egui::Color32::from_rgb(180, 75, 75),
                egui::Color32::from_rgb(75, 180, 95),
                egui::Color32::from_rgb(75, 110, 210),
            ];
            for (index, color) in colors.into_iter().enumerate() {
                let row = egui::Rect::from_min_size(
                    egui::pos2(
                        rect.left() + 4.0,
                        rect.top() + 3.0 + index as f32 * (rect.height() - 6.0) / 3.0,
                    ),
                    egui::vec2(rect.width() - 8.0, (rect.height() - 9.0) / 3.0),
                );
                painter.rect_filled(row, 2.0, color);
            }
        }
        _ => {
            painter.text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "preview",
                egui::FontId::proportional(9.0),
                egui::Color32::from_gray(160),
            );
        }
    }
}

fn draw_graph_noise_preview(painter: &egui::Painter, rect: egui::Rect, record: &NodeRecord) {
    let columns = 28;
    let rows = 12;
    let scale = graph_record_scalar_named(record, "scale", 1.0).max(0.01);
    let detail_key = if record
        .socket_defaults
        .contains_key(&SocketKey("octaves".into()))
    {
        "octaves"
    } else {
        "detail"
    };
    let octaves = graph_record_scalar_named(record, detail_key, 4.0).clamp(1.0, 8.0);
    let roughness = graph_record_scalar_named(record, "roughness", 0.5).clamp(0.0, 1.0);
    for row in 0..rows {
        for column in 0..columns {
            let x = column as f32 / columns as f32;
            let y = row as f32 / rows as f32;
            let value =
                graph_preview_fbm([x * 4.0 * scale, y * 4.0 * scale, 0.0], octaves, roughness);
            let shade = (value.clamp(0.0, 1.0) * 255.0) as u8;
            let cell = egui::Rect::from_min_max(
                egui::pos2(
                    rect.left() + rect.width() * column as f32 / columns as f32,
                    rect.top() + rect.height() * row as f32 / rows as f32,
                ),
                egui::pos2(
                    rect.left() + rect.width() * (column + 1) as f32 / columns as f32 + 0.4,
                    rect.top() + rect.height() * (row + 1) as f32 / rows as f32 + 0.4,
                ),
            );
            painter.rect_filled(cell, 0.0, egui::Color32::from_gray(shade));
        }
    }
}

fn draw_graph_color_ramp_preview(painter: &egui::Painter, rect: egui::Rect, record: &NodeRecord) {
    let color_a = graph_record_color_named(record, "color_a", [0.08, 0.2, 0.03, 1.0]);
    let color_b = graph_record_color_named(record, "color_b", [0.55, 0.8, 0.12, 1.0]);
    let position_a = graph_record_scalar_named(record, "position_a", 0.25);
    let position_b = graph_record_scalar_named(record, "position_b", 0.75);
    let span = (position_b - position_a).abs().max(0.000001);
    let columns = 32;
    for column in 0..columns {
        let factor = column as f32 / (columns - 1) as f32;
        let t = ((factor - position_a) / span).clamp(0.0, 1.0);
        let color: [f32; 4] =
            std::array::from_fn(|index| color_a[index] * (1.0 - t) + color_b[index] * t);
        let column_rect = egui::Rect::from_min_max(
            egui::pos2(
                rect.left() + rect.width() * column as f32 / columns as f32,
                rect.top(),
            ),
            egui::pos2(
                rect.left() + rect.width() * (column + 1) as f32 / columns as f32 + 0.5,
                rect.bottom(),
            ),
        );
        painter.rect_filled(
            column_rect,
            0.0,
            egui::Color32::from_rgba_unmultiplied(
                (color[0].clamp(0.0, 1.0) * 255.0) as u8,
                (color[1].clamp(0.0, 1.0) * 255.0) as u8,
                (color[2].clamp(0.0, 1.0) * 255.0) as u8,
                (color[3].clamp(0.0, 1.0) * 255.0) as u8,
            ),
        );
    }
}

fn graph_preview_hash(point: [f32; 3]) -> f32 {
    (point[0] * 127.1 + point[1] * 311.7 + point[2] * 74.7)
        .sin()
        .mul_add(43_758.547, 0.0)
        .fract()
        .abs()
}

fn graph_preview_value_noise(point: [f32; 3]) -> f32 {
    let cell = point.map(f32::floor);
    let local = point.map(f32::fract);
    let blend = local.map(|value| value * value * (3.0 - 2.0 * value));
    let sample =
        |dx: f32, dy: f32, dz: f32| graph_preview_hash([cell[0] + dx, cell[1] + dy, cell[2] + dz]);
    let x00 = sample(0.0, 0.0, 0.0) * (1.0 - blend[0]) + sample(1.0, 0.0, 0.0) * blend[0];
    let x10 = sample(0.0, 1.0, 0.0) * (1.0 - blend[0]) + sample(1.0, 1.0, 0.0) * blend[0];
    let x01 = sample(0.0, 0.0, 1.0) * (1.0 - blend[0]) + sample(1.0, 0.0, 1.0) * blend[0];
    let x11 = sample(0.0, 1.0, 1.0) * (1.0 - blend[0]) + sample(1.0, 1.0, 1.0) * blend[0];
    let y0 = x00 * (1.0 - blend[1]) + x10 * blend[1];
    let y1 = x01 * (1.0 - blend[1]) + x11 * blend[1];
    y0 * (1.0 - blend[2]) + y1 * blend[2]
}

fn graph_preview_fbm(point: [f32; 3], octaves: f32, roughness: f32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut normalisation = 0.0;
    for octave in 0..8 {
        if (octave as f32) < octaves {
            total += graph_preview_value_noise(point.map(|value| value * frequency)) * amplitude;
            normalisation += amplitude;
        }
        frequency *= 2.0;
        amplitude *= roughness;
    }
    if normalisation > 0.0 {
        total / normalisation
    } else {
        0.0
    }
}

fn draw_graph_color_wheel(painter: &egui::Painter, rect: egui::Rect, color: [f32; 4]) {
    let wheel_radius = rect.height().min(rect.width() * 0.62) * 0.42;
    let wheel_center = egui::pos2(rect.left() + wheel_radius + 7.0, rect.center().y);
    let segments = 32;
    for segment in 0..segments {
        let start = segment as f32 / segments as f32 * std::f32::consts::TAU;
        let end = (segment + 1) as f32 / segments as f32 * std::f32::consts::TAU;
        let points = vec![
            wheel_center,
            wheel_center + egui::vec2(start.cos() * wheel_radius, start.sin() * wheel_radius),
            wheel_center + egui::vec2(end.cos() * wheel_radius, end.sin() * wheel_radius),
        ];
        painter.add(egui::Shape::convex_polygon(
            points,
            egui::Color32::from_rgb(
                (hsv_to_rgb(segment as f32 / segments as f32, 1.0, 1.0)[0] * 255.0) as u8,
                (hsv_to_rgb(segment as f32 / segments as f32, 1.0, 1.0)[1] * 255.0) as u8,
                (hsv_to_rgb(segment as f32 / segments as f32, 1.0, 1.0)[2] * 255.0) as u8,
            ),
            egui::Stroke::NONE,
        ));
    }
    painter.circle_filled(
        wheel_center,
        wheel_radius * 0.58,
        egui::Color32::from_white_alpha(215),
    );
    let marker = egui::pos2(
        wheel_center.x + (color[0] - color[1]) * wheel_radius * 0.2,
        wheel_center.y + (color[2] - color[1]) * wheel_radius * 0.2,
    );
    painter.circle_stroke(
        marker,
        4.0,
        egui::Stroke::new(1.0, egui::Color32::from_gray(40)),
    );
    let value_bar = egui::Rect::from_min_max(
        egui::pos2(rect.right() - 22.0, rect.top() + 7.0),
        egui::pos2(rect.right() - 10.0, rect.bottom() - 7.0),
    );
    for row in 0..16 {
        let t = row as f32 / 15.0;
        let shade = ((1.0 - t) * 255.0) as u8;
        let row_rect = egui::Rect::from_min_max(
            egui::pos2(value_bar.left(), value_bar.top() + value_bar.height() * t),
            egui::pos2(
                value_bar.right(),
                value_bar.top() + value_bar.height() * (t + 1.0 / 15.0).min(1.0),
            ),
        );
        painter.rect_filled(row_rect, 0.0, egui::Color32::from_gray(shade));
    }
    let swatch = egui::Rect::from_min_max(
        egui::pos2(rect.left() + 8.0, rect.bottom() - 17.0),
        egui::pos2(rect.right() - 31.0, rect.bottom() - 5.0),
    );
    painter.rect_filled(
        swatch,
        2.0,
        egui::Color32::from_rgba_unmultiplied(
            (color[0].clamp(0.0, 1.0) * 255.0) as u8,
            (color[1].clamp(0.0, 1.0) * 255.0) as u8,
            (color[2].clamp(0.0, 1.0) * 255.0) as u8,
            (color[3].clamp(0.0, 1.0) * 255.0) as u8,
        ),
    );
}

fn hsv_to_rgb(hue: f32, saturation: f32, value: f32) -> [f32; 3] {
    let h = (hue.rem_euclid(1.0) * 6.0).floor() as i32;
    let f = hue.rem_euclid(1.0) * 6.0 - h as f32;
    let p = value * (1.0 - saturation);
    let q = value * (1.0 - f * saturation);
    let t = value * (1.0 - (1.0 - f) * saturation);
    match h {
        0 => [value, t, p],
        1 => [q, value, p],
        2 => [p, value, t],
        3 => [p, q, value],
        4 => [t, p, value],
        _ => [value, p, q],
    }
}

fn graph_record_color(record: &NodeRecord) -> [f32; 4] {
    record
        .properties
        .values()
        .chain(record.socket_defaults.values())
        .find_map(|value| match value {
            PropertyValue::Color(color) => Some(*color),
            _ => None,
        })
        .unwrap_or([0.35, 0.5, 0.85, 1.0])
}

fn graph_record_scalar(record: &NodeRecord) -> f32 {
    record
        .properties
        .values()
        .chain(record.socket_defaults.values())
        .find_map(|value| match value {
            PropertyValue::Scalar(value) => Some(*value),
            _ => None,
        })
        .unwrap_or(0.5)
}

fn graph_record_scalar_named(record: &NodeRecord, name: &str, fallback: f32) -> f32 {
    record
        .properties
        .get(name)
        .or_else(|| record.socket_defaults.get(&SocketKey(name.to_string())))
        .and_then(|value| match value {
            PropertyValue::Scalar(value) => Some(*value),
            PropertyValue::Integer(value) => Some(*value as f32),
            _ => None,
        })
        .unwrap_or(fallback)
}

fn graph_record_color_named(record: &NodeRecord, name: &str, fallback: [f32; 4]) -> [f32; 4] {
    record
        .properties
        .get(name)
        .or_else(|| record.socket_defaults.get(&SocketKey(name.to_string())))
        .and_then(|value| match value {
            PropertyValue::Color(value) => Some(*value),
            _ => None,
        })
        .unwrap_or(fallback)
}

fn draw_graph_wire(
    painter: &egui::Painter,
    from: egui::Pos2,
    to: egui::Pos2,
    color: egui::Color32,
) {
    let handle = ((to.x - from.x).abs() * 0.45).max(28.0);
    let control_a = egui::pos2(from.x + handle, from.y);
    let control_b = egui::pos2(to.x - handle, to.y);
    let mut points = Vec::with_capacity(25);
    for index in 0..=24 {
        let t = index as f32 / 24.0;
        let inverse = 1.0 - t;
        points.push(egui::pos2(
            inverse.powi(3) * from.x
                + 3.0 * inverse.powi(2) * t * control_a.x
                + 3.0 * inverse * t.powi(2) * control_b.x
                + t.powi(3) * to.x,
            inverse.powi(3) * from.y
                + 3.0 * inverse.powi(2) * t * control_a.y
                + 3.0 * inverse * t.powi(2) * control_b.y
                + t.powi(3) * to.y,
        ));
    }
    painter.add(egui::Shape::line(
        points.clone(),
        egui::Stroke::new(3.0, egui::Color32::from_black_alpha(90)),
    ));
    painter.add(egui::Shape::line(points, egui::Stroke::new(1.7, color)));
}

/// The wire colour code. Two socket types can only ever interconnect when they
/// are the SAME type, so every type needs its own colour: a shared colour is a
/// lie about what will connect. Families are still legible — data primitives,
/// the material reds, the sampled-field blues, the definition teals — but no
/// two entries are byte-identical, and `socket_colors_are_unique` enforces it.
fn graph_socket_color(socket_type: SocketType) -> egui::Color32 {
    match socket_type {
        // Data primitives.
        SocketType::Scalar => egui::Color32::from_rgb(170, 170, 170),
        SocketType::Integer => egui::Color32::from_rgb(110, 190, 120),
        SocketType::Vector3 => egui::Color32::from_rgb(154, 109, 208),
        SocketType::Color => egui::Color32::from_rgb(222, 201, 66),
        SocketType::Boolean => egui::Color32::from_rgb(208, 110, 196),
        SocketType::Text => egui::Color32::from_rgb(232, 232, 232),
        SocketType::Asset => egui::Color32::from_rgb(176, 132, 90),
        // Material reds.
        SocketType::MaterialSurface => egui::Color32::from_rgb(182, 81, 94),
        SocketType::MaterialRole => egui::Color32::from_rgb(192, 115, 82),
        SocketType::MaterialBinding => egui::Color32::from_rgb(206, 146, 120),
        // Field-like: sampled at a point. Blue family.
        SocketType::ScalarField => egui::Color32::from_rgb(85, 156, 205),
        SocketType::MaskField => egui::Color32::from_rgb(104, 186, 214),
        SocketType::VoxelField => egui::Color32::from_rgb(66, 124, 180),
        SocketType::PointField => egui::Color32::from_rgb(120, 205, 232),
        SocketType::SplineField => egui::Color32::from_rgb(48, 102, 150),
        SocketType::BiomeField => egui::Color32::from_rgb(140, 196, 180),
        // Definition-like: a recipe carried around, not sampled. Teal family.
        SocketType::BiomeDefinition => egui::Color32::from_rgb(92, 178, 158),
        SocketType::SurfaceProfile => egui::Color32::from_rgb(72, 150, 138),
        SocketType::SurfaceRule => egui::Color32::from_rgb(120, 196, 176),
        SocketType::Environment => egui::Color32::from_rgb(58, 124, 120),
        // Domain signals and pipeline plumbing.
        SocketType::FeatureSet => egui::Color32::from_rgb(122, 154, 72),
        SocketType::AudioSignal => egui::Color32::from_rgb(160, 91, 186),
        SocketType::AnimationSignal => egui::Color32::from_rgb(191, 87, 151),
        SocketType::QualityProfile => egui::Color32::from_rgb(100, 116, 168),
        SocketType::RenderTarget => egui::Color32::from_rgb(77, 96, 132),
    }
}

/// Every socket type, listed once so the colour test cannot silently skip one.
/// The exhaustive match in `graph_socket_color` fails to compile when a variant
/// is added, and `socket_type_list_is_complete` catches a stale list here.
#[cfg(test)]
const ALL_SOCKET_TYPES: &[SocketType] = &[
    SocketType::Scalar,
    SocketType::Integer,
    SocketType::Vector3,
    SocketType::Color,
    SocketType::Boolean,
    SocketType::Text,
    SocketType::Asset,
    SocketType::MaterialSurface,
    SocketType::MaterialRole,
    SocketType::MaterialBinding,
    SocketType::ScalarField,
    SocketType::MaskField,
    SocketType::VoxelField,
    SocketType::PointField,
    SocketType::SplineField,
    SocketType::BiomeField,
    SocketType::BiomeDefinition,
    SocketType::SurfaceProfile,
    SocketType::SurfaceRule,
    SocketType::Environment,
    SocketType::FeatureSet,
    SocketType::AudioSignal,
    SocketType::AnimationSignal,
    SocketType::QualityProfile,
    SocketType::RenderTarget,
];

fn graph_node_header_color(declaration: &NodeDeclaration) -> egui::Color32 {
    let [red, green, blue] = declaration.category.color();
    egui::Color32::from_rgb(red, green, blue)
}

fn draw_graph_inspector(
    ui: &mut egui::Ui,
    state: &mut GraphEditorState,
    registry: &NodeRegistry,
    node_id: &voxel_graph::NodeId,
) {
    let Some(record) = state.graph.nodes.get(node_id).cloned() else {
        return;
    };
    let Some(declaration) = registry.find(&record.node_type) else {
        ui.label(format!("Unknown node type `{}`", record.node_type.0));
        return;
    };
    ui.collapsing(format!("Inspector — {}", declaration.title), |ui| {
        ui.label(declaration.description);
        if ui.button("Delete node").clicked() {
            state.remove_nodes(vec![node_id.clone()], registry);
            state.selected_node = None;
            state.selected_nodes.clear();
            return;
        }
        for field in declaration.fields {
            let mut value = match field.target {
                FieldTarget::Property => record.properties.get(field.key),
                FieldTarget::InputSocket => record
                    .socket_defaults
                    .get(&SocketKey(field.key.to_string())),
            }
            .cloned()
            .unwrap_or_else(|| field.default.value());
            let changed = ui
                .add_enabled_ui(!field.read_only, |ui| draw_property(ui, field, &mut value))
                .inner;
            if !changed {
                continue;
            }
            match field.target {
                FieldTarget::Property => state.apply(
                    GraphCommand::SetProperty {
                        node: node_id.clone(),
                        property: field.key.to_string(),
                        value,
                    },
                    registry,
                ),
                FieldTarget::InputSocket => state.apply(
                    GraphCommand::SetSocketDefault {
                        node: node_id.clone(),
                        socket: SocketKey(field.key.to_string()),
                        value,
                    },
                    registry,
                ),
            };
        }
        if declaration.fields.is_empty() {
            ui.label("This node has no editable defaults yet.");
        }
    });
}

/// The canvas is a zoomed document, not a normal form. At readable zoom it
/// uses compact, bounded widgets; further out it draws a clipped value summary
/// instead of allowing egui's fixed-size controls to escape their node rows.
fn draw_graph_property(
    ui: &mut egui::Ui,
    field: &FieldDeclarationStatic,
    value: &mut PropertyValue,
    zoom: f32,
) -> bool {
    let before = value.clone();
    ui.scope(|ui| {
        let style = ui.style_mut();
        style.spacing.item_spacing = egui::vec2(4.0 * zoom, 0.0);
        style.spacing.interact_size = egui::vec2(32.0 * zoom, 18.0 * zoom);
        for font in style.text_styles.values_mut() {
            font.size *= zoom;
        }
        ui.horizontal(|ui| {
            let label_width = (ui.available_width() * 0.44).clamp(52.0 * zoom, 96.0 * zoom);
            ui.add_sized(
                egui::vec2(label_width, 18.0 * zoom),
                egui::Label::new(field.label).truncate(),
            )
            .on_hover_ui(|hover_ui| {
                graph_tooltip_body(hover_ui, &graph_field_tooltip(field, &before));
            });
            let control_width = ui.available_width().max(20.0 * zoom);
            match value {
                PropertyValue::Scalar(value) => {
                    let range = field
                        .soft_range
                        .or(field.hard_range)
                        .map(|range| range.min..=range.max)
                        .unwrap_or(-1.0..=1.0);
                    ui.add_sized(
                        egui::vec2(control_width, 18.0 * zoom),
                        egui::Slider::new(value, range).text("").show_value(true),
                    );
                }
                PropertyValue::Color(value) => {
                    let mut rgba = *value;
                    if ui.color_edit_button_rgba_unmultiplied(&mut rgba).changed() {
                        *value = rgba;
                    }
                }
                PropertyValue::Vector3(value) => {
                    let range = field
                        .soft_range
                        .or(field.hard_range)
                        .map(|range| range.min..=range.max)
                        .unwrap_or(-10.0..=10.0);
                    let component_width = (control_width / 3.0 - 2.0 * zoom).max(12.0 * zoom);
                    for component in value {
                        ui.add_sized(
                            egui::vec2(component_width, 18.0 * zoom),
                            egui::DragValue::new(component).range(range.clone()),
                        );
                    }
                }
                PropertyValue::Boolean(value) => {
                    ui.checkbox(value, "");
                }
                PropertyValue::Integer(value) => {
                    let mut widget = egui::DragValue::new(value);
                    if let Some(range) = field.hard_range {
                        widget = widget.range(range.min as i64..=range.max as i64);
                    }
                    if let Some(step) = field.step {
                        widget = widget.speed(step);
                    }
                    ui.add_sized(egui::vec2(control_width, 18.0 * zoom), widget);
                }
                PropertyValue::Text(value) => {
                    if !field.choices.is_empty() {
                        let selected_label = field
                            .choice(value.as_str())
                            .map_or(value.as_str(), |choice| choice.label);
                        egui::ComboBox::from_id_salt(("graph-node-enum", field.key))
                            .width(control_width)
                            .selected_text(selected_label)
                            .show_ui(ui, |ui| {
                                for choice in field.choices {
                                    ui.selectable_value(
                                        value,
                                        choice.value.to_string(),
                                        choice.label,
                                    )
                                    .on_hover_text(choice.description);
                                }
                            });
                    } else {
                        ui.add_sized(
                            egui::vec2(control_width, 18.0 * zoom),
                            egui::TextEdit::singleline(value),
                        );
                    }
                }
                PropertyValue::Asset(value) => {
                    ui.add_sized(
                        egui::vec2(control_width, 18.0 * zoom),
                        egui::Label::new(&value.0).truncate(),
                    );
                }
            }
        });
    });
    *value != before
}

fn draw_graph_property_summary(
    painter: &egui::Painter,
    rect: egui::Rect,
    field: &FieldDeclarationStatic,
    value: &PropertyValue,
    zoom: f32,
) {
    let summary = match value {
        PropertyValue::Scalar(value) => format!("{}  {value:.3}", field.label),
        PropertyValue::Color([red, green, blue, _]) => format!(
            "{}  #{:02X}{:02X}{:02X}",
            field.label,
            (red.clamp(0.0, 1.0) * 255.0) as u8,
            (green.clamp(0.0, 1.0) * 255.0) as u8,
            (blue.clamp(0.0, 1.0) * 255.0) as u8,
        ),
        PropertyValue::Vector3([x, y, z]) => format!("{}  {x:.1}, {y:.1}, {z:.1}", field.label),
        PropertyValue::Boolean(value) => {
            format!("{}  {}", field.label, if *value { "on" } else { "off" })
        }
        PropertyValue::Integer(value) => format!("{}  {value}", field.label),
        PropertyValue::Text(value) => format!(
            "{}  {}",
            field.label,
            field
                .choice(value.as_str())
                .map_or(value.as_str(), |choice| choice.label)
        ),
        PropertyValue::Asset(value) => format!("{}  {}", field.label, value.0),
    };
    painter.with_clip_rect(rect).text(
        rect.left_center() + egui::vec2(4.0 * zoom, 0.0),
        egui::Align2::LEFT_CENTER,
        summary,
        egui::FontId::proportional((11.0 * zoom).max(5.0)),
        egui::Color32::from_gray(205),
    );
}

fn draw_property(
    ui: &mut egui::Ui,
    field: &FieldDeclarationStatic,
    value: &mut PropertyValue,
) -> bool {
    let before = value.clone();
    ui.horizontal(|ui| {
        ui.label(field.label).on_hover_ui(|hover_ui| {
            graph_tooltip_body(hover_ui, &graph_field_tooltip(field, &before));
        });
        match value {
            PropertyValue::Scalar(value) => {
                let range = field
                    .soft_range
                    .or(field.hard_range)
                    .map(|range| range.min..=range.max)
                    .unwrap_or(-1.0..=1.0);
                ui.add(egui::Slider::new(value, range).text("").show_value(true));
            }
            PropertyValue::Color(value) => {
                let mut rgba = *value;
                if ui.color_edit_button_rgba_unmultiplied(&mut rgba).changed() {
                    *value = rgba;
                }
            }
            PropertyValue::Vector3(value) => {
                let range = field
                    .soft_range
                    .or(field.hard_range)
                    .map(|range| range.min..=range.max)
                    .unwrap_or(-10.0..=10.0);
                for component in value {
                    ui.add(egui::Slider::new(component, range.clone()).show_value(true));
                }
            }
            PropertyValue::Boolean(value) => {
                ui.checkbox(value, "on");
            }
            PropertyValue::Integer(value) => {
                let mut widget = egui::DragValue::new(value);
                if let Some(range) = field.hard_range {
                    widget = widget.range(range.min as i64..=range.max as i64);
                }
                if let Some(step) = field.step {
                    widget = widget.speed(step);
                }
                ui.add(widget);
            }
            PropertyValue::Text(value) => {
                if !field.choices.is_empty() {
                    let selected_label = field
                        .choice(value.as_str())
                        .map_or(value.as_str(), |choice| choice.label);
                    egui::ComboBox::from_id_salt(("graph-enum", field.key))
                        .selected_text(selected_label)
                        .show_ui(ui, |ui| {
                            for choice in field.choices {
                                ui.selectable_value(value, choice.value.to_string(), choice.label)
                                    .on_hover_text(choice.description);
                            }
                        });
                } else {
                    ui.text_edit_singleline(value);
                }
            }
            PropertyValue::Asset(value) => {
                ui.label(&value.0);
            }
        }
    });
    *value != before
}

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
#[allow(clippy::too_many_arguments)]
fn draw_output_depth(
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
fn draw_quality_section(ui: &mut egui::Ui, quality: &mut RenderQuality) {
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

fn format_pass_milliseconds(milliseconds: Option<f32>) -> String {
    match milliseconds {
        Some(value) => format!("{value:.2} ms"),
        None => "-- ms".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rolling_samples_keep_the_newest_values() {
        let mut samples = VecDeque::new();
        for sample in [1.0, 2.0, 3.0] {
            push_rolling_sample(&mut samples, 2, sample);
        }

        assert_eq!(samples, VecDeque::from([2.0, 3.0]));
        assert_eq!(rolling_average(&samples), Some(2.5));
        assert_eq!(rolling_average(&VecDeque::new()), None);
    }

    /// Sockets only ever interconnect when their types are identical, so any
    /// two types sharing a colour make the wire colour a lie about what will
    /// connect — which is exactly what `Scalar` and `Color` used to do.
    #[test]
    fn socket_colors_are_unique() {
        for (index, left) in ALL_SOCKET_TYPES.iter().enumerate() {
            for right in &ALL_SOCKET_TYPES[index + 1..] {
                assert_ne!(
                    graph_socket_color(*left),
                    graph_socket_color(*right),
                    "{left:?} and {right:?} cannot interconnect but share a wire colour",
                );
            }
        }
    }

    #[test]
    fn socket_type_list_is_complete() {
        let mut unique = BTreeSet::new();
        for socket_type in ALL_SOCKET_TYPES {
            assert!(
                unique.insert(format!("{socket_type:?}")),
                "{socket_type:?} listed twice",
            );
        }
        assert_eq!(
            ALL_SOCKET_TYPES.len(),
            25,
            "a socket type was added — list it in ALL_SOCKET_TYPES and give it its own colour",
        );
    }
}
