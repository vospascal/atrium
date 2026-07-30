//! egui overlay: stats panel (window/render sizes, moving-average frame time,
//! FPS, per-pass GPU times), the perf levers (vsync checkbox, render-scale
//! slider), and a collapsible sun-position section, drawn on top of the
//! rendered frame in its own render pass (LoadOp::Load). The overlay only
//! mutates the state it is handed (`vsync_enabled`, `render_scale`,
//! [`SunSettings`]) — reconfiguring the surface, resizing the storage
//! texture, and writing the lighting uniform stay in the platform layer.

use std::collections::VecDeque;

use winit::event::WindowEvent;
use winit::window::Window;

use crate::frame_timing::FrameTimings;
use crate::lighting::SunSettings;
use crate::render::{MAX_RENDER_SCALE, MIN_RENDER_SCALE};

const FRAME_TIME_SAMPLE_COUNT: usize = 120;

/// Read-only per-frame display data for the stats panel.
pub struct OverlayFrameData {
    /// Storage-texture (ray-traced) resolution, pixels.
    pub render_resolution: (u32, u32),
    /// Latest completed GPU pass timings; `None` when the device has no
    /// timestamp-query support (the panel says so instead of showing numbers).
    pub gpu_timings: Option<FrameTimings>,
}

pub struct Overlay {
    context: egui::Context,
    winit_state: egui_winit::State,
    renderer: egui_wgpu::Renderer,
    frame_time_samples: VecDeque<f32>,
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
            frame_time_samples: VecDeque::with_capacity(FRAME_TIME_SAMPLE_COUNT),
        }
    }

    /// Feed a window event to egui; returns true when egui consumed it (e.g.
    /// a click on the overlay panel) so the platform layer can skip its own
    /// handling.
    pub fn handle_window_event(&mut self, window: &Window, event: &WindowEvent) -> bool {
        self.winit_state.on_window_event(window, event).consumed
    }

    pub fn record_frame_time(&mut self, frame_time_seconds: f32) {
        if self.frame_time_samples.len() == FRAME_TIME_SAMPLE_COUNT {
            self.frame_time_samples.pop_front();
        }
        self.frame_time_samples.push_back(frame_time_seconds);
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
        render_scale: &mut f32,
        sun_settings: &mut SunSettings,
    ) {
        let average_frame_time_seconds = if self.frame_time_samples.is_empty() {
            0.0
        } else {
            self.frame_time_samples.iter().sum::<f32>() / self.frame_time_samples.len() as f32
        };
        let frame_time_milliseconds = average_frame_time_seconds * 1000.0;
        let frames_per_second = if average_frame_time_seconds > 0.0 {
            1.0 / average_frame_time_seconds
        } else {
            0.0
        };

        // Surface the Retina trap: on macOS the swapchain is PHYSICAL pixels,
        // which can be 4x the logical window area at scale factor 2.0.
        let physical_size = window.inner_size();
        let scale_factor = window.scale_factor();
        let logical_size = physical_size.to_logical::<f64>(scale_factor);

        let raw_input = self.winit_state.take_egui_input(window);
        let full_output = self.context.run_ui(raw_input, |root_ui| {
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
                            "{frame_time_milliseconds:.2} ms  |  {frames_per_second:.0} FPS"
                        ));
                        match &frame_data.gpu_timings {
                            Some(timings) => {
                                ui.label(format!(
                                    "DDA pass: {}",
                                    format_pass_milliseconds(timings.dda_milliseconds())
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
                        ui.checkbox(vsync_enabled, "VSync");
                        ui.add(
                            egui::Slider::new(render_scale, MIN_RENDER_SCALE..=MAX_RENDER_SCALE)
                                .text("render scale"),
                        );
                        ui.collapsing("Sun", |ui| {
                            ui.add(
                                egui::Slider::new(&mut sun_settings.azimuth_degrees, 0.0..=360.0)
                                    .text("azimuth"),
                            );
                            ui.add(
                                egui::Slider::new(&mut sun_settings.elevation_degrees, 2.0..=90.0)
                                    .text("elevation"),
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

fn format_pass_milliseconds(milliseconds: Option<f32>) -> String {
    match milliseconds {
        Some(value) => format!("{value:.2} ms"),
        None => "-- ms".to_string(),
    }
}
