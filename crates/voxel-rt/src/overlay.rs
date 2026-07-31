//! egui overlay: stats panel (window/render sizes, moving-average frame time,
//! FPS, per-pass GPU times), the vsync lever, a collapsible sun-position
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

use std::collections::VecDeque;

use winit::event::WindowEvent;
use winit::window::Window;

use crate::ao::AoMode;
use crate::character::Submersion;
use crate::debug_pool::{POOL_DEPTH_METERS, POOL_DISTANCE_AHEAD_METERS, POOL_WATER_RADIUS_METERS};
use crate::frame_timing::FrameTimings;
use crate::lighting::SunSettings;
use crate::material_edit::{draw_material_section, MaterialPanelState};
use crate::material_table::MaterialTable;
use crate::material_tune::ProvenanceTable;
use crate::shadows::ShadowMode;
use crate::variants::{
    levers_of, Lever, LeverId, LeverRange, LeverSubsystem, LeverValue, QualityPreset,
    RenderQuality, QUALITY_PRESETS, VOXELS_PER_METER,
};
use crate::water::WaterMode;
use crate::world_edit::ClearanceUpdateMode;
use crate::world_host::WorldEditStats;

const FRAME_TIME_SAMPLE_COUNT: usize = 120;

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
        sun_settings: &mut SunSettings,
        quality: &mut RenderQuality,
        carve_test_pool_requested: &mut bool,
        material_table: &mut MaterialTable,
        material_panel: &mut MaterialPanelState,
        material_provenance: &mut ProvenanceTable,
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
                             one coalesced delta per bulk edit (hence the separate voxel count). \
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
                        draw_quality_section(ui, quality);
                        draw_material_section(
                            ui,
                            material_table,
                            material_panel,
                            material_provenance,
                        );
                        draw_debug_section(
                            ui,
                            carve_test_pool_requested,
                            frame_data.movement.studio_orbit_distance_meters.is_none(),
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

/// Test tools that CHANGE THE WORLD, kept in their own section and worded so
/// that is unmistakable.
///
/// Deliberately not registry levers: [`crate::variants::REGISTRY`] rows carry
/// measured frame-time verdicts and drive shader permutations, and a one-shot
/// world edit has neither. The overlay only *asks* — the platform layer owns the
/// edit, exactly as with every other control here.
fn draw_debug_section(
    ui: &mut egui::Ui,
    carve_test_pool_requested: &mut bool,
    world_edits_allowed: bool,
) {
    ui.collapsing("Debug tools", |ui| {
        // Greyed out rather than left clickable-but-ignored: a button that does
        // nothing reads as a bug, and the studio deliberately has no world to edit.
        let hover = if world_edits_allowed {
            format!(
                "MODIFIES THE WORLD. Carves a {:.0} m wide, {POOL_DEPTH_METERS:.0} m deep pool \
                 with a walk-in shore, centred {POOL_DISTANCE_AHEAD_METERS:.0} m in front of the \
                 eye, and fills it with water — the island's own water is at most 1.75 m deep, \
                 under the 1.44 m the body needs to swim. Applied through E2's edit pipeline on \
                 the world thread; the light volume re-floods afterwards.",
                POOL_WATER_RADIUS_METERS * 2.0,
            )
        } else {
            "Disabled in the material studio: its scene is composed, not dug, and \
             the whole point is that the voxel in frame is a known sample. Restart \
             without `--studio` to edit the world."
                .to_string()
        };
        if ui
            .add_enabled(
                world_edits_allowed,
                egui::Button::new(format!(
                    "Carve {POOL_DEPTH_METERS:.0} m water pool ahead (P)"
                )),
            )
            .on_hover_text(hover)
            .clicked()
        {
            *carve_test_pool_requested = true;
        }
    });
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
