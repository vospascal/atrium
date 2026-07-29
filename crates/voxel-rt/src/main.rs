//! voxel-rt: standalone ray-traced voxel renderer (winit + wgpu + egui).
//! This file is the thin platform layer — window, event loop, and the raw
//! input -> [`camera::CameraInput`] mapping — so it can be swapped for an
//! OpenXR entry point later without touching the renderer. All winit types
//! stay in this file; camera.rs is pure math.

mod brickmap;
mod camera;
mod gpu;
mod overlay;
mod passes;
mod render;

use std::sync::Arc;
use std::time::Instant;

use voxel_core::world::VoxelWorld;
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{DeviceEvent, DeviceId, ElementState, MouseButton, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use brickmap::Brickmap;
use camera::{CameraInput, FlyCamera};
use gpu::GpuContext;
use overlay::Overlay;
use render::Renderer;

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
    fly_camera: FlyCamera,
    input_state: InputState,
    cursor_grabbed: bool,
    vsync_enabled: bool,
    previous_frame_time: Instant,
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

        Self {
            window,
            gpu_context,
            renderer,
            overlay,
            fly_camera: FlyCamera::default(),
            input_state: InputState::default(),
            cursor_grabbed: false,
            vsync_enabled: true,
            previous_frame_time: Instant::now(),
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

        let camera_input = self.input_state.drain_camera_input();
        self.fly_camera.update(&camera_input, frame_time_seconds);
        let camera_uniform = self.fly_camera.gpu_uniform(self.renderer.resolution());

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

        self.renderer.encode_frame(
            &self.gpu_context.queue,
            &mut encoder,
            &camera_uniform,
            &target_view,
        );
        let previous_vsync_enabled = self.vsync_enabled;
        self.overlay.render(
            &self.window,
            &self.gpu_context.device,
            &self.gpu_context.queue,
            &mut encoder,
            &target_view,
            self.renderer.resolution(),
            &mut self.vsync_enabled,
        );

        self.gpu_context.queue.submit([encoder.finish()]);
        self.window.pre_present_notify();
        surface_frame.present();

        if self.vsync_enabled != previous_vsync_enabled {
            self.gpu_context.set_vsync(self.vsync_enabled);
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
        if let Some(state) = &self.state {
            state.window.request_redraw();
        }
    }
}

fn main() {
    let event_loop = EventLoop::new().expect("failed to create event loop");
    event_loop.set_control_flow(ControlFlow::Poll);
    let mut app = App::default();
    event_loop.run_app(&mut app).expect("event loop error");
}
