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
use voxel_rt::debug_pool::{WaterBlob, WaterPool};
use voxel_rt::frame_timing::{GpuFrameTimers, SPAN_POST};
use voxel_rt::gpu::GpuContext;
use voxel_rt::lighting::SunSettings;
use voxel_rt::material;
use voxel_rt::material_edit::{MaterialPanelState, VoxImportState};
use voxel_rt::material_table::MaterialTable;
use voxel_rt::material_tune::ProvenanceTable;
use voxel_rt::overlay::{MovementReadout, Overlay, OverlayFrameData, WorldEditReadout};
use voxel_rt::passes::cagi::AttributeSource;
use voxel_rt::passes::{cagi, dda};
use voxel_rt::render::Renderer;
use voxel_rt::studio;
use voxel_rt::variants::{QualityPreset, RenderQuality, QUALITY_PRESETS};
use voxel_rt::vox_material;
use voxel_rt::voxel_dda;
use voxel_rt::water;
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

/// S0 — how close and how far the studio orbit can get.
///
/// The floor is two voxels off the sample centre, so the eye never ends up inside
/// the subject; the ceiling still shows the whole plate and no more, because past
/// that the sample is a few pixels and there is nothing left to judge.
const STUDIO_MIN_DISTANCE_METERS: f32 = 0.25;
const STUDIO_MAX_DISTANCE_METERS: f32 = 8.0;

/// S0 — how far the studio orbit can tip. Just short of straight down and straight
/// up: at exactly +/-90 degrees the forward vector is parallel to world up and the
/// `forward x Y` basis is degenerate, which would collapse the frame.
const STUDIO_MIN_PITCH_RADIANS: f32 = -1.5;
const STUDIO_MAX_PITCH_RADIANS: f32 = 1.5;

/// S2 — the voxel the studio should be showing, or `None` to leave it alone.
///
/// Pulled out of [`AppState::follow_selected_row_in_the_studio`] so the decision is
/// testable without a GPU: the method it serves can only run inside a live app, and
/// the three cases that must NOT rebuild the world are exactly the ones worth pinning.
fn studio_sample_to_follow(
    control_mode: ControlMode,
    has_subject: bool,
    selected: u8,
    current_sample: Voxel,
) -> Option<Voxel> {
    // Only in the studio. In the island the selection is the PLACEMENT material, and
    // rebuilding the world because you picked a different block to place would be
    // absurd.
    if control_mode != ControlMode::StudioOrbit {
        return None;
    }
    // A loaded `.vox` model brings its own geometry and its own palette, so there the
    // selection names a row of the table rather than the subject on screen.
    if has_subject {
        return None;
    }
    // Air is the miss sentinel and builds nothing. Selecting row 0 is a legitimate
    // thing to do while inspecting the table and must not empty the studio.
    if selected == material::AIR_MATERIAL_ID {
        return None;
    }
    let sample = material::material_voxel(selected);
    // Already showing it: the caller runs every frame, and rebuilding the world 60
    // times a second because nothing changed would be the worst bug in the file.
    if sample == current_sample {
        return None;
    }
    Some(sample)
}

/// What a right click places — the default building block.
const PLACE_MATERIAL: Voxel = Voxel::Stone;

/// What the L key places (M1b): a plain light-emitting block, so the emissive
/// table rows are reachable in the app and not only from tests.
///
/// It does NOT glow yet — the CA has no emissive injection rule (E5), so this
/// currently builds a pale solid block. Placing one is what gives E5 something
/// to light the world with.
const GLOW_BLOCK_MATERIAL: Voxel = Voxel::GlowBlock;

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
    /// S0 — the material studio's orbit camera: yaw/pitch turn the SUBJECT rather
    /// than the eye, and the wheel pulls in and out.
    ///
    /// A third mode rather than a re-pointed fly camera because the two want
    /// opposite things. A fly camera keeps its position and rotates its view, which
    /// at the studio's 0.9 m working distance swings a 0.125 m voxel straight out of
    /// frame; the studio has to keep the subject centred while you turn it. Reuses
    /// the fly camera's yaw/pitch (mouse-look is identical) and derives the position
    /// from them, so there is no second look-input path.
    StudioOrbit,
}

impl ControlMode {
    /// Whether this mode may change the world (Pascal, 2026-07-31: *"disable
    /// removing editing for now in the studio"*).
    ///
    /// A property of the MODE rather than of the app, so it is a pure function that
    /// can be pinned by a test — and so the answer lives next to the enum a future
    /// mode would be added to, instead of in a method on a struct that needs a GPU
    /// device to exist.
    ///
    /// The studio's scene is composed, not dug: its whole value is that the thing in
    /// frame is *known* — one voxel of a known material on a known plate. A click
    /// that digs the sample away or buries it under stone destroys the only subject
    /// there is, and mouse-look shares the same button hand, so it is one mis-click
    /// away at all times.
    fn allows_world_edits(self) -> bool {
        !matches!(self, ControlMode::StudioOrbit)
    }
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
    /// S0 — the LIVE material rows. Edited by the panel, uploaded once per frame in
    /// which anything changed; starts as the compiled table, which is what
    /// `WorldBindings::new` already sent.
    material_table: MaterialTable,
    /// S0 — what the material panel has selected and asked for.
    material_panel: MaterialPanelState,
    /// S0 — what the studio is showing. Only meaningful in
    /// [`ControlMode::StudioOrbit`], but held unconditionally so the orbit target is
    /// available without an `Option` dance.
    studio_scene: studio::StudioScene,
    /// S0 — the studio orbit radius, world meters (wheel-tuned).
    studio_distance_meters: f32,
    /// S0b — the last loaded `.vox`, kept for its GEOMETRY. The panel works off the
    /// palette side; this is what \`Show model in studio\` builds a subject from.
    loaded_vox: Option<voxel_core::vox::VoxFile>,
    /// S0b — which rows came from a `.vox` and what they looked like when they
    /// arrived, so a re-import can refresh the file's values without discarding
    /// hand tuning.
    material_provenance: ProvenanceTable,
}

impl AppState {
    fn new(event_loop: &ActiveEventLoop) -> Self {
        // S0 — `--studio` swaps the generated island for the material studio: one
        // voxel on a plate, orbit camera. Excludable by construction (the plan's
        // isolation rule): the island path is not merely skipped, no world is
        // generated at all, which is also why the studio starts instantly.
        let studio_mode = std::env::args().any(|argument| argument == "--studio");
        let studio_scene = studio::StudioScene::default();
        let brickmap = if studio_mode {
            let brickmap_start = Instant::now();
            let brickmap = studio_scene.build();
            println!(
                "material studio: {:?} on {:?}, built in {:.2?} ({} occupied bricks)",
                studio_scene.sample,
                studio_scene.plate,
                brickmap_start.elapsed(),
                brickmap.occupied_brick_count()
            );
            brickmap
        } else {
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
            brickmap
        };

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
            control_mode: if studio_mode {
                ControlMode::StudioOrbit
            } else {
                ControlMode::Fly
            },
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
            // Selecting the sample's own row means the panel opens on the thing the
            // camera is pointed at, which is the only row worth defaulting to.
            material_panel: MaterialPanelState {
                selected: material::material_id(studio_scene.sample),
                import: VoxImportState::new(),
                ..MaterialPanelState::default()
            },
            material_table: MaterialTable::default(),
            studio_scene,
            studio_distance_meters: studio::CAMERA_DISTANCE_METERS,
            loaded_vox: None,
            material_provenance: ProvenanceTable::default(),
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
            // Water test tools, both acting ON THE CROSSHAIR like every other
            // edit: P carves a swimmable pool (E2b), Shift+P spawns a
            // free-standing body of the same size (E6's isolated optics target).
            // Shift is already tracked as `down_held`, so this needs no modifier
            // plumbing of its own.
            KeyCode::KeyP => {
                if pressed {
                    match self.input_state.down_held {
                        true => self.spawn_test_water_body(),
                        false => self.carve_test_pool(),
                    }
                }
            }
            // M1b: L places a glow block against the aimed face — the same edit
            // a right click makes, with the emissive material instead of stone.
            // Deliberately NOT hold-to-repeat: a light source is placed one at a
            // time, and repeating would stack a column of them on one hold.
            KeyCode::KeyL => {
                if pressed {
                    self.edit_at_crosshair(Some(GLOW_BLOCK_MATERIAL));
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
            // The studio is a mode you LAUNCH into (`--studio`), not one you walk
            // out of: there is one voxel on a plate and nothing to walk to, and
            // handing the body a 3 m plate floating at world centre would be a
            // pointless way to fall off it. Left as an explicit no-op rather than
            // silently falling through, so the intent is readable.
            ControlMode::StudioOrbit => {
                println!("studio orbit: movement modes are disabled (restart without --studio)");
            }
        }
    }

    /// Whether edits may change the world right now — [`ControlMode::allows_world_edits`]
    /// for the active mode.
    ///
    /// ONE predicate that every edit entry point consults, rather than a guard bolted
    /// onto each, for the same reason the edit path has one notion of emptiness: the
    /// studio grows more poses in later stages (S2's wall and cube), and every one of
    /// them wants this answer without anybody remembering to add another branch.
    ///
    /// Deliberately does NOT disable the eyedropper: picking a material reads the
    /// world instead of writing it, and it is the fastest way to select the plate's
    /// row. That distinction is why this gates the *edit*, not the *click*.
    fn world_edits_allowed(&self) -> bool {
        self.control_mode.allows_world_edits()
    }

    /// The eye pose the active movement model produces — what the frame's rays,
    /// the edit ray and (at E8) the audio listener are built from.
    fn active_pose(&self) -> CameraPose {
        match self.control_mode {
            ControlMode::Fly => self.fly_camera.pose(),
            ControlMode::Walk => self.character.pose(),
            ControlMode::StudioOrbit => studio::orbit_pose(
                &self.studio_scene,
                self.fly_camera.yaw,
                self.fly_camera.pitch,
                self.studio_distance_meters,
            ),
        }
    }

    /// E2 — one edit at the crosshair: DDA from the eye through the CPU brickmap
    /// ([`voxel_dda::cast`], the same traversal atrium's audio rays will use), then
    /// hand the change to the authority. Never touches the GPU and never blocks:
    /// the read lock is held only for the ray.
    ///
    /// **To the editor, water IS air** (the plan's rule, E6): the ray is
    /// [`voxel_dda::CastTarget::EditableVoxel`] in BOTH directions — one predicate,
    /// no per-direction liquid handling. A click into a pond therefore lands on the
    /// bed rather than on the skin: removing takes the bed voxel, placing puts the
    /// new block in the water cell against it and displaces the water. Placing a
    /// lantern in a submerged niche is the same click as placing one in a cave.
    fn edit_at_crosshair(&mut self, placed_material: Option<Voxel>) {
        let pose = self.active_pose();
        let hit = {
            let brickmap = self.world_host.read();
            voxel_dda::cast(
                &brickmap,
                pose.position.to_array(),
                pose.forward.to_array(),
                EDIT_REACH_METERS,
                voxel_dda::CastTarget::EditableVoxel,
            )
        };
        let Some(hit) = hit else {
            return;
        };
        // S0 — the eyedropper, which costs almost nothing because the cast above
        // already carries the hit voxel's material byte. Consumes the click: while
        // armed, picking a material is what a click MEANS, so it must not also dig a
        // hole in the thing you were trying to inspect. One-shot, so the next click
        // is an ordinary edit again.
        if std::mem::take(&mut self.material_panel.eyedropper_armed) {
            self.material_panel.selected = hit.material;
            let name = self
                .material_table
                .row(hit.material)
                .map_or("<none>", |row| row.name);
            println!("eyedropper: material {} ({name})", hit.material);
            return;
        }
        // Checked AFTER the eyedropper on purpose: a pick is a read, and it is the
        // one click the studio still wants.
        if !self.world_edits_allowed() {
            return;
        }
        let (voxel, material) = if let Some(placed_material) = placed_material {
            if hit.face_normal == [0, 0, 0] {
                return; // the eye is inside geometry: there is no face to build on
            }
            (hit.face_voxel, placed_material)
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

    /// S0 — push the frame's material edits to the GPU, and service a requested
    /// CAGI re-pack.
    ///
    /// Runs AFTER the overlay pass, because that is where the panel mutated the
    /// table: an edit therefore shows up in the next frame, which is imperceptible
    /// on a slider drag and is the same one-frame latency the sun and quality knobs
    /// already have.
    ///
    /// The two tiers the panel advertises are both honoured here:
    ///
    /// * the material table itself is 2080 bytes and goes straight out, gated by
    ///   [`MaterialTable::take_dirty`] so an idle panel costs nothing;
    /// * CAGI's baked cell attributes are a ~50 ms rebuild, so a re-pack is handed
    ///   to the world thread through the SAME seam an edit's light attributes use
    ///   and lands via `WorldUpdate::LightAttributes`. Never automatic on a slider
    ///   tick — that would be a hitch per pixel of mouse travel.
    fn upload_material_edits(&mut self) {
        if let Some(rows) = self.material_table.take_dirty() {
            self.renderer
                .write_material_table(&self.gpu_context.queue, &rows);
        }
        if std::mem::take(&mut self.material_panel.repack_gi_requested)
            && self.quality.global_illumination.enabled
        {
            self.world_host
                .request_light_attributes(self.renderer.light_volume_grid());
        }
        if std::mem::take(&mut self.material_panel.import.load_requested) {
            self.load_vox_palette();
        }
        if std::mem::take(&mut self.material_panel.import.show_in_studio_requested) {
            self.show_vox_model_in_studio();
        }
        if let Some(pose) = self.material_panel.studio_pose_requested.take() {
            self.set_studio_pose(pose);
        }
        self.follow_selected_row_in_the_studio();
    }

    /// S2 — in the studio, the row being EDITED is the voxel being LOOKED AT.
    ///
    /// The panel used to only seed its selection from the sample once at startup, and
    /// after that the two drifted apart silently: picking `stone` in the dropdown
    /// edited stone's row while the camera went on showing a grass voxel, so every
    /// slider appeared to do nothing (Pascal, 2026-07-31: *"it doesnt re apply i only
    /// ever see grass"*). Two things called "selected" that are not the same thing is
    /// exactly the kind of seam this arc exists to remove.
    ///
    /// So the studio's subject FOLLOWS the selection. Not the reverse — the panel is
    /// where the choice is made, and the eyedropper already covers going the other
    /// way (click a voxel, select its row).
    ///
    /// Only in the studio: in the island the selection is the *placement* material,
    /// and rebuilding the world because you picked a different block to place would be
    /// absurd.
    fn follow_selected_row_in_the_studio(&mut self) {
        let Some(sample) = studio_sample_to_follow(
            self.control_mode,
            self.studio_scene.subject.is_some(),
            self.material_panel.selected,
            self.studio_scene.sample,
        ) else {
            return;
        };
        self.studio_scene.sample = sample;
        let bricks = self.rebuild_studio_world();
        println!(
            "studio sample: {} ({} occupied bricks)",
            self.material_table
                .row(self.material_panel.selected)
                .map(|row| row.name)
                .unwrap_or("<none>"),
            bricks
        );
    }

    /// S2 — rebuild the studio into `pose`.
    ///
    /// Clears any loaded `.vox` subject, because the subject overrides the pose:
    /// asking for a wall and getting the campfire you loaded ten minutes ago would
    /// read as the button not working.
    fn set_studio_pose(&mut self, pose: studio::StudioPose) {
        if self.control_mode != ControlMode::StudioOrbit {
            self.material_panel.import.status =
                "poses need the studio — restart with --studio".to_string();
            return;
        }
        self.studio_scene.pose = pose;
        self.studio_scene.subject = None;
        let bricks = self.rebuild_studio_world();
        self.material_panel.import.status =
            format!("studio pose: {} ({bricks} occupied bricks)", pose.label());
        println!("{}", self.material_panel.import.status);
    }

    /// Replace the world with the current studio scene, re-frame the camera, and
    /// return the occupied brick count.
    ///
    /// Shared by the pose buttons and by `.vox` model display. Replacing the whole
    /// world rather than editing it is the honest shape of the operation — the
    /// subject changed, so every derived structure changes — and it reuses
    /// `WorldHost::new` plus [`Renderer::reupload_world`], the path that already
    /// exists for an edit outgrowing the brick headroom. A full ~41 MB re-upload is
    /// fine for something a human triggers by pressing a button.
    fn rebuild_studio_world(&mut self) -> u32 {
        let brickmap = self.studio_scene.build();
        let bricks = brickmap.occupied_brick_count();
        self.world_host = WorldHost::new(brickmap);
        self.world_host
            .set_world_thread(self.quality.world_edit.world_thread);
        {
            let brickmap = self.world_host.read();
            self.renderer
                .reupload_world(&self.gpu_context.device, &brickmap);
        }
        // The light volume was flooded for the old geometry; every injected value and
        // every pinned sun-source flag now describes a world that is gone.
        self.renderer.mark_light_volume_dirty();
        // Re-frame: a 16-voxel wall at a 0.9 m working distance is a wall of pixels,
        // and scrolling out by hand every time is friction that makes a tool feel
        // broken.
        self.studio_distance_meters = self
            .studio_scene
            .framing_distance_meters()
            .clamp(STUDIO_MIN_DISTANCE_METERS, STUDIO_MAX_DISTANCE_METERS);
        bricks
    }

    /// S0b — rebuild the studio around a loaded `.vox` model.
    ///
    /// Only in the studio: the island is a generated world and dropping a campfire
    /// into the middle of it is a placement feature, not a material one.
    ///
    /// Replaces the whole world rather than editing the existing one. That is the
    /// honest shape of the operation — the subject changed, so every derived
    /// structure changes — and it reuses `WorldHost::new` plus
    /// [`Renderer::reupload_world`], the path that already exists for an edit
    /// outgrowing the brick headroom. A full ~41 MB re-upload is fine for something
    /// a human triggers by pressing a button.
    fn show_vox_model_in_studio(&mut self) {
        if self.control_mode != ControlMode::StudioOrbit {
            self.material_panel.import.status =
                "showing a model needs the studio — restart with --studio".to_string();
            return;
        }
        let Some(file) = &self.loaded_vox else {
            self.material_panel.import.status = "nothing loaded".to_string();
            return;
        };
        let index = self
            .material_panel
            .import
            .selected_model
            .min(file.models.len().saturating_sub(1));
        let Some(model) = file.models.get(index) else {
            self.material_panel.import.status = "that file has no models".to_string();
            return;
        };
        let subject = vox_material::VoxSubject::from_model(
            model,
            &file.palette,
            &self.material_panel.import.rows,
        );
        let occupied = subject.occupied_count();
        let dimensions = (subject.size_x, subject.size_y, subject.size_z);

        self.studio_scene.subject = Some(subject);
        let bricks = self.rebuild_studio_world();

        self.material_panel.import.status = format!(
            "showing model {index}: {}x{}x{} voxels, {occupied} filled, {bricks} bricks",
            dimensions.0, dimensions.1, dimensions.2
        );
        println!("vox studio: {}", self.material_panel.import.status);
    }

    /// S0b — read a `.vox` and list what it offers as import sources.
    ///
    /// Done here rather than in the panel because it is blocking file I/O and the
    /// overlay pass is inside the frame. Loading changes NOTHING on its own: it
    /// populates the picker, and applying an entry to a row is a separate, explicit
    /// click. A failure sets the status string and leaves any previous load alone,
    /// so a typo does not throw away what you were working with.
    fn load_vox_palette(&mut self) {
        let import = &mut self.material_panel.import;
        let path = std::path::PathBuf::from(import.path.trim());
        match voxel_core::vox::VoxFile::load(&path) {
            Ok(file) => {
                let rows = vox_material::importable_rows(&file);
                let described = file.described_material_count();
                import.model_count = file.models.len();
                import.selected_model = 0;
                import.status = format!(
                    "{}: {} model(s), {} importable entr{}, {} with material properties",
                    path.display(),
                    file.models.len(),
                    rows.len(),
                    if rows.len() == 1 { "y" } else { "ies" },
                    described,
                );
                if described == 0 {
                    // The common case for external tools, and worth saying out loud
                    // so "why is only the colour importing?" answers itself.
                    import.status.push_str(" — colours only, no MATL chunks");
                }
                println!("vox import: {}", import.status);
                import.selected_row = 0;
                import.rows = rows;
                // Kept so "Show model in studio" has geometry to build from; the
                // panel only ever needs the palette side.
                self.loaded_vox = Some(file);
            }
            Err(error) => {
                import.status = error.clone();
                println!("vox import failed: {error}");
            }
        }
    }

    /// E2b test tool — hand the authority a whole POOL to carve
    /// ([`voxel_rt::debug_pool`]), so the swim states can be felt in the app and
    /// not only asserted in tests.
    ///
    /// Same seam as a click: a request in, a coalesced delta out, uploaded by
    /// [`Self::upload_world_updates`]. The frame thread pays for a `Box` here; the
    /// hundreds of thousands of `set_voxel` calls happen on the world thread.
    fn carve_test_pool(&mut self) {
        if !self.world_edits_allowed() {
            return;
        }
        let pose = self.active_pose();
        let pool = match self.crosshair_voxel() {
            Some(voxel) => WaterPool::at_voxel(voxel),
            None => WaterPool::in_front_of(pose.position, pose.forward),
        };
        let light_grid = self.bulk_edit_light_grid();
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

    /// Shift+P — a free-standing body of water at the crosshair
    /// ([`voxel_rt::debug_pool::WaterBlob`]): the E6 optics target, with sky
    /// behind it instead of a lit pool bed.
    ///
    /// Aiming at open sky is not a failure here, it is the interesting case: the
    /// miss path hangs the body in mid-air, which is what makes it a clean
    /// refraction test.
    fn spawn_test_water_body(&mut self) {
        if !self.world_edits_allowed() {
            return;
        }
        let pose = self.active_pose();
        let blob = match self.crosshair_voxel() {
            Some(voxel) => WaterBlob::at_voxel(voxel),
            None => WaterBlob::in_front_of(pose.position, pose.forward),
        };
        let light_grid = self.bulk_edit_light_grid();
        println!(
            "spawning the {} centred on voxel ({}, {}, {}), world {:.2?}",
            blob.label(),
            blob.centre_voxel_x,
            blob.centre_voxel_y,
            blob.centre_voxel_z,
            blob.centre(),
        );
        self.world_host.request_bulk_edit(
            BulkEditRequest {
                shape: Box::new(blob),
                light_grid,
            },
            &self.quality.world_edit,
        );
    }

    /// The voxel the crosshair is on, or `None` past [`EDIT_REACH_METERS`] — so a
    /// bulk tool lands exactly where a click would. Uses the same
    /// [`voxel_dda::CastTarget::EditableVoxel`] as the click path, which means it
    /// sees through water like air (the plan's "to the EDITOR, water IS air"): aim
    /// into a pond and the tool targets the bed, not the skin.
    fn crosshair_voxel(&self) -> Option<[i32; 3]> {
        let pose = self.active_pose();
        let brickmap = self.world_host.read();
        voxel_dda::cast(
            &brickmap,
            pose.position.to_array(),
            pose.forward.to_array(),
            EDIT_REACH_METERS,
            voxel_dda::CastTarget::EditableVoxel,
        )
        .map(|hit| hit.voxel)
    }

    /// The light volume a bulk edit must repair, or `None` when GI is off.
    fn bulk_edit_light_grid(&self) -> Option<voxel_rt::cagi::CagiGrid> {
        self.quality
            .global_illumination
            .enabled
            .then(|| self.renderer.light_volume_grid())
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
        self.edit_at_crosshair(place.then_some(PLACE_MATERIAL));
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
        self.edit_at_crosshair(self.input_state.place_held.then_some(PLACE_MATERIAL));
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
            // Look only. The orbit derives its position from yaw/pitch in
            // `active_pose`, so translating the fly camera here would move a point
            // nothing reads — and WASD must not drift the subject out of frame,
            // which is the entire reason this mode exists.
            ControlMode::StudioOrbit => {
                self.fly_camera.yaw +=
                    camera_input.mouse_delta.0 * self.fly_camera.mouse_sensitivity;
                self.fly_camera.pitch = (self.fly_camera.pitch
                    - camera_input.mouse_delta.1 * self.fly_camera.mouse_sensitivity)
                    .clamp(STUDIO_MIN_PITCH_RADIANS, STUDIO_MAX_PITCH_RADIANS);
            }
        }
        // E2: edits are requested BEFORE the frame is encoded and their deltas are
        // uploaded right after, so a click shows up in the very next frame.
        self.apply_held_edits(now);
        self.upload_world_updates();
        let camera_uniform = match self.control_mode {
            ControlMode::Fly => self.fly_camera.gpu_uniform(self.renderer.resolution()),
            ControlMode::Walk => self.character.gpu_uniform(self.renderer.resolution()),
            // Built from the orbit pose itself rather than a camera struct, so
            // `active_pose` stays the single definition of where the studio eye is —
            // the edit ray and the underwater test read the same one.
            ControlMode::StudioOrbit => self.active_pose().gpu_uniform(
                self.fly_camera.vertical_fov_radians,
                self.renderer.resolution(),
            ),
        };
        // Sun sliders and the runtime quality knobs were mutated during LAST
        // frame's overlay pass; a change shows up one frame later, which is
        // imperceptible.
        let lighting_uniform = self.sun_settings.lighting_uniform(
            self.quality.shading_params(),
            self.quality.gi_params(),
            self.quality.water_params(),
        );
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
        // E6 — is the view underwater? Asked of the ACTIVE eye against the
        // authority, so it is the same question the shading pass asks of the
        // primary ray's origin, and it holds in fly mode too.
        let eye_submerged = {
            let brickmap = self.world_host.read();
            water::eye_is_submerged(&brickmap, self.active_pose().position)
        };
        let frame_data = OverlayFrameData {
            render_resolution: self.renderer.resolution(),
            gpu_timings,
            world_edit: WorldEditReadout {
                threaded: self.world_host.is_threaded(),
                in_flight: self.world_host.in_flight(),
                stats: self.world_host.stats(),
            },
            movement: MovementReadout {
                studio_orbit_distance_meters: (self.control_mode == ControlMode::StudioOrbit)
                    .then_some(self.studio_distance_meters),
                walking: self.control_mode == ControlMode::Walk,
                speed_meters_per_second: match self.control_mode {
                    ControlMode::Fly => self.fly_camera.movement_speed,
                    ControlMode::Walk => self.character.settings.walk_speed,
                    ControlMode::StudioOrbit => self.studio_distance_meters,
                },
                grounded: self.character.grounded(),
                submersion: self.character.submersion(),
                head_submerged: self.character.head_submerged(),
                eye_submerged,
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
            &mut self.material_table,
            &mut self.material_panel,
            &mut self.material_provenance,
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
        self.upload_material_edits();
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
            // L places a glow block the same way, single-shot (M1b).
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
                        // In the studio there is nowhere to walk to, so the wheel
                        // does the thing you actually want: pull in and out.
                        // Multiplicative so a notch feels the same at every zoom.
                        ControlMode::StudioOrbit => {
                            state.studio_distance_meters = (state.studio_distance_meters
                                * 1.15_f32.powf(-notches))
                            .clamp(STUDIO_MIN_DISTANCE_METERS, STUDIO_MAX_DISTANCE_METERS);
                        }
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

#[cfg(test)]
mod tests {
    use super::*;

    /// The studio must not be diggable, and the two world modes must stay editable.
    /// Spelled out per variant rather than as `!= StudioOrbit`, so adding a mode
    /// forces a decision here instead of inheriting one.
    #[test]
    fn only_the_studio_forbids_world_edits() {
        assert!(ControlMode::Fly.allows_world_edits());
        assert!(ControlMode::Walk.allows_world_edits());
        assert!(!ControlMode::StudioOrbit.allows_world_edits());
    }

    /// The studio orbit distance must stay inside a band that neither puts the eye
    /// inside the sample nor shrinks it to nothing — the two failure modes a
    /// wheel-driven radius has.
    #[test]
    fn the_studio_orbit_band_is_usable() {
        const { assert!(STUDIO_MIN_DISTANCE_METERS > voxel_core::world::VOXEL_SIZE) };
        const { assert!(STUDIO_MIN_DISTANCE_METERS < STUDIO_MAX_DISTANCE_METERS) };
        // The default framing must be reachable by the wheel from either end.
        const { assert!(studio::CAMERA_DISTANCE_METERS > STUDIO_MIN_DISTANCE_METERS) };
        const { assert!(studio::CAMERA_DISTANCE_METERS < STUDIO_MAX_DISTANCE_METERS) };
    }

    /// Pitch must stop short of straight up and straight down: at exactly +/-PI/2
    /// the forward vector is parallel to world up and the `forward x Y` camera basis
    /// is degenerate, which collapses the frame.
    #[test]
    fn the_studio_pitch_clamp_avoids_the_degenerate_basis() {
        let limit = std::f32::consts::FRAC_PI_2;
        assert!(STUDIO_MAX_PITCH_RADIANS < limit);
        assert!(STUDIO_MIN_PITCH_RADIANS > -limit);
        for pitch in [STUDIO_MIN_PITCH_RADIANS, STUDIO_MAX_PITCH_RADIANS] {
            let pose = studio::orbit_pose(
                &studio::StudioScene::default(),
                0.0,
                pitch,
                studio::CAMERA_DISTANCE_METERS,
            );
            assert!(
                pose.right.is_finite() && (pose.right.length() - 1.0).abs() < 1e-4,
                "the camera basis degenerated at pitch {pitch}"
            );
        }
    }

    /// S2 — the studio subject must follow the selected row, which is the whole point
    /// of the dropdown in the studio: two things called "selected" that were not the
    /// same thing made every slider look broken.
    #[test]
    fn the_studio_subject_follows_the_selected_row() {
        let grass = material::material_id(Voxel::Grass);
        let stone = material::material_id(Voxel::Stone);
        assert_eq!(
            studio_sample_to_follow(ControlMode::StudioOrbit, false, stone, Voxel::Grass),
            Some(Voxel::Stone)
        );
        // Every row the table has, so a new row cannot be one the studio refuses to
        // show — Air excepted, which the next test owns.
        for id in 1..material::MATERIAL_COUNT as u8 {
            assert_eq!(
                studio_sample_to_follow(ControlMode::StudioOrbit, false, id, Voxel::Air),
                Some(material::material_voxel(id)),
                "the studio would not show row {id}"
            );
        }
        // Selecting the row already on screen must not rebuild: this runs every frame.
        assert_eq!(
            studio_sample_to_follow(ControlMode::StudioOrbit, false, grass, Voxel::Grass),
            None
        );
    }

    /// The three cases that must NOT rebuild the world. Each is a real hazard rather
    /// than a hypothetical: a per-frame world rebuild, an emptied studio, and the
    /// island rebuilding itself because you picked a block to place.
    #[test]
    fn following_the_selected_row_is_confined_to_the_studio() {
        let stone = material::material_id(Voxel::Stone);
        for mode in [ControlMode::Fly, ControlMode::Walk] {
            assert_eq!(
                studio_sample_to_follow(mode, false, stone, Voxel::Grass),
                None,
                "{mode:?} would rebuild the island from the placement material"
            );
        }
        // A loaded .vox model owns the subject; the selection names a table row there.
        assert_eq!(
            studio_sample_to_follow(ControlMode::StudioOrbit, true, stone, Voxel::Grass),
            None
        );
        // Air must not empty the studio, from any current sample.
        for sample in [Voxel::Grass, Voxel::Stone, Voxel::Water] {
            assert_eq!(
                studio_sample_to_follow(
                    ControlMode::StudioOrbit,
                    false,
                    material::AIR_MATERIAL_ID,
                    sample
                ),
                None,
                "selecting air emptied a studio showing {sample:?}"
            );
        }
    }
}
