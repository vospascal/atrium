//! voxel-rt: standalone ray-traced voxel renderer (winit + wgpu + egui).
//! This file is the thin platform layer — window, event loop, and the raw
//! input -> [`camera::CameraInput`] mapping — so it can be swapped for an
//! OpenXR entry point later without touching the renderer. All winit types
//! stay in this file; camera.rs is pure math.

use std::sync::Arc;
use std::time::{Duration, Instant};

use voxel_core::world::{Voxel, VoxelWorld};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use voxel_rt::brickmap::Brickmap;
use voxel_rt::camera::{CameraInput, CameraPose, FlyCamera};
use voxel_rt::character::CharacterController;
use voxel_rt::debug_pool::WaterPool;
use voxel_rt::frame_timing::{GpuFrameTimers, SPAN_POST};
use voxel_rt::gpu::GpuContext;
use voxel_rt::lighting::SunSettings;
use voxel_rt::overlay::{MovementReadout, Overlay, OverlayFrameData, WorldEditReadout};
use voxel_rt::passes::cagi::AttributeSource;
use voxel_rt::passes::{cagi, dda};
use voxel_rt::render::Renderer;
use voxel_rt::variants::{QualityPreset, RenderQuality, QUALITY_PRESETS};
use voxel_rt::voxel_dda;
use voxel_rt::world_edit::{BulkEdit, BulkEditRequest, VoxelEdit};
use voxel_rt::world_host::{WorldHost, WorldUpdate};

/// World generation parameters, matching voxel-sandbox's defaults
/// (`WorldSeed(1)`, season 0.0 = high summer) so both renderers show the
/// same island.
const WORLD_SEED: u32 = 1;
const WORLD_SEASON: f32 = 0.0;

/// Movement speed multiplier while a Ctrl key is held.
const BOOST_SPEED_MULTIPLIER: f32 = 4.0;

/// How far the edit ray reaches from the eye, world meters (E2). Long enough to
/// build across a clearing, short enough that a click always has an obvious
/// target.
const EDIT_REACH_METERS: f32 = 24.0;

/// Hold-to-repeat rate for place/remove, edits per second. 8/s is fast enough to
/// carve a tunnel by holding the button and slow enough that one click is one
/// voxel — and it is a platform-layer input feel, not a lever (the registry holds
/// what the pipeline's COST depends on).
const EDIT_REPEAT_HZ: f32 = 8.0;

/// What a right click places. Emissive materials are E5 and out of scope here.
const PLACE_MATERIAL: Voxel = Voxel::Stone;

/// E2b — how far below the camera entering walk mode looks for ground, world
/// meters. The world is only 32 m tall, so this always reaches the terrain (or
/// proves there is none, over the void outside the island).
const WALK_GROUND_SEARCH_METERS: f32 = 64.0;

/// E2b — which movement model drives the view. Fly stays the default: bench and
/// dev work want a camera that goes anywhere, and walk mode is for judging the
/// look from eye level and for the presence work E9 needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ControlMode {
    Fly,
    Walk,
}

/// A body whose head is where the fly camera's eye is, looking the same way and
/// carrying the same look/FOV feel — the fly -> walk handover. The ground snap is
/// the caller's next step (it needs the world).
fn character_from_fly_camera(fly_camera: &FlyCamera) -> CharacterController {
    let mut character =
        CharacterController::from_eye(fly_camera.position, fly_camera.yaw, fly_camera.pitch);
    character.mouse_sensitivity = fly_camera.mouse_sensitivity;
    character.vertical_fov_radians = fly_camera.vertical_fov_radians;
    character
}

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
    /// E2: left mouse held (remove) / right mouse held (place), and when the next
    /// hold-to-repeat edit is due.
    remove_held: bool,
    place_held: bool,
    next_repeat_edit: Option<Instant>,
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
    /// E2 — the world authority. Owns the CPU brickmap (on the world thread when
    /// the lever says so), applies edits, and hands back GPU deltas. Also the read
    /// side voxel picking traverses, and the handle atrium's audio resolver will
    /// take at E8.
    world_host: WorldHost,
    overlay: Overlay,
    /// GPU pass timers; `None` when the adapter lacks TIMESTAMP_QUERY (the
    /// overlay then reports the readout as unavailable).
    frame_timers: Option<GpuFrameTimers>,
    fly_camera: FlyCamera,
    /// E2b — the walking body. Rebuilt from the fly camera's eye every time walk
    /// mode is entered, so it never holds a stale pose.
    character: CharacterController,
    control_mode: ControlMode,
    /// CPU cost of the last movement + collision step, microseconds (overlay).
    character_step_micros: f32,
    sun_settings: SunSettings,
    /// The sun the light volume was flooded for: a change means every injected
    /// value and every pinned sun-source flag is stale, so the volume is thrown
    /// away and re-flooded (E4's only invalidation — the world is static).
    flooded_sun_settings: SunSettings,
    /// Overlay-mutated quality levers + preset (E1c): traversal, AO, shadows
    /// and the render scale in one struct.
    quality: RenderQuality,
    /// The quality the current DDA pipeline was compiled with and the render
    /// scale was sized for; when a compile-time lever drifts from the live
    /// settings the pipeline is switched (from the prewarmed cache) after the
    /// overlay pass.
    applied_quality: RenderQuality,
    input_state: InputState,
    cursor_grabbed: bool,
    vsync_enabled: bool,
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
        let quality = RenderQuality::default();
        let mut renderer = Renderer::new(
            &gpu_context.device,
            gpu_context.surface_format(),
            gpu_context.surface_config.width,
            gpu_context.surface_config.height,
            &brickmap,
            &quality.global_illumination,
        );
        let light_volume_grid = renderer.light_volume_grid();
        println!(
            "CAGI light volume {}x{}x{} cells at {} voxels ({:.2} m): {:.1} MB \
             (2 x {:.1} MB ping-pong + {:.1} MB attributes)",
            light_volume_grid.size[0],
            light_volume_grid.size[1],
            light_volume_grid.size[2],
            light_volume_grid.cell_voxels,
            light_volume_grid.cell_meters(),
            light_volume_grid.total_bytes() as f32 / 1e6,
            light_volume_grid.volume_bytes() as f32 / 1e6,
            light_volume_grid.volume_bytes() as f32 / 1e6,
        );
        // Precompile every preset's pipeline permutation (E1c): switching
        // preset in-app must never compile a shader mid-frame. Duplicate
        // sources collapse in the cache — Quest and Balanced differ only by
        // runtime knobs (render scale, cell size, iterations), none of which is a
        // shader const.
        let prewarm_start = Instant::now();
        let preset_qualities: Vec<RenderQuality> = QUALITY_PRESETS
            .iter()
            .filter(|spec| spec.preset != QualityPreset::Custom)
            .map(|spec| spec.resolve())
            .collect();
        let dda_shader_sources: Vec<String> = preset_qualities
            .iter()
            .map(dda::build_shader_source)
            .collect();
        let cagi_shader_sources: Vec<String> = preset_qualities
            .iter()
            .map(cagi::build_shader_source)
            .collect();
        let (cached_dda_pipelines, cached_cagi_pipelines) = renderer.prewarm_pipelines(
            &gpu_context.device,
            &dda_shader_sources,
            &cagi_shader_sources,
        );
        println!(
            "{cached_dda_pipelines} shading + {cached_cagi_pipelines} CAGI pipeline \
             permutations cached for {} presets in {:.2?}",
            preset_qualities.len(),
            prewarm_start.elapsed()
        );
        let overlay = Overlay::new(&window, &gpu_context.device, gpu_context.surface_format());
        let frame_timers = GpuFrameTimers::new(&gpu_context.device, &gpu_context.queue);
        if frame_timers.is_none() {
            println!("GPU timestamp queries unsupported — per-pass timings disabled");
        }

        // E2: hand the world to the authority. It starts inline and immediately
        // takes the lever's value, so the shipped default spawns the world thread
        // here and the frame thread stops owning the brickmap.
        let mut world_host = WorldHost::new(brickmap);
        world_host.set_world_thread(quality.world_edit.world_thread);

        Self {
            window,
            gpu_context,
            renderer,
            world_host,
            overlay,
            frame_timers,
            fly_camera: FlyCamera::default(),
            character: character_from_fly_camera(&FlyCamera::default()),
            control_mode: ControlMode::Fly,
            character_step_micros: 0.0,
            sun_settings: SunSettings::default(),
            flooded_sun_settings: SunSettings::default(),
            quality,
            applied_quality: quality,
            input_state: InputState::default(),
            cursor_grabbed: false,
            vsync_enabled: true,
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
            // E2b: F swaps the fly camera for the walking body. The only free
            // single key near WASD, and it reads as "fly".
            KeyCode::KeyF => {
                if pressed {
                    self.toggle_control_mode();
                }
            }
            // E2b test tool: P carves a swimmable pool ahead of the eye. Free
            // (WASD / Space / Shift / Ctrl / Escape / F are the taken keys) and it
            // reads as "pool".
            KeyCode::KeyP => {
                if pressed {
                    self.carve_test_pool();
                }
            }
            _ => {}
        }
    }

    /// E2b — swap movement models, carrying the view across.
    ///
    /// Entering walk mode takes the fly camera's eye as the head, then snaps the
    /// body to the ground below it (and lifts it clear if the camera was inside
    /// terrain). Entering fly mode keeps the eye exactly where the body's head
    /// was, so the frame the switch happens in does not jump.
    fn toggle_control_mode(&mut self) {
        match self.control_mode {
            ControlMode::Fly => {
                let walk_speed = self.character.settings.walk_speed;
                self.character = character_from_fly_camera(&self.fly_camera);
                // The wheel-tuned walk speed is a preference, not a pose: keep it
                // across toggles.
                self.character.settings.walk_speed = walk_speed;
                let found_ground = {
                    let brickmap = self.world_host.read();
                    self.character
                        .snap_to_ground(&brickmap, WALK_GROUND_SEARCH_METERS)
                };
                self.control_mode = ControlMode::Walk;
                println!(
                    "walk mode: feet at {:.2?}{}",
                    self.character.feet_position,
                    if found_ground {
                        ""
                    } else {
                        " (no ground within 64 m — falling)"
                    }
                );
            }
            ControlMode::Walk => {
                self.fly_camera.position = self.character.eye_position();
                self.fly_camera.yaw = self.character.yaw;
                self.fly_camera.pitch = self.character.pitch;
                self.control_mode = ControlMode::Fly;
                println!("fly mode: eye at {:.2?}", self.fly_camera.position);
            }
        }
    }

    /// The eye pose the active movement model produces — what the frame's rays,
    /// the edit ray and (at E8) the audio listener are built from.
    fn active_pose(&self) -> CameraPose {
        match self.control_mode {
            ControlMode::Fly => self.fly_camera.pose(),
            ControlMode::Walk => self.character.pose(),
        }
    }

    /// E2 — one edit at the crosshair: DDA from the eye through the CPU brickmap
    /// ([`voxel_dda::cast`], the same traversal atrium's audio rays will use), then
    /// hand the change to the authority. Never touches the GPU and never blocks:
    /// the read lock is held only for the ray.
    fn edit_at_crosshair(&mut self, place: bool) {
        let pose = self.active_pose();
        let hit = {
            let brickmap = self.world_host.read();
            voxel_dda::cast(
                &brickmap,
                pose.position.to_array(),
                pose.forward.to_array(),
                EDIT_REACH_METERS,
            )
        };
        let Some(hit) = hit else {
            return;
        };
        let (voxel, material) = if place {
            if hit.face_normal == [0, 0, 0] {
                return; // the eye is inside geometry: there is no face to build on
            }
            (hit.face_voxel, PLACE_MATERIAL)
        } else {
            (hit.voxel, Voxel::Air)
        };
        let light_grid = self
            .quality
            .global_illumination
            .enabled
            .then(|| self.renderer.light_volume_grid());
        self.world_host.request_edit(
            VoxelEdit {
                voxel,
                material,
                light_grid,
            },
            &self.quality.world_edit,
        );
    }

    /// E2b test tool — hand the authority a whole POOL to carve
    /// ([`voxel_rt::debug_pool`]), so the swim states can be felt in the app and
    /// not only asserted in tests.
    ///
    /// Same seam as a click: a request in, a coalesced delta out, uploaded by
    /// [`Self::upload_world_updates`]. The frame thread pays for a `Box` here; the
    /// hundreds of thousands of `set_voxel` calls happen on the world thread.
    fn carve_test_pool(&mut self) {
        let pose = self.active_pose();
        let pool = WaterPool::in_front_of(pose.position, pose.forward);
        let light_grid = self
            .quality
            .global_illumination
            .enabled
            .then(|| self.renderer.light_volume_grid());
        println!(
            "carving the {} at voxel ({}, {}), water surface {:.2?}",
            pool.label(),
            pool.centre_voxel_x,
            pool.centre_voxel_z,
            pool.surface_centre(),
        );
        self.world_host.request_bulk_edit(
            BulkEditRequest {
                shape: Box::new(pool),
                light_grid,
            },
            &self.quality.world_edit,
        );
    }

    /// A mouse button changed: enter mouse-look, or start/stop a hold-to-repeat
    /// edit. All winit types stay in this file (platform-layer rule).
    fn handle_mouse_button(&mut self, button: MouseButton, pressed: bool, egui_consumed: bool) {
        let place = match button {
            MouseButton::Left => false,
            MouseButton::Right => true,
            _ => return,
        };
        if !pressed {
            match place {
                true => self.input_state.place_held = false,
                false => self.input_state.remove_held = false,
            }
            if !self.input_state.place_held && !self.input_state.remove_held {
                self.input_state.next_repeat_edit = None;
            }
            return;
        }
        if egui_consumed {
            return;
        }
        if !self.cursor_grabbed {
            // First click enters mouse-look rather than editing, so a click on the
            // window to focus it never carves a hole in the world.
            if !place {
                self.set_cursor_grabbed(true);
            }
            return;
        }
        match place {
            true => self.input_state.place_held = true,
            false => self.input_state.remove_held = true,
        }
        self.edit_at_crosshair(place);
        self.input_state.next_repeat_edit =
            Some(Instant::now() + Duration::from_secs_f32(1.0 / EDIT_REPEAT_HZ));
    }

    /// Hold-to-repeat: one more edit per `1 / EDIT_REPEAT_HZ` while a button is
    /// down. Placing wins over removing if both are held.
    fn apply_held_edits(&mut self, now: Instant) {
        if !self.input_state.place_held && !self.input_state.remove_held {
            return;
        }
        let Some(due) = self.input_state.next_repeat_edit else {
            return;
        };
        if now < due {
            return;
        }
        self.edit_at_crosshair(self.input_state.place_held);
        self.input_state.next_repeat_edit =
            Some(now + Duration::from_secs_f32(1.0 / EDIT_REPEAT_HZ));
    }

    /// E2 — drain the authority's finished work and upload it. The frame thread
    /// does exactly this much: write the touched words, and re-flood the light
    /// volume if geometry moved. It never waits for the world thread.
    fn upload_world_updates(&mut self) {
        let updates = self.world_host.drain();
        if updates.is_empty() {
            return;
        }
        let mut geometry_changed = false;
        let mut needs_full_reupload = false;
        for update in &updates {
            match update {
                WorldUpdate::Delta(delta) => {
                    geometry_changed = true;
                    let upload_start = Instant::now();
                    if !self
                        .renderer
                        .apply_world_delta(&self.gpu_context.queue, delta)
                    {
                        // The brick headroom ran out: the buffers must be
                        // reallocated from the brickmap instead of patched.
                        needs_full_reupload = true;
                    }
                    // A bulk delta is the one case worth a line: it is the only
                    // edit big enough that "did the frame notice?" is a question,
                    // and these are the four numbers that answer it.
                    if delta.voxels_written > 1 {
                        println!(
                            "bulk edit: {} voxels applied off-frame in {:.1} ms, \
                             {:.0} KB in {} uploads costing this frame {:.2} ms",
                            delta.voxels_written,
                            delta.apply_micros / 1000.0,
                            delta.upload_bytes() as f32 / 1024.0,
                            delta.writes.len(),
                            upload_start.elapsed().as_secs_f32() * 1000.0,
                        );
                    }
                }
                WorldUpdate::LightAttributes {
                    grid,
                    attributes,
                    build_micros,
                } => {
                    let installed = self.renderer.write_light_volume_attributes(
                        &self.gpu_context.queue,
                        grid,
                        attributes,
                    );
                    println!(
                        "CAGI attributes rebuilt off-frame in {:.1} ms ({}installed)",
                        build_micros / 1000.0,
                        if installed { "" } else { "NOT " }
                    );
                }
            }
        }
        if needs_full_reupload {
            let brickmap = self.world_host.read();
            self.renderer
                .reupload_world(&self.gpu_context.device, &brickmap);
            println!(
                "brick headroom exhausted — world buffers reallocated ({} bricks, capacity {})",
                brickmap.occupied_brick_count(),
                brickmap.brick_capacity()
            );
        }
        // E5 owns dirty-region re-flooding; E2 does the measured global one.
        if geometry_changed && self.quality.world_edit.gi_reflood {
            self.renderer.mark_light_volume_dirty();
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
        match self.control_mode {
            ControlMode::Fly => self.fly_camera.update(&camera_input, frame_time_seconds),
            ControlMode::Walk => {
                // E2b: the body reads the authoritative brickmap (the same one
                // picking and, at E8, the audio rays read) and holds the read lock
                // only for the sweep.
                let started = Instant::now();
                {
                    let brickmap = self.world_host.read();
                    self.character
                        .step(&brickmap, &camera_input, frame_time_seconds);
                }
                self.character_step_micros = started.elapsed().as_secs_f32() * 1e6;
            }
        }
        // E2: edits are requested BEFORE the frame is encoded and their deltas are
        // uploaded right after, so a click shows up in the very next frame.
        self.apply_held_edits(now);
        self.upload_world_updates();
        let camera_uniform = match self.control_mode {
            ControlMode::Fly => self.fly_camera.gpu_uniform(self.renderer.resolution()),
            ControlMode::Walk => self.character.gpu_uniform(self.renderer.resolution()),
        };
        // Sun sliders and the runtime quality knobs were mutated during LAST
        // frame's overlay pass; a change shows up one frame later, which is
        // imperceptible.
        let lighting_uniform = self
            .sun_settings
            .lighting_uniform(self.quality.shading_params(), self.quality.gi_params());
        // A moved sun invalidates the whole light volume (E4: the world is
        // static, the sun is not). Dragging the slider therefore re-floods every
        // frame of the drag, which is what makes the GI follow the drag instead of
        // lagging a second behind it.
        if self.sun_settings != self.flooded_sun_settings {
            self.renderer.mark_light_volume_dirty();
            self.flooded_sun_settings = self.sun_settings;
        }

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

        // TWO command buffers per frame: the CAGI compute pass gets its own,
        // because Metal resolves pass-boundary timestamp counters to zero once a
        // command buffer holds more than one compute pass (bench doc) — and the
        // per-pass readout is how every gate in this ladder is judged.
        let mut light_volume_encoder =
            self.gpu_context
                .device
                .create_command_encoder(&wgpu::CommandEncoderDescriptor {
                    label: Some("light volume encoder"),
                });
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

        self.renderer.encode_light_volume(
            &self.gpu_context.queue,
            &mut light_volume_encoder,
            &lighting_uniform,
            self.quality.gi_iterations_per_frame(),
            self.frame_timers.as_ref(),
        );
        self.renderer.encode_frame(
            &self.gpu_context.queue,
            &mut encoder,
            &camera_uniform,
            &target_view,
            self.frame_timers.as_ref(),
        );
        let previous_vsync_enabled = self.vsync_enabled;
        let mut carve_test_pool_requested = false;
        let frame_data = OverlayFrameData {
            render_resolution: self.renderer.resolution(),
            gpu_timings,
            world_edit: WorldEditReadout {
                threaded: self.world_host.is_threaded(),
                in_flight: self.world_host.in_flight(),
                stats: self.world_host.stats(),
            },
            movement: MovementReadout {
                walking: self.control_mode == ControlMode::Walk,
                speed_meters_per_second: match self.control_mode {
                    ControlMode::Fly => self.fly_camera.movement_speed,
                    ControlMode::Walk => self.character.settings.walk_speed,
                },
                grounded: self.character.grounded(),
                submersion: self.character.submersion(),
                head_submerged: self.character.head_submerged(),
                step_micros: self.character_step_micros,
            },
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
            &mut self.sun_settings,
            &mut self.quality,
            &mut carve_test_pool_requested,
        );
        let readback_slot = self
            .frame_timers
            .as_ref()
            .and_then(|frame_timers| frame_timers.encode_resolve(&mut encoder));

        self.gpu_context
            .queue
            .submit([light_volume_encoder.finish(), encoder.finish()]);
        if let (Some(frame_timers), Some(slot_index)) = (&self.frame_timers, readback_slot) {
            frame_timers.after_submit(slot_index);
        }
        self.window.pre_present_notify();
        surface_frame.present();
        self.fps_log_frame_count += 1;

        if carve_test_pool_requested {
            self.carve_test_pool();
        }
        if self.vsync_enabled != previous_vsync_enabled {
            self.gpu_context.set_vsync(self.vsync_enabled);
        }
        if self.quality.render_scale != self.renderer.render_scale() {
            self.renderer
                .set_render_scale(&self.gpu_context.device, self.quality.render_scale);
            // set_render_scale clamps — keep the lever value in sync.
            self.quality.render_scale = self.renderer.render_scale();
        }
        if self
            .quality
            .requires_pipeline_rebuild(&self.applied_quality)
        {
            // A prewarmed permutation (every preset) is a hash lookup here; an
            // arbitrary Custom combination compiles once and stays cached.
            self.renderer.set_dda_shader_source(
                &self.gpu_context.device,
                &dda::build_shader_source(&self.quality),
            );
            self.renderer.set_cagi_shader_source(
                &self.gpu_context.device,
                &cagi::build_shader_source(&self.quality),
            );
        }
        // E2: the world-thread lever spawns or stops the authority's thread.
        if self.quality.world_edit.world_thread != self.applied_quality.world_edit.world_thread {
            self.world_host
                .set_world_thread(self.quality.world_edit.world_thread);
        }
        // E4: the CAGI resolution / master lever reallocates the volume; any
        // change to what the CA injects or transports re-floods it.
        if self
            .quality
            .requires_light_volume_rebuild(&self.applied_quality)
        {
            // E2: the ~50 ms attribute build goes to the world thread when there is
            // one — the volume is then allocated zeroed (valid, just unlit) and the
            // flood starts when the attributes arrive, so a resolution switch costs
            // latency instead of a frame hitch.
            let attribute_source = if self.world_host.is_threaded() {
                AttributeSource::Deferred
            } else {
                AttributeSource::BuildNow
            };
            {
                let brickmap = self.world_host.read();
                self.renderer.rebuild_light_volume(
                    &self.gpu_context.device,
                    &brickmap,
                    &self.quality.global_illumination,
                    attribute_source,
                );
            }
            if attribute_source == AttributeSource::Deferred
                && self.quality.global_illumination.enabled
            {
                self.world_host
                    .request_light_attributes(self.renderer.light_volume_grid());
            }
        } else if self
            .quality
            .requires_light_volume_reflood(&self.applied_quality)
        {
            self.renderer.mark_light_volume_dirty();
        }
        self.applied_quality = self.quality;
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
            // Left click enters mouse-look, then REMOVES the aimed voxel; right
            // click PLACES one against the aimed face (E2). Both hold-to-repeat.
            WindowEvent::MouseInput {
                state: button_state,
                button,
                ..
            } => {
                state.handle_mouse_button(
                    button,
                    button_state == ElementState::Pressed,
                    egui_consumed,
                );
            }
            // Mouse wheel tunes the ACTIVE mode's speed: 12 m/s was too fast to
            // line voxels up by eye, so the base is slow and the wheel covers the
            // range. In walk mode the same notch tunes the walk speed (E2b).
            WindowEvent::MouseWheel { delta, .. } => {
                if !egui_consumed {
                    let notches = match delta {
                        MouseScrollDelta::LineDelta(_, vertical_lines) => vertical_lines,
                        MouseScrollDelta::PixelDelta(position) => position.y as f32 / 50.0,
                    };
                    match state.control_mode {
                        ControlMode::Fly => state.fly_camera.adjust_movement_speed(notches),
                        ControlMode::Walk => state.character.adjust_walk_speed(notches),
                    }
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
