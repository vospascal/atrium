//! voxel-rt: standalone ray-traced voxel renderer (winit + wgpu + egui).
//! This file is the thin platform layer — window, event loop, and the raw
//! input -> [`camera::CameraInput`] mapping — so it can be swapped for an
//! OpenXR entry point later without touching the renderer. All winit types
//! stay in this file; camera.rs is pure math.

use std::sync::Arc;
use std::time::Instant;

use voxel_core::world::VoxelWorld;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use voxel_rt::ao::AoSettings;
use voxel_rt::brickmap::Brickmap;
use voxel_rt::camera::{CameraInput, FlyCamera};
use voxel_rt::frame_timing::{GpuFrameTimers, SPAN_POST};
use voxel_rt::gpu::GpuContext;
use voxel_rt::lighting::SunSettings;
use voxel_rt::overlay::{Overlay, OverlayFrameData};
use voxel_rt::render::Renderer;

/// World generation parameters, matching voxel-sandbox's defaults
/// (`WorldSeed(1)`, season 0.0 = high summer) so both renderers show the
/// same island.
const WORLD_SEED: u32 = 1;
const WORLD_SEASON: f32 = 0.0;

/// Movement speed multiplier while a Ctrl key is held.
const BOOST_SPEED_MULTIPLIER: f32 = 4.0;

/// Raw input accumulated between frames, drained into a [`CameraInput`] once
/// per redraw.
#[derive(Default)]
struct InputState {
    forward_held: bool,
    backward_held: bool,
    left_held: bool,
    right_held: bool,
    up_held: bool,
    down_held: bool,
    boost_held: bool,
    /// Mouse motion accumulated since the last frame, pixels.
    mouse_delta: (f32, f32),
}

impl InputState {
    /// Convert to this frame's camera input and reset the per-frame deltas.
    fn drain_camera_input(&mut self) -> CameraInput {
        let camera_input = CameraInput {
            forward: self.forward_held,
            backward: self.backward_held,
            left: self.left_held,
            right: self.right_held,
            up: self.up_held,
            down: self.down_held,
            mouse_delta: self.mouse_delta,
            speed_multiplier: if self.boost_held {
                BOOST_SPEED_MULTIPLIER
            } else {
                1.0
            },
        };
        self.mouse_delta = (0.0, 0.0);
        camera_input
    }
}

struct AppState {
    window: Arc<Window>,
    gpu_context: GpuContext,
    renderer: Renderer,
    overlay: Overlay,
    /// GPU pass timers; `None` when the adapter lacks TIMESTAMP_QUERY (the
    /// overlay then reports the readout as unavailable).
    frame_timers: Option<GpuFrameTimers>,
    fly_camera: FlyCamera,
    sun_settings: SunSettings,
    /// Overlay-mutated AO levers (E1).
    ao_settings: AoSettings,
    /// The AO configuration the current DDA pipeline was compiled with; when
    /// a compile-time field drifts from `ao_settings`, the pipeline is
    /// rebuilt after the overlay pass.
    applied_ao_settings: AoSettings,
    input_state: InputState,
    cursor_grabbed: bool,
    vsync_enabled: bool,
    /// Overlay-mutated render-scale lever; applied to the renderer after the
    /// overlay pass whenever it drifts from the renderer's current scale.
    render_scale: f32,
    previous_frame_time: Instant,
    /// Terminal FPS diagnostic: frames counted since the last 2-second log
    /// line (fps + present mode + host monitor and its refresh rate). The
    /// on-screen FPS can only be judged against the monitor actually pacing
    /// the window — this makes that pairing visible.
    fps_log_timer: Instant,
    fps_log_frame_count: u32,
}

impl AppState {
    fn new(event_loop: &ActiveEventLoop) -> Self {
        let world_start = Instant::now();
        let world = VoxelWorld::generate(WORLD_SEED, WORLD_SEASON);
        println!(
            "world generated in {:.2?} (seed {WORLD_SEED}, season {WORLD_SEASON})",
            world_start.elapsed()
        );
        let brickmap_start = Instant::now();
        let brickmap = Brickmap::build(&world);
        println!(
            "brickmap built in {:.2?} ({} occupied bricks)",
            brickmap_start.elapsed(),
            brickmap.occupied_brick_count()
        );

        let window_attributes = Window::default_attributes()
            .with_title("voxel-rt")
            .with_inner_size(PhysicalSize::new(1280, 720));
        let window = Arc::new(
            event_loop
                .create_window(window_attributes)
                .expect("failed to create window"),
        );

        let gpu_context = GpuContext::new(window.clone());
        let renderer = Renderer::new(
            &gpu_context.device,
            gpu_context.surface_format(),
            gpu_context.surface_config.width,
            gpu_context.surface_config.height,
            &brickmap,
        );
        let overlay = Overlay::new(&window, &gpu_context.device, gpu_context.surface_format());
        let frame_timers = GpuFrameTimers::new(&gpu_context.device, &gpu_context.queue);
        if frame_timers.is_none() {
            println!("GPU timestamp queries unsupported — per-pass timings disabled");
        }
        let render_scale = renderer.render_scale();

        Self {
            window,
            gpu_context,
            renderer,
            overlay,
            frame_timers,
            fly_camera: FlyCamera::default(),
            sun_settings: SunSettings::default(),
            ao_settings: AoSettings::default(),
            applied_ao_settings: AoSettings::default(),
            input_state: InputState::default(),
            cursor_grabbed: false,
            vsync_enabled: true,
            render_scale,
            previous_frame_time: Instant::now(),
            fps_log_timer: Instant::now(),
            fps_log_frame_count: 0,
        }
    }

    fn resize(&mut self, new_size: PhysicalSize<u32>) {
        self.gpu_context.resize(new_size);
        self.renderer
            .resize(&self.gpu_context.device, new_size.width, new_size.height);
    }

    /// Grab-and-hide (mouse-look on) or release-and-show the cursor. Locked
    /// is preferred; Confined is the fallback for platforms without it.
    fn set_cursor_grabbed(&mut self, grabbed: bool) {
        if grabbed {
            if self.window.set_cursor_grab(CursorGrabMode::Locked).is_err() {
                let _ = self.window.set_cursor_grab(CursorGrabMode::Confined);
            }
        } else {
            let _ = self.window.set_cursor_grab(CursorGrabMode::None);
        }
        self.window.set_cursor_visible(!grabbed);
        self.cursor_grabbed = grabbed;
    }

    fn handle_keyboard(&mut self, key_code: KeyCode, pressed: bool) {
        match key_code {
            KeyCode::KeyW => self.input_state.forward_held = pressed,
            KeyCode::KeyS => self.input_state.backward_held = pressed,
            KeyCode::KeyA => self.input_state.left_held = pressed,
            KeyCode::KeyD => self.input_state.right_held = pressed,
            KeyCode::Space => self.input_state.up_held = pressed,
            KeyCode::ShiftLeft | KeyCode::ShiftRight => self.input_state.down_held = pressed,
            KeyCode::ControlLeft | KeyCode::ControlRight => {
                self.input_state.boost_held = pressed;
            }
            KeyCode::Escape => {
                if pressed {
                    self.set_cursor_grabbed(false);
                }
            }
            _ => {}
        }
    }

    fn redraw(&mut self) {
        let now = Instant::now();
        let frame_time_seconds = (now - self.previous_frame_time).as_secs_f32();
        self.previous_frame_time = now;
        self.overlay.record_frame_time(frame_time_seconds);

        // Counted at present time (bottom of this function), so frames that
        // bail early on an outdated surface never inflate the number — the
        // reconfigure transient after a vsync toggle read 248k "fps" when
        // skipped frames were counted here.
        let seconds_since_fps_log = (now - self.fps_log_timer).as_secs_f32();
        if seconds_since_fps_log >= 2.0 {
            let monitor_description = self
                .window
                .current_monitor()
                .map(|monitor| {
                    format!(
                        "{} @ {:.0} Hz",
                        monitor.name().unwrap_or_else(|| "unknown".to_string()),
                        monitor.refresh_rate_millihertz().unwrap_or(0) as f32 / 1000.0
                    )
                })
                .unwrap_or_else(|| "unknown monitor".to_string());
            println!(
                "{:.1} fps | {:?} | {}",
                self.fps_log_frame_count as f32 / seconds_since_fps_log,
                self.gpu_context.surface_config.present_mode,
                monitor_description
            );
            self.fps_log_timer = now;
            self.fps_log_frame_count = 0;
        }

        // A monitor change (dragging between a 1x and a 2x display) updates
        // the window's physical size BEFORE the Resized event reaches us, and
        // macOS delivers this redraw inside draw_rect where a panic aborts.
        // The frame must be internally consistent: sync the surface to the
        // window here, or the egui scissor (built from the window size) can
        // exceed the still-old surface texture — a wgpu validation panic.
        let window_size = self.window.inner_size();
        if window_size.width > 0
            && window_size.height > 0
            && (window_size.width != self.gpu_context.surface_config.width
                || window_size.height != self.gpu_context.surface_config.height)
        {
            self.resize(window_size);
        }

        let camera_input = self.input_state.drain_camera_input();
        self.fly_camera.update(&camera_input, frame_time_seconds);
        let camera_uniform = self.fly_camera.gpu_uniform(self.renderer.resolution());
        // Sun/AO sliders were mutated during LAST frame's overlay pass; a
        // change shows up one frame later, which is imperceptible.
        let lighting_uniform = self
            .sun_settings
            .lighting_uniform(self.ao_settings.strength);

        let surface_frame = match self.gpu_context.surface.get_current_texture() {
            wgpu::CurrentSurfaceTexture::Success(surface_frame)
            | wgpu::CurrentSurfaceTexture::Suboptimal(surface_frame) => surface_frame,
            wgpu::CurrentSurfaceTexture::Outdated | wgpu::CurrentSurfaceTexture::Lost => {
                self.gpu_context.reconfigure();
                return;
            }
            // Timeout / occluded / validation: skip this frame.
            _ => return,
        };
        let target_view = surface_frame
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());

        let mut encoder =
            self.gpu_context
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("frame encoder"),
                });

        // Harvest GPU timings from earlier frames (non-blocking; a value a
        // few frames old is fine for the readout).
        let gpu_timings = self
            .frame_timers
            .as_mut()
            .map(|frame_timers| frame_timers.collect(&self.gpu_context.device));

        self.renderer.encode_frame(
            &self.gpu_context.queue,
            &mut encoder,
            &camera_uniform,
            &lighting_uniform,
            &target_view,
            self.frame_timers.as_ref(),
        );
        let previous_vsync_enabled = self.vsync_enabled;
        let frame_data = OverlayFrameData {
            render_resolution: self.renderer.resolution(),
            gpu_timings,
        };
        self.overlay.render(
            &self.window,
            &self.gpu_context.device,
            &self.gpu_context.queue,
            &mut encoder,
            &target_view,
            &frame_data,
            self.frame_timers
                .as_ref()
                .map(|frame_timers| frame_timers.render_span_end_writes(SPAN_POST)),
            &mut self.vsync_enabled,
            &mut self.render_scale,
            &mut self.sun_settings,
            &mut self.ao_settings,
        );
        let readback_slot = self
            .frame_timers
            .as_ref()
            .and_then(|frame_timers| frame_timers.encode_resolve(&mut encoder));

        self.gpu_context.queue.submit([encoder.finish()]);
        if let (Some(frame_timers), Some(slot_index)) = (&self.frame_timers, readback_slot) {
            frame_timers.after_submit(slot_index);
        }
        self.window.pre_present_notify();
        surface_frame.present();
        self.fps_log_frame_count += 1;

        if self.vsync_enabled != previous_vsync_enabled {
            self.gpu_context.set_vsync(self.vsync_enabled);
        }
        if self.render_scale != self.renderer.render_scale() {
            self.renderer
                .set_render_scale(&self.gpu_context.device, self.render_scale);
            // set_render_scale clamps — keep the slider value in sync.
            self.render_scale = self.renderer.render_scale();
        }
        if self
            .ao_settings
            .requires_pipeline_rebuild(&self.applied_ao_settings)
        {
            self.renderer
                .rebuild_dda_pipeline(&self.gpu_context.device, &self.ao_settings.shader_source());
            self.applied_ao_settings = self.ao_settings;
        }
    }
}

#[derive(Default)]
struct App {
    state: Option<AppState>,
}

impl ApplicationHandler for App {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.state.is_none() {
            self.state = Some(AppState::new(event_loop));
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        _window_id: WindowId,
        event: WindowEvent,
    ) {
        let Some(state) = &mut self.state else {
            return;
        };
        let egui_consumed = state.overlay.handle_window_event(&state.window, &event);

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(new_size) => state.resize(new_size),
            WindowEvent::RedrawRequested => state.redraw(),
            WindowEvent::KeyboardInput { event, .. } => {
                if let PhysicalKey::Code(key_code) = event.physical_key {
                    state.handle_keyboard(key_code, event.state == ElementState::Pressed);
                }
            }
            WindowEvent::MouseInput {
                state: ElementState::Pressed,
                button: MouseButton::Left,
                ..
            } => {
                // Click to enter mouse-look — unless the click was on the
                // overlay (e.g. the vsync checkbox).
                if !egui_consumed && !state.cursor_grabbed {
                    state.set_cursor_grabbed(true);
                }
            }
            _ => {}
        }
    }

    fn device_event(
        &mut self,
        _event_loop: &ActiveEventLoop,
        _device_id: DeviceId,
        event: DeviceEvent,
    ) {
        let Some(state) = &mut self.state else {
            return;
        };
        if let DeviceEvent::MouseMotion { delta } = event {
            if state.cursor_grabbed {
                state.input_state.mouse_delta.0 += delta.0 as f32;
                state.input_state.mouse_delta.1 += delta.1 as f32;
            }
        }
    }

    fn about_to_wait(&mut self, _event_loop: &ActiveEventLoop) {
        if let Some(state) = &mut self.state {
            if state.vsync_enabled {
                state.window.request_redraw();
            } else {
                // macOS delivers RedrawRequested through the display link, so
                // a request_redraw-driven loop can never exceed the monitor's
                // refresh rate regardless of present mode. With vsync off,
                // drive the frame directly from the event loop instead. NOTE
                // (measured): even then macOS pins windowed apps near the
                // refresh rate — the compositor recycles drawables at display
                // cadence — so vsync-off is about latency, not throughput,
                // here. On platforms without that pacing this path uncaps for
                // real; for throughput numbers on macOS use bench_dda (it
                // renders offscreen, no swapchain).
                state.redraw();
            }
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("event loop error");
}
