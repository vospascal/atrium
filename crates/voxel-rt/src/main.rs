//! voxel-rt: standalone ray-traced voxel renderer (winit + wgpu + egui).
//! This file is the thin platform layer — window, event loop, and the raw
//! input -> [`camera::CameraInput`] mapping — so it can be swapped for an
//! OpenXR entry point later without touching the renderer. All winit types
//! stay in this file; camera.rs is pure math.

use std::path::PathBuf;
use std::sync::Arc;
use std::time::{Duration, Instant};

use voxel_core::world::{Voxel, VoxelWorld, WorldVoxelCoord};
use winit::application::ApplicationHandler;
use winit::dpi::PhysicalSize;
use winit::event::{
    DeviceEvent, DeviceId, ElementState, MouseButton, MouseScrollDelta, WindowEvent,
};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop};
use winit::keyboard::{KeyCode, PhysicalKey};
use winit::window::{CursorGrabMode, Window, WindowId};

use atrium_profile::cpu::SpanRecorder;
use voxel_color::OutputDepth;
use voxel_color::{HeadroomChoice, TonemapCurve};
use voxel_environment::SunSettings;
use voxel_graph::GraphAsset;
use voxel_material::animation_clock::AnimationClock;
use voxel_material::material;
use voxel_material::world_event::{EventKey, EventSpec, WorldEventField, CHANNEL_PRESENCE};
use voxel_material_graph::layers::sync_pattern_layers_from_graph;
use voxel_material_graph::lowering::{
    compile as compile_material_graph, GraphEditorState, MaterialGraphShaderSet,
    MaterialSampleContext,
};
use voxel_rt::brickmap::Brickmap;
use voxel_rt::camera::{CameraInput, CameraPose, FlyCamera};
use voxel_rt::character::CharacterController;
use voxel_rt::engine_runtime::{RuntimeMode, VoxelEngineConfig, VoxelEngineRuntime};
use voxel_rt::environment::{RuntimeEnvironmentState, Season};
use voxel_rt::gpu::GpuContext;
use voxel_rt::light_fixture;
use voxel_rt::lighting::OutputParams;
use voxel_rt::material_edit::{MaterialPanelState, VoxImportState, WORLD_HOTBAR_BLOCKS};
use voxel_rt::material_graph_assets::MaterialGraphAssetService;
use voxel_rt::material_table::MaterialTable;
use voxel_rt::overlay::{
    MovementReadout, Overlay, OverlayFrameData, TargetHighlightReadout, WorldEditReadout,
};
use voxel_rt::passes::cagi::AttributeSource;
use voxel_rt::passes::{cagi, dda};
use voxel_rt::profiling::{self, FrameTimers, GPU_OVERLAY};
use voxel_rt::render::Renderer;
use voxel_rt::studio;
use voxel_rt::studio_assets::{
    live_state_fingerprint, StudioAssetPanelState, StudioProject, StudioProjectStore,
};
use voxel_rt::variants::{QualityPreset, RenderQuality, QUALITY_PRESETS};
use voxel_rt::vox_material;
use voxel_rt::voxel_dda;
use voxel_rt::water;
use voxel_rt::world_edit::VoxelEdit;
use voxel_rt::world_host::{WorldHost, WorldUpdate};
use voxel_rt::world_profile::CompiledWorldProfile;
use voxel_rt::world_profile_runtime::apply_initial_generation_profile;

/// Deterministic world-generation defaults.
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

/// S3 — how far the player's presence reaches, world metres.
///
/// A per-entity reach rather than a global constant in the shader: a sensor
/// intersects its own authored radius with this, so a large creature can be
/// felt further away than a person without every material being re-authored.
/// Generous, because a sensor that wants a tight radius simply says so.
const PRESENCE_RADIUS_METERS: f32 = 12.0;

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

/// The single native-input routing decision for the split Studio surface.
/// Rendering remains shared, but only the region under the pointer receives
/// camera, editor, or overlay input; a future panel can add one enum variant
/// without another collection of boolean guards.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum InputRegion {
    Viewport,
    GraphStudio,
    Overlay,
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
    frame_timers: Option<FrameTimers>,
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
    /// The exact DDA source the live pipeline was built from, so
    /// [`App::rebuild_dda_shader`] can refuse to recompile identical source.
    applied_dda_shader_source: Option<String>,
    input_state: InputState,
    cursor_grabbed: bool,
    /// Latest physical cursor position. The viewport and Graph Studio use
    /// this same boundary for routing camera versus editor input.
    cursor_position: Option<(f64, f64)>,
    vsync_enabled: bool,
    /// Requested output bit depth (see the `voxel-color` crate). A WISH — the
    /// device may veto it, and `renderer.output_format().depth()` is what happened.
    output_depth: OutputDepth,
    /// Trust the display's reported headroom, or pin a value to see what it does.
    headroom_choice: HeadroomChoice,
    /// Which tonemap the shading pass applies. Runtime, so two curves can be compared on
    /// the same frame.
    tonemap_curve: TonemapCurve,
    /// What BT.2390 assumes the scene's brightest pixel is. Unmeasurable today — see
    /// `voxel_color::tonemap::DEFAULT_CONTENT_PEAK`.
    content_peak: f32,
    /// Scene exposure, applied before the tonemap. 1.0 reproduces the pre-exposure look.
    exposure: f32,
    previous_frame_time: Instant,
    /// CPU span accumulators for this frame, drained once per frame into the
    /// performance panel. See [`profiling::CPU_SPANS`] for what each index means.
    ///
    /// Replaced a 2-second stdout fps line, which averaged away every wave it
    /// might have shown and could not say which phase was slow.
    span_recorder: SpanRecorder,
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
    /// Phase 0 — the persisted Studio project and the overlay's save/load requests.
    studio_project: StudioProject,
    /// Validated composition root for biome, generation, presentation, audio,
    /// and animation resolution. Authored assets remain the source of truth.
    world_profile: Option<CompiledWorldProfile>,
    environment_runtime: RuntimeEnvironmentState,
    studio_assets: StudioAssetPanelState,
    /// Phase 2 — successfully compiled graph functions selected by material slot.
    material_graph_shaders: MaterialGraphShaderSet,
    graph_editor: GraphEditorState,
    /// Content-aware autosave debounce. Renderer cache changes never enter this
    /// fingerprint, so a quiet project does not keep rewriting JSON every frame.
    observed_project_fingerprint: Option<u64>,
    saved_project_fingerprint: Option<u64>,
    autosave_due_at: Option<Instant>,
    /// A fresh material-attribute upload needs a short CAGI catch-up burst so
    /// world lighting follows Graph Studio edits instead of visibly trailing it.
    gi_settle_frames: u8,
    /// S3 — the animation clock every oscillator and envelope reads. Advanced
    /// once per frame by the scaled frame delta; see `animation_clock.rs` for
    /// why it is a split epoch/remainder rather than one float.
    animation_clock: AnimationClock,
    /// S3 — unscaled simulation time. Events are stamped and retired against
    /// this clock, so animation speed affects artwork only.
    world_clock: AnimationClock,
    /// S3 — what materials can react to. The active eye is raised into it each
    /// frame as the presence entity; a mob system later raises alongside.
    world_events: WorldEventField,
}

impl AppState {
    fn new(event_loop: &ActiveEventLoop) -> Self {
        let arguments: Vec<String> = std::env::args().collect();
        let engine_config = VoxelEngineConfig::from_args(&arguments)
            .unwrap_or_else(|error| panic!("invalid voxel engine arguments: {error}"));
        let project_path = engine_config.project_root.clone();
        let studio_mode = engine_config.mode == RuntimeMode::StudioEdit;
        let studio_scene = studio::StudioScene::default();
        let engine_runtime = VoxelEngineRuntime::load(engine_config.clone());
        let brickmap_start = Instant::now();
        let mut brickmap = engine_runtime.build_world(&studio_scene);
        println!(
            "{} graph built in {:.2?} ({} occupied bricks)",
            if studio_mode {
                "studio preview"
            } else {
                "world"
            },
            brickmap_start.elapsed(),
            brickmap.occupied_brick_count()
        );
        let voxel_rt::engine_runtime::ProjectRuntime {
            materials: mut material_table,
            mut quality,
            project: studio_project,
            material_graphs: material_graph_shaders,
            diagnostics,
            ..
        } = engine_runtime.project;
        let world_profile = None;
        let mut project_status = diagnostics
            .iter()
            .map(|diagnostic| diagnostic.message.clone())
            .collect::<Vec<_>>()
            .join("; ");
        project_status.push_str(&format!(
            "; loaded {} canonical material graphs",
            material_graph_shaders.len()
        ));
        let recovery_available = StudioProjectStore::new(&project_path)
            .load_recovery()
            .ok()
            .flatten()
            .is_some();
        let project_fingerprint = live_state_fingerprint(&material_table, &quality).ok();
        // S0 — `--studio` swaps the generated island for the material studio: one
        // voxel on a plate, orbit camera. Excludable by construction (the plan's
        // isolation rule): the island path is not merely skipped, no world is
        // generated at all, which is also why the studio starts instantly.
        let environment_runtime = RuntimeEnvironmentState {
            season: Season::Summer,
            ..RuntimeEnvironmentState::default()
        };
        if !studio_mode {
            if let Some(profile) = &world_profile {
                let applied = apply_initial_generation_profile(
                    &mut brickmap,
                    profile,
                    &environment_runtime,
                    u64::from(WORLD_SEED),
                )
                .unwrap_or_else(|error| panic!("world profile generation failed: {error}"));
                println!(
                    "world profile sampled {} columns and added {} voxels",
                    applied.sampled_columns, applied.changed_voxels
                );
            }
        }

        // L0 — `--light-fixture` stamps the rainbow corridor into the world and
        // spawns inside it. Carved AFTER the profile pass on purpose: the fixture's
        // whole value is that its geometry is a function of its own constants, so
        // it has to be the last thing to touch these voxels.
        let light_fixture_camera = engine_config.light_fixture.map(|notch| {
            let corridor = light_fixture::RainbowCorridor::new(notch);
            let written = corridor.carve(&mut brickmap);
            let eye = corridor.viewer_eye_meters();
            println!(
                "light fixture: rainbow corridor ({notch:?}), {written} blocks, interior \
                 {}x{}x{} at {:?} — spawning inside at {eye:?}",
                light_fixture::INTERIOR_WIDTH,
                light_fixture::INTERIOR_HEIGHT,
                light_fixture::INTERIOR_LENGTH,
                corridor.interior_min,
            );
            println!(
                "  sun re-aimed and the ambient floor zeroed: with GI off this room is BLACK, \
                 so everything you see is the light volume"
            );
            corridor
        });

        let window_attributes = Window::default_attributes()
            .with_title("voxel-rt")
            .with_inner_size(PhysicalSize::new(1280, 720));
        let window = Arc::new(
            event_loop
                .create_window(window_attributes)
                .expect("failed to create window"),
        );

        let gpu_context = GpuContext::new(window.clone());
        let mut renderer = Renderer::new(
            &gpu_context.device,
            gpu_context.output_format(),
            gpu_context.surface_config.width,
            gpu_context.surface_config.height,
            &brickmap,
            &quality.global_illumination,
            &material_table.cagi_attributes(),
        );
        // WorldBindings starts from compiled defaults for its standalone path;
        // the project asset may have restored overrides before the renderer was
        // created, so install those once before the first frame.
        renderer.write_material_table(&gpu_context.queue, &material_table.gpu_rows());
        // Renderer::new starts from the source's unpatched shader and native
        // render scale. A restored quality recipe must be installed before the
        // first visible frame rather than waiting for a future UI edit.
        // S3 — the generator mask is DERIVED, never authored: it must be installed
        // before the first pipeline is built, or the first frame compiles a shader
        // holding all fourteen generator bodies and then recompiles once anything
        // else moves. `requires_pipeline_rebuild` already compares the mask, so
        // every later change is picked up by the ordinary rebuild path.
        quality.materials.pattern_generator_mask = material::generator_mask(material_table.rows());
        renderer.set_dda_shader_source(
            &gpu_context.device,
            &dda::build_shader_source_for_output(
                &quality,
                &material_graph_shaders,
                gpu_context.output_format(),
            ),
        );
        renderer.set_cagi_shader_source(&gpu_context.device, &cagi::build_shader_source(&quality));
        renderer.set_render_scale(&gpu_context.device, quality.render_scale);
        quality.render_scale = renderer.render_scale();
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
            .map(|quality| {
                dda::build_shader_source_for_output(
                    quality,
                    &material_graph_shaders,
                    gpu_context.output_format(),
                )
            })
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
        let frame_timers = FrameTimers::new(
            &gpu_context.device,
            &gpu_context.queue,
            profiling::GPU_SPANS,
        );
        if frame_timers.is_none() {
            println!("GPU timestamp queries unsupported — per-pass timings disabled");
        }

        // E2: hand the world to the authority. It starts inline and immediately
        // takes the lever's value, so the shipped default spawns the world thread
        // here and the frame thread stops owning the brickmap.
        let mut world_host = WorldHost::new(brickmap);
        world_host.set_world_thread(quality.world_edit.world_thread);
        let graph_editor_slot = material::material_id(studio_scene.sample);

        // The fixture's camera and sun, or the ordinary island's. Both come from
        // `RainbowCorridor` so the interactive view and bench section 15 cannot
        // disagree about what the room is lit by.
        let fly_camera = match &light_fixture_camera {
            Some(corridor) => {
                let eye = corridor.viewer_eye_meters();
                FlyCamera {
                    position: glam::Vec3::new(eye[0], eye[1], eye[2]),
                    yaw: light_fixture::RainbowCorridor::yaw_down_corridor(),
                    pitch: -0.12,
                    ..FlyCamera::default()
                }
            }
            None => FlyCamera::default(),
        };
        let sun_settings = match &light_fixture_camera {
            Some(_) => light_fixture::RainbowCorridor::sun(),
            None => SunSettings::default(),
        };

        Self {
            window,
            gpu_context,
            renderer,
            world_host,
            overlay,
            frame_timers,
            fly_camera,
            character: character_from_fly_camera(&fly_camera),
            control_mode: if studio_mode {
                ControlMode::StudioOrbit
            } else {
                ControlMode::Fly
            },
            character_step_micros: 0.0,
            sun_settings,
            flooded_sun_settings: sun_settings,
            quality,
            applied_quality: quality,
            // Startup builds the source inline (the mask has to be derived before
            // the first pipeline), so the guard starts empty and the first
            // `rebuild_dda_shader` populates it.
            applied_dda_shader_source: None,
            input_state: InputState::default(),
            cursor_grabbed: false,
            cursor_position: None,
            vsync_enabled: true,
            output_depth: OutputDepth::default(),
            headroom_choice: HeadroomChoice::default(),
            tonemap_curve: TonemapCurve::default(),
            content_peak: voxel_color::tonemap::DEFAULT_CONTENT_PEAK,
            exposure: 1.0,
            previous_frame_time: Instant::now(),
            span_recorder: SpanRecorder::new(profiling::CPU_SPANS),
            // Selecting the sample's own row means the panel opens on the thing the
            // camera is pointed at, which is the only row worth defaulting to.
            material_panel: MaterialPanelState {
                selected: material::material_id(studio_scene.sample),
                import: VoxImportState::new(),
                ..MaterialPanelState::default()
            },
            material_table: {
                // The initial GPU write above already consumed a restored table.
                // Do not make the first idle frame upload it once more.
                let _ = material_table.take_dirty();
                material_table
            },
            studio_scene,
            studio_distance_meters: studio::CAMERA_DISTANCE_METERS,
            loaded_vox: None,
            studio_project,
            world_profile,
            environment_runtime,
            studio_assets: StudioAssetPanelState {
                status: project_status,
                recovery_available,
                ..StudioAssetPanelState::new(project_path.display().to_string())
            },
            material_graph_shaders,
            graph_editor: GraphEditorState::new(graph_editor_slot),
            observed_project_fingerprint: project_fingerprint,
            saved_project_fingerprint: None,
            autosave_due_at: None,
            gi_settle_frames: 0,
            animation_clock: AnimationClock::new(),
            world_clock: AnimationClock::new(),
            world_events: WorldEventField::new(),
        }
    }

    fn save_studio_project(&mut self, autosave: bool) -> bool {
        let project_path = PathBuf::from(self.studio_assets.project_path.trim());
        if project_path.as_os_str().is_empty() {
            self.studio_assets.status = "Choose a project folder before saving".to_string();
            return false;
        }
        let quality_name = if self.studio_assets.quality_name.trim().is_empty() {
            "Active quality"
        } else {
            self.studio_assets.quality_name.trim()
        };
        let store = StudioProjectStore::new(&project_path);
        match self.studio_project.save_live_state(
            &store,
            quality_name,
            &self.material_table,
            &self.quality,
        ) {
            Ok(()) => {
                self.studio_assets.status = if autosave {
                    format!("Autosaved project to {}", project_path.display())
                } else {
                    format!("Saved project to {}", project_path.display())
                };
                // Saving does NOT arm autosave. It used to, which meant one
                // manual save quietly turned on a writer that then overwrote
                // the project files from memory every two idle seconds — and
                // anything edited on disk meanwhile lost the race. Autosave is
                // opt-in from the checkbox, and stays where the user put it.
                self.studio_assets.recovery_available = false;
                self.saved_project_fingerprint =
                    live_state_fingerprint(&self.material_table, &self.quality).ok();
                self.observed_project_fingerprint = self.saved_project_fingerprint;
                self.autosave_due_at = None;
                true
            }
            Err(error) => {
                self.studio_assets.status = format!("Save failed: {error}");
                false
            }
        }
    }

    fn load_studio_project_from_panel(&mut self) {
        let project_path = PathBuf::from(self.studio_assets.project_path.trim());
        if project_path.as_os_str().is_empty() {
            self.studio_assets.status = "Choose a project folder before loading".to_string();
            return;
        }
        // Load into copies so a malformed project never partially replaces the
        // last-known-good materials or quality in the live renderer.
        let mut table = self.material_table.clone();
        let mut quality = self.quality;
        match StudioProject::load_live_state(
            &StudioProjectStore::new(&project_path),
            &mut table,
            &mut quality,
        ) {
            Ok((project, warnings)) => {
                let store = StudioProjectStore::new(&project_path);
                let world_profile = match project.compile_active_world_profile(&store) {
                    Ok(profile) => profile,
                    Err(error) => {
                        self.studio_assets.status =
                            format!("Load failed; kept current project: world profile {error}");
                        return;
                    }
                };
                let (graph_shaders, graph_diagnostics) =
                    MaterialGraphAssetService::load_shader_set_for_editing(
                        &project_path,
                        &project,
                        &mut table,
                    );
                self.studio_project = project;
                self.world_profile = world_profile;
                self.material_table = table;
                self.quality = quality;
                self.material_graph_shaders = graph_shaders;
                self.material_panel.repack_gi_requested = true;
                if let Err(error) = self.rebuild_generated_world_from_profile() {
                    self.studio_assets.status =
                        format!("Load failed while applying world profile: {error}");
                    return;
                }
                let fingerprint = live_state_fingerprint(&self.material_table, &self.quality).ok();
                self.observed_project_fingerprint = fingerprint;
                self.saved_project_fingerprint = fingerprint;
                self.autosave_due_at = None;
                let warning_count = warnings.len() + graph_diagnostics.len();
                self.studio_assets.status = if warning_count == 0 {
                    format!("Loaded project from {}", project_path.display())
                } else {
                    let warning_status = format!(
                        "{} warning{}",
                        warning_count,
                        if warning_count == 1 { "" } else { "s" }
                    );
                    [
                        format!("Loaded project from {}", project_path.display()),
                        warning_status,
                    ]
                    .into_iter()
                    .filter(|part| !part.is_empty())
                    .collect::<Vec<_>>()
                    .join("; ")
                };
                self.rebuild_dda_shader();
            }
            Err(error) => {
                self.studio_assets.status = format!("Load failed; kept current project: {error}");
            }
        }
    }

    fn restore_studio_recovery(&mut self) {
        let project_path = PathBuf::from(self.studio_assets.project_path.trim());
        let store = StudioProjectStore::new(&project_path);
        let mut table = self.material_table.clone();
        let mut quality = self.quality;
        match store.load_recovery() {
            Ok(Some(snapshot)) => match snapshot.apply_to_live(&mut table, &mut quality) {
                Ok(warnings) => {
                    let recovered_project = StudioProject {
                        manifest: snapshot.manifest,
                    };
                    let world_profile = match recovered_project.compile_active_world_profile(&store)
                    {
                        Ok(profile) => profile,
                        Err(error) => {
                            self.studio_assets.status =
                                format!("Recovery world profile could not load: {error}");
                            return;
                        }
                    };
                    let (graph_shaders, graph_diagnostics) =
                        MaterialGraphAssetService::load_shader_set_for_editing(
                            &project_path,
                            &recovered_project,
                            &mut table,
                        );
                    self.studio_project = recovered_project;
                    self.world_profile = world_profile;
                    self.material_table = table;
                    self.quality = quality;
                    self.material_graph_shaders = graph_shaders;
                    self.material_panel.repack_gi_requested = true;
                    if let Err(error) = self.rebuild_generated_world_from_profile() {
                        self.studio_assets.status =
                            format!("Recovery world profile could not apply: {error}");
                        return;
                    }
                    let warning_count = warnings.len() + graph_diagnostics.len();
                    self.studio_assets.status = if warning_count == 0 {
                        "Recovered interrupted save; committing it now".to_string()
                    } else {
                        format!(
                            "Recovered interrupted save with {} warning(s); committing it now",
                            warning_count
                        )
                    };
                    let _ = self.save_studio_project(false);
                }
                Err(error) => {
                    self.studio_assets.status =
                        format!("Recovery failed; kept current project: {error}")
                }
            },
            Ok(None) => {
                self.studio_assets.recovery_available = false;
                self.studio_assets.status = "No recovery snapshot found".to_string();
            }
            Err(error) => {
                self.studio_assets.status = format!("Could not read recovery snapshot: {error}")
            }
        }
    }

    fn discard_studio_recovery(&mut self) {
        let store = StudioProjectStore::new(self.studio_assets.project_path.trim());
        match store.clear_recovery() {
            Ok(()) => {
                self.studio_assets.recovery_available = false;
                self.studio_assets.status = "Discarded interrupted-save recovery".to_string();
            }
            Err(error) => {
                self.studio_assets.status = format!("Could not discard recovery: {error}")
            }
        }
    }

    fn compile_graph_editor(&mut self) {
        let registry = voxel_rt::graph::CATALOGUE;
        let slot = self.graph_editor.material_slot;
        // Cloned rather than borrowed: `rebuild_dda_shader` below needs `&mut
        // self`, and an editor compile is a keystroke-rate path, not a frame one.
        let graph = self.graph_editor.graph.clone();
        match compile_material_graph(&graph, &registry) {
            Ok(program) => {
                let pattern_changed = self
                    .material_table
                    .row_mut(slot)
                    .map(|row| sync_pattern_layers_from_graph(&graph, row))
                    .transpose();
                match pattern_changed {
                    Ok(_) => {
                        // The graph is canonical.  Its representative surface
                        // sample backs both the material-table fallback and the
                        // CAGI attribute rebuild; the DDA program below still
                        // evaluates the complete graph per hit.
                        let _ = self.material_table.apply_graph_sample(
                            slot,
                            &program,
                            MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0]),
                        );
                        self.material_panel.repack_gi_requested = true;
                    }
                    Err(error) => {
                        self.graph_editor.diagnostics = graph.resolve(&registry).diagnostics;
                        self.graph_editor.status = error.to_string();
                        return;
                    }
                }
                let cache = program.cache.clone();
                self.material_graph_shaders
                    .insert(self.graph_editor.material_slot, program);
                self.rebuild_dda_shader();
                // Cacheability warnings ride along with the resolve diagnostics
                // rather than in a panel of their own: an author wants one list
                // of "things worth knowing about this graph", and a layer that
                // cannot be cached is exactly that — valid, renderable, and
                // costing its full price every frame instead of once.
                let mut diagnostics = graph.resolve(&registry).diagnostics;
                diagnostics.extend(cache.diagnostics());
                self.graph_editor.diagnostics = diagnostics;
                let live = cache.live_layers().count();
                self.graph_editor.status = if live == 0 {
                    format!(
                        "Compiled graph for material slot {} — {} pattern layer(s), all cacheable",
                        self.graph_editor.material_slot,
                        cache.layers.len()
                    )
                } else {
                    format!(
                        "Compiled graph for material slot {} — {} of {} pattern layer(s) NOT \
                         cacheable",
                        self.graph_editor.material_slot,
                        live,
                        cache.layers.len()
                    )
                };
            }
            Err(error) => {
                self.graph_editor.diagnostics = graph.resolve(&registry).diagnostics;
                self.graph_editor.status = error.to_string();
            }
        }
    }

    fn open_graph_editor_graph(&mut self) {
        let slot = self.graph_editor.material_slot;
        let project_path = PathBuf::from(self.studio_assets.project_path.trim());
        // Graph Studio and the material preview share one selected row. Opening
        // a graph therefore changes the studio subject as well as the editor;
        // the next material-upload pass applies the row to the visible sample.
        self.material_panel.selected = slot;
        match MaterialGraphAssetService::open(
            &project_path,
            &self.studio_project,
            &self.material_table,
            slot,
        ) {
            Ok(Some(opened)) => {
                self.graph_editor.open_graph(slot, opened.graph);
                if !opened.status.is_empty() {
                    self.graph_editor.status = opened.status;
                }
            }
            Ok(None) => {
                self.graph_editor.status = format!("No material row for slot {slot}");
            }
            Err(error) => self.graph_editor.status = format!("Could not open graph: {error}"),
        }
    }

    fn save_graph_editor_graph(&mut self) {
        let slot = self.graph_editor.material_slot;
        let project_path = PathBuf::from(self.studio_assets.project_path.trim());
        let graph = self.graph_editor.graph.clone();
        match MaterialGraphAssetService::save(&project_path, &mut self.studio_project, slot, &graph)
        {
            Ok(()) => {
                self.graph_editor.save_requested = false;
                self.graph_editor.status = format!(
                    "Saved graph for material slot {}",
                    self.graph_editor.material_slot
                );
            }
            Err(error) => self.graph_editor.status = format!("Save graph failed: {error}"),
        }
    }

    fn duplicate_graph_editor_graph(&mut self) {
        let mut graph = self.graph_editor.graph.clone();
        let replacement = GraphAsset::new(format!("{} Copy", graph.name), graph.kind);
        graph.id = replacement.id;
        graph.name = replacement.name;
        self.graph_editor
            .open_graph(self.graph_editor.material_slot, graph);
        self.graph_editor.status = "Graph duplicated — save it to persist the copy".to_string();
    }

    fn update_project_autosave(&mut self) {
        let Ok(fingerprint) = live_state_fingerprint(&self.material_table, &self.quality) else {
            return;
        };
        if self.observed_project_fingerprint != Some(fingerprint) {
            self.observed_project_fingerprint = Some(fingerprint);
            self.autosave_due_at = Some(Instant::now() + Duration::from_secs(2));
        }
        if self.studio_assets.autosave_enabled
            && !self.studio_assets.recovery_available
            && self.saved_project_fingerprint != Some(fingerprint)
            && self
                .autosave_due_at
                .is_some_and(|due| Instant::now() >= due)
        {
            self.save_studio_project(true);
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
        if pressed {
            let hotbar_index = match key_code {
                KeyCode::Digit1 => Some(0),
                KeyCode::Digit2 => Some(1),
                KeyCode::Digit3 => Some(2),
                KeyCode::Digit4 => Some(3),
                KeyCode::Digit5 => Some(4),
                KeyCode::Digit6 => Some(5),
                KeyCode::Digit7 => Some(6),
                KeyCode::Digit8 => Some(7),
                KeyCode::Digit9 => Some(8),
                _ => None,
            };
            if let Some(index) = hotbar_index {
                self.material_panel.selected = material::material_id(WORLD_HOTBAR_BLOCKS[index]);
                return;
            }
        }
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
            // O for options: everything that used to be permanently on screen —
            // resolutions, edit counters, movement, vsync, output depth, quality
            // levers, sun. A debug UI that covers the render hides the thing it
            // is there to help you judge.
            KeyCode::KeyO => {
                if pressed {
                    self.overlay.toggle_settings_panel();
                }
            }
            // P for performance: spans, pacing and wave detection. A WINDOW, not
            // another always-on corner — the overlay already costs too much
            // permanent screen area for a number you consult occasionally.
            KeyCode::KeyP => {
                if pressed {
                    self.overlay.toggle_performance_panel();
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
    /// Advance the S3 animation clock and refresh the world-event field.
    ///
    /// Deterministic mode is resolved HERE rather than at the uniform, because
    /// it has two halves and both must happen: the clock is pinned AND the
    /// event field is emptied. A frozen clock alone is not a stable frame — a
    /// sensor's nearness still tracks the camera — which is exactly why the
    /// speed lever and this one are separate.
    fn advance_animation(&mut self, frame_time_seconds: f32) {
        if self.quality.materials.animation_deterministic {
            self.animation_clock.reset();
            self.world_clock.reset();
            self.world_events.clear();
            return;
        }
        self.animation_clock
            .advance(frame_time_seconds, self.quality.materials.animation_speed);
        self.world_clock.advance(frame_time_seconds, 1.0);
        let clock = self.world_clock.sample();
        // The active eye is an entity like any other. It raises a presence
        // event and no sensor node ever learns which entity it was, so a mob
        // system later raises alongside it without the renderer, the shader or
        // any authored graph changing.
        let pose = self.active_pose();
        self.world_events.raise(
            EventKey::CAMERA,
            EventSpec {
                position_meters: pose.position.to_array(),
                radius_meters: PRESENCE_RADIUS_METERS,
                channel: CHANNEL_PRESENCE,
                strength: 1.0,
            },
            clock,
        );
        self.world_events.retire_expired(clock);
    }

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

    /// The current edit face, projected into logical window coordinates for the
    /// overlay. This deliberately asks the exact same DDA question as
    /// place/remove, so the outline is a promise of where the next block will
    /// attach rather than a separate visual approximation.
    fn target_highlight(&self) -> Option<TargetHighlightReadout> {
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
        }?;
        let voxel = WorldVoxelCoord::from_detail_cell(hit.voxel);
        let physical_size = self.window.inner_size();
        let viewport_height = self.rendered_viewport_height();
        let scale = self.window.scale_factor() as f32;
        let logical_width = physical_size.width as f32 / scale;
        let logical_height = viewport_height as f32 / scale;
        let aspect = physical_size.width as f32 / viewport_height.max(1) as f32;
        let half_tangent = (self.fly_camera.vertical_fov_radians * 0.5).tan();
        let origin = glam::Vec3::new(voxel.x as f32, voxel.y as f32, voxel.z as f32);
        let face = match hit.face_normal {
            [1, 0, 0] => Some([
                origin + glam::vec3(1.0, 0.0, 0.0),
                origin + glam::vec3(1.0, 1.0, 0.0),
                origin + glam::vec3(1.0, 0.0, 1.0),
                origin + glam::vec3(1.0, 1.0, 1.0),
            ]),
            [-1, 0, 0] => Some([
                origin,
                origin + glam::vec3(0.0, 1.0, 0.0),
                origin + glam::vec3(0.0, 0.0, 1.0),
                origin + glam::vec3(0.0, 1.0, 1.0),
            ]),
            [0, 1, 0] => Some([
                origin + glam::vec3(0.0, 1.0, 0.0),
                origin + glam::vec3(1.0, 1.0, 0.0),
                origin + glam::vec3(0.0, 1.0, 1.0),
                origin + glam::vec3(1.0, 1.0, 1.0),
            ]),
            [0, -1, 0] => Some([
                origin,
                origin + glam::vec3(1.0, 0.0, 0.0),
                origin + glam::vec3(0.0, 0.0, 1.0),
                origin + glam::vec3(1.0, 0.0, 1.0),
            ]),
            [0, 0, 1] => Some([
                origin + glam::vec3(0.0, 0.0, 1.0),
                origin + glam::vec3(1.0, 0.0, 1.0),
                origin + glam::vec3(0.0, 1.0, 1.0),
                origin + glam::vec3(1.0, 1.0, 1.0),
            ]),
            [0, 0, -1] => Some([
                origin,
                origin + glam::vec3(1.0, 0.0, 0.0),
                origin + glam::vec3(0.0, 1.0, 0.0),
                origin + glam::vec3(1.0, 1.0, 0.0),
            ]),
            // The eye is inside this voxel, so DDA has no crossed face to show.
            _ => None,
        };
        let Some(face) = face else {
            return Some(TargetHighlightReadout {
                material: hit.material,
                voxel: [voxel.x, voxel.y, voxel.z],
                distance_meters: hit.distance_meters,
                screen_corners: None,
            });
        };
        let mut corners = [[0.0; 2]; 4];
        for (index, point) in face.into_iter().enumerate() {
            let offset = point - pose.position;
            let depth = offset.dot(pose.forward);
            // A box containing the eye has no stable screen-space outline. It
            // can still be removed/picked; only its preview is suppressed.
            if depth <= 0.02 {
                return Some(TargetHighlightReadout {
                    material: hit.material,
                    voxel: [voxel.x, voxel.y, voxel.z],
                    distance_meters: hit.distance_meters,
                    screen_corners: None,
                });
            }
            let ndc_x = offset.dot(pose.right) / (depth * half_tangent * aspect);
            let ndc_y = offset.dot(pose.up) / (depth * half_tangent);
            corners[index] = [
                (ndc_x + 1.0) * 0.5 * logical_width,
                (1.0 - ndc_y) * 0.5 * logical_height,
            ];
        }
        Some(TargetHighlightReadout {
            material: hit.material,
            voxel: [voxel.x, voxel.y, voxel.z],
            distance_meters: hit.distance_meters,
            screen_corners: Some(corners),
        })
    }

    fn rendered_viewport_height(&self) -> u32 {
        let surface_height = self.gpu_context.surface_config.height.max(1);
        let logical_panel_height = if self.graph_editor.visible {
            self.graph_editor.drawer_height
        } else {
            34.0
        };
        let panel_height = (logical_panel_height * self.window.scale_factor() as f32)
            .round()
            .max(1.0) as u32;
        surface_height.saturating_sub(panel_height).max(1)
    }

    fn pointer_over_graph_editor(&self) -> bool {
        let Some((_, physical_y)) = self.cursor_position else {
            return false;
        };
        let scale = self.window.scale_factor().max(f64::EPSILON);
        let logical_y = physical_y / scale;
        let logical_height = self.window.inner_size().height as f64 / scale;
        let panel_height = if self.graph_editor.visible {
            self.graph_editor.drawer_height as f64
        } else {
            34.0
        };
        logical_y >= (logical_height - panel_height).max(0.0)
    }

    fn input_region(&self) -> InputRegion {
        if self.pointer_over_graph_editor() {
            InputRegion::GraphStudio
        } else if self.overlay.wants_pointer_input() {
            InputRegion::Overlay
        } else {
            InputRegion::Viewport
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
            self.select_picked_material(hit.material);
            return;
        }
        // Checked AFTER the eyedropper on purpose: a pick is a read, and it is the
        // one click the studio still wants.
        if !self.world_edits_allowed() {
            return;
        }
        let hit_world = WorldVoxelCoord::from_detail_cell(hit.voxel);
        let (voxel, material) = if let Some(placed_material) = placed_material {
            if hit.face_normal == [0, 0, 0] {
                return; // the eye is inside geometry: there is no face to build on
            }
            (
                [
                    hit_world.x + hit.face_normal[0],
                    hit_world.y + hit.face_normal[1],
                    hit_world.z + hit.face_normal[2],
                ],
                placed_material,
            )
        } else {
            ([hit_world.x, hit_world.y, hit_world.z], Voxel::Air)
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
                material_attributes: self.material_table.cagi_attributes(),
            },
            &self.quality.world_edit,
        );
    }

    /// Select the material under the crosshair without changing the world.
    /// Middle-click uses this directly; the existing eyedropper calls the same
    /// selection path after consuming its next edit click.
    fn pick_material_at_crosshair(&mut self) {
        let pose = self.active_pose();
        let material = {
            let brickmap = self.world_host.read();
            voxel_dda::cast(
                &brickmap,
                pose.position.to_array(),
                pose.forward.to_array(),
                EDIT_REACH_METERS,
                voxel_dda::CastTarget::EditableVoxel,
            )
            .map(|hit| hit.material)
        };
        if let Some(material) = material {
            self.select_picked_material(material);
        }
    }

    fn select_picked_material(&mut self, material: u8) {
        self.material_panel.selected = material;
        let name = self
            .material_table
            .row(material)
            .map_or("<none>", |row| row.name);
        println!("picked material {material} ({name})");
    }

    /// Right-click always builds with the active hotbar material. Air is a
    /// sentinel rather than a placeable block, so choosing it can never turn a
    /// place click into an unexpected removal.
    fn place_selected_block(&mut self) {
        let selected = material::material_voxel(self.material_panel.selected);
        if selected != Voxel::Air {
            self.edit_at_crosshair(Some(selected));
        }
    }
    /// Rebuild the shading pipeline from the current quality, graphs and material
    /// table — THE ONLY WAY the app builds that source, and the reason it is a
    /// method rather than four call sites.
    ///
    /// The generator mask is derived, not authored, so it has to be refreshed from
    /// the table immediately before the source is assembled. Two of the four
    /// original call sites got that wrong in the obvious way: loading a project and
    /// compiling a graph both rewrite pattern layers and then build a pipeline, so
    /// a project introducing a generator no previous table used would have compiled
    /// a shader without it and rendered those materials silently flat. Deriving
    /// here makes that unrepresentable rather than remembered.
    fn rebuild_dda_shader(&mut self) {
        self.quality.materials.pattern_generator_mask =
            material::generator_mask(self.material_table.rows());
        let source = dda::build_shader_source_for_output(
            &self.quality,
            &self.material_graph_shaders,
            self.renderer.output_format(),
        );
        // NEVER rebuild from identical source. `GraphEditor::apply` requests a
        // compile for EVERY graph command, and graph constants are emitted as WGSL
        // literals (`format_float` in `material_graph.rs`), so most edits really do
        // produce new source and really do need a pipeline. But not all of them:
        // moving a node, selecting one, collapsing one, or any edit the compiler
        // folds away leaves this string unchanged, and those must not touch the
        // hottest pipeline in the engine.
        //
        // This is a GUARD, not the fix. The fix is to stop baking authored values
        // into the source at all — see the arc note in
        // `docs/voxel-rt-optimization-ledger.md` (6.35) — because a slider dragged
        // through fifty values is still fifty distinct sources and fifty compiles.
        if self.applied_dda_shader_source.as_deref() == Some(source.as_str()) {
            return;
        }
        self.renderer
            .set_dda_shader_source(&self.gpu_context.device, &source);
        self.applied_dda_shader_source = Some(source);
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
    /// * the material table itself is 6912 bytes and goes straight out, gated by
    ///   [`MaterialTable::take_dirty`] so an idle panel costs nothing;
    /// * CAGI's baked cell attributes are a ~0.5 s rebuild, so a re-pack is handed
    ///   to the world thread through the SAME seam an edit's light attributes use
    ///   and lands via `WorldUpdate::LightAttributes`. Never automatic on a slider
    ///   tick — that would be a hitch per pixel of mouse travel.
    fn upload_material_edits(&mut self) {
        if let Some(rows) = self.material_table.take_dirty() {
            self.renderer
                .write_material_table(&self.gpu_context.queue, &rows);
            // Re-derive the generator mask from what the edited table can now
            // reach. Authoring a generator no row used before is the one edit that
            // needs a new pipeline, and it is rare — the mask only ever grows
            // within a session, so this recompiles once per newly-used generator
            // rather than once per slider tick. The assignment is all that is
            // needed: `requires_pipeline_rebuild` compares the mask and the
            // ordinary rebuild path does the rest.
            self.quality.materials.pattern_generator_mask =
                material::generator_mask(self.material_table.rows());
            // E5b emission lives in the per-cell buffer. A material emission edit
            // reaches GI when the explicit attribute re-pack below is requested;
            // direct shading still updates immediately through the material table.
        }
        if std::mem::take(&mut self.material_panel.repack_gi_requested)
            && self.quality.global_illumination.enabled
        {
            let attributes = self.material_table.cagi_attributes();
            // S3b: the response table holds seven shapes. Say so when a material
            // wanted an eighth, because the symptom otherwise is one emitter
            // quietly refusing to react while every other one does.
            let overflow = attributes.event_response_overflow();
            if overflow > 0 {
                println!(
                    "{overflow} material(s) want an event response and the light \
                     volume has only {} slots — they keep their peak emission and \
                     stop reacting",
                    voxel_rt::cagi::EVENT_RESPONSE_SLOTS - 1
                );
            }
            self.world_host
                .request_light_attributes(self.renderer.light_volume_grid(), attributes);
            // Attribute generation is asynchronous when world editing is
            // threaded.  The burst starts when its result is installed below.
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
    /// asking for a wall and getting an earlier imported asset would read as the
    /// button not working.
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

    fn rebuild_generated_world_from_profile(&mut self) -> Result<(), String> {
        if self.control_mode == ControlMode::StudioOrbit {
            return Ok(());
        }
        let world = VoxelWorld::generate(WORLD_SEED, WORLD_SEASON);
        let mut brickmap = Brickmap::build(&world);
        if let Some(profile) = &self.world_profile {
            apply_initial_generation_profile(
                &mut brickmap,
                profile,
                &self.environment_runtime,
                u64::from(WORLD_SEED),
            )
            .map_err(|error| error.to_string())?;
        }
        self.world_host = WorldHost::new(brickmap);
        self.world_host
            .set_world_thread(self.quality.world_edit.world_thread);
        {
            let brickmap = self.world_host.read();
            self.renderer
                .reupload_world(&self.gpu_context.device, &brickmap);
        }
        self.renderer.mark_light_volume_dirty();
        Ok(())
    }

    /// S0b — rebuild the studio around a loaded `.vox` model.
    ///
    /// Only in the studio: placing an asset in the generated world is a world-edit
    /// feature, not a material-preview operation.
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

    /// A mouse button changed: enter mouse-look, or start/stop a hold-to-repeat
    /// edit. All winit types stay in this file (platform-layer rule).
    fn handle_mouse_button(&mut self, button: MouseButton, pressed: bool, egui_consumed: bool) {
        if button == MouseButton::Middle {
            if pressed && !egui_consumed && self.cursor_grabbed {
                self.pick_material_at_crosshair();
            }
            return;
        }
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
        if place {
            self.place_selected_block();
        } else {
            self.edit_at_crosshair(None);
        }
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
        if self.input_state.place_held {
            self.place_selected_block();
        } else {
            self.edit_at_crosshair(None);
        }
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
                    if !self
                        .renderer
                        .apply_world_delta(&self.gpu_context.queue, delta)
                    {
                        // The brick headroom ran out: the buffers must be
                        // reallocated from the brickmap instead of patched.
                        needs_full_reupload = true;
                    }
                }
                WorldUpdate::LightAttributes {
                    grid,
                    attributes,
                    emissions,
                    responses,
                    build_micros,
                } => {
                    let installed = self.renderer.write_light_volume_attributes(
                        &self.gpu_context.queue,
                        grid,
                        attributes,
                        emissions,
                        responses.as_ref(),
                    );
                    println!(
                        "CAGI attributes rebuilt off-frame in {:.1} ms ({}installed)",
                        build_micros / 1000.0,
                        if installed { "" } else { "NOT " }
                    );
                    if installed {
                        self.gi_settle_frames = 12;
                    }
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

        // Phase timing is marked explicitly rather than with `scope` guards: a
        // guard borrows `span_recorder` for its whole scope, and every phase
        // below needs `&mut self`.
        let input_started = Instant::now();
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
                // Still its own stopwatch as well as part of CPU_INPUT: the
                // movement readout reports this one number on its own line.
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
        self.span_recorder
            .record(profiling::CPU_INPUT, input_started.elapsed());

        let edits_started = Instant::now();
        self.apply_held_edits(now);
        self.upload_world_updates();
        self.span_recorder
            .record(profiling::CPU_EDITS, edits_started.elapsed());

        let uniforms_started = Instant::now();
        // The graph editor is a reserved bottom panel, not a transparent overlay.
        // Keep the ray-traced storage texture and camera aspect ratio aligned with
        // the remaining upper viewport so the Studio subject stays centered there.
        self.renderer
            .set_viewport_height(&self.gpu_context.device, self.rendered_viewport_height());
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
        self.sun_settings.advance_day_cycle(frame_time_seconds);
        self.environment_runtime.day_phase = self.sun_settings.day_phase;
        self.advance_animation(frame_time_seconds);
        // Sun sliders and the runtime quality knobs were mutated during LAST
        // frame's overlay pass; a change shows up one frame later, which is
        // imperceptible.
        let (animation_params, event_params) = self.quality.animation_params(
            self.animation_clock.sample(),
            self.world_clock.sample(),
            self.world_events.len(),
        );
        let lighting_uniform = voxel_rt::lighting::lighting_uniform(
            &self.sun_settings,
            self.quality.shading_params(),
            self.quality.gi_params(),
            self.quality.water_params(),
            // The DISPATCH height, not the window height: the octave cutoff asks
            // what a shaded pixel can resolve, and a half-scale preset resolves
            // half as much.
            self.quality.material_params(self.renderer.resolution().1),
            animation_params,
            event_params,
        );
        // PROBED EVERY FRAME, not cached: EDR headroom changes while the user drags the
        // brightness slider or moves the window between displays, and a stale value means
        // tone-mapping into range the panel does not have. The windowed app is the only
        // caller that has a display to ask, which is why this is a builder call rather
        // than a parameter — see `LightingUniform::with_output_params`.
        let display_headroom = self.gpu_context.display_headroom(self.headroom_choice);
        let lighting_uniform = lighting_uniform.with_output_params(OutputParams {
            hdr_headroom: display_headroom.ratio(),
            tonemap: self.tonemap_curve,
            content_peak: self.content_peak,
            exposure: self.exposure,
        });
        // A moved sun invalidates the whole light volume (E4: the world is
        // static, the sun is not). Dragging the slider therefore re-floods every
        // frame of the drag, which is what makes the GI follow the drag instead of
        // lagging a second behind it.
        if self
            .sun_settings
            .requires_light_reflood(&self.flooded_sun_settings)
        {
            self.renderer.mark_light_volume_dirty();
            self.flooded_sun_settings = self.sun_settings;
        }

        self.span_recorder
            .record(profiling::CPU_UNIFORMS, uniforms_started.elapsed());

        // THE BACKPRESSURE MEASUREMENT: under FIFO the CPU blocks here when the
        // presentation queue is full, so a wave that lives in this span is vsync
        // pacing rather than renderer cost.
        let acquire_started = Instant::now();
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
        self.span_recorder
            .record(profiling::CPU_ACQUIRE, acquire_started.elapsed());
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

        let encode_started = Instant::now();
        let gi_iterations = if self.gi_settle_frames > 0 {
            self.quality.gi_iterations_per_frame().max(8)
        } else {
            self.quality.gi_iterations_per_frame()
        };
        self.renderer.encode_light_volume(
            &self.gpu_context.queue,
            &mut light_volume_encoder,
            &lighting_uniform,
            &camera_uniform,
            &self.world_events.upload_array(),
            gi_iterations,
            self.frame_timers.as_ref(),
        );
        self.gi_settle_frames = self.gi_settle_frames.saturating_sub(1);
        self.renderer.encode_frame(
            &self.gpu_context.queue,
            &mut encoder,
            &camera_uniform,
            &target_view,
            self.frame_timers.as_ref(),
        );
        let previous_vsync_enabled = self.vsync_enabled;
        let previous_output_depth = self.output_depth;
        // E6 — is the view underwater? Asked of the ACTIVE eye against the
        // authority, so it is the same question the shading pass asks of the
        // primary ray's origin, and it holds in fly mode too.
        let eye_submerged = {
            let brickmap = self.world_host.read();
            water::eye_is_submerged(&brickmap, self.active_pose().position)
        };
        self.span_recorder
            .record(profiling::CPU_ENCODE, encode_started.elapsed());

        let overlay_started = Instant::now();
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
            target: self.target_highlight(),
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
                .map(|frame_timers| frame_timers.render_span_writes(GPU_OVERLAY)),
            &mut self.vsync_enabled,
            &mut self.output_depth,
            self.gpu_context.output_support(),
            self.gpu_context.color_space(),
            self.gpu_context.display_headroom(self.headroom_choice),
            self.gpu_context.headroom_backend(),
            &mut self.headroom_choice,
            &mut self.tonemap_curve,
            &mut self.content_peak,
            &mut self.exposure,
            &mut self.sun_settings,
            &mut self.quality,
            &mut self.material_table,
            &mut self.material_panel,
            &mut self.studio_assets,
            &mut self.graph_editor,
        );
        self.span_recorder
            .record(profiling::CPU_OVERLAY, overlay_started.elapsed());

        let readback_slot = self
            .frame_timers
            .as_ref()
            .and_then(|frame_timers| frame_timers.encode_resolve(&mut encoder));

        let submit_started = Instant::now();
        self.gpu_context
            .queue
            .submit([light_volume_encoder.finish(), encoder.finish()]);
        self.span_recorder
            .record(profiling::CPU_SUBMIT, submit_started.elapsed());
        if let (Some(frame_timers), Some(slot_index)) = (&self.frame_timers, readback_slot) {
            frame_timers.after_submit(slot_index);
        }
        // The other half of the backpressure question: a driver may block in
        // present rather than in acquire, and which one it picks is a platform
        // detail to observe rather than assume.
        let present_started = Instant::now();
        self.window.pre_present_notify();
        surface_frame.present();
        self.span_recorder
            .record(profiling::CPU_PRESENT, present_started.elapsed());
        // Report what we hold, before the frame boundary latches the drift
        // baseline. The CPU brickmap size is read under the world lock, which is
        // already taken and released elsewhere in this frame, so this is a second
        // brief read rather than a held borrow.
        {
            let memory = self.overlay.performance_memory();
            memory.set(
                profiling::MEMORY_WORLD_CPU,
                self.world_host.read().cpu_bytes() as u64,
            );
            memory.set(profiling::MEMORY_WORLD_GPU, self.renderer.world_gpu_bytes());
            memory.set(
                profiling::MEMORY_LIGHT_VOLUME,
                self.renderer.light_volume_gpu_bytes(),
            );
            memory.set(
                profiling::MEMORY_STORAGE_TEXTURE,
                self.renderer.storage_texture_bytes(),
            );
        }

        // Drain every span and fold this frame in. LAST in the frame on purpose:
        // the spans collected here are then all from this frame, present
        // included, rather than one frame stale.
        self.overlay.record_frame(&self.span_recorder, gpu_timings);

        if std::mem::take(&mut self.studio_assets.save_requested) {
            self.save_studio_project(false);
        }
        if std::mem::take(&mut self.studio_assets.load_requested) {
            self.load_studio_project_from_panel();
        }
        if std::mem::take(&mut self.studio_assets.restore_recovery_requested) {
            self.restore_studio_recovery();
        }
        if std::mem::take(&mut self.studio_assets.discard_recovery_requested) {
            self.discard_studio_recovery();
        }
        if let Some(slot) = self.graph_editor.material_select_requested.take() {
            self.graph_editor.material_slot = slot;
            self.open_graph_editor_graph();
        }
        if std::mem::take(&mut self.graph_editor.open_requested) {
            self.open_graph_editor_graph();
        }
        if std::mem::take(&mut self.graph_editor.duplicate_requested) {
            self.duplicate_graph_editor_graph();
        }
        if std::mem::take(&mut self.graph_editor.reset_requested) {
            self.graph_editor.reset_graph();
        }
        if std::mem::take(&mut self.graph_editor.save_requested) {
            self.save_graph_editor_graph();
        }
        if std::mem::take(&mut self.graph_editor.compile_requested) {
            self.compile_graph_editor();
        }
        self.update_project_autosave();
        self.upload_material_edits();
        // The heaviest toggle in the engine: a surface reconfigure, a storage-texture
        // reallocation and two pipeline rebuilds. `set_output_depth` returns Some only
        // when something actually moved, so an unchanged toggle — or one the device
        // vetoed — costs nothing.
        if self.output_depth != previous_output_depth {
            if let Some(resolved) = self.gpu_context.set_output_depth(self.output_depth) {
                // Move the tonemap to the one that suits the new depth, so the toggle is
                // not a silent look change. Reinhard+HDR is exactly the SDR curve through
                // scene white and becomes exactly plain Reinhard at 1x headroom; only its
                // bounded highlight continuation changes. The identity knee remains one
                // click away for anyone who wants the brighter reading.
                self.tonemap_curve = TonemapCurve::default_for(resolved.writes_extended_range());
                self.renderer
                    .set_output_format(&self.gpu_context.device, resolved);
                // egui builds its own render pipeline against the surface format, so
                // it is a consumer of the output format too — the sixth, and the one
                // outside our own passes.
                self.overlay.set_surface_format(
                    &self.window,
                    &self.gpu_context.device,
                    resolved.surface(),
                );
                // The shading source carries the storage-texture TYPE, so the
                // pipeline has to be rebuilt from patched source, not just rebound.
                self.rebuild_dda_shader();
            }
            // Snap the request back to what was actually resolved, so the overlay
            // shows the truth rather than a wish the device refused.
            self.output_depth = self.renderer.output_format().depth();
        }
        if self.vsync_enabled != previous_vsync_enabled {
            self.gpu_context.set_vsync(self.vsync_enabled);
            // A present-mode change is a deliberate discontinuity in frame
            // pacing. Keeping the old history would leave the reconfigure
            // transient sitting in the percentiles, reading as a regression that
            // is really just the toggle.
            self.overlay.reset_performance_history();
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
            self.rebuild_dda_shader();
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
            // E2: the ~0.5 s attribute build goes to the world thread when there is
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
                    &self.material_table.cagi_attributes(),
                );
            }
            if attribute_source == AttributeSource::Deferred
                && self.quality.global_illumination.enabled
            {
                self.world_host.request_light_attributes(
                    self.renderer.light_volume_grid(),
                    self.material_table.cagi_attributes(),
                );
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
        match &event {
            WindowEvent::CursorMoved { position, .. } => {
                state.cursor_position = Some((position.x, position.y));
            }
            WindowEvent::CursorLeft { .. } => state.cursor_position = None,
            _ => {}
        }
        let input_region = state.input_region();
        let egui_consumed = state.overlay.handle_window_event(&state.window, &event);

        match event {
            WindowEvent::CloseRequested => event_loop.exit(),
            WindowEvent::Resized(new_size) => state.resize(new_size),
            WindowEvent::RedrawRequested => state.redraw(),
            WindowEvent::KeyboardInput { event, .. } => {
                if input_region == InputRegion::Viewport
                    && !egui_consumed
                    && !state.overlay.wants_keyboard_input()
                {
                    if let PhysicalKey::Code(key_code) = event.physical_key {
                        state.handle_keyboard(key_code, event.state == ElementState::Pressed);
                    }
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
                if input_region != InputRegion::Viewport && state.cursor_grabbed {
                    state.set_cursor_grabbed(false);
                }
                state.handle_mouse_button(
                    button,
                    button_state == ElementState::Pressed,
                    input_region != InputRegion::Viewport || egui_consumed,
                );
            }
            // Mouse wheel tunes the ACTIVE mode's speed: 12 m/s was too fast to
            // line voxels up by eye, so the base is slow and the wheel covers the
            // range. In walk mode the same notch tunes the walk speed (E2b).
            WindowEvent::MouseWheel { delta, .. } => {
                if input_region == InputRegion::Viewport && !egui_consumed {
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
            if state.cursor_grabbed
                && state.input_region() == InputRegion::Viewport
                && !state.overlay.wants_pointer_input()
            {
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
    let arguments: Vec<String> = std::env::args().collect();
    if arguments
        .iter()
        .any(|argument| argument == "--validate-project")
    {
        let config = VoxelEngineConfig::from_args(&arguments)
            .unwrap_or_else(|error| panic!("invalid voxel engine arguments: {error}"));
        match voxel_rt::engine_runtime::ProjectRuntime::validate_strict(&config.project_root) {
            Ok(()) => println!("project {} is valid", config.project_root.display()),
            Err(diagnostics) => {
                for diagnostic in diagnostics {
                    eprintln!("{}", diagnostic.message);
                }
                std::process::exit(1);
            }
        }
        return;
    }
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
