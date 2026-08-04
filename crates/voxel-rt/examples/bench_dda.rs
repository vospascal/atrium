//! Headless DDA-pass benchmark — the PERMANENT perf harness for voxel-rt.
//!
//! Baseline numbers, how to read the output, and the regression protocol
//! for new features live in `docs/voxel-rt-bench.md`. Run with:
//! ```text
//! cargo run -p voxel-rt --example bench_dda --release
//! ```
//!
//! Trailing section numbers run a subset — `... --release -- 3` measures only
//! the E1b section. Sections are independent (isolation rule), so a subset run
//! yields exactly the rows a full run would print for it.
//!
//! No window, no surface: instance → adapter → device, the real island
//! world (seed 1, season 0.0) + brickmap, and the real
//! [`voxel_rt::passes::dda::DdaPass`] dispatched at exactly 2560x1440 (the
//! Retina 2x resolution the app renders at on the dev machine) — or at that
//! size times a preset's render scale in section 4. Each (variant, scenario)
//! pair times [`BATCH_COUNT`] batches of [`DISPATCHES_PER_BATCH`] back-to-back
//! dispatches (wall-clock per batch / batch size — Metal resolves pass-boundary
//! timestamp counters to zero when several passes share a command buffer, and
//! the batch amortizes submit/poll overhead the way continuous rendering does)
//! and reports median + p95 per-dispatch milliseconds.
//!
//! Scenarios (fixed, deterministic — poses documented at the definitions):
//!   A  top-down over the island center from 60 m altitude, default sun
//!      (azimuth 32.5°, elevation 50.8°)
//!   B  same view, low sun (elevation 5° — worst case for shadow rays)
//!   C  ground-level at the spawn point looking across the island, default sun
//!   D  same view, low sun
//!
//! **The variant tables are DERIVED from the lever registry**
//! (`voxel_rt::variants::REGISTRY`, E1c): each section collects the registry's
//! [`BenchPoint`](voxel_rt::variants::BenchPoint)s for itself and applies them
//! to that section's baseline quality, so adding a lever row adds a bench
//! column forever after and no parallel list can drift. Only the *anchors* —
//! the baselines and reference rows a section is judged against — are spelled
//! out here.
//!
//! Fourteen sections, each its own variant table or report (isolation rule).
//! 1-8 are summarised here; **9-14 are documented at their own definitions**, and
//! `-- 14` is the one to reach for after touching the output path:
//!
//! 1. **Traversal levers, AO off** — the Stage 2 regression gate. Every column
//!    has `AO_MODE = AO_MODE_OFF`, CAGI off and E6's water optics off, so the
//!    medians stay comparable with the recorded pre-E1 baseline. Correctness evidence: the low-sun scenarios
//!    (B, D) are rendered per variant and compared pixel-by-pixel against the
//!    no-fast-path reference (`stage2-baseline`).
//!
//! 2. **E1 ray-traced AO variants** — the ray-count / distance / direction /
//!    falloff ladder around the grid center. The default-sun scenarios (A, C)
//!    are captured, and each variant's differing-pixel count vs `ao-off`
//!    reports how much of the frame AO touches.
//!
//! 3. **E1b cheap occlusion + soft shadows** — the analytic estimators, the
//!    three AO cost-cutting levers, the hard-vs-soft shadow sweep, and E1c's
//!    const-vs-uniform A/B for the fade distances. All four scenarios captured.
//!
//! 4. **E1c quality presets** — Potato / Quest / Balanced / Beautiful, each
//!    dispatched at ITS OWN render scale (the tier knob is a resolution, so a
//!    preset table measured at a single size would be fiction). This is the
//!    headline table future gates quote. It also reports the startup cost of
//!    the preset pipeline cache.
//!
//! 5. **E4 CAGI light volume** — propagation rule / resolution / sky test /
//!    sampling contenders, the memory table, the convergence tables and the CPU
//!    cross-check of the transport rule.
//!
//! 6. **E2 world authority + edit pipeline** — a *pipeline*, not a shader, so it
//!    reports median / p99 / max per frame across four edit-storm patterns, plus
//!    build, snapshot and GPU-readback costs and the audio-ray seam.
//!
//! 7. **E2b character movement + collision** — also a CPU pipeline and also a
//!    distribution: per-step cost of the walking body across the cost axes the
//!    sweep has (open-air cross-section scans, auto-step frequency, substep
//!    count from the frame delta), plus the ground search that entering walk
//!    mode runs. GPU-free, so `-- 7` finishes in seconds.
//!
//! 8. **E6 water optics** — the four cost tiers (no secondary rays / reflection
//!    only / refraction only / both) and the bounce budget, on scenes that
//!    actually contain water. It runs over its OWN brickmap — the island plus a
//!    carved debug pool, because the island's natural water is only 0.6-1.75 m
//!    deep and refraction needs depth to be visible — and over its own four
//!    scenarios, two of them with the camera INSIDE the pool (Snell's window
//!    looking up, extinction looking sideways). Its numbers therefore do not
//!    compare with sections 1-5, by construction.
//!
//! All PNGs land in `target/bench_dda/`.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Instant;

use std::path::PathBuf;
use voxel_environment::SunSettings;
use voxel_material::animation_clock::AnimationClockSample;
use voxel_material::material::{GpuMaterial, MaterialKind, MATERIALS};
use voxel_material::pattern::{
    PatternBlend, PatternFaces, PatternFrame, PatternGenerator, PatternLayer, PatternStack,
    PatternTarget, DEFAULT_TEXELS_PER_VOXEL,
};
use voxel_rt::ao::{AoMode, AoSettings};
use voxel_rt::brickmap::{
    Brickmap, ClearanceUpdate, BRICK_SIZE, MATERIAL_WORDS_PER_BRICK, OCCUPANCY_WORDS_PER_BRICK,
};
use voxel_rt::cagi::{
    cell_sees_sky_by_column, propagate_reference, unpack_light, CagiRule, CagiSettings, CELL_SOLID,
};
use voxel_rt::camera::{CameraInput, CameraPose, CameraUniform, DEFAULT_VERTICAL_FOV_RADIANS};
use voxel_rt::character::{self, CharacterController, CharacterSettings};
use voxel_rt::light_fixture::{self, NotchState, RainbowCorridor};
use voxel_rt::lighting::LightingUniform;
use voxel_rt::passes::cagi::{CagiPass, LightVolume};
use voxel_rt::passes::composer::{Composition, FragmentEdit, ShaderProgram};
use voxel_rt::passes::dda::{build_shader_source, DdaPass, SHADER_SOURCE};
use voxel_rt::passes::world_bindings::WorldBindings;

use voxel_material::world_event::{GpuWorldEvent, MAX_WORLD_EVENTS};
use voxel_rt::material_graph_assets::MaterialGraphAssetService;
use voxel_rt::material_table::MaterialTable;
use voxel_rt::studio_assets::{StudioProject, StudioProjectStore};
use voxel_rt::variants::{
    bench_points_of, BenchSection, LeverId, LeverValue, QualityPreset, RenderQuality,
    QUALITY_PRESETS,
};
use voxel_rt::voxel_dda;
use voxel_rt::world_edit::{VoxelEdit, WorldEditSettings};
use voxel_rt::world_host::{WorldHost, WorldUpdate};

use glam::Vec3;
use voxel_core::world::{
    Voxel, VoxelWorld, WorldVoxelCoord, DETAIL_CELLS_PER_WORLD_VOXEL, VOXEL_SIZE, WATER_LEVEL,
    WORLD_SIZE_X, WORLD_SIZE_Z,
};

const POOL_DEPTH_METERS: f32 = 5.0;

/// Water-optics benchmark fixture. Its geometry is composed exclusively from
/// aligned one-metre world voxels; the detail coordinates are only camera-facing
/// compatibility fields.
#[derive(Clone, Copy)]
struct WaterPool {
    centre_voxel_x: i32,
    centre_voxel_z: i32,
}

impl WaterPool {
    fn in_front_of(eye: Vec3, forward: Vec3) -> Self {
        let centre = eye + Vec3::new(forward.x, 0.0, forward.z).normalize_or_zero() * 10.0;
        Self {
            centre_voxel_x: (centre.x / VOXEL_SIZE).floor() as i32,
            centre_voxel_z: (centre.z / VOXEL_SIZE).floor() as i32,
        }
    }

    fn surface_centre(self) -> Vec3 {
        Vec3::new(
            (self.centre_voxel_x as f32 + 0.5) * VOXEL_SIZE,
            (WATER_LEVEL + 1) as f32 * VOXEL_SIZE,
            (self.centre_voxel_z as f32 + 0.5) * VOXEL_SIZE,
        )
    }

    fn label(self) -> &'static str {
        "one-metre-block water basin"
    }
}

/// Output resolution at render scale 1.0: the dev machine's physical Retina
/// size (2560x1440, reported as 1280x720 logical). All historical numbers were
/// taken here.
const OUTPUT_WIDTH: u32 = 2560;
const OUTPUT_HEIGHT: u32 = 1440;

/// Dispatches encoded back-to-back in one command buffer per timed batch.
/// The GPU stays busy through the whole batch, so per-dispatch time =
/// wall-clock / batch size reflects app-like continuous rendering.
const DISPATCHES_PER_BATCH: usize = 25;
/// Timed batches per (variant, scenario).
const BATCH_COUNT: usize = 12;
/// Untimed warmup batches (pipeline residency, clock ramp).
const WARMUP_BATCHES: usize = 2;

/// World generation parameters — must match `main.rs` so the bench measures
/// the island the app shows.
const WORLD_SEED: u32 = 1;
const WORLD_SEASON: f32 = 0.0;

/// One fixed camera + sun combination. The camera uniform is built per variant
/// because a variant's render scale sets the resolution.
struct Scenario {
    label: &'static str,
    pose: CameraPose,
    sun: SunSettings,
    /// Captured scenarios get rendered per variant, written as PNGs, and
    /// pixel-compared against the section's reference variant.
    capture_image: bool,
}

impl Scenario {
    fn camera_uniform(&self, resolution: (u32, u32)) -> CameraUniform {
        self.pose
            .gpu_uniform(DEFAULT_VERTICAL_FOV_RADIANS, resolution)
    }

    /// This scenario's sun with `quality`'s RUNTIME knobs (AO strength,
    /// penumbra scale, fade ramp, the E4 GI knobs) — the levers that need no
    /// pipeline rebuild are swept exactly the way the app applies them.
    fn lighting_uniform(&self, quality: &RenderQuality) -> LightingUniform {
        let (animation_params, event_params) = quality.animation_params(
            AnimationClockSample::FROZEN,
            AnimationClockSample::FROZEN,
            0,
        );
        voxel_rt::lighting::lighting_uniform(
            &self.sun,
            quality.shading_params(),
            quality.gi_params(),
            quality.water_params(),
            // The DISPATCH height — `OUTPUT_HEIGHT` through this variant's render
            // scale, since section 4 measures presets at their own resolutions and
            // the octave cutoff is a per-pixel question.
            quality.material_params((OUTPUT_HEIGHT as f32 * quality.render_scale) as u32),
            // S3: the bench pins the animation clock at zero and ships no world
            // events, so a measured frame is reproducible. This is stated here
            // rather than left to the deterministic lever, because a scenario
            // sweeping presets would otherwise inherit whatever that preset
            // happened to set — and a bench that animates is not a bench.
            animation_params,
            event_params,
        )
    }
}

/// One shader build to measure: a full [`RenderQuality`] (so the runtime knobs
/// and the render scale ride along, not just the compile-time consts) plus any
/// extra source surgery this row needs.
struct Variant {
    label: String,
    quality: RenderQuality,
    /// Text substitutions for A/B rows that cannot be expressed as a lever, as
    /// `(fragment file, from, to)`.
    ///
    /// Naming the fragment is required rather than cosmetic: the shader is composed
    /// per fragment now, so a substitution has to be applied to the file that owns
    /// the anchor. Both current rows are honest about which that is — the tonemap
    /// dispatch lives in `voxel-color`'s `tonemap.wgsl` and the AO fade ramp in
    /// `dda.wgsl`.
    edits: Vec<(&'static str, String, String)>,
}

impl Variant {
    /// The normal case: the shader is exactly what this quality compiles to.
    fn new(label: String, quality: RenderQuality) -> Variant {
        Variant {
            label,
            quality,
            edits: Vec::new(),
        }
    }

    /// The shading program this variant compiles to.
    fn dda_program(&self) -> ShaderProgram {
        Composition::shading().build(&self.quality.shading_shader_defs(), &self.fragment_edits())
    }

    /// The CA program. No row substitutes into the CA pass, so this is the plain build.
    fn cagi_program(&self) -> ShaderProgram {
        voxel_rt::passes::cagi::build_program(&self.quality)
    }

    fn fragment_edits(&self) -> Vec<FragmentEdit<'_>> {
        self.edits
            .iter()
            .map(|(file, from, to)| FragmentEdit {
                file,
                apply: Box::new(move |text: &str| text.replacen(from.as_str(), to.as_str(), 1)),
            })
            .collect()
    }

    /// Dispatch size: the tier knob is a resolution, so a variant with a render
    /// scale below 1.0 is measured at its real pixel count.
    fn resolution(&self) -> (u32, u32) {
        let width = ((OUTPUT_WIDTH as f32 * self.quality.render_scale) as u32).max(1);
        let height = ((OUTPUT_HEIGHT as f32 * self.quality.render_scale) as u32).max(1);
        (width, height)
    }
}

/// One independent measurement section.
struct Section {
    heading: &'static str,
    scenarios: Vec<Scenario>,
    variants: Vec<Variant>,
    /// Label of the variant every other row is pixel-compared against.
    reference_label: &'static str,
    compare_heading: &'static str,
    /// Zoomed crops written per captured scenario and variant: the
    /// compromise-checklist evidence (E4). `(name, x, y, width, height)` in
    /// render-target pixels, written at CROP_ZOOM. Empty for sections that judge
    /// numbers rather than artifacts.
    crop_regions: &'static [(&'static str, u32, u32, u32, u32)],
}

impl Section {
    fn reference_index(&self) -> usize {
        self.variants
            .iter()
            .position(|variant| variant.label == self.reference_label)
            .unwrap_or_else(|| {
                panic!(
                    "section `{}` has no variant labelled `{}`",
                    self.heading, self.reference_label
                )
            })
    }
}

/// Timing table of one section: `[variant][scenario] = (median, p95)` ms.
type TimingTable = Vec<Vec<(f32, f32)>>;

/// Uncaptured GPU errors seen during the run. See the handler in `gpu()`.
static GPU_ERRORS: AtomicUsize = AtomicUsize::new(0);

fn main() {
    // Optional section filter: `-- 1 3` runs sections 1 and 3 only. No
    // argument = the full run (the documented default). Sections are
    // independent by the isolation rule, so running one in isolation is
    // exactly equivalent to reading its rows out of a full run — and it keeps
    // a single section inside a shell timeout.
    // `--no-collapse` builds the brickmap WITHOUT the uniform collapse, which is
    // the only valid way to turn the uniform-brick fast path off (tag and fast
    // path are one data format — see Brickmap::build_uncollapsed). It is a whole
    // separate run rather than a variant column because every variant in a
    // section shares one uploaded brickmap; compare the `current` column of a
    // normal run against the `current` column of a --no-collapse run, and take
    // several of each, because cross-run noise is the same order as the effect.
    let collapse_uniform = !std::env::args().any(|argument| argument == "--no-collapse");
    let use_project = std::env::args().any(|argument| argument == "--project");
    let selected_sections: Vec<usize> = std::env::args()
        .skip(1)
        .filter(|argument| argument != "--no-collapse" && argument != "--project")
        .map(|argument| {
            argument
                .parse()
                .unwrap_or_else(|_| panic!("section filter must be a number, got `{argument}`"))
        })
        .collect();
    let runs_section =
        |section: usize| selected_sections.is_empty() || selected_sections.contains(&section);

    let world_start = Instant::now();
    let world = VoxelWorld::generate(WORLD_SEED, WORLD_SEASON);
    let brickmap = if collapse_uniform {
        Brickmap::build(&world)
    } else {
        Brickmap::build_uncollapsed(&world)
    };
    println!(
        "world + brickmap ready in {:.2?} ({} occupied bricks, uniform collapse {})",
        world_start.elapsed(),
        brickmap.occupied_brick_count(),
        if collapse_uniform { "on" } else { "OFF" }
    );

    // The CPU-side E5b attribute/emission build is useful even on machines
    // without a compatible GPU. Keep this report before adapter acquisition so
    // a failed hardware bench still records the cost that does not need one.
    if runs_section(5) {
        report_light_volume_memory(&brickmap);
    }
    if runs_section(6) {
        report_edit_build_times(&world, &brickmap);
    }

    let (device, queue) = create_headless_device();
    // The brickmap/lighting buffers are uploaded ONCE and shared by every
    // variant's shading and CAGI passes (E4's WorldBindings seam) — a section
    // with a dozen variants would otherwise upload ~30 MB a dozen times.
    let world_bindings = WorldBindings::new(&device, &brickmap);
    // ...including the event field, which the bench keeps EMPTY for the whole
    // run. One write at setup rather than one per batch: nothing here ever
    // raises an event, so a per-batch write would re-upload the same 768 zero
    // bytes into every measured section.
    world_bindings.write_world_events(&queue, &BENCH_WORLD_EVENTS);

    // `--project` uploads the CHECKED-IN PROJECT's material table over the
    // compiled one, through the same `load_live_state` call the app's startup
    // uses.
    //
    // WHY THIS FLAG EXISTS. The two tables are not the same, and the difference
    // is not cosmetic: the compiled table authors pattern layers only on `lava`
    // and `slate tile`, and the generated island contains NEITHER — it is grass,
    // dirt, sediment, stone and water. So a default bench run measures pattern
    // layers that nothing on screen carries, and reports patterns as free. The
    // project puts two layers on `stone`, which is 83,979 voxels and most of
    // what the camera sees. Measure the shipped look with this flag; without it,
    // the material columns are measuring the compiled table's authoring, which
    // the app replaces at startup.
    if use_project {
        let project_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../studio-project");
        let store = StudioProjectStore::new(&project_path);
        let mut table = MaterialTable::default();
        let mut project_quality = RenderQuality::default();
        match StudioProject::load_live_state(&store, &mut table, &mut project_quality) {
            Ok((project, warnings)) => {
                // `load_live_state` applies the authored material ROWS only. The
                // pattern layers live in the graphs and reach the table through
                // this second call, exactly as the app's startup does — without
                // it the "project" table still has zero layers on stone and this
                // flag measures nothing, which is how the first version of it was
                // wrong.
                let (_graphs, diagnostics) = MaterialGraphAssetService::load_shader_set_for_editing(
                    &project_path,
                    &project,
                    &mut table,
                );
                world_bindings.write_material_table(&queue, &table.gpu_rows());
                println!(
                    "material table: LOADED FROM PROJECT ({} warning(s), {} graph diagnostic(s))",
                    warnings.len(),
                    diagnostics.len()
                );
            }
            Err(error) => panic!("--project asked for the checked-in project: {error}"),
        }
    } else {
        println!("material table: compiled defaults (pass --project for the shipped authoring)");
    }

    if runs_section(1) {
        run_section(
            &device,
            &queue,
            &world_bindings,
            &brickmap,
            traversal_section(),
        );
    }
    if runs_section(2) {
        run_section(
            &device,
            &queue,
            &world_bindings,
            &brickmap,
            ray_traced_ao_section(),
        );
    }
    if runs_section(3) {
        run_section(
            &device,
            &queue,
            &world_bindings,
            &brickmap,
            cheap_occlusion_section(),
        );
    }
    if runs_section(4) {
        report_preset_pipeline_cache(&device, &world_bindings, &brickmap);
        run_section(
            &device,
            &queue,
            &world_bindings,
            &brickmap,
            preset_section(),
        );
    }
    if runs_section(5) {
        run_section(&device, &queue, &world_bindings, &brickmap, cagi_section());
        report_cagi_convergence(&device, &queue, &world_bindings, &brickmap);
        report_cagi_cpu_cross_check(&device, &queue, &world_bindings, &brickmap);
    }
    if runs_section(6) {
        // E2 — the ARCHITECTURE section. It measures a pipeline, not a shader, so
        // it prints distributions (median / p99 / max) instead of medians: the
        // whole question is whether an edit can hitch a frame.
        report_edit_memory(&device, &brickmap);
        report_edit_storm(&device, &queue, &brickmap, &edit_storm_runs());
        report_edit_reflood(&device, &queue, &brickmap);
        report_occupancy_readback(&device, &queue, &brickmap);
        report_audio_ray_cost(&brickmap);
    }
    if runs_section(7) {
        // E2b — the walking body. Also a pipeline rather than a shader, and also
        // reported as a distribution: what matters is the WORST movement step a
        // frame can be handed, not the average one. GPU-free, so it runs in
        // seconds.
        report_character_movement_cost(&brickmap);
    }
    if runs_section(8) {
        // E6 — water optics. Its own world (island + one carved pool) and its own
        // bindings, because the island's natural water is too shallow for
        // refraction or an underwater camera to say anything. Sections 1-7 are
        // untouched by the carve, which is why no baseline above had to move for it.
        let pool = section_eight_pool();
        let pooled_brickmap = water_section_world(&brickmap, pool);
        let pooled_bindings = WorldBindings::new(&device, &pooled_brickmap);
        run_section(
            &device,
            &queue,
            &pooled_bindings,
            &pooled_brickmap,
            water_section(pool),
        );
    }
    if runs_section(9) {
        // S1/S2 — the material model. Its own bindings, holding a SATURATED material
        // table: every visible row carries four pattern layers.
        //
        // That is the whole reason this section needs bindings of its own, and it is
        // not a convenience. No row in the compiled table authors a layer (that is
        // S6's step), so a sweep over the shipped table would find the flag test
        // short-circuiting on every hit and report four layers as free. The number
        // this section exists to produce is the PER-LAYER SLOPE, and only a table
        // that authors layers can produce it.
        //
        // Sections 1-8 are untouched: they keep the shared bindings and the compiled
        // table, so no baseline above moves for this.
        let material_bindings = WorldBindings::new(&device, &brickmap);
        material_bindings.write_material_table(&queue, &saturated_material_rows());
        run_section(
            &device,
            &queue,
            &material_bindings,
            &brickmap,
            materials_section(),
        );
    }
    if runs_section(10) {
        report_emitter_probes(&device, &queue);
    }
    if runs_section(11) {
        report_generator_costs(&device, &queue, &brickmap);
    }
    if runs_section(12) {
        write_generator_swatches(&device, &queue);
    }
    if runs_section(13) {
        report_material_costs(&device, &queue, use_project);
    }
    if runs_section(14) {
        // The output path. Runs on the SHARED bindings and the shared brickmap —
        // the tonemap is a per-pixel term applied after shading, so it needs no
        // special world and disturbs no baseline above.
        report_tonemap_costs(&device, &queue, &world_bindings, &brickmap);
    }
    if runs_section(15) {
        // L0 — the authored light fixture. Its own world and its own bindings,
        // like section 8: the question is what indirect light does in a room
        // whose right answer is known by construction, and the island cannot
        // pose that question. Sections 1-14 measure the untouched island, so
        // nothing above moves for this.
        //
        // Both notch configurations run, and the pair IS the corner-seal
        // experiment: the only geometric difference between them is two voxels,
        // so any lighting difference is attributable to that corner and nothing
        // else.
        for notch in [NotchState::Sealed, NotchState::Open] {
            let corridor = RainbowCorridor::new(notch);
            let corridor_brickmap = light_corridor_world(&brickmap, corridor);
            let corridor_bindings = WorldBindings::new(&device, &corridor_brickmap);
            run_section(
                &device,
                &queue,
                &corridor_bindings,
                &corridor_brickmap,
                light_corridor_section(corridor),
            );
        }
    }

    // Last word, so it is the thing still on screen when a run ends and cannot be
    // lost above a thousand lines of tables.
    let gpu_errors = GPU_ERRORS.load(Ordering::Relaxed);
    if gpu_errors > 0 {
        println!();
        println!(
            "!! {gpu_errors} GPU VALIDATION ERROR(S) DURING THIS RUN — do not record these \
             numbers without reading stderr first. A dropped dispatch times as fast, not as \
             broken."
        );
    }
}

/// Section 13 — what each MATERIAL costs, measured on a wall of nothing else.
///
/// **Why this exists, and why the preset table could not answer it.** Sections 1-9
/// measure the generated island, and the island is made of five materials: grass,
/// dirt, sediment, stone and water. The compiled table authors pattern layers on
/// `lava` and `slate tile` — NEITHER of which the island contains. So a default run
/// prices pattern layers that nothing on screen carries and reports them as free,
/// which is true and useless.
///
/// This section removes the world from the question. One studio wall, filled with a
/// single material, camera square on to a lit face — so the measured number is that
/// material's cost per pixel at full coverage, which is the worst case a surface can
/// present and the one worth budgeting against.
///
/// Each material is timed twice on the SAME scene and the same pipeline: once with
/// its row exactly as authored, once with that row's pattern stack emptied. The
/// difference is the layer stack's cost, isolated from the base shading, the
/// geometry and the lighting — all of which are identical between the two columns
/// because only the uploaded table changes.
fn report_material_costs(device: &wgpu::Device, queue: &wgpu::Queue, use_project: bool) {
    use std::f32::consts::FRAC_PI_2;
    use voxel_core::world::Voxel;
    use voxel_material::material::{material_voxel, MATERIAL_COUNT};
    use voxel_material::pattern::PatternStack;
    use voxel_rt::studio::{orbit_pose, StudioPose, StudioScene};

    println!();
    println!("== section 13: per-material cost (studio wall, one material, full coverage) ==");

    let mut table = MaterialTable::default();
    if use_project {
        let project_path = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../studio-project");
        let store = StudioProjectStore::new(&project_path);
        let mut project_quality = RenderQuality::default();
        let (project, _warnings) =
            StudioProject::load_live_state(&store, &mut table, &mut project_quality)
                .expect("the checked-in project");
        // See the `--project` flag: the pattern layers arrive with the GRAPHS, not
        // with the rows.
        let _ = MaterialGraphAssetService::load_shader_set_for_editing(
            &project_path,
            &project,
            &mut table,
        );
    }
    println!(
        "  table: {}",
        if use_project {
            "the checked-in project"
        } else {
            "compiled defaults"
        }
    );

    let authored = table.gpu_rows();
    let variant = Variant::new("material".to_string(), RenderQuality::default());
    let (width, height) = variant.resolution();
    let target = create_render_target(device, width, height);
    println!("  per-dispatch median ms at {width}x{height}");

    let mut rows: Vec<(String, usize, f32, f32)> = Vec::new();
    for id in 0..MATERIAL_COUNT {
        let material = id as u8;
        let voxel = material_voxel(material);
        if voxel == Voxel::Air {
            continue;
        }
        let layer_count = table
            .rows()
            .get(id)
            .map(|row| row.patterns.layers.iter().flatten().count())
            .unwrap_or(0);

        let scene = StudioScene {
            sample: voxel,
            pose: StudioPose::Wall,
            plate: None,
            subject: None,
        };
        let brickmap = scene.build();
        let bindings = WorldBindings::new(device, &brickmap);
        // Same framing as the section 12 swatches: the +Z face, which is the LIT
        // one under the default sun. A shadowed wall measures shadow, not material.
        let scenario = Scenario {
            label: "studio wall",
            pose: orbit_pose(&scene, -FRAC_PI_2, -0.05, 5.0),
            sun: SunSettings::default(),
            capture_image: false,
        };
        let mut resources =
            VariantResources::new(device, &bindings, &brickmap, &variant, &target.view);

        let mut stripped = table.clone();
        if let Some(row) = stripped.row_mut(material) {
            row.patterns = PatternStack::of(&[]);
        }
        let tables = [authored.clone(), stripped.gpu_rows()];

        bindings.write_material_table(queue, &tables[0]);
        resources.flood_to_convergence(device, queue, &bindings, &variant, &scenario);
        for table_variant in &tables {
            bindings.write_material_table(queue, table_variant);
            for _ in 0..WARMUP_BATCHES {
                time_one_batch(
                    device,
                    queue,
                    &bindings,
                    &resources,
                    &variant,
                    &scenario,
                    &scenario.lighting_uniform(&variant.quality),
                );
            }
        }

        let mut samples: Vec<Vec<f32>> = vec![Vec::with_capacity(BATCH_COUNT); tables.len()];
        for round in 0..BATCH_COUNT {
            for offset in 0..tables.len() {
                let column = (round + offset) % tables.len();
                bindings.write_material_table(queue, &tables[column]);
                samples[column].push(time_one_batch(
                    device,
                    queue,
                    &bindings,
                    &resources,
                    &variant,
                    &scenario,
                    &scenario.lighting_uniform(&variant.quality),
                ));
            }
        }
        let (authored_median, _) = summarize(&mut samples[0]);
        let (stripped_median, _) = summarize(&mut samples[1]);
        let name = table
            .rows()
            .get(id)
            .map(|row| row.name.to_string())
            .unwrap_or_default();
        rows.push((name, layer_count, authored_median, stripped_median));
    }

    rows.sort_by(|a, b| {
        (b.2 - b.3)
            .partial_cmp(&(a.2 - a.3))
            .expect("NaN timing sample")
    });
    println!(
        "  {:<18} {:>6} {:>10} {:>10} {:>10}",
        "material", "layers", "authored", "no-pattern", "delta"
    );
    for (name, layers, authored_median, stripped_median) in &rows {
        println!(
            "  {name:<18} {layers:>6} {authored_median:>10.3} {stripped_median:>10.3} {:>+10.3}",
            authored_median - stripped_median
        );
    }
}

/// Section 14 — what the OUTPUT PATH costs: the six tonemap curves, priced
/// against each other on the shipped quality.
///
/// The curve is a RUNTIME uniform (`lighting.output_params.y`), and that is what
/// makes this section both cheap and exact. Every column runs the same shader, the
/// same pipeline object and the same converged light volume; the only thing that
/// differs between two timings is four bytes in a buffer. No rebuild, so no
/// pipeline-cache, shader-residency or brickmap effect can leak into the delta the
/// way it can in a section whose columns are separate builds.
///
/// **Two headrooms, because two curves change SHAPE and not just constants at the
/// SDR boundary.** GT7 switches to its `peakTarget = 2.5` path at headroom <= 1.0
/// and pays a correction multiply for it; BT.2390's knee collapses when the display
/// peak meets the content peak. A single-headroom table would price the HDR curves
/// on a display that cannot show them, which is the one number nobody needs.
///
/// **Two scenarios, because the tonemap is the only per-pixel term here that does
/// not scale with scene complexity.** It runs once per pixel at the end of shading,
/// so its absolute cost should be identical on the aerial and ground shots while
/// the frame around it is not — and if the two deltas disagree, the measurement is
/// picking up something other than the curve and should not be recorded.
///
/// Columns are INTERLEAVED (`(round + offset) % columns`), as sections 1-5 and 13
/// do it: over a 26-second run the GPU clock ramps and the die warms, so a column
/// measured last would otherwise be charged for the ones before it.
fn report_tonemap_costs(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world_bindings: &WorldBindings,
    brickmap: &Brickmap,
) {
    use voxel_color::TonemapCurve;
    use voxel_rt::lighting::OutputParams;

    println!();
    println!("== section 14: output path — tonemap curve cost ==");

    let variant = Variant::new("tonemap".to_string(), RenderQuality::default());
    let (width, height) = variant.resolution();
    let target = create_render_target(device, width, height);
    let megapixels = (width as f64 * height as f64) / 1.0e6;

    // SDR is what every display gives us until one reports otherwise (the unmeasured
    // fallback is 1.0, deliberately); 4.0 is a real EDR headroom on the dev machine's
    // XDR panel and reproduces the value this engine used to hard-code.
    let headrooms = [1.0f32, 4.0];
    let columns: Vec<(String, OutputParams)> = headrooms
        .iter()
        .flat_map(|&hdr_headroom| {
            TonemapCurve::ALL.into_iter().map(move |tonemap| {
                (
                    format!("{}@{:.0}x", tonemap.label(), hdr_headroom),
                    OutputParams {
                        hdr_headroom,
                        tonemap,
                        ..OutputParams::default()
                    },
                )
            })
        })
        .collect();

    // Every scenario in this section shares one `VariantResources` — one pipeline,
    // one light volume — so the volume is flooded once per scenario's sun and then
    // reused across all twelve columns. A curve cannot change what the CA pass
    // computes: the tonemap is applied after shading, in the DDA pass alone.
    let mut resources =
        VariantResources::new(device, world_bindings, brickmap, &variant, &target.view);

    println!("  per-dispatch median ms at {width}x{height} ({megapixels:.1} Mpx), exposure 1.0");

    for scenario in build_scenarios(&[]).into_iter().filter(|scenario| {
        // A (aerial) and C (ground): the two default-sun shots. The low-sun pair adds
        // shadow-ray cost that the tonemap has nothing to do with.
        scenario.label.starts_with('A') || scenario.label.starts_with('C')
    }) {
        resources.flood_to_convergence(device, queue, world_bindings, &variant, &scenario);

        let uniforms: Vec<LightingUniform> = columns
            .iter()
            .map(|(_, output_params)| {
                scenario
                    .lighting_uniform(&variant.quality)
                    .with_output_params(*output_params)
            })
            .collect();

        for uniform in &uniforms {
            for _ in 0..WARMUP_BATCHES {
                time_one_batch(
                    device,
                    queue,
                    world_bindings,
                    &resources,
                    &variant,
                    &scenario,
                    uniform,
                );
            }
        }

        let mut samples: Vec<Vec<f32>> = vec![Vec::with_capacity(BATCH_COUNT); columns.len()];
        for round in 0..BATCH_COUNT {
            for offset in 0..columns.len() {
                let column = (round + offset) % columns.len();
                samples[column].push(time_one_batch(
                    device,
                    queue,
                    world_bindings,
                    &resources,
                    &variant,
                    &scenario,
                    &uniforms[column],
                ));
            }
        }

        let medians: Vec<(f32, f32)> = samples.iter_mut().map(|s| summarize(s)).collect();
        // Reinhard at headroom 1.0 is the baseline: the shipped SDR curve, one divide,
        // and the cheapest thing the branch can do. Every delta is against it.
        let baseline = medians[0].0;
        println!();
        println!("  scenario: {}", scenario.label);
        println!(
            "  {:<20} {:>9} {:>9} {:>10} {:>9} {:>11}",
            "curve@headroom", "median", "p95", "vs base", "% frame", "ns/Mpx"
        );
        for ((label, _), (median, p95)) in columns.iter().zip(medians.iter()) {
            let delta = median - baseline;
            println!(
                "  {label:<20} {median:>9.3} {p95:>9.3} {delta:>+10.3} {:>8.1}% {:>11.0}",
                100.0 * delta / baseline,
                (delta as f64 * 1.0e6) / megapixels,
            );
        }
    }

    report_tonemap_residency(device, queue, world_bindings, brickmap, &target);
}

/// The question the curve table CANNOT answer: what do the five curves nobody
/// selected cost, purely by being resident in the kernel?
///
/// Every column above runs the same shader, which is what made the comparison
/// exact — and is also exactly why none of them can price the arc itself. A
/// dispatch branch is a few instructions, but GT7's two ICtCp matrices and
/// BT.2390's PQ constants are live values in the same function, and register
/// pressure is decided for the whole kernel by its worst path. If that pushed
/// occupancy down a step, every column above would be slow together and the table
/// would show a flat, innocent-looking zero.
///
/// So this is a compile-time A/B, in the shape of `fade_range_as_shader_consts_variant`:
/// the shipped six-curve source against one with `apply_tonemap` collapsed to its
/// Reinhard return, which makes the other five unreachable and lets the Metal
/// compiler drop them before register allocation. That second variant is what the
/// output path looked like before this arc, so the delta IS the arc's resident cost.
fn report_tonemap_residency(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world_bindings: &WorldBindings,
    brickmap: &Brickmap,
    target: &RenderTarget,
) {
    use voxel_rt::lighting::OutputParams;

    // Byte-exact against `shaders/dda.wgsl`. If the dispatch is ever reshaped this
    // assert fires rather than the row silently measuring the same shader twice —
    // which would read as "the arc is free" and be the most expensive kind of wrong.
    let six_curve_dispatch = "\
fn apply_tonemap(color: vec3<f32>, headroom: f32, curve: u32, content_peak: f32) -> vec3<f32> {
    if (curve == TONEMAP_GT7) {";
    let one_curve_dispatch = "\
fn apply_tonemap(color: vec3<f32>, headroom: f32, curve: u32, content_peak: f32) -> vec3<f32> {
    return tonemap_reinhard(color);
}
fn apply_tonemap_unreachable(color: vec3<f32>, headroom: f32, curve: u32, content_peak: f32) -> vec3<f32> {
    if (curve == TONEMAP_GT7) {";

    let quality = RenderQuality::default();
    let shipped_source = build_shader_source(&quality);
    assert!(
        shipped_source.contains(six_curve_dispatch),
        "`apply_tonemap`'s dispatch no longer matches the residency A/B's anchor — \
         update or drop this row"
    );
    let variants = [
        Variant::new("six-curve (shipped)".to_string(), quality),
        Variant {
            label: "one-curve (pre-arc)".to_string(),
            quality,
            edits: vec![(
                "tonemap.wgsl",
                six_curve_dispatch.to_string(),
                one_curve_dispatch.to_string(),
            )],
        },
    ];

    println!();
    println!("  resident cost of the five unselected curves (both columns RUN Reinhard):");

    for scenario in build_scenarios(&[])
        .into_iter()
        .filter(|scenario| scenario.label.starts_with('A') || scenario.label.starts_with('C'))
    {
        let mut resources: Vec<VariantResources> = variants
            .iter()
            .map(|variant| {
                VariantResources::new(device, world_bindings, brickmap, variant, &target.view)
            })
            .collect();
        for (variant, resource) in variants.iter().zip(resources.iter_mut()) {
            resource.flood_to_convergence(device, queue, world_bindings, variant, &scenario);
        }
        // Reinhard at headroom 1.0 in BOTH columns: the collapsed variant can render
        // nothing else, so the shipped one must be pinned to the same curve or the row
        // would be measuring a curve change wearing a residency label.
        let uniform = scenario
            .lighting_uniform(&quality)
            .with_output_params(OutputParams::default());

        let mut samples: Vec<Vec<f32>> = vec![Vec::with_capacity(BATCH_COUNT); variants.len()];
        for (index, variant) in variants.iter().enumerate() {
            for _ in 0..WARMUP_BATCHES {
                time_one_batch(
                    device,
                    queue,
                    world_bindings,
                    &resources[index],
                    variant,
                    &scenario,
                    &uniform,
                );
            }
        }
        for round in 0..BATCH_COUNT {
            for offset in 0..variants.len() {
                let index = (round + offset) % variants.len();
                samples[index].push(time_one_batch(
                    device,
                    queue,
                    world_bindings,
                    &resources[index],
                    &variants[index],
                    &scenario,
                    &uniform,
                ));
            }
        }
        let (six_median, _) = summarize(&mut samples[0]);
        let (one_median, _) = summarize(&mut samples[1]);
        println!(
            "  {:<28} six-curve {six_median:>7.3}   one-curve {one_median:>7.3}   \
             resident {:>+7.3} ms ({:>+5.1}%)",
            scenario.label,
            six_median - one_median,
            100.0 * (six_median - one_median) / one_median,
        );
    }
}

/// Section 12 — what each generator LOOKS like on a real material, as PNGs.
///
/// Section 11 prices the generators; this one shows them, and the two answer
/// different questions. A number tells you simplex is 39% cheaper than value noise;
/// only a picture tells you whether the surface it produces is the one you wanted.
/// Switching a material's generator is a re-authoring decision, not a free speedup,
/// because a different generator is a DIFFERENT PATTERN and not a cheaper rendering
/// of the same one.
///
/// One 4 x 4 m studio wall per (material, generator), framed close enough that the
/// texel grid is visible — a landscape shot averages the pattern away at exactly the
/// scale the choice is about. Everything except the generator is pinned: same wall,
/// same camera, same sun, same period, same texel count, same amount.
fn write_generator_swatches(device: &wgpu::Device, queue: &wgpu::Queue) {
    use voxel_core::world::Voxel;
    use voxel_rt::studio::{orbit_pose, StudioPose, StudioScene};

    println!();
    println!("== section 12: generator swatches (studio wall, 4 x 4 m) ==");

    let output_directory = std::path::Path::new("target/bench_dda/swatches");
    std::fs::create_dir_all(output_directory).expect("failed to create the swatch directory");

    let variant = Variant::new("swatch".to_string(), RenderQuality::default());
    let (width, height) = variant.resolution();
    let target = create_render_target(device, width, height);

    // The materials worth looking at, and why each: lava is the one row that already
    // authors a layer, stone is the archetypal patterned surface, grass is the row
    // that authors face roles so the pattern has to compose with them, and sand is
    // the fine-grained case where a coarse generator will read as blotches.
    let subjects = [
        ("lava", Voxel::Lava),
        ("stone", Voxel::Stone),
        ("grass", Voxel::Grass),
        ("sand", Voxel::Sand),
        ("slate", Voxel::SlateTile),
    ];

    for (material_name, sample) in subjects {
        let scene = StudioScene {
            sample,
            pose: StudioPose::Wall,
            plate: None,
            subject: None,
        };
        let brickmap = scene.build();
        let bindings = WorldBindings::new(device, &brickmap);
        // NEGATIVE quarter turn, so the camera sees the +Z face. The default sun
        // points along normalize(0.55, 0.8, 0.35), so the -Z face this pose first
        // used had `dot(normal, sun) = -0.35` — fully shadowed, and every swatch
        // came out a flat dark rectangle with the pattern technically present and
        // visually absent. A swatch has to be LIT to be a swatch.
        let scenario = Scenario {
            label: "studio wall",
            pose: orbit_pose(&scene, -std::f32::consts::FRAC_PI_2, -0.05, 5.0),
            sun: SunSettings::default(),
            capture_image: true,
        };
        let mut resources =
            VariantResources::new(device, &bindings, &brickmap, &variant, &target.view);

        // The row EXACTLY as the table authors it, before any swatch override. For
        // most subjects that is the flat base colour and a useful reference; for
        // `slate` it is the four-layer tessellated stack, which is the only way to
        // see the material this section was extended for — every other image here
        // replaces the row's patterns with a single generator.
        bindings.write_material_table(queue, &gpu_materials_for_swatches());
        resources.flood_to_convergence(device, queue, &bindings, &variant, &scenario);
        render_once(device, queue, &bindings, &resources, &variant, &scenario);
        let authored = read_back_image(device, queue, &target);
        write_png(
            &output_directory.join(format!("{material_name}_AUTHORED.png")),
            &authored,
            width,
            height,
        );
        write_downsampled_crop(
            &output_directory.join(format!("thumb_{material_name}_AUTHORED.png")),
            &authored,
            width,
            height,
        );

        for generator in PatternGenerator::ALL {
            for warp in [0.0f32, 0.6] {
                let rows = swatch_rows(sample, generator, warp);
                bindings.write_material_table(queue, &rows);
                resources.flood_to_convergence(device, queue, &bindings, &variant, &scenario);
                render_once(device, queue, &bindings, &resources, &variant, &scenario);
                let image = read_back_image(device, queue, &target);
                let generator_name = generator
                    .label()
                    .split('(')
                    .next()
                    .unwrap_or("generator")
                    .trim()
                    .replace(' ', "-");
                let suffix = if warp > 0.0 { "-warped" } else { "" };
                write_png(
                    &output_directory.join(format!("{material_name}_{generator_name}{suffix}.png")),
                    &image,
                    width,
                    height,
                );
                write_downsampled_crop(
                    &output_directory.join(format!(
                        "thumb_{material_name}_{generator_name}{suffix}.png"
                    )),
                    &image,
                    width,
                    height,
                );
            }
        }
        println!(
            "  {material_name}: {} generators x warp on/off",
            PatternGenerator::ALL.len()
        );
    }
    println!("  written to {}", output_directory.display());
}

/// The compiled table, untouched — what the renderer actually ships.
fn gpu_materials_for_swatches() -> Vec<GpuMaterial> {
    MATERIALS.iter().map(|row| row.to_gpu()).collect()
}

/// One material row carrying one generator, everything else pinned — the swatch
/// table. Only the named row is patterned, so the surrounding plate and any other
/// visible material stay flat and the eye has an unpatterned reference in frame.
fn swatch_rows(
    sample: voxel_core::world::Voxel,
    generator: PatternGenerator,
    domain_warp: f32,
) -> Vec<GpuMaterial> {
    let target_row = voxel_material::material::material_id(sample) as usize;
    let layer = PatternLayer {
        generator,
        // TILE frame with a 2:1 running bond, so the two tile generators have a wall
        // to divide and every other generator shows what per-tile isolation does to
        // it — which is the comparison these swatches exist for.
        frame: PatternFrame::Tile,
        period_meters: 0.5,
        target: PatternTarget::Albedo,
        // MULTIPLY at full amount, which maps the generator's raw 0..1 straight onto
        // the material: black where the generator reads 0, the row's own albedo
        // where it reads 1. Two earlier attempts used MixToColor and both failed to
        // show anything — 0.85 toward near-black crushed every swatch dark, and 0.7
        // toward light grey barely moved a grey material. Multiply is the only blend
        // whose output spans the generator's ENTIRE range regardless of what the row
        // is coloured, which is what a swatch needs.
        //
        // It also shows each generator's value DISTRIBUTION honestly rather than
        // flattering it: worley F1 rarely exceeds ~0.6 of its range, so its swatch
        // is genuinely darker than perlin's, and that is information rather than a
        // rendering artefact.
        blend: PatternBlend::Multiply,
        amount: 1.0,
        target_color: [1.0, 1.0, 1.0],
        faces: PatternFaces::ALL,
        texels_per_voxel: DEFAULT_TEXELS_PER_VOXEL,
        vary_per_face: true,
        domain_warp,
        tile_aspect: 2.0,
        tile_bond: 0.5,
        tile_gap: 0.06,
        emission_intensity: 1.0,
    };
    MATERIALS
        .iter()
        .enumerate()
        .map(|(index, row)| {
            let mut patterned = *row;
            if index == target_row {
                patterned.patterns = PatternStack::of(&[layer]);
            }
            patterned.to_gpu()
        })
        .collect()
}

/// Section 11 — what each pattern generator costs, one layer deep.
///
/// **Why this is not a variant table.** Every other section sweeps LEVERS, which
/// are shader consts, so each column is a different pipeline and the harness builds
/// one `VariantResources` per column. A generator is not a lever: it is four bits of
/// an uploaded row. One shader, one pipeline, one bind group — and the sweep is a
/// buffer write between batches.
///
/// That makes this the cleanest measurement in the file, because the usual
/// confound is absent. Nothing about the compiled code differs between columns, so
/// a difference cannot be register allocation or a lost fold; it is the generator
/// doing more or less work. The `gen-checker` column is the floor: it produces a
/// full-coverage pattern for two floors and a bit-and, so everything above it is
/// what that generator costs over the cheapest possible one.
///
/// The round-robin rotation is kept from `measure_section` for the reason recorded
/// there — timing columns in sequential blocks showed ~10% drift on identical
/// shaders, which is the same order as the effects being measured.
fn report_generator_costs(device: &wgpu::Device, queue: &wgpu::Queue, brickmap: &Brickmap) {
    println!();
    println!("== section 11: pattern generator costs (one layer, shipped quality) ==");

    let bindings = WorldBindings::new(device, brickmap);
    let variant = Variant::new("generators".to_string(), RenderQuality::default());
    let (width, height) = variant.resolution();
    let target = create_render_target(device, width, height);
    // Ground-level, default sun — scenario C, the one where material detail covers
    // the whole frame, so a generator's cost is not diluted by sky.
    //
    // `build_scenarios` returns ALL FOUR regardless of its argument; the chars only
    // choose which ones get PNG-captured. Indexing is therefore the selection, and
    // the index is asserted rather than trusted.
    let mut scenarios = build_scenarios(&[]);
    let scenario = scenarios.remove(2);
    assert!(
        scenario.label.starts_with("C ground"),
        "scenario order changed: got `{}`",
        scenario.label
    );

    let mut resources = VariantResources::new(device, &bindings, brickmap, &variant, &target.view);
    let columns = generator_sweep();
    let tables: Vec<Vec<GpuMaterial>> = columns
        .iter()
        .map(|column| {
            single_generator_rows(
                column.generator,
                column.domain_warp,
                column.frame,
                column.vary_per_face,
                column.layers,
            )
        })
        .collect();

    // Flood once: the light volume bakes its own cell attributes and never reads the
    // material table, so no column can move it and re-flooding per column would only
    // add noise. Section 9 measured exactly that — CAGI flat across all seven columns.
    bindings.write_material_table(queue, &tables[0]);
    resources.flood_to_convergence(device, queue, &bindings, &variant, &scenario);

    let mut samples: Vec<Vec<f32>> = vec![Vec::with_capacity(BATCH_COUNT); columns.len()];
    for table in &tables {
        bindings.write_material_table(queue, table);
        for _ in 0..WARMUP_BATCHES {
            time_one_batch(
                device,
                queue,
                &bindings,
                &resources,
                &variant,
                &scenario,
                &scenario.lighting_uniform(&variant.quality),
            );
        }
    }
    for round in 0..BATCH_COUNT {
        for offset in 0..columns.len() {
            let column = (round + offset) % columns.len();
            bindings.write_material_table(queue, &tables[column]);
            samples[column].push(time_one_batch(
                device,
                queue,
                &bindings,
                &resources,
                &variant,
                &scenario,
                &scenario.lighting_uniform(&variant.quality),
            ));
        }
    }

    println!(
        "  scenario: {} — per-dispatch median / p95 ms at {width}x{height}",
        scenario.label
    );
    let mut summarised: Vec<(String, f32, f32)> = columns
        .iter()
        .zip(samples.iter_mut())
        .map(|(column, column_samples)| {
            let (median, p95) = summarize(column_samples);
            (column.label.clone(), median, p95)
        })
        .collect();
    let floor = summarised
        .iter()
        .find(|(label, _, _)| label == "gen-checker")
        .map(|(_, median, _)| *median)
        .unwrap_or(0.0);
    summarised.sort_by(|a, b| a.1.partial_cmp(&b.1).expect("NaN timing sample"));
    for (label, median, p95) in &summarised {
        println!(
            "  {label:<22} median {median:>7.3} ms   p95 {p95:>7.3} ms   over checker {:>+7.3} ms",
            median - floor
        );
    }
}

/// E5c — the numbers behind *"does a small light look like a small light"*, measured
/// headlessly instead of eyeballed from a screenshot.
///
/// **Why this section exists.** Every emitter question in the E5b/E5c arc was answered
/// by hand-rolling a throwaway probe — the 1/16 area weighting, the 45-vs-152
/// diffusion asymmetry, the patterned mean reading 0.0 above a 0.25 m period. Two of
/// those rigs were written and deleted in a single session, and the one bug that
/// actually shipped (`pack_emission` writing five words into a two-word stride) got
/// past 256 unit tests precisely because nothing reads the GPU's own cell data back.
/// So this reads the volume AND the rendered pixels, and prints both.
///
/// The metric that matters for a *sub-cell* emitter is not peak brightness, it is the
/// **half-width**: how far from the block the wall's brightness falls to half. A tight
/// pool and a broad dim wash can carry the same flux, and only the falloff tells them
/// apart — which is exactly the judgement a screenshot is worst at.
///
/// Its own bindings and brickmap: the studio prop, not the island.
fn report_emitter_probes(device: &wgpu::Device, queue: &wgpu::Queue) {
    use voxel_rt::studio::{orbit_pose, StudioPose, StudioScene};

    println!();
    println!("== E5c emitter probes (studio `wall + glow block`) ==");

    let scene = StudioScene {
        pose: StudioPose::EmitterWall,
        ..StudioScene::default()
    };
    let brickmap = scene.build();
    let bindings = WorldBindings::new(device, &brickmap);
    let block = scene.emitter_block_voxel();

    // Yaw a quarter turn, NOT zero: `orbit_pose` looks along
    // `(cos yaw, sin pitch, sin yaw)` and the wall is one voxel thick in z, so yaw 0
    // views it edge-on as a sliver and every measurement below reads sky instead.
    let scenario = Scenario {
        label: "emitter wall",
        pose: orbit_pose(&scene, std::f32::consts::FRAC_PI_2, 0.0, 4.0),
        sun: SunSettings::default(),
        capture_image: false,
    };

    let cell_voxels = CagiSettings::default().cell_voxels as i32;
    let emitter_cell = [
        block[0] / cell_voxels,
        block[1] / cell_voxels,
        block[2] / cell_voxels,
    ];

    println!(
        "  block voxel {:?} -> cell {:?} at {} voxels/cell",
        block, emitter_cell, cell_voxels
    );
    println!();
    println!(
        "  {:<28} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
        "rule / bounce", "pin", "+1", "+2", "+3", "sky", "peak", "half"
    );

    for rule in [
        CagiRule::MaxDecrement,
        CagiRule::Diffusion6,
        CagiRule::Diffusion26,
    ] {
        for emitter_bounce in [true, false] {
            let mut quality = RenderQuality::default();
            quality.global_illumination.rule = rule;
            quality.global_illumination.emitter_bounce = emitter_bounce;
            let variant = Variant::new(format!("{rule:?}-bounce-{emitter_bounce}"), quality);
            let (width, height) = variant.resolution();
            let target = create_render_target(device, width, height);
            let mut resources =
                VariantResources::new(device, &bindings, &brickmap, &variant, &target.view);
            resources.flood_to_convergence(device, queue, &bindings, &variant, &scenario);

            // The volume: the emitter cell's pinned value and the radiance stepping
            // away from the wall along its normal (the wall is one voxel thick in z).
            let volume = read_back_volume(device, queue, &resources.light_volume);
            let grid = resources.light_volume.grid();
            let read = |cell: [i32; 3]| -> u32 {
                if (0..3).any(|axis| cell[axis] < 0 || cell[axis] >= grid.size[axis] as i32) {
                    return 0;
                }
                let index = grid.cell_index([cell[0] as u32, cell[1] as u32, cell[2] as u32]);
                unpack_light(volume[index])[0]
            };
            let along_normal: Vec<u32> = (0..4)
                .map(|step| read([emitter_cell[0], emitter_cell[1], emitter_cell[2] + step]))
                .collect();
            // The AMBIENT level the emitter has to beat, read in open air well clear of
            // the wall at the same height. This is the comparison that decides whether a
            // light reads as a light at all: an emitter dimmer than the sky around it
            // cannot, however correct its transport is.
            let sky_reference = read([emitter_cell[0] + 12, emitter_cell[1], emitter_cell[2] + 12]);

            // And the pixels: peak brightness plus the half-width, in the rendered
            // image the app would actually show.
            render_once(device, queue, &bindings, &resources, &variant, &scenario);
            let image = read_back_image(device, queue, &target);
            let profile = wall_brightness_profile(&image, width, height);
            let (peak, half_width) = peak_and_half_width(&profile);

            println!(
                "  {:<28} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7} {:>7}",
                format!("{rule:?} / {}", if emitter_bounce { "on" } else { "OFF" }),
                along_normal[0],
                along_normal[1],
                along_normal[2],
                along_normal[3],
                sky_reference,
                peak,
                half_width,
            );

            let path = std::path::Path::new("target").join(format!(
                "emitter-probe-{rule:?}-bounce-{emitter_bounce}.png"
            ));
            write_png(&path, &image, width, height);
        }
    }
    println!(
        "  pin/+N/sky are 0..1023 red from the light volume — pin is the emitter cell, +N \
         steps away from the wall along its normal, sky is open air 12 cells clear."
    );
    println!(
        "  peak is the brightest red (0..255) on the rendered wall; half is how many \
         pixels from that peak brightness falls by half — the tight-pool/broad-wash \
         number a screenshot cannot give."
    );
    println!("  PNGs written to target/emitter-probe-*.png");

    // ---- E5c step 2: what should `emissive_scale` actually be? ------------------
    //
    // Not a taste question, a ratio. The emitter has to out-brighten the ambient
    // ALREADY in the cells around it (sky, plus the sun bounce off the plate) or it
    // cannot read as a light no matter how correct its transport is. So sweep the knob
    // and print emitter-over-ambient.
    println!();
    println!("== E5c emissive scale sweep (shipped rule) ==");
    println!(
        "  {:<8} {:>7} {:>9} {:>7} {:>28}",
        "scale", "pin", "ambient", "ratio", "lateral, +1 out (0..5 cells)"
    );
    for scale in [1.0_f32, 2.0, 4.0, 8.0, 16.0] {
        let mut quality = RenderQuality::default();
        quality.global_illumination.emissive_scale = scale;
        let variant = Variant::new(format!("emissive-scale-{scale}"), quality);
        let (width, height) = variant.resolution();
        let target = create_render_target(device, width, height);
        let mut resources =
            VariantResources::new(device, &bindings, &brickmap, &variant, &target.view);
        resources.flood_to_convergence(device, queue, &bindings, &variant, &scenario);

        let volume = read_back_volume(device, queue, &resources.light_volume);
        let grid = resources.light_volume.grid();
        let read = |cell: [i32; 3]| -> u32 {
            if (0..3).any(|axis| cell[axis] < 0 || cell[axis] >= grid.size[axis] as i32) {
                return 0;
            }
            let index = grid.cell_index([cell[0] as u32, cell[1] as u32, cell[2] as u32]);
            unpack_light(volume[index])[0]
        };
        let pin = read(emitter_cell);
        // The ambient in the cell the wall's surface actually samples: one step out
        // along the normal, which is where `cagi_sample_surface` lands.
        let ambient = read([emitter_cell[0] + 6, emitter_cell[1], emitter_cell[2] + 1]);

        // The LATERAL profile: the cells one step out from the wall, walking sideways
        // away from the emitter. Measured in the volume rather than in pixels on
        // purpose — the volume is the ground truth for "where is the light", and a
        // pixel row through this scene is mostly sky and plate, which is how two
        // earlier attempts at this ended up measuring the background instead.
        let lateral: Vec<u32> = (0..6)
            .map(|step| read([emitter_cell[0] + step, emitter_cell[1], emitter_cell[2] + 1]))
            .collect();

        render_once(device, queue, &bindings, &resources, &variant, &scenario);
        let image = read_back_image(device, queue, &target);

        println!(
            "  {:<8} {:>7} {:>9} {:>7.2} {:>28}",
            scale,
            pin,
            ambient,
            pin as f32 / ambient.max(1) as f32,
            format!("{lateral:?}"),
        );
        let path = std::path::Path::new("target").join(format!("emitter-scale-{scale}.png"));
        write_png(&path, &image, width, height);
    }
    println!(
        "  ambient is the same-height cell 6 cells along the wall, one step out — what \
         the wall's own surface sample sees where the emitter does not reach. A light \
         has to beat it, and `ratio` is by how much."
    );
}

/// Peak of a brightness profile and its half-width in pixels: walk out from the peak
/// until brightness has fallen by half.
///
/// The half-width is the point of this section. Flux is conserved by E5b, so a tight
/// bright pool and a broad dim wash can carry the SAME total light and differ only in
/// this number — and "too spread out" is exactly the judgement an eye makes badly.
fn peak_and_half_width(profile: &[u8]) -> (u8, usize) {
    let Some((peak_index, &peak)) = profile.iter().enumerate().max_by_key(|(_, value)| **value)
    else {
        return (0, 0);
    };
    let half = peak / 2;
    let mut width = 0;
    for step in 1..profile.len() {
        let low = profile.get(peak_index.saturating_sub(step)).copied();
        let high = profile.get(peak_index + step).copied();
        if low.is_none_or(|value| value <= half) || high.is_none_or(|value| value <= half) {
            width = step;
            break;
        }
    }
    (peak, width)
}

/// Red-channel brightness along the image's horizontal centre line — the falloff a
/// sub-cell emitter is judged on, sampled where the block sits.
fn wall_brightness_profile(rgba_bytes: &[u8], width: u32, height: u32) -> Vec<u8> {
    let row = height / 2;
    (0..width)
        .map(|column| {
            let offset = ((row * width + column) * 4) as usize;
            rgba_bytes.get(offset).copied().unwrap_or(0)
        })
        .collect()
}

/// The material table with four pattern layers on every row a ray can hit — the
/// worst case the layer model can be asked to shade.
///
/// The four are ordered **cheapest-first**, which is deliberate and is what makes the
/// sweep readable: the cap drops the tail, so `1` measures the cheapest generator and
/// `4` adds the dearest. Reading the four deltas therefore gives a per-generator cost
/// as well as a per-layer slope.
///
/// | slot | generator | period | work per hit |
/// |---|---|---|---|
/// | 1 | `Flat`, voxel frame | 0.125 m | one cell hash, no interpolation |
/// | 2 | `Speckle`, 8 texels | 0.05 m | a texel snap, four cell hashes and a `length` |
/// | 3 | `Noise` x2 octaves, 8 texels | 0.25 m | a texel snap and 16 lattice hashes |
/// | 4 | `Noise` x3 octaves, 8 texels | 0.02 m | **24 lattice hashes** — the dearest thing S2 has |
///
/// Three of the four snap to the 8-texel grid, because that is what almost every real
/// layer will do and the snap is not free (two `floor`s and a multiply-add per layer).
/// Measuring the continuous form would price a configuration nobody ships.
///
/// A sweep over four copies of the cheapest generator would understate the slope,
/// which is the one thing this section must not do. (The original stack was half
/// brick coursing; those generators were cut at the S2 gate, so this was re-authored
/// and section 9 re-recorded rather than left describing code that no longer exists.)
///
/// Air is left alone: it is the miss sentinel and is never shaded.
fn saturated_material_rows() -> Vec<GpuMaterial> {
    let stack = PatternStack::of(&[
        PatternLayer {
            generator: PatternGenerator::Flat,
            frame: PatternFrame::Voxel,
            period_meters: VOXEL_SIZE,
            target: PatternTarget::Albedo,
            blend: PatternBlend::Multiply,
            amount: 0.3,
            target_color: [1.0, 1.0, 1.0],
            faces: PatternFaces::ALL,
            // The voxel frame is one point per voxel, so a snap here would be a no-op.
            texels_per_voxel: 0,
            // Also a no-op outside the face frame, but spelled out rather than
            // defaulted: this stack is the priced configuration, so every field of it
            // should be visible in it.
            vary_per_face: false,
            // The warp is priced by its own bench column, not baked into the
            // saturated stack — that stack has to stay the four-generator slope.
            domain_warp: 0.0,
            tile_aspect: 2.0,
            tile_bond: 0.5,
            tile_gap: 0.06,
            // Albedo target, so the intensity is not read.
            emission_intensity: 1.0,
        },
        PatternLayer {
            generator: PatternGenerator::Speckle { density: 0.3 },
            frame: PatternFrame::World,
            period_meters: 0.05,
            target: PatternTarget::Albedo,
            blend: PatternBlend::Multiply,
            amount: 0.5,
            ..PatternLayer::IDENTITY
        },
        PatternLayer {
            generator: PatternGenerator::Noise { octaves: 2 },
            frame: PatternFrame::World,
            period_meters: 0.25,
            target: PatternTarget::Albedo,
            blend: PatternBlend::MixToColor,
            amount: 0.4,
            target_color: [0.55, 0.52, 0.48],
            faces: PatternFaces::ALL,
            texels_per_voxel: DEFAULT_TEXELS_PER_VOXEL,
            vary_per_face: false,
            domain_warp: 0.0,
            tile_aspect: 2.0,
            tile_bond: 0.5,
            tile_gap: 0.06,
            emission_intensity: 1.0,
        },
        PatternLayer {
            generator: PatternGenerator::Noise { octaves: 3 },
            frame: PatternFrame::World,
            period_meters: 0.02,
            target: PatternTarget::Albedo,
            blend: PatternBlend::Multiply,
            amount: 0.4,
            ..PatternLayer::IDENTITY
        },
    ]);
    MATERIALS
        .iter()
        .map(|row| {
            let mut patterned = *row;
            if !matches!(row.kind, MaterialKind::Air) {
                patterned.patterns = stack;
            }
            patterned.to_gpu()
        })
        .collect()
}

/// One layer of exactly one generator on every visible row — the table section 11
/// sweeps.
///
/// A SINGLE layer, not a stack, because the question this section answers is "what
/// does this generator cost", and section 9 already established that the first
/// layer carries a fixed entry cost the later ones do not. Holding the layer count
/// at one puts that entry cost in every column equally, so the differences between
/// columns are generator differences and nothing else.
///
/// Everything except the generator and the warp is pinned: same world frame, same
/// period, same texel grid, same target, same blend, same amount. A column that
/// moved two things at once would not be a measurement.
fn single_generator_rows(
    generator: PatternGenerator,
    domain_warp: f32,
    frame: PatternFrame,
    vary_per_face: bool,
    layers: usize,
) -> Vec<GpuMaterial> {
    let layer = PatternLayer {
        generator,
        frame,
        period_meters: 0.25,
        target: PatternTarget::Albedo,
        blend: PatternBlend::Multiply,
        amount: 0.6,
        target_color: [1.0, 1.0, 1.0],
        faces: PatternFaces::ALL,
        texels_per_voxel: DEFAULT_TEXELS_PER_VOXEL,
        vary_per_face,
        domain_warp,
        tile_aspect: 2.0,
        tile_bond: 0.5,
        tile_gap: 0.06,
        emission_intensity: 1.0,
    };
    let stack = PatternStack::of(&vec![layer; layers]);
    MATERIALS
        .iter()
        .map(|row| {
            let mut patterned = *row;
            if !matches!(row.kind, MaterialKind::Air) {
                patterned.patterns = stack;
            }
            patterned.to_gpu()
        })
        .collect()
}

/// The section 11 sweep: every generator, then the two orthogonal knobs.
///
/// The generator is TABLE DATA rather than a lever, which is why this cannot be a
/// registry-derived variant table like every other section: the shader is identical
/// across all of these columns and only the uploaded row changes. That is also what
/// makes the sweep cheap — one pipeline, one bind group, a 6.6 KB buffer write
/// between batches.
struct GeneratorColumn {
    label: String,
    generator: PatternGenerator,
    domain_warp: f32,
    frame: PatternFrame,
    vary_per_face: bool,
    layers: usize,
}

fn generator_sweep() -> Vec<GeneratorColumn> {
    let mut columns: Vec<GeneratorColumn> = PatternGenerator::ALL
        .iter()
        .map(|generator| {
            // Everything before the parenthesised description, hyphenated — the
            // first word alone collides, since three generators are called
            // "worley something".
            let name = generator
                .label()
                .split('(')
                .next()
                .unwrap_or("generator")
                .trim()
                .replace(' ', "-");
            GeneratorColumn {
                label: format!("gen-{name}"),
                generator: *generator,
                domain_warp: 0.0,
                frame: PatternFrame::World,
                vary_per_face: false,
                layers: 1,
            }
        })
        .collect();
    let world_noise = |label: &str, generator, domain_warp, layers| GeneratorColumn {
        label: label.to_string(),
        generator,
        domain_warp,
        frame: PatternFrame::World,
        vary_per_face: false,
        layers,
    };
    // The warp priced against its own un-warped column, on the two generators it
    // changes most: a lattice noise and a cellular one.
    columns.push(world_noise(
        "warp-noise",
        PatternGenerator::Noise { octaves: 3 },
        0.5,
        1,
    ));
    columns.push(world_noise("warp-worley", PatternGenerator::Worley, 0.5, 1));

    // The FACE-FRAME columns, and the reason they exist: `pattern_variation_salt`
    // returns zero for every world-frame layer, so every column above folds it away
    // and none of them can price it. It is only live on a face-frame layer with
    // variation on — which is what lava authors.
    //
    // The hash it computes depends on the SAMPLE alone; the layer argument only
    // selects whether to compute it. So a four-layer face-frame stack computes the
    // identical hash four times per hit, which looks like the same redundancy the
    // row copy was.
    //
    // DOCUMENTED NEGATIVE (2026-08-02). It is not. Measured salted against
    // unsalted: 4.158 vs 4.157 at one layer, 6.680 vs 6.678 at four — 0.002 ms for
    // four redundant hashes, i.e. nothing. The row copy was 256 bytes forced into
    // thread-local scratch; this is ~24 ALU ops against a pass doing thousands.
    // Same shape in the source, different order of magnitude, and hoisting it
    // would have meant a structural split between two hand-mirrored files for no
    // gain. These columns stay so the negative keeps its evidence.
    for (label, vary) in [("face-salted", true), ("face-unsalted", false)] {
        for layers in [1usize, 4] {
            columns.push(GeneratorColumn {
                label: format!("{label}-{layers}L"),
                generator: PatternGenerator::Noise { octaves: 3 },
                domain_warp: 0.0,
                frame: PatternFrame::Face,
                vary_per_face: vary,
                layers,
            });
        }
    }
    columns
}

/// Section 9: the material model. Baseline = the shipped configuration, which reads
/// neither face roles nor pattern layers; the registry's columns switch each on and
/// then walk the per-hit layer cap 0 / 1 / 2 / 4 over the saturated table.
///
/// `material-flat` is the anchor: face roles off, patterns off. Everything this
/// section costs is measured against it, and the pixel compare against it is how
/// much of the frame the layer model actually changes — which for a grain at 2 cm
/// should be nearly all of it up close and almost none of it at range, since the
/// fade is derived from the period.
fn materials_section() -> Section {
    let baseline = materials_off(RenderQuality::default());
    let mut variants = vec![Variant::new("material-flat".to_string(), baseline)];
    variants.extend(registry_variants(BenchSection::Materials, &baseline));

    Section {
        heading: "section 9: S1 face roles + S2 pattern layers",
        scenarios: build_scenarios(&['a', 'b', 'c', 'd']),
        variants,
        reference_label: "material-flat",
        compare_heading: "material coverage (differing pixels vs material-flat — how much of the \
                          frame the face roles and layer stack change)",
        crop_regions: &[],
    }
}

/// Where section 8 carves its pool: exactly where the app's `P` key would put it
/// from the ground-level scenario pose (10 m ahead of an eye at the spawn looking
/// across the island), so the bench's water is the water Pascal gates in-app.
fn section_eight_pool() -> WaterPool {
    let ground_pose = CameraPose::from_yaw_pitch(
        Vec3::new(
            WORLD_SIZE_X as f32 * VOXEL_SIZE * 0.5,
            WATER_LEVEL as f32 * VOXEL_SIZE + 1.7,
            WORLD_SIZE_Z as f32 * VOXEL_SIZE * 0.86,
        ),
        -std::f32::consts::FRAC_PI_2,
        0.0,
    );
    WaterPool::in_front_of(ground_pose.position, ground_pose.forward)
}

/// Section 6's variants: the shipped edit pipeline plus every registry bench point
/// of [`BenchSection::EditStorm`] — the same derivation the shader sections use, so
/// adding an E2 lever adds a storm variant forever after.
fn edit_storm_runs() -> Vec<(String, RenderQuality)> {
    let shipped = RenderQuality::default();
    let mut runs = vec![("edit-shipped".to_string(), shipped)];
    runs.extend(bench_points_of(BenchSection::EditStorm).map(|point| {
        let mut quality = shipped;
        for (lever_id, value) in point.overrides {
            lever_id.apply(&mut quality, *value);
        }
        (point.label.to_string(), quality)
    }));
    runs
}

fn run_section(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world_bindings: &WorldBindings,
    brickmap: &Brickmap,
    section: Section,
) {
    println!();
    println!("== {} ==", section.heading);
    let (table, footprints) = measure_section(device, queue, world_bindings, brickmap, &section);
    print_table(&section, &table);
    print_memory_table(
        &section,
        &footprints,
        world_bindings.gpu_bytes(),
        brickmap.cpu_bytes() as u64,
    );
}

// ---- Sections ----------------------------------------------------------------

/// Section 1: traversal levers around the shipped defaults, ALL with AO forced
/// off so the table stays comparable with the recorded pre-E1 baseline. The
/// per-lever columns come from the registry; the anchors are `current` (the
/// shipped shader with AO off) and `stage2-baseline` (every traversal aid off),
/// which doubles as the pixel-compare reference.
fn traversal_section() -> Section {
    let baseline = water_off(gi_off(ao_off(RenderQuality::default())));
    let mut variants = vec![Variant::new("current".to_string(), baseline)];
    variants.extend(registry_variants(BenchSection::Traversal, &baseline));

    let mut stage2_baseline = baseline;
    stage2_baseline.traversal.global_max_terminate = false;
    stage2_baseline.traversal.distance_skip = false;
    variants.push(Variant::new("stage2-baseline".to_string(), stage2_baseline));

    Section {
        heading: "section 1: traversal levers (AO off)",
        scenarios: build_scenarios(&['b', 'd']),
        variants,
        reference_label: "stage2-baseline",
        compare_heading: "shadow correctness",
        crop_regions: &[],
    }
}

/// Section 2: E1's ray-traced-AO ladder. Baseline = the GRID CENTER (2 rays,
/// 16 voxels, cosine-weighted, distance falloff); the registry's one-factor
/// columns vary a single knob around it. Anchors: the center itself and the
/// cheap-combo interaction the one-factor grid misses (fewest rays x shortest
/// distance).
fn ray_traced_ao_section() -> Section {
    let center = water_off(gi_off(RenderQuality {
        ambient_occlusion: AoSettings {
            mode: AoMode::RayTraced,
            max_distance_voxels: 16,
            ..AoSettings::default()
        },
        ..RenderQuality::default()
    }));
    let mut variants = registry_variants(BenchSection::RayTracedAo, &center);
    variants.push(Variant::new("ao-2ray-d16".to_string(), center));

    let mut one_ray_short = center;
    one_ray_short.ambient_occlusion.ray_count = 1;
    one_ray_short.ambient_occlusion.max_distance_voxels = 8;
    variants.push(Variant::new("ao-1ray-d8".to_string(), one_ray_short));

    Section {
        heading: "section 2: E1 ray-traced AO variants",
        scenarios: build_scenarios(&['a', 'c']),
        variants,
        reference_label: "ao-off",
        compare_heading: "AO coverage (differing pixels vs ao-off — larger = more of the \
                          frame touched)",
        crop_regions: &[],
    }
}

/// Section 3: E1b's cheap-occlusion / soft-shadow shootout. Baseline = E1's
/// shipped default (2 rays / 8 voxels / cosine / falloff), which is also the
/// row every cheap contender is judged against. Anchors: that baseline, and
/// E1c's const-vs-uniform A/B for the fade distances.
fn cheap_occlusion_section() -> Section {
    let e1_default = water_off(gi_off(RenderQuality {
        ambient_occlusion: AoSettings {
            mode: AoMode::RayTraced,
            ..AoSettings::default()
        },
        ..RenderQuality::default()
    }));
    let mut variants = registry_variants(BenchSection::CheapOcclusion, &e1_default);
    variants.push(Variant::new("ao-2ray-d8".to_string(), e1_default));
    variants.push(fade_range_as_shader_consts_variant(&e1_default));

    Section {
        heading: "section 3: E1b cheap occlusion + soft shadows",
        scenarios: build_scenarios(&['a', 'b', 'c', 'd']),
        variants,
        reference_label: "ao-off",
        compare_heading: "E1b coverage (differing pixels vs ao-off/hard — AO rows = darkening \
                          reach, soft-shadow rows = penumbra reach)",
        crop_regions: &[],
    }
}

/// Section 4: the quality presets, each at its own render scale — the headline
/// table. Balanced (full scale, the shipped configuration) is the compare
/// reference; the lower-scale tiers render at a different pixel count, so only
/// Beautiful can be pixel-compared against it (RT-AO's extra reach over corner
/// AO).
fn preset_section() -> Section {
    let variants = QUALITY_PRESETS
        .iter()
        .filter(|spec| spec.preset != QualityPreset::Custom)
        .map(|spec| {
            let quality = spec.resolve();
            let (width, height) = (
                ((OUTPUT_WIDTH as f32 * quality.render_scale) as u32).max(1),
                ((OUTPUT_HEIGHT as f32 * quality.render_scale) as u32).max(1),
            );
            Variant::new(format!("{} @{width}x{height}", spec.label), quality)
        })
        .collect();

    Section {
        heading: "section 4: E1c quality presets (each at its own render scale)",
        scenarios: build_scenarios(&['a', 'b', 'c', 'd']),
        variants,
        reference_label: "Balanced @2560x1440",
        compare_heading: "preset coverage (differing pixels vs Balanced; skipped for tiers \
                          that render at another resolution)",
        crop_regions: &[],
    }
}

/// Section 5: E4's CAGI contenders. Baseline = the shipped configuration (0.5 m
/// cells, 6-neighbour diffusion, trilinear sampling, column-max sky test, pinned
/// sun sources, 2 iterations per frame); the registry's columns vary one lever
/// around it. The `gi-off` row is the anchor every cost is measured against, and
/// the pixel compare against it is the frame coverage of the whole experiment.
fn cagi_section() -> Section {
    let shipped = RenderQuality::default();
    let mut variants = vec![Variant::new("gi-shipped".to_string(), shipped)];
    variants.extend(registry_variants(BenchSection::Cagi, &shipped));

    Section {
        heading: "section 5: E4 CAGI light volume",
        scenarios: build_scenarios(&['a', 'b', 'c', 'd']),
        variants,
        reference_label: "gi-off",
        compare_heading: "CAGI coverage (differing pixels vs gi-off — how much of the frame the \
                          light volume changes)",
        crop_regions: CAGI_CROP_REGIONS,
    }
}

/// Section 8: E6's water optics. Baseline = the shipped configuration (Fresnel
/// reflection + refraction at one interface); the registry's columns are the cost
/// ladder (opaque / zero-ray tint / reflection only / refraction only) plus the
/// second bounce. `water-off` is the anchor every cost is measured against, and
/// the pixel compare against it is the frame coverage of the whole experiment.
///
/// It gets its own brickmap and its own scenarios (see [`water_section_world`] and
/// [`water_scenarios`]) because the island's natural water is 0.6-1.75 m deep —
/// too shallow for extinction or for an underwater camera to exist at all.
fn water_section(pool: WaterPool) -> Section {
    let shipped = RenderQuality::default();
    let mut variants = vec![Variant::new("water-full".to_string(), shipped)];
    variants.extend(registry_variants(BenchSection::Water, &shipped));

    Section {
        heading: "section 8: E6 water reflection, refraction and the underwater view",
        scenarios: water_scenarios(pool),
        variants,
        reference_label: "water-off",
        compare_heading: "water coverage (differing pixels vs water-off — how much of the frame \
                          the water model changes)",
        crop_regions: WATER_CROP_REGIONS,
    }
}

/// Section 8's world: the seed-1 island with one block-aligned water basin.
///
/// Why a separate brickmap rather than the shared one: the island's own water is
/// 0.6-1.75 m deep, so refraction has almost nothing to travel through and an
/// underwater camera has nowhere to stand. The pool is 8 m across and 5 m deep,
/// which is the depth E2b built it for and the depth extinction becomes legible
/// at. It is a *carve*, not a generation change, so the plan's
/// baseline-versioning rule is satisfied without re-recording anything: sections
/// 1-7 still measure the untouched island.
fn water_section_world(brickmap: &Brickmap, pool: WaterPool) -> Brickmap {
    let mut pooled = brickmap.clone();
    let detail = DETAIL_CELLS_PER_WORLD_VOXEL as i32;
    let centre_x = pool.centre_voxel_x.div_euclid(detail);
    let centre_z = pool.centre_voxel_z.div_euclid(detail);
    let clearance = ClearanceUpdate::LocalBox { radius_cells: 8 };
    let mut blocks_written = 0_usize;
    for dz in -4_i32..=4 {
        for dx in -4_i32..=4 {
            let radius = dx.abs().max(dz.abs());
            let bed_y = match radius {
                4 => 9,
                3 => 8,
                2 => 7,
                _ => 5,
            };
            let x = centre_x + dx;
            let z = centre_z + dz;
            pooled.set_world_voxel(WorldVoxelCoord::new(x, bed_y, z), Voxel::Stone, clearance);
            blocks_written += 1;
            for y in (bed_y + 1)..voxel_core::world::WORLD_VOXELS_Y as i32 {
                pooled.set_world_voxel(WorldVoxelCoord::new(x, y, z), Voxel::Air, clearance);
            }
            for y in (bed_y + 1)..=10 {
                pooled.set_world_voxel(WorldVoxelCoord::new(x, y, z), Voxel::Water, clearance);
                blocks_written += 1;
            }
            pooled.set_world_voxel(WorldVoxelCoord::new(x, 11, z), Voxel::Air, clearance);
        }
    }
    println!();
    println!(
        "== section 8 world: island + one {} at voxel ({}, {}) ==",
        pool.label(),
        pool.centre_voxel_x,
        pool.centre_voxel_z,
    );
    println!(
        "  {} one-metre blocks written, water surface at {:.2} m, {:.0} m deep",
        blocks_written,
        pool.surface_centre().y,
        POOL_DEPTH_METERS,
    );
    pooled
}

/// Section 8's four poses. Two look AT water from the air (one grazing, one
/// steep — the two ends of the Fresnel curve), two look at it from INSIDE.
///
/// - `E` from the shore, eye 1.7 m above the waterline, pitched slightly down at
///   the pool 10 m ahead: the ray meets the surface at ~10 deg off grazing, where
///   Fresnel is ~0.4 and the mirror term dominates. This is the "mirror at
///   grazing angles" half of the gate.
/// - `F` the top-down pose over the island centre, which meets the natural lakes
///   almost head-on: Fresnel ~0.02, so it is the "see-through when steep" half,
///   and it is the scenario with the most water pixels in frame.
/// - `G` INSIDE the pool, 2 m under the surface, looking straight up — Snell's
///   window: the sky compressed into a 48.6-degree cone with a mirror around it.
/// - `H` the same eye looking horizontally: pure extinction, the depth cue the
///   whole experiment exists for.
/// - `I` the same eye looking up at **45 degrees** — added after the E6 look gate
///   failed. G and H are the two poses that structurally CANNOT show the region
///   outside Snell's window: at a 68-degree vertical FOV the frame reaches only
///   34 degrees off-axis vertically (50 horizontally, 54 into the corners) while
///   the critical angle is 48.6, so looking straight up puts almost the whole
///   frame INSIDE the window and looking sideways puts all of it outside. Neither
///   shows the rim. At 45 degrees of pitch the rim crosses the middle of the
///   frame, so the cone, its edge and the mirrored world beyond it are all in one
///   picture — which is the view Pascal was actually judging when he called it
///   broken, and the one a flat fallback ruins.
fn water_scenarios(pool: WaterPool) -> Vec<Scenario> {
    let world_x_meters = WORLD_SIZE_X as f32 * VOXEL_SIZE;
    let world_z_meters = WORLD_SIZE_Z as f32 * VOXEL_SIZE;
    let water_meters = WATER_LEVEL as f32 * VOXEL_SIZE;
    let surface = pool.surface_centre();
    let default_sun = SunSettings::default();

    vec![
        Scenario {
            label: "E shore -> pool, grazing",
            pose: CameraPose::from_yaw_pitch(
                Vec3::new(
                    world_x_meters * 0.5,
                    water_meters + 1.7,
                    world_z_meters * 0.86,
                ),
                -std::f32::consts::FRAC_PI_2,
                -0.16,
            ),
            sun: default_sun,
            capture_image: true,
        },
        Scenario {
            label: "F top-down over the lakes, steep",
            pose: CameraPose::from_yaw_pitch(
                Vec3::new(world_x_meters * 0.5, 60.0, world_z_meters * 0.5),
                -std::f32::consts::FRAC_PI_2,
                -(std::f32::consts::FRAC_PI_2 - 0.01),
            ),
            sun: default_sun,
            capture_image: true,
        },
        Scenario {
            label: "G underwater, looking up",
            pose: CameraPose::from_yaw_pitch(
                Vec3::new(surface.x, surface.y - 2.0, surface.z),
                -std::f32::consts::FRAC_PI_2,
                std::f32::consts::FRAC_PI_2 - 0.01,
            ),
            sun: default_sun,
            capture_image: true,
        },
        Scenario {
            label: "H underwater, looking sideways",
            pose: CameraPose::from_yaw_pitch(
                Vec3::new(surface.x, surface.y - 2.0, surface.z),
                -std::f32::consts::FRAC_PI_2,
                0.0,
            ),
            sun: default_sun,
            capture_image: true,
        },
        Scenario {
            label: "I underwater, up 45 deg (the window rim)",
            pose: CameraPose::from_yaw_pitch(
                Vec3::new(surface.x, surface.y - 2.0, surface.z),
                -std::f32::consts::FRAC_PI_2,
                std::f32::consts::FRAC_PI_4,
            ),
            sun: default_sun,
            capture_image: true,
        },
    ]
}

/// Section 15: L0's authored light-transport fixture — the rainbow corridor.
///
/// Every other section measures the ISLAND, where the correct picture is
/// whatever the renderer happens to produce and a regression is only visible as
/// a difference from a previously recorded run. This one measures a room whose
/// answer is known before the frame is drawn: a white ceiling that receives no
/// direct light at all, over six saturated floor and wall bands. If the bounce
/// term carries albedo, the ceiling is striped; if it does not, the ceiling is
/// grey. That is a *correctness* read, not a diff, and it is the one x1m4's own
/// 2022-11-01 albedo bug would have failed.
///
/// Levers come from [`BenchSection::Cagi`], the same sweep section 5 runs, so
/// the two tables are directly comparable: section 5 says what a propagation
/// rule COSTS on real terrain, this one says what it GETS RIGHT.
fn light_corridor_section(corridor: RainbowCorridor) -> Section {
    // Ambient off, so nothing but the slot lights the room. With the shipped
    // 1.0 ambient the whole interior reads at the ambient floor and indirect
    // light contributes a fraction nobody can see — the exact trap
    // `SunSettings::ambient_scale` was added for.
    let shipped = RenderQuality::default();
    let mut variants = vec![Variant::new("corridor-shipped".to_string(), shipped)];
    variants.extend(registry_variants(BenchSection::Cagi, &shipped));

    Section {
        heading: "section 15: L0 rainbow corridor — GI correctness on an authored fixture",
        scenarios: light_corridor_scenarios(corridor),
        variants,
        reference_label: "gi-off",
        compare_heading: "indirect coverage (differing pixels vs gi-off — in this room EVERY \
                          non-slot pixel is indirect, so a low number is a broken bounce)",
        crop_regions: LIGHT_CORRIDOR_CROP_REGIONS,
    }
}

/// Section 15's world: the seed-1 island with the corridor stamped into the air
/// above it.
///
/// A separate brickmap for the same reason section 8 needs one — the shared
/// island cannot answer this question — and stamped rather than generated so
/// sections 1-14 keep measuring the untouched island and no baseline moves.
fn light_corridor_world(brickmap: &Brickmap, corridor: RainbowCorridor) -> Brickmap {
    let mut authored = brickmap.clone();
    let written = corridor.carve(&mut authored);
    let (outer_min, outer_max) = corridor.outer_bounds();
    println!();
    println!(
        "== section 15 world: island + rainbow corridor, interior {}x{}x{} at {:?} ==",
        light_fixture::INTERIOR_WIDTH,
        light_fixture::INTERIOR_HEIGHT,
        light_fixture::INTERIOR_LENGTH,
        corridor.interior_min,
    );
    println!(
        "  {written} one-metre blocks written, outer box {outer_min:?}..={outer_max:?}, \
         {} colour bands of {} voxels, notch {:?}",
        light_fixture::BAND_MATERIALS.len(),
        light_fixture::SEGMENT_LENGTH,
        corridor.notch,
    );
    authored
}

/// Section 15's poses. All three stand inside the room, because the room is the
/// experiment.
///
/// The slot runs the corridor's whole length and the sun crosses it square-on, so
/// every band is lit alike and there is no bright end and dark end any more.
/// That changes what the poses are for: they are no longer sampling different
/// light levels, they are looking at the same lighting from three angles.
///
/// - `J` at the near end looking down the corridor: all six bands receding, each
///   with its own directly lit floor strip. The composition of the reference shot
///   and the one to read colour bleed from.
/// - `K` at the far end looking back: the same six bands in reverse order. Worth
///   having because the palette is asymmetric — reading it from both ends is what
///   separates a falloff in the LIGHT from a difference between the colours.
/// - `L` pitched up at the ceiling: the readout surface filling the frame. The
///   ceiling never sees direct light, so every band visible up there arrived by a
///   bounce.
fn light_corridor_scenarios(corridor: RainbowCorridor) -> Vec<Scenario> {
    // Sun and yaws come from the fixture, not from here: the app's
    // `--light-fixture` flag reads the same definitions, so a change to the
    // lighting cannot make the bench and the interactive view disagree.
    let sun = RainbowCorridor::sun();
    let near = corridor.viewer_eye_meters();
    let far = corridor.far_eye_meters();
    let down_corridor = RainbowCorridor::yaw_down_corridor();
    let up_corridor = RainbowCorridor::yaw_up_corridor();

    vec![
        Scenario {
            label: "J near end -> down the corridor",
            pose: CameraPose::from_yaw_pitch(
                Vec3::new(near[0], near[1], near[2]),
                down_corridor,
                -0.12,
            ),
            sun,
            capture_image: true,
        },
        Scenario {
            label: "K far end -> back up the corridor",
            pose: CameraPose::from_yaw_pitch(Vec3::new(far[0], far[1], far[2]), up_corridor, 0.10),
            sun,
            capture_image: true,
        },
        Scenario {
            // Pitched up from the near end. The ceiling runs the whole length and
            // is lit along the whole length now, so any upward aim inside the room
            // lands on lit ceiling — unlike the across-the-end slot, where aiming
            // up from here framed the dark far half and read as a broken bounce.
            label: "L near end -> up at the ceiling",
            pose: CameraPose::from_yaw_pitch(
                Vec3::new(near[0], near[1], near[2]),
                down_corridor,
                0.55,
            ),
            sun,
            capture_image: true,
        },
    ]
}

/// AO forced off — spelled once, used by section 1.
fn ao_off(mut quality: RenderQuality) -> RenderQuality {
    quality.ambient_occlusion.mode = AoMode::Off;
    quality
}

/// CAGI forced off — sections 1-3 measure the layers BELOW E4 (isolation rule),
/// so their numbers must stay directly comparable with the recorded pre-E4
/// baselines instead of carrying a light-volume sample.
fn gi_off(mut quality: RenderQuality) -> RenderQuality {
    quality.global_illumination.enabled = false;
    quality
}

/// E6's water optics forced off — same reason as `gi_off`, one layer up: the
/// island has lakes, so leaving reflection/refraction on would put secondary rays
/// into the very scenarios whose medians are the pre-E6 regression gate. With this
/// the sections below E6 render opaque water, i.e. exactly what every recorded
/// baseline describes.
fn water_off(mut quality: RenderQuality) -> RenderQuality {
    quality.water.mode = voxel_rt::water::WaterMode::Opaque;
    // Opaque water must also stop the SUN again, or the shadow rays in these
    // sections would still walk through the island's lakes and the Stage 2 pixel
    // gate would move. Off means off.
    quality.water.sun_through_liquid = false;
    quality
}

/// The material model forced off — the same isolation move `ao_off` / `gi_off` /
/// `water_off` make, one arc later, and section 9 cannot mean anything without it.
///
/// S1 and S2 were both promoted into `RenderQuality::default()` once they gated, so
/// `RenderQuality::default()` IS the four-layer variant. A section-9 run anchored on
/// the shipped defaults therefore compares patterns against patterns: it reports 0
/// differing pixels for `material-face-roles` and `material-patterns` and a delta
/// inside noise, which is exactly what the 2026-08-02 re-run produced before this.
/// The anchor has to be built off, not inherited.
fn materials_off(mut quality: RenderQuality) -> RenderQuality {
    quality.materials.face_roles = false;
    quality.materials.patterns = false;
    quality
}

/// Every registry bench point of `section`, applied to `baseline`. THIS is the
/// derivation that keeps the harness and the lever registry in step.
fn registry_variants(section: BenchSection, baseline: &RenderQuality) -> Vec<Variant> {
    bench_points_of(section)
        .map(|point| {
            let mut quality = *baseline;
            for (lever_id, value) in point.overrides {
                lever_id.apply(&mut quality, *value);
            }
            Variant::new(point.label.to_string(), quality)
        })
        .collect()
}

/// E1c's compile-time-vs-runtime A/B: the SAME fade configuration as
/// `ao-2ray-fade15-30`, but with the ramp bounds substituted back into the
/// shader as literals instead of read from the lighting uniform. The delta
/// between the two rows is the entire cost of making the fade range a runtime
/// knob (the plan's ~2% rule decides whether it stays one).
fn fade_range_as_shader_consts_variant(baseline: &RenderQuality) -> Variant {
    let mut quality = *baseline;
    for (lever_id, value) in [
        (LeverId::AoDistanceFade, LeverValue::Flag(true)),
        (LeverId::AoFadeStart, LeverValue::VoxelDistance(120)),
        (LeverId::AoFadeEnd, LeverValue::VoxelDistance(240)),
    ] {
        lever_id.apply(&mut quality, value);
    }
    let uniform_read = "smoothstep(lighting.shading_params.z, lighting.shading_params.w,";
    let folded_literals = "smoothstep(120.0, 240.0,";
    let shader_source = build_shader_source(&quality);
    assert!(
        shader_source.contains(uniform_read),
        "the AO fade ramp no longer reads shading_params.z/w — update or drop this A/B row"
    );
    Variant {
        label: "ao-2ray-fade15-30-const".to_string(),
        quality,
        edits: vec![(
            "dda.wgsl",
            uniform_read.to_string(),
            folded_literals.to_string(),
        )],
    }
}

/// Startup cost of the preset pipeline cache: what the app pays in
/// `AppState::new` so that switching preset in-app is a hash lookup instead of
/// a shader compile.
fn report_preset_pipeline_cache(
    device: &wgpu::Device,
    world_bindings: &WorldBindings,
    brickmap: &Brickmap,
) {
    let target = create_render_target(device, OUTPUT_WIDTH, OUTPUT_HEIGHT);
    let light_volume = LightVolume::new(
        device,
        brickmap,
        &CagiSettings::default(),
        &voxel_rt::cagi::MaterialAttributes::compiled(),
    );
    let mut pass = DdaPass::new(
        device,
        world_bindings,
        &light_volume,
        &target.view,
        voxel_color::OutputFormat::default(),
    );
    println!();
    println!("== preset pipeline cache ==");
    let mut programs = Vec::new();
    for spec in QUALITY_PRESETS
        .iter()
        .filter(|spec| spec.preset != QualityPreset::Custom)
    {
        // Composition is part of what a preset switch costs now, so it is inside the timer.
        let compile_start = Instant::now();
        let program = Variant::new(spec.label.to_string(), spec.resolve()).dda_program();
        let cached = pass.prewarm_pipelines(device, std::slice::from_ref(&program));
        println!(
            "  {:<10} {:>8.2?}  (cache holds {cached})",
            spec.label,
            compile_start.elapsed()
        );
        programs.push(program);
    }
    let total_start = Instant::now();
    let cached = pass.prewarm_pipelines(device, &programs);
    println!(
        "  re-prewarm of all {} presets: {:.2?} (cache holds {cached} distinct pipelines, \
         {} WGSL sources of ~{} KB)",
        programs.len(),
        total_start.elapsed(),
        programs.len(),
        SHADER_SOURCE.len() / 1024
    );
}

// ---- E4 reports ---------------------------------------------------------------

/// CA iterations run before any E4 measurement or capture — well past the point
/// where the image stops changing (the convergence table below shows where that
/// is), so every number describes a CONVERGED volume, i.e. the app's steady state.
const CONVERGENCE_ITERATIONS: u32 = 160;

/// The LOW-MEMORY table: what each resolution rung costs in VRAM. Printed before
/// section 5's timings because the memory verdict is half of the resolution
/// decision.
fn report_light_volume_memory(brickmap: &Brickmap) {
    println!();
    println!("== E4 light volume memory (125x32x125 m world; 1000x256x1000 detail grid) ==");
    println!(
        "{:<12} {:>18} {:>12} {:>14} {:>14} {:>12} {:>14}",
        "cell voxels", "grid", "cells", "one buffer", "ping-pong", "total", "CPU attr build"
    );
    for cell_voxels in [2, 4, 8] {
        let grid = voxel_rt::cagi::CagiGrid::for_world(
            cell_voxels,
            brickmap.metadata().max_occupied_brick_y,
        );
        // The static attribute buffer is rebuilt whenever the resolution lever
        // moves, so its CPU cost is a real (one-off) hitch the app pays.
        let build_start = Instant::now();
        let (attributes, _) = voxel_rt::cagi::build_cell_attributes_with_emission(
            brickmap,
            &grid,
            &voxel_rt::cagi::MaterialAttributes::compiled(),
        );
        let build_time = build_start.elapsed();
        let absorbing_cells = attributes
            .iter()
            .filter(|word| *word & voxel_rt::cagi::CELL_SOLID != 0)
            .count();
        println!(
            "{:<12} {:>18} {:>12} {:>11.1} MB {:>11.1} MB {:>9.1} MB {:>14.2?}   \
             ({:.1}% absorbing)",
            format!("{cell_voxels} ({:.2} m)", grid.cell_meters()),
            format!("{}x{}x{}", grid.size[0], grid.size[1], grid.size[2]),
            grid.cell_count(),
            grid.volume_bytes() as f32 / 1e6,
            grid.volume_bytes() as f32 * 2.0 / 1e6,
            grid.total_bytes() as f32 / 1e6,
            build_time,
            absorbing_cells as f32 / grid.cell_count() as f32 * 100.0,
        );
    }
    println!(
        "  (total = 2 ping-pong buffers + the packed attribute/emission buffer; the vertical \
         extent is clamped to the world's occupied height + 2 cells, which is the \
         difference between {} and {} cells of height at 4 voxels)",
        voxel_rt::cagi::CagiGrid::for_world(4, brickmap.metadata().max_occupied_brick_y).size[1],
        (voxel_rt::cagi::WORLD_SIZE_VOXELS[1]) / 4,
    );
}

/// The CONVERGENCE table: how many iterations (and therefore frames) the flood
/// needs before the image stops changing, cold and after a sun change. This is
/// the number E4's gate ("sun-drag re-floods in ~1 s") is judged on.
fn report_cagi_convergence(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world_bindings: &WorldBindings,
    brickmap: &Brickmap,
) {
    let quality = RenderQuality::default();
    let variant = Variant::new("gi-shipped".to_string(), quality);
    let scenarios = build_scenarios(&[]);
    let (width, height) = variant.resolution();
    let target = create_render_target(device, width, height);
    let mut resources =
        VariantResources::new(device, world_bindings, brickmap, &variant, &target.view);
    let iteration_rungs = [0_u32, 1, 2, 4, 8, 16, 32, 64, 128];

    for (label, scenario_index, previous_sun) in [
        ("cold start (nothing flooded)", 2_usize, None),
        ("sun change (default -> 5 deg)", 3_usize, Some(2_usize)),
    ] {
        let scenario = &scenarios[scenario_index];
        // The converged image this scenario's flood is heading toward.
        resources.flood_to_convergence(device, queue, world_bindings, &variant, scenario);
        render_once(
            device,
            queue,
            world_bindings,
            &resources,
            &variant,
            scenario,
        );
        let converged_image = read_back_image(device, queue, &target);

        // Cold start floods from an empty volume; the sun-change case starts from
        // a volume converged for the PREVIOUS sun, which is what dragging the
        // slider actually does.
        resources.light_volume.mark_dirty();
        if let Some(previous_index) = previous_sun {
            resources.flood_to_convergence(
                device,
                queue,
                world_bindings,
                &variant,
                &scenarios[previous_index],
            );
            resources.light_volume.mark_dirty();
        }

        println!();
        println!("== E4 convergence: {label}, scenario {} ==", scenario.label);
        println!(
            "{:>10} {:>8} {:>18} {:>10} {:>22}",
            "iterations", "frames", "differing pixels", "%", "max channel delta"
        );
        let mut iterations_done = 0_u32;
        for rung in iteration_rungs {
            let delta = rung - iterations_done;
            if delta > 0 {
                resources.run_iterations(
                    device,
                    queue,
                    world_bindings,
                    &scenario.lighting_uniform(&variant.quality),
                    delta,
                );
                iterations_done = rung;
            }
            render_once(
                device,
                queue,
                world_bindings,
                &resources,
                &variant,
                scenario,
            );
            let image = read_back_image(device, queue, &target);
            let (differing_pixels, max_channel_delta) = compare_images(&image, &converged_image);
            let total_pixels = u64::from(width) * u64::from(height);
            println!(
                "{rung:>10} {:>8.1} {differing_pixels:>18} {:>9.4}% {max_channel_delta:>22}",
                rung as f32 / variant.quality.global_illumination.iterations_per_frame as f32,
                differing_pixels as f64 / total_pixels as f64 * 100.0,
            );
        }
    }
}

/// The CPU cross-check: read the GPU volume back, run ONE more GPU iteration, and
/// verify every purely-propagating cell against [`propagate_reference`] — the same
/// integer arithmetic reimplemented on the CPU. Source cells are excluded the way
/// the shader identifies them (sky by the column test, sun by the pinned flag), so
/// what is compared is exactly the transport rule.
///
/// Also the DETERMINISM check: two floods from scratch must produce byte-identical
/// volumes.
fn report_cagi_cpu_cross_check(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world_bindings: &WorldBindings,
    brickmap: &Brickmap,
) {
    println!();
    println!("== E4 CPU cross-check of the propagation rule ==");
    let scenarios = build_scenarios(&[]);
    let scenario = &scenarios[2];
    for rule in [
        CagiRule::Diffusion6,
        CagiRule::MaxDecrement,
        CagiRule::Diffusion26,
    ] {
        let quality = RenderQuality {
            global_illumination: CagiSettings {
                rule,
                ..CagiSettings::default()
            },
            ..RenderQuality::default()
        };
        let variant = Variant::new(format!("{rule:?}"), quality);
        let (width, height) = variant.resolution();
        let target = create_render_target(device, width, height);
        let mut resources =
            VariantResources::new(device, world_bindings, brickmap, &variant, &target.view);
        let lighting_uniform = scenario.lighting_uniform(&variant.quality);

        resources.light_volume.mark_dirty();
        resources.run_iterations(device, queue, world_bindings, &lighting_uniform, 32);
        let volume_before = read_back_volume(device, queue, &resources.light_volume);
        resources.run_iterations(device, queue, world_bindings, &lighting_uniform, 1);
        let volume_after = read_back_volume(device, queue, &resources.light_volume);

        let grid = resources.light_volume.grid();
        let (attributes, _) = voxel_rt::cagi::build_cell_attributes_with_emission(
            brickmap,
            &grid,
            &voxel_rt::cagi::MaterialAttributes::compiled(),
        );
        let sky_light = voxel_rt::cagi::quantize_radiance(scenario_sky_radiance());
        let mut checked = 0_u64;
        let mut mismatches = 0_u64;
        let mut first_mismatch = None;
        for cell_z in 0..grid.size[2] {
            for cell_y in 0..grid.size[1] {
                for cell_x in 0..grid.size[0] {
                    let cell = [cell_x, cell_y, cell_z];
                    let index = grid.cell_index(cell);
                    if attributes[index] & CELL_SOLID != 0 {
                        continue; // absorber: the shader stores 0, nothing to predict
                    }
                    if cell_sees_sky_by_column(&grid, &brickmap.column_max_brick_y, cell) {
                        continue; // sky source
                    }
                    let mut neighbour_light = |neighbour: [i32; 3]| {
                        if neighbour[1] >= grid.size[1] as i32 {
                            return sky_light;
                        }
                        if neighbour.iter().any(|value| *value < 0)
                            || (0..3).any(|axis| neighbour[axis] >= grid.size[axis] as i32)
                        {
                            return [0, 0, 0];
                        }
                        unpack_light(
                            volume_before[grid.cell_index([
                                neighbour[0] as u32,
                                neighbour[1] as u32,
                                neighbour[2] as u32,
                            ])],
                        )
                    };
                    let expected = propagate_reference(
                        rule,
                        &grid,
                        [cell_x as i32, cell_y as i32, cell_z as i32],
                        &mut neighbour_light,
                    );
                    let actual = unpack_light(volume_after[index]);
                    checked += 1;
                    if expected != actual {
                        mismatches += 1;
                        if first_mismatch.is_none() {
                            first_mismatch = Some((cell, expected, actual));
                        }
                    }
                }
            }
        }

        // Determinism: the same inputs must give the same volume, bit for bit.
        resources.light_volume.mark_dirty();
        resources.run_iterations(device, queue, world_bindings, &lighting_uniform, 33);
        let repeated = read_back_volume(device, queue, &resources.light_volume);
        let deterministic = repeated == volume_after;

        println!(
            "  {:<14} {checked} propagating cells checked, {mismatches} mismatches; \
             deterministic re-flood: {}",
            format!("{rule:?}"),
            if deterministic { "yes" } else { "NO" }
        );
        if let Some((cell, expected, actual)) = first_mismatch {
            println!("    first mismatch at cell {cell:?}: expected {expected:?}, got {actual:?}");
        }
        // Invariants that hold regardless of the rule.
        let solid_cells_with_light = (0..grid.cell_count())
            .filter(|index| attributes[*index] & CELL_SOLID != 0 && volume_after[*index] != 0)
            .count();
        let over_saturated = volume_after
            .iter()
            .filter(|word| unpack_light(**word).iter().any(|channel| *channel > 1023))
            .count();
        println!(
            "    solid cells holding light: {solid_cells_with_light} (must be 0); \
             channels over 1023: {over_saturated} (must be 0)"
        );
    }
}

// ---- E2 reports (world authority, threading, the edit pipeline) ---------------

/// Edits per frame during a storm, per pattern. A hold-to-repeat human produces 8
/// per SECOND ([`EDIT_REPEAT_HZ`](../src/main.rs)); these rates are 30-120x that,
/// on purpose — the question is where the pipeline breaks, not whether a human can
/// outrun it.
const STORM_EDITS_PER_FRAME: usize = 4;
/// The dig pattern runs hotter so it empties whole bricks inside the run.
const DIG_EDITS_PER_FRAME: usize = 16;
/// Frames measured for the idle anchor (no edits at all).
const IDLE_FRAMES: usize = 64;
/// Scattered / wall storms place this many voxels.
const STORM_EDIT_COUNT: usize = 256;
/// Surface bricks the dig pattern clears completely (8^3 = 512 voxels each).
const DIG_BRICK_COUNT: usize = 4;
/// Audio-style occlusion rays timed over the CPU mirror.
const AUDIO_RAY_COUNT: usize = 4096;

/// What an edit storm does to the world.
#[derive(Clone, Copy, PartialEq, Eq)]
enum StormPattern {
    /// No edits at all — the regression anchor: an edit-capable renderer with no
    /// edits must cost exactly what E4's renderer cost.
    Idle,
    /// 256 stone voxels at scattered positions a few metres above the terrain:
    /// almost every one MATERIALIZES a brick, so this is the allocation /
    /// clearance-shrink / column-height worst case.
    ScatterPlace,
    /// A dense 16x16 wall built voxel by voxel — the gate's "hold-to-place blocks"
    /// case, where most edits patch two words inside an existing brick.
    WallPlace,
    /// Clear four brick-aligned surface bricks completely: the only pattern that
    /// FREES bricks, i.e. the only one the clearance-update lever can move.
    DigBricks,
}

impl StormPattern {
    fn label(self) -> &'static str {
        match self {
            StormPattern::Idle => "idle (no edits)",
            StormPattern::ScatterPlace => "scatter-place",
            StormPattern::WallPlace => "wall-place",
            StormPattern::DigBricks => "dig-bricks",
        }
    }

    fn edits_per_frame(self) -> usize {
        match self {
            StormPattern::Idle => 0,
            StormPattern::DigBricks => DIG_EDITS_PER_FRAME,
            _ => STORM_EDITS_PER_FRAME,
        }
    }

    const ALL: [StormPattern; 4] = [
        StormPattern::Idle,
        StormPattern::ScatterPlace,
        StormPattern::WallPlace,
        StormPattern::DigBricks,
    ];
}

/// The highest occupied voxel of a column, or `None` for water/air columns.
fn surface_voxel_y(brickmap: &Brickmap, x: i32, z: i32) -> Option<i32> {
    (0..voxel_core::world::WORLD_SIZE_Y as i32)
        .rev()
        .find(|y| brickmap.is_occupied(x, *y, z))
}

/// Build a pattern's edit list, verified against a scratch copy of the world so
/// that EVERY entry really changes something. That 1:1 guarantee is what lets the
/// storm match the k-th delta to the k-th request and measure per-edit latency.
fn storm_edit_list(brickmap: &Brickmap, pattern: StormPattern) -> Vec<([i32; 3], Voxel)> {
    let mut scratch = brickmap.clone();
    let clearance = WorldEditSettings::default().clearance();
    let mut edits = Vec::new();
    /// Keep an edit only if it really changes the scratch world.
    fn keep(
        scratch: &mut Brickmap,
        edits: &mut Vec<([i32; 3], Voxel)>,
        voxel: [i32; 3],
        material: Voxel,
        clearance: ClearanceUpdate,
    ) {
        if scratch
            .set_voxel(voxel[0], voxel[1], voxel[2], material, clearance)
            .is_some()
        {
            edits.push((voxel, material));
        }
    }
    match pattern {
        StormPattern::Idle => {}
        StormPattern::ScatterPlace => {
            // Deterministic LCG over the island's middle, placing 3 m above the
            // surface so the target voxel is open air.
            let mut lcg_state = 0x5eed_1234_u32;
            let mut next = || {
                lcg_state = lcg_state
                    .wrapping_mul(1_664_525)
                    .wrapping_add(1_013_904_223);
                lcg_state
            };
            while edits.len() < STORM_EDIT_COUNT {
                let x = 250 + (next() % 500) as i32;
                let z = 250 + (next() % 500) as i32;
                let Some(surface_y) = surface_voxel_y(&scratch, x, z) else {
                    continue;
                };
                keep(
                    &mut scratch,
                    &mut edits,
                    [x, surface_y + 24, z],
                    Voxel::Stone,
                    clearance,
                );
            }
        }
        StormPattern::WallPlace => {
            // A 16x16 wall in the XY plane at the island centre, on the surface.
            let (base_x, base_z) = (492, 500);
            let Some(surface_y) = surface_voxel_y(&scratch, base_x, base_z) else {
                panic!("the island centre has no surface");
            };
            for height in 0..16 {
                for offset in 0..16 {
                    keep(
                        &mut scratch,
                        &mut edits,
                        [base_x + offset, surface_y + 1 + height, base_z],
                        Voxel::Stone,
                        clearance,
                    );
                }
            }
        }
        StormPattern::DigBricks => {
            for brick_index in 0..DIG_BRICK_COUNT {
                let (x, z) = (480 + brick_index as i32 * 16, 500);
                let Some(surface_y) = surface_voxel_y(&scratch, x, z) else {
                    panic!("dig column ({x}, {z}) has no surface");
                };
                let base = [
                    x - x.rem_euclid(BRICK_SIZE as i32),
                    surface_y - surface_y.rem_euclid(BRICK_SIZE as i32),
                    z - z.rem_euclid(BRICK_SIZE as i32),
                ];
                for local_z in 0..BRICK_SIZE as i32 {
                    for local_y in 0..BRICK_SIZE as i32 {
                        for local_x in 0..BRICK_SIZE as i32 {
                            keep(
                                &mut scratch,
                                &mut edits,
                                [base[0] + local_x, base[1] + local_y, base[2] + local_z],
                                Voxel::Air,
                                clearance,
                            );
                        }
                    }
                }
            }
        }
    }
    edits
}

/// One measured distribution, in milliseconds. The E2 gate is about HITCHES, so
/// the median is the least interesting column here.
struct Distribution {
    median: f32,
    p99: f32,
    maximum: f32,
}

fn summarize_distribution(samples: &mut [f32]) -> Distribution {
    samples.sort_by(|a, b| a.partial_cmp(b).expect("NaN timing sample"));
    let index = |quantile: f32| {
        ((samples.len() as f32 * quantile) as usize).min(samples.len().saturating_sub(1))
    };
    Distribution {
        median: samples[index(0.5)],
        p99: samples[index(0.99)],
        maximum: *samples.last().expect("at least one sample"),
    }
}

/// What one (variant, pattern) run measured.
struct StormResult {
    frame_pipeline: Distribution,
    frame_total: Distribution,
    apply_micros: Distribution,
    latency_milliseconds: Distribution,
    latency_frames_max: usize,
    edits_applied: usize,
    upload_bytes: usize,
    brick_allocations: usize,
    brick_frees: usize,
    clearance_cells: usize,
    tail_frames: usize,
}

/// The E2 build table: what the CPU-authoritative pipeline costs ONCE, and what
/// the plan's snapshot-swap alternative would cost per edit.
fn report_edit_build_times(world: &VoxelWorld, brickmap: &Brickmap) {
    println!();
    println!("== E2 build + snapshot costs (CPU) ==");
    let build_start = Instant::now();
    let rebuilt = Brickmap::build(world);
    let full_build = build_start.elapsed();
    let clone_start = Instant::now();
    let snapshot = rebuilt.clone();
    let clone_time = clone_start.elapsed();
    println!(
        "  full brickmap build (world -> every derived structure): {:>10.2?}",
        full_build
    );
    println!(
        "  DEEP COPY of the brickmap (the plan's Arc<Brickmap> snapshot swap, PER EDIT): \
         {:>10.2?} for {:.1} MB",
        clone_time,
        snapshot.cpu_bytes() as f32 / 1e6
    );
    let grid = voxel_rt::cagi::CagiGrid::for_world(4, brickmap.metadata().max_occupied_brick_y);
    let attributes = voxel_rt::cagi::MaterialAttributes::compiled();
    let probe_cell = [grid.size[0] / 2, grid.size[1] / 2, grid.size[2] / 2];
    let mut e5b_samples = Vec::with_capacity(32);
    for _ in 0..32 {
        let started = Instant::now();
        let mut checksum = 0_u32;
        for offset in [
            [0_i32, 0, 0],
            [1, 0, 0],
            [-1, 0, 0],
            [0, 1, 0],
            [0, -1, 0],
            [0, 0, 1],
            [0, 0, -1],
        ] {
            let cell = [
                (probe_cell[0] as i32 + offset[0]).clamp(0, grid.size[0] as i32 - 1) as u32,
                (probe_cell[1] as i32 + offset[1]).clamp(0, grid.size[1] as i32 - 1) as u32,
                (probe_cell[2] as i32 + offset[2]).clamp(0, grid.size[2] as i32 - 1) as u32,
            ];
            checksum ^=
                voxel_rt::cagi::cell_attribute(brickmap, &grid, cell, &attributes).attribute;
        }
        std::hint::black_box(checksum);
        e5b_samples.push(started.elapsed().as_secs_f32() * 1000.0);
    }
    let e5b = summarize(&mut e5b_samples);
    println!(
        "  E5b self+six cell attribute/emission recompute: median {:>7.3} ms, p95 {:>7.3} ms",
        e5b.0, e5b.1
    );
    // The clearance field alone: the cost the FullRebuild strategy pays per freed
    // brick, isolated by rebuilding it through a removal on a scratch copy.
    let mut scratch = brickmap.clone();
    let surface_y = surface_voxel_y(&scratch, 500, 500).expect("occupied column");
    let base = [
        496,
        surface_y - surface_y.rem_euclid(BRICK_SIZE as i32),
        496,
    ];
    // Empty all but the last voxel of a brick with the cheap strategy, then time
    // the two strategies on the voxel that actually frees it.
    let mut last_voxel = None;
    for local_z in 0..BRICK_SIZE as i32 {
        for local_y in 0..BRICK_SIZE as i32 {
            for local_x in 0..BRICK_SIZE as i32 {
                let voxel = [base[0] + local_x, base[1] + local_y, base[2] + local_z];
                if scratch.is_occupied(voxel[0], voxel[1], voxel[2]) {
                    if last_voxel.is_none() {
                        last_voxel = Some(voxel);
                        continue; // keep one occupied voxel so the brick survives
                    }
                    scratch.set_voxel(
                        voxel[0],
                        voxel[1],
                        voxel[2],
                        Voxel::Air,
                        ClearanceUpdate::LocalBox { radius_cells: 8 },
                    );
                }
            }
        }
    }
    let last_voxel = last_voxel.expect("the brick held occupied voxels");
    for (label, clearance) in [
        (
            "local box r=2",
            ClearanceUpdate::LocalBox { radius_cells: 2 },
        ),
        (
            "local box r=8",
            ClearanceUpdate::LocalBox { radius_cells: 8 },
        ),
        (
            "local box r=16",
            ClearanceUpdate::LocalBox { radius_cells: 16 },
        ),
        ("full rebuild", ClearanceUpdate::FullRebuild),
    ] {
        let mut freeing = scratch.clone();
        let started = Instant::now();
        let edit = freeing
            .set_voxel(
                last_voxel[0],
                last_voxel[1],
                last_voxel[2],
                Voxel::Air,
                clearance,
            )
            .expect("the last voxel of the brick frees it");
        let elapsed = started.elapsed();
        assert!(edit.brick_freed, "the fixture did not free a brick");
        println!(
            "  clearance repair on a FREED brick, {label:<14} {:>10.2?}  \
             ({} cells written, {} bytes of delta)",
            elapsed,
            edit.clearance_cells_written,
            edit.dirty_bytes(),
        );
    }
}

/// The headline E2 table: per-frame cost DISTRIBUTIONS under an edit storm, for
/// every authority/threading variant, plus edit latency and upload bytes.
fn report_edit_storm(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    brickmap: &Brickmap,
    runs: &[(String, RenderQuality)],
) {
    println!();
    println!("== E2 edit storm ==");
    println!(
        "  {} edits/frame (scatter, wall) and {} (dig); {} scattered placements, \
         a 16x16 wall, {} bricks dug out",
        STORM_EDITS_PER_FRAME, DIG_EDITS_PER_FRAME, STORM_EDIT_COUNT, DIG_BRICK_COUNT
    );
    let scenario = &build_scenarios(&[])[2]; // C: ground level, default sun

    for (label, quality) in runs {
        for pattern in StormPattern::ALL {
            let edits = storm_edit_list(brickmap, pattern);
            let result =
                run_edit_storm(device, queue, brickmap, quality, scenario, pattern, &edits);
            println!();
            println!("  -- {label} / {} --", pattern.label());
            println!(
                "     frame pipeline (CPU: request + drain + upload)  median {:>7.3} ms  \
                 p99 {:>7.3} ms  max {:>7.3} ms",
                result.frame_pipeline.median,
                result.frame_pipeline.p99,
                result.frame_pipeline.maximum
            );
            println!(
                "     whole frame  (+ CAGI + shading dispatch, blocked) median {:>7.3} ms  \
                 p99 {:>7.3} ms  max {:>7.3} ms",
                result.frame_total.median, result.frame_total.p99, result.frame_total.maximum
            );
            if result.edits_applied > 0 {
                println!(
                    "     CPU apply per edit                              median {:>7.1} us  \
                     p99 {:>7.1} us  max {:>7.1} us",
                    result.apply_micros.median * 1000.0,
                    result.apply_micros.p99 * 1000.0,
                    result.apply_micros.maximum * 1000.0
                );
                println!(
                    "     edit -> uploaded latency                        median {:>7.3} ms  \
                     max {:>7.3} ms  ({} frames worst case, {} tail frames)",
                    result.latency_milliseconds.median,
                    result.latency_milliseconds.maximum,
                    result.latency_frames_max,
                    result.tail_frames
                );
                println!(
                    "     {} edits: {} bytes uploaded = {:.0} B/edit; {} brick allocs, \
                     {} frees, {} clearance cells",
                    result.edits_applied,
                    result.upload_bytes,
                    result.upload_bytes as f32 / result.edits_applied as f32,
                    result.brick_allocations,
                    result.brick_frees,
                    result.clearance_cells
                );
            }
        }
    }
}

/// One (variant, pattern) run: its own world copy, its own GPU buffers, its own
/// light volume — so a storm can never contaminate the next run.
#[allow(clippy::too_many_arguments)]
fn run_edit_storm(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    brickmap: &Brickmap,
    quality: &RenderQuality,
    scenario: &Scenario,
    pattern: StormPattern,
    edits: &[([i32; 3], Voxel)],
) -> StormResult {
    let mut host = WorldHost::new(brickmap.clone());
    host.set_world_thread(quality.world_edit.world_thread);
    let variant = Variant::new("storm".to_string(), *quality);
    let (width, height) = variant.resolution();
    let target = create_render_target(device, width, height);
    let (world_bindings, mut resources) = {
        let world = host.read();
        let world_bindings = WorldBindings::new(device, &world);
        let resources =
            VariantResources::new(device, &world_bindings, &world, &variant, &target.view);
        (world_bindings, resources)
    };
    let light_grid = quality
        .global_illumination
        .enabled
        .then(|| resources.light_volume.grid());
    // Flood once so the storm starts from the app's steady state.
    resources.flood_to_convergence(device, queue, &world_bindings, &variant, scenario);

    let frames = if pattern == StormPattern::Idle {
        IDLE_FRAMES
    } else {
        edits.len().div_ceil(pattern.edits_per_frame())
    };
    let mut pipeline_samples = Vec::with_capacity(frames);
    let mut total_samples = Vec::with_capacity(frames);
    let mut apply_samples = Vec::new();
    let mut latency_samples = Vec::new();
    let mut latency_frames_max = 0;
    let mut request_times: Vec<(Instant, usize)> = Vec::new();
    let mut deltas_seen = 0_usize;
    let mut result = StormResult {
        frame_pipeline: Distribution {
            median: 0.0,
            p99: 0.0,
            maximum: 0.0,
        },
        frame_total: Distribution {
            median: 0.0,
            p99: 0.0,
            maximum: 0.0,
        },
        apply_micros: Distribution {
            median: 0.0,
            p99: 0.0,
            maximum: 0.0,
        },
        latency_milliseconds: Distribution {
            median: 0.0,
            p99: 0.0,
            maximum: 0.0,
        },
        latency_frames_max: 0,
        edits_applied: 0,
        upload_bytes: 0,
        brick_allocations: 0,
        brick_frees: 0,
        clearance_cells: 0,
        tail_frames: 0,
    };

    let mut next_edit = 0_usize;
    let mut frame = 0_usize;
    // Keep going past the edit list until the authority has caught up, so the
    // measured latency includes the tail (variant B's whole point is that the tail
    // exists and does not cost the frame).
    while frame < frames || next_edit < edits.len() || host.in_flight() > 0 {
        let frame_start = Instant::now();
        for _ in 0..pattern.edits_per_frame() {
            if next_edit >= edits.len() {
                break;
            }
            let (voxel, material) = edits[next_edit];
            host.request_edit(
                VoxelEdit {
                    voxel,
                    material,
                    light_grid,
                    material_attributes: voxel_rt::cagi::MaterialAttributes::compiled(),
                },
                &quality.world_edit,
            );
            request_times.push((Instant::now(), frame));
            next_edit += 1;
        }

        let mut geometry_changed = false;
        for update in host.drain() {
            match update {
                WorldUpdate::Delta(delta) => {
                    geometry_changed = true;
                    assert!(
                        !delta.arrays_grew,
                        "the storm outgrew the brick headroom — raise EDIT_BRICK_HEADROOM \
                         or shorten the storm"
                    );
                    for write in &delta.writes {
                        world_bindings.apply_array_write(queue, write);
                    }
                    if let Some(metadata) = &delta.metadata {
                        world_bindings.write_metadata(queue, metadata);
                    }
                    resources.light_volume.write_cell_attributes(
                        queue,
                        delta.light_grid,
                        &delta.light_cells,
                    );
                    apply_samples.push(delta.apply_micros / 1000.0);
                    result.upload_bytes += delta.upload_bytes();
                    result.clearance_cells += delta.clearance_cells_written;
                    if delta.metadata.is_some() {
                        // A metadata change means a brick materialized or was freed;
                        // which one is decided by the material.
                        if delta.material == 0 {
                            result.brick_frees += 1;
                        } else {
                            result.brick_allocations += 1;
                        }
                    }
                    if let Some((requested_at, requested_frame)) =
                        request_times.get(deltas_seen).copied()
                    {
                        latency_samples.push(requested_at.elapsed().as_secs_f32() * 1000.0);
                        latency_frames_max = latency_frames_max.max(frame - requested_frame);
                    }
                    deltas_seen += 1;
                }
                WorldUpdate::LightAttributes { .. } => {}
            }
        }
        if geometry_changed && quality.world_edit.gi_reflood {
            resources.light_volume.mark_dirty();
        }
        pipeline_samples.push(frame_start.elapsed().as_secs_f32() * 1000.0);

        // The GPU half of the frame: this frame's CA iterations plus one shading
        // dispatch, blocked to completion so the sample is a whole frame.
        world_bindings.write_lighting(queue, &scenario.lighting_uniform(quality));
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench edit storm frame"),
        });
        resources.cagi_pass.encode(
            &mut encoder,
            &mut resources.light_volume,
            quality.gi_iterations_per_frame(),
            None,
        );
        resources.dda_pass.encode(
            queue,
            &mut encoder,
            &scenario.camera_uniform((width, height)),
            resources.light_volume.front(),
            width,
            height,
            None,
        );
        queue.submit([encoder.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("device poll failed");
        total_samples.push(frame_start.elapsed().as_secs_f32() * 1000.0);

        if frame >= frames {
            result.tail_frames += 1;
        }
        frame += 1;
        assert!(
            frame < frames + 600,
            "the storm never drained: {} of {} edits, {} in flight",
            deltas_seen,
            edits.len(),
            host.in_flight()
        );
    }

    result.edits_applied = deltas_seen;
    result.latency_frames_max = latency_frames_max;
    result.frame_pipeline = summarize_distribution(&mut pipeline_samples);
    result.frame_total = summarize_distribution(&mut total_samples);
    if !apply_samples.is_empty() {
        result.apply_micros = summarize_distribution(&mut apply_samples);
        result.latency_milliseconds = summarize_distribution(&mut latency_samples);
    }
    assert_eq!(
        deltas_seen,
        edits.len(),
        "every edit in the list was verified to change something, so every one must \
         produce exactly one delta"
    );
    result
}

/// Variant C's decisive numbers: what it costs to keep a CPU occupancy mirror
/// fresh by reading it back from the GPU, in bandwidth AND in frames of staleness.
///
/// Both halves matter, and the second is the one that decides: even a fast copy is
/// only *readable* after the GPU has finished the frame that wrote it, so a
/// GPU-authoritative world hands the audio thread a mirror that is structurally
/// behind — where the CPU-authoritative variants hand it the authority itself.
fn report_occupancy_readback(device: &wgpu::Device, queue: &wgpu::Queue, brickmap: &Brickmap) {
    println!();
    println!("== E2 variant C: GPU -> CPU occupancy readback ==");
    let cases: [(&str, usize); 4] = [
        (
            "one brick's occupancy words (an edit's delta)",
            OCCUPANCY_WORDS_PER_BRICK * 4,
        ),
        (
            "brick occupancy bit grid (1 bit / brick)",
            brickmap.brick_occupancy_bit_words.len() * 4,
        ),
        (
            "voxel occupancy words (1 bit / voxel, occupied bricks)",
            brickmap.occupancy_words.len() * 4,
        ),
        (
            "occupancy + materials (the level-1 mirror audio would want)",
            (brickmap.occupancy_words.len() + brickmap.material_words.len()) * 4,
        ),
    ];
    println!(
        "  {:<58} {:>10} {:>12} {:>12}",
        "mirror", "bytes", "blocked ms", "GB/s"
    );
    for (label, bytes) in cases {
        let source = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bench readback source"),
            size: bytes as u64,
            usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_SRC,
            mapped_at_creation: false,
        });
        let staging = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("bench readback staging"),
            size: bytes as u64,
            usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
            mapped_at_creation: false,
        });
        // Median of a few blocked round trips: copy, submit, map, wait.
        let mut samples = Vec::new();
        for _ in 0..8 {
            let started = Instant::now();
            let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
                label: Some("bench readback"),
            });
            encoder.copy_buffer_to_buffer(&source, 0, &staging, 0, bytes as u64);
            queue.submit([encoder.finish()]);
            let slice = staging.slice(..);
            slice.map_async(wgpu::MapMode::Read, |result| {
                result.expect("readback map failed")
            });
            device
                .poll(wgpu::PollType::Wait {
                    submission_index: None,
                    timeout: None,
                })
                .expect("device poll failed");
            let mapped = slice.get_mapped_range();
            let checksum = mapped.first().copied().unwrap_or(0);
            drop(mapped);
            staging.unmap();
            std::hint::black_box(checksum);
            samples.push(started.elapsed().as_secs_f32() * 1000.0);
        }
        let distribution = summarize_distribution(&mut samples);
        println!(
            "  {label:<58} {bytes:>10} {:>12.3} {:>12.2}",
            distribution.median,
            bytes as f32 / 1e9 / (distribution.median / 1000.0),
        );
    }

    // STALENESS: how many FRAMES pass before a readback issued in frame N can be
    // read, when the GPU keeps doing a frame's worth of work in between and the CPU
    // never blocks — the only way a readback is usable in a real frame loop.
    let bytes = brickmap.occupancy_words.len() * 4;
    let storage_buffer = |label: &'static str| {
        device.create_buffer(&wgpu::BufferDescriptor {
            label: Some(label),
            size: bytes as u64,
            usage: wgpu::BufferUsages::STORAGE
                | wgpu::BufferUsages::COPY_SRC
                | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        })
    };
    let source = storage_buffer("bench staleness source");
    let frame_work = storage_buffer("bench staleness frame work");
    let staging = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench staleness staging"),
        size: bytes as u64,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench staleness copy"),
    });
    encoder.copy_buffer_to_buffer(&source, 0, &staging, 0, bytes as u64);
    queue.submit([encoder.finish()]);
    let mapped_flag = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
    let flag = std::sync::Arc::clone(&mapped_flag);
    staging
        .slice(..)
        .map_async(wgpu::MapMode::Read, move |result| {
            flag.store(result.is_ok(), std::sync::atomic::Ordering::SeqCst);
        });
    let started = Instant::now();
    let mut frames_waited = 0_usize;
    while !mapped_flag.load(std::sync::atomic::Ordering::SeqCst) && frames_waited < 64 {
        // One "frame" of unrelated GPU work, then a NON-BLOCKING poll: exactly what
        // a render loop can afford to do.
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench staleness frame"),
        });
        encoder.copy_buffer_to_buffer(&source, 0, &frame_work, 0, bytes as u64);
        queue.submit([encoder.finish()]);
        device
            .poll(wgpu::PollType::Poll)
            .expect("device poll failed");
        frames_waited += 1;
    }
    let mapped = mapped_flag.load(std::sync::atomic::Ordering::SeqCst);
    if mapped {
        staging.unmap();
    }
    println!(
        "  NON-BLOCKING mapping of the {:.1} MB voxel mirror became readable after \
         {} simulated frames ({:.2?}), mapped: {}",
        bytes as f32 / 1e6,
        frames_waited,
        started.elapsed(),
        mapped
    );
}

/// What one audio-style occlusion query over the CPU mirror costs — the number
/// that says whether keeping the mirror on the CPU is worth anything (E8).
fn report_audio_ray_cost(brickmap: &Brickmap) {
    println!();
    println!("== E2 CPU mirror queries (the E8 seam) ==");
    let listener = [62.5, WATER_LEVEL as f32 * VOXEL_SIZE + 1.7, 107.5];
    let mut lcg_state = 0xa11c_e001_u32;
    let mut next = || {
        lcg_state = lcg_state
            .wrapping_mul(1_664_525)
            .wrapping_add(1_013_904_223);
        lcg_state
    };
    let sources: Vec<[f32; 3]> = (0..AUDIO_RAY_COUNT)
        .map(|_| {
            [
                (next() % 1000) as f32 * 0.125,
                WATER_LEVEL as f32 * VOXEL_SIZE + (next() % 160) as f32 * 0.125,
                (next() % 1000) as f32 * 0.125,
            ]
        })
        .collect();
    let started = Instant::now();
    let mut blocked = 0_usize;
    for source in &sources {
        if !voxel_dda::path_is_clear(brickmap, listener, *source) {
            blocked += 1;
        }
    }
    let elapsed = started.elapsed();
    println!(
        "  {AUDIO_RAY_COUNT} listener->source occlusion rays over the island: {:.2?} total, \
         {:.2} us per ray ({} blocked)",
        elapsed,
        elapsed.as_secs_f32() * 1e6 / AUDIO_RAY_COUNT as f32,
        blocked
    );
    let started = Instant::now();
    let mut hits = 0_usize;
    for source in &sources {
        let direction = [
            source[0] - listener[0],
            source[1] - listener[1],
            source[2] - listener[2],
        ];
        if voxel_dda::cast(
            brickmap,
            listener,
            direction,
            160.0,
            voxel_dda::CastTarget::AnyVoxel,
        )
        .is_some()
        {
            hits += 1;
        }
    }
    let elapsed = started.elapsed();
    println!(
        "  {AUDIO_RAY_COUNT} full reflection casts (hit voxel + face + material): {:.2?} total, \
         {:.2} us per ray ({hits} hits)",
        elapsed,
        elapsed.as_secs_f32() * 1e6 / AUDIO_RAY_COUNT as f32
    );
}

/// E2b — what the walking body costs the frame thread, per movement step.
///
/// Reported as a distribution for the same reason E2's storm is: a median tells
/// you it is cheap, a maximum tells you whether it can hitch. The scenarios span
/// the cost axes the sweep actually has — how much open air the body's
/// cross-section scans (an EMPTY box test has no early-out, so open sky is the
/// expensive case, not dense terrain), how often the auto-step fires (up to four
/// extra sweeps per horizontal axis), and how many substeps the frame delta
/// forces.
fn report_character_movement_cost(brickmap: &Brickmap) {
    println!();
    println!("== E2b character movement + collision (CPU) ==");
    let settings = CharacterSettings::default();
    println!(
        "  body {:.2} x {:.2} m ({:.1} x {:.1} voxels), eye {:.2} m, step-up {:.3} m \
         ({:.0} voxels), jump {:.2} m/s -> {:.2} m apex",
        settings.body_width_meters,
        settings.body_height_meters,
        settings.body_width_meters / VOXEL_SIZE,
        settings.body_height_meters / VOXEL_SIZE,
        settings.eye_height_meters,
        settings.step_up_meters,
        settings.step_up_meters / VOXEL_SIZE,
        settings.jump_speed(),
        settings.jump_apex_meters,
    );

    let walking = CameraInput {
        forward: true,
        ..CameraInput::default()
    };
    let sprinting = CameraInput {
        forward: true,
        speed_multiplier: 100.0, // clamped to the sprint ceiling by the controller
        ..CameraInput::default()
    };
    let idle = CameraInput::default();
    let (uphill_x, uphill_z) = steepest_uphill_column(brickmap);
    println!(
        "  steepest +X rise found for the auto-step row: voxel column ({uphill_x}, {uphill_z})"
    );

    // (label, start column, spawn height above the surface, input, delta, steps)
    let scenarios: [(&str, i32, i32, f32, &CameraInput, f32, usize); 6] = [
        (
            "idle on terrain, 60 fps",
            500,
            500,
            0.0,
            &idle,
            1.0 / 60.0,
            1200,
        ),
        (
            "walk over terrain, 60 fps",
            500,
            500,
            0.0,
            &walking,
            1.0 / 60.0,
            1200,
        ),
        (
            "sprint over terrain, 60 fps",
            420,
            560,
            0.0,
            &sprinting,
            1.0 / 60.0,
            1200,
        ),
        (
            "sprint into a rise (auto-step every frame)",
            uphill_x,
            uphill_z,
            0.0,
            &sprinting,
            1.0 / 60.0,
            1200,
        ),
        (
            "free fall through open air (no early-out), 60 fps",
            500,
            500,
            28.0,
            &sprinting,
            1.0 / 60.0,
            1200,
        ),
        (
            "sprint + fall through a 40 ms hitch",
            500,
            500,
            28.0,
            &sprinting,
            0.04,
            600,
        ),
    ];
    for (label, voxel_x, voxel_z, height_above, input, delta_seconds, steps) in scenarios {
        let mut body = spawn_character(brickmap, voxel_x, voxel_z, 0.35, height_above);
        let mut samples = Vec::with_capacity(steps);
        for _ in 0..steps {
            let started = Instant::now();
            body.step(brickmap, input, delta_seconds);
            samples.push(started.elapsed().as_secs_f32() * 1e6);
        }
        let distribution = summarize_distribution(&mut samples);
        println!(
            "  {label:<50} median {:>7.2} us  p99 {:>7.2} us  max {:>7.2} us",
            distribution.median, distribution.p99, distribution.maximum
        );
    }

    // The two pathological deltas, each measured on its own so the substep count
    // is visible in the number.
    for hitch_seconds in [0.25_f32, 1.0] {
        let mut body = spawn_character(brickmap, 500, 500, 0.35, 28.0);
        let mut samples = Vec::with_capacity(200);
        for _ in 0..200 {
            let started = Instant::now();
            body.step(brickmap, &sprinting, hitch_seconds);
            samples.push(started.elapsed().as_secs_f32() * 1e6);
        }
        let distribution = summarize_distribution(&mut samples);
        println!(
            "  {:<50} median {:>7.2} us  p99 {:>7.2} us  max {:>7.2} us",
            format!(
                "sprint + fall through a {:.0} ms hitch",
                hitch_seconds * 1000.0
            ),
            distribution.median,
            distribution.p99,
            distribution.maximum
        );
    }

    // Entering walk mode: the one-off ground search under the fly camera.
    let mut samples = Vec::with_capacity(200);
    for index in 0..200 {
        let voxel_x = 400 + (index % 100) * 2;
        let mut body = CharacterController::from_eye(
            Vec3::new(
                (voxel_x as f32 + 0.5) * VOXEL_SIZE,
                WATER_LEVEL as f32 * VOXEL_SIZE + 17.5,
                (500.0 + 0.5) * VOXEL_SIZE,
            ),
            0.0,
            0.0,
        );
        let started = Instant::now();
        let found = body.snap_to_ground(brickmap, 64.0);
        samples.push(started.elapsed().as_secs_f32() * 1e6);
        assert!(found, "the island column ({voxel_x}, 500) has no ground");
    }
    let distribution = summarize_distribution(&mut samples);
    println!(
        "  {:<50} median {:>7.2} us  p99 {:>7.2} us  max {:>7.2} us",
        "enter walk mode (ground search from ~17 m up)",
        distribution.median,
        distribution.p99,
        distribution.maximum
    );
}

/// A body standing on the terrain at a voxel column (or `height_above` meters
/// over it), facing `yaw`.
fn spawn_character(
    brickmap: &Brickmap,
    voxel_x: i32,
    voxel_z: i32,
    yaw: f32,
    height_above: f32,
) -> CharacterController {
    let surface_y = surface_voxel_y(brickmap, voxel_x, voxel_z).expect("occupied column");
    let eye = Vec3::new(
        (voxel_x as f32 + 0.5) * VOXEL_SIZE,
        (surface_y + 1) as f32 * VOXEL_SIZE + character::EYE_HEIGHT_METERS + height_above,
        (voxel_z as f32 + 0.5) * VOXEL_SIZE,
    );
    let mut body = CharacterController::from_eye(eye, yaw, 0.0);
    if height_above <= 0.0 {
        body.snap_to_ground(brickmap, 8.0);
    }
    body
}

/// The steepest +X rise the island has over 4 m, sampled on a coarse grid — the
/// worst case for the auto-step, which fires on every frame that walks into a
/// rise and costs up to four extra sweeps when it does.
fn steepest_uphill_column(brickmap: &Brickmap) -> (i32, i32) {
    let mut best = (500, 500);
    let mut best_rise = i32::MIN;
    for voxel_z in (200..800).step_by(16) {
        for voxel_x in (200..768).step_by(16) {
            let (Some(here), Some(ahead)) = (
                surface_voxel_y(brickmap, voxel_x, voxel_z),
                surface_voxel_y(brickmap, voxel_x + 32, voxel_z),
            ) else {
                continue;
            };
            let rise = ahead - here;
            if rise > best_rise {
                best_rise = rise;
                best = (voxel_x, voxel_z);
            }
        }
    }
    best
}

/// How the CAGI volume responds to an edit: E2's answer is a GLOBAL re-flood, so
/// the question is how many frames it takes to converge — measured on the volume
/// itself (bit-exact), not on the image.
fn report_edit_reflood(device: &wgpu::Device, queue: &wgpu::Queue, brickmap: &Brickmap) {
    println!();
    println!("== E2 CAGI re-flood after an edit ==");
    let quality = RenderQuality::default();
    let variant = Variant::new("edit-reflood".to_string(), quality);
    let scenario = &build_scenarios(&[])[2];
    let (width, height) = variant.resolution();
    let target = create_render_target(device, width, height);
    let mut host = WorldHost::new(brickmap.clone());
    let (world_bindings, mut resources) = {
        let world = host.read();
        let world_bindings = WorldBindings::new(device, &world);
        let resources =
            VariantResources::new(device, &world_bindings, &world, &variant, &target.view);
        (world_bindings, resources)
    };
    let light_grid = resources.light_volume.grid();

    // Converge, then build a 16x16 wall in one go and re-flood.
    resources.flood_to_convergence(device, queue, &world_bindings, &variant, scenario);
    let edits = storm_edit_list(brickmap, StormPattern::WallPlace);
    for (voxel, material) in &edits {
        host.request_edit(
            VoxelEdit {
                voxel: *voxel,
                material: *material,
                light_grid: Some(light_grid),
                material_attributes: voxel_rt::cagi::MaterialAttributes::compiled(),
            },
            &quality.world_edit,
        );
    }
    for update in host.drain() {
        if let WorldUpdate::Delta(delta) = update {
            for write in &delta.writes {
                world_bindings.apply_array_write(queue, write);
            }
            if let Some(metadata) = &delta.metadata {
                world_bindings.write_metadata(queue, metadata);
            }
            resources.light_volume.write_cell_attributes(
                queue,
                delta.light_grid,
                &delta.light_cells,
            );
        }
    }
    let lighting_uniform = scenario.lighting_uniform(&quality);

    // The converged volume the re-flood is heading toward.
    resources.light_volume.mark_dirty();
    resources.run_iterations(
        device,
        queue,
        &world_bindings,
        &lighting_uniform,
        CONVERGENCE_ITERATIONS,
    );
    let converged = read_back_volume(device, queue, &resources.light_volume);

    println!(
        "  {} edits applied (a 16x16 wall), then the volume is thrown away and re-flooded",
        edits.len()
    );
    println!(
        "  {:>10} {:>8} {:>18} {:>10}",
        "iterations", "frames", "differing cells", "%"
    );
    resources.light_volume.mark_dirty();
    // Zero iterations still has to ENCODE the clear (the pass clears lazily on its
    // next encode), or the rung-0 row would report the old converged volume.
    resources.run_iterations(device, queue, &world_bindings, &lighting_uniform, 0);
    let mut iterations_done = 0_u32;
    for rung in [0_u32, 2, 4, 8, 16, 32, 64, 128] {
        if rung > iterations_done {
            resources.run_iterations(
                device,
                queue,
                &world_bindings,
                &lighting_uniform,
                rung - iterations_done,
            );
            iterations_done = rung;
        }
        let volume = read_back_volume(device, queue, &resources.light_volume);
        let differing = volume
            .iter()
            .zip(&converged)
            .filter(|(current, target)| current != target)
            .count();
        println!(
            "  {rung:>10} {:>8.1} {differing:>18} {:>9.4}%",
            rung as f32 / quality.global_illumination.iterations_per_frame as f32,
            differing as f64 / converged.len() as f64 * 100.0,
        );
    }
}

fn report_edit_memory(device: &wgpu::Device, brickmap: &Brickmap) {
    println!();
    println!("== E2 memory ==");
    let world_bindings = WorldBindings::new(device, brickmap);
    let light_volume = LightVolume::new(
        device,
        brickmap,
        &CagiSettings::default(),
        &voxel_rt::cagi::MaterialAttributes::compiled(),
    );
    let headroom_bytes = (brickmap.brick_capacity() - brickmap.occupied_brick_count()) as usize
        * (OCCUPANCY_WORDS_PER_BRICK + MATERIAL_WORDS_PER_BRICK)
        * 4;
    println!(
        "  CPU brickmap (the authority AND the audio mirror): {:>8.1} MB",
        brickmap.cpu_bytes() as f32 / 1e6
    );
    println!(
        "  GPU world buffers:                                 {:>8.1} MB",
        world_bindings.gpu_bytes() as f32 / 1e6
    );
    println!(
        "  GPU CAGI light volume:                             {:>8.1} MB",
        light_volume.gpu_bytes() as f32 / 1e6
    );
    println!(
        "  of which edit headroom ({} spare brick slots):    {:>8.1} MB (CPU and GPU each)",
        brickmap.brick_capacity() - brickmap.occupied_brick_count(),
        headroom_bytes as f32 / 1e6
    );
    println!(
        "  {} occupied bricks, {} free slots, capacity {}",
        brickmap.occupied_brick_count(),
        brickmap.free_brick_slot_count(),
        brickmap.brick_capacity()
    );
}

/// S3: the bench ships an EMPTY world-event field, so no material sensor can
/// fire and a measured frame stays reproducible. Paired with the frozen clock
/// in `Scenario::lighting_uniform`; the two together are what the deterministic
/// animation lever does in the app.
///
/// S3b made this the CA pass's input too — with no events live, every cell's
/// event gate reads its material's RESTING emission, which is the un-animated
/// value the recorded baselines were measured against.
const BENCH_WORLD_EVENTS: [GpuWorldEvent; MAX_WORLD_EVENTS] =
    [GpuWorldEvent::INACTIVE; MAX_WORLD_EVENTS];

/// The sky radiance the CA injects — the hemisphere constants of `lighting.rs`,
/// mirrored here for the CPU cross-check's out-of-volume neighbour values.
fn scenario_sky_radiance() -> [f32; 3] {
    let quality = RenderQuality::default();
    let (animation_params, event_params) = quality.animation_params(
        AnimationClockSample::FROZEN,
        AnimationClockSample::FROZEN,
        0,
    );
    let uniform = voxel_rt::lighting::lighting_uniform(
        &SunSettings::default(),
        quality.shading_params(),
        quality.gi_params(),
        quality.water_params(),
        quality.material_params(OUTPUT_HEIGHT),
        animation_params,
        event_params,
    );
    [
        uniform.sky_ambient[0] * uniform.sky_ambient[3],
        uniform.sky_ambient[1] * uniform.sky_ambient[3],
        uniform.sky_ambient[2] * uniform.sky_ambient[3],
    ]
}

/// Read the light volume's front buffer back to the CPU.
fn read_back_volume(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    light_volume: &LightVolume,
) -> Vec<u32> {
    let size = light_volume.grid().volume_bytes() as u64;
    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench volume readback"),
        size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench volume readback encoder"),
    });
    encoder.copy_buffer_to_buffer(light_volume.front_buffer(), 0, &readback_buffer, 0, size);
    queue.submit([encoder.finish()]);
    let slice = readback_buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("volume readback map failed")
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll failed");
    let words = bytemuck::cast_slice::<u8, u32>(&slice.get_mapped_range()).to_vec();
    readback_buffer.unmap();
    words
}

// ---- Scenarios ---------------------------------------------------------------

/// The four fixed poses. Derivations (voxel-core constants, 0.125 m/voxel:
/// world 125 m x 32 m x 125 m, water plane at 10.5 m):
///
/// - Top-down: island center x/z (62.5, 62.5), 60 m altitude (nearly twice
///   the 32 m world ceiling), pitch -(PI/2 - 0.01) — straight down minus the
///   fly camera's own clamp so the basis never degenerates; yaw -PI/2.
/// - Ground: the fly camera's spawn x/z (62.5, 107.5 = 0.86 * 125 over the
///   southern rim) at eye height above the water plane (10.5 + 1.7 m), yaw
///   -PI/2 (facing -Z, across the island toward the center), pitch 0.
/// - Default sun = `SunSettings::default()` (azimuth 32.5°, elevation 50.8°,
///   the Stage 1 constant). Low sun = same azimuth, elevation 5°.
///
/// `capture_prefixes` selects which scenarios (by lowercase letter prefix)
/// get per-variant renders + PNGs + the pixel compare.
fn build_scenarios(capture_prefixes: &[char]) -> Vec<Scenario> {
    let world_x_meters = WORLD_SIZE_X as f32 * VOXEL_SIZE;
    let world_z_meters = WORLD_SIZE_Z as f32 * VOXEL_SIZE;
    let water_meters = WATER_LEVEL as f32 * VOXEL_SIZE;

    let top_down_pose = CameraPose::from_yaw_pitch(
        Vec3::new(world_x_meters * 0.5, 60.0, world_z_meters * 0.5),
        -std::f32::consts::FRAC_PI_2,
        -(std::f32::consts::FRAC_PI_2 - 0.01),
    );
    let ground_pose = CameraPose::from_yaw_pitch(
        Vec3::new(
            world_x_meters * 0.5,
            water_meters + 1.7,
            world_z_meters * 0.86,
        ),
        -std::f32::consts::FRAC_PI_2,
        0.0,
    );

    let default_sun = SunSettings::default();
    let low_sun = SunSettings {
        elevation_degrees: 5.0,
        ..default_sun
    };

    let scenarios = vec![
        Scenario {
            label: "A top-down, default sun",
            pose: top_down_pose,
            sun: default_sun,
            capture_image: false,
        },
        Scenario {
            label: "B top-down, low sun 5 deg",
            pose: top_down_pose,
            sun: low_sun,
            capture_image: false,
        },
        Scenario {
            label: "C ground, default sun",
            pose: ground_pose,
            sun: default_sun,
            capture_image: false,
        },
        Scenario {
            label: "D ground, low sun 5 deg",
            pose: ground_pose,
            sun: low_sun,
            capture_image: false,
        },
    ];
    scenarios
        .into_iter()
        .map(|mut scenario| {
            let prefix = scenario
                .label
                .chars()
                .next()
                .expect("scenario label has a letter prefix")
                .to_ascii_lowercase();
            scenario.capture_image = capture_prefixes.contains(&prefix);
            scenario
        })
        .collect()
}

// ---- Measurement -------------------------------------------------------------

/// A storage texture the DDA pass writes into, plus its size. One per DISTINCT
/// resolution in a section (the preset section has three).
struct RenderTarget {
    texture: wgpu::Texture,
    view: wgpu::TextureView,
    width: u32,
    height: u32,
}

impl RenderTarget {
    /// Allocated bytes, from the texture's own format rather than an assumed
    /// bytes-per-pixel — the bench and the app can render at different depths.
    fn bytes(&self) -> u64 {
        let bytes_per_pixel = self.texture.format().block_copy_size(None).unwrap_or(0);
        u64::from(self.width) * u64::from(self.height) * u64::from(bytes_per_pixel)
    }
}

fn create_render_target(device: &wgpu::Device, width: u32, height: u32) -> RenderTarget {
    let texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench output texture"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let view = texture.create_view(&wgpu::TextureViewDescriptor::default());
    RenderTarget {
        texture,
        view,
        width,
        height,
    }
}

/// Measure one section: every variant on every scenario, interleaved. Returns
/// the timing table (`table[variant][scenario] = (median, p95)`).
///
/// TIMING FIRST, image capture after — every scenario is timed before any
/// readback or PNG encode happens. Interleaving them (encoding ten 2560x1440
/// PNGs between two timed scenarios) measurably inflated the following
/// scenario: the CPU burst leaves the SoC clocked for a different workload, and
/// the E1b run showed p95 up to 25 ms on scenarios that follow a capture while
/// scenario A (nothing before it) stayed tight. The capture loop then holds
/// only ONE scenario's frames at a time — a dozen variants x four scenarios of
/// RGBA frames would otherwise be gigabytes resident.
fn measure_section(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world_bindings: &WorldBindings,
    brickmap: &Brickmap,
    section: &Section,
) -> (TimingTable, Vec<VariantFootprint>) {
    // One render target per distinct resolution; every variant binds the target
    // its render scale asks for.
    let mut targets: Vec<RenderTarget> = Vec::new();
    let mut target_of_variant: Vec<usize> = Vec::new();
    for variant in &section.variants {
        let (width, height) = variant.resolution();
        let target_index = targets
            .iter()
            .position(|target| target.width == width && target.height == height)
            .unwrap_or_else(|| {
                targets.push(create_render_target(device, width, height));
                targets.len() - 1
            });
        target_of_variant.push(target_index);
    }

    let mut variant_resources: Vec<VariantResources> = section
        .variants
        .iter()
        .zip(&target_of_variant)
        .map(|(variant, target_index)| {
            VariantResources::new(
                device,
                world_bindings,
                brickmap,
                variant,
                &targets[*target_index].view,
            )
        })
        .collect();

    let mut table: TimingTable = vec![Vec::new(); section.variants.len()];
    let measures_light_volume = section
        .variants
        .iter()
        .any(|variant| variant.quality.global_illumination.enabled);
    let mut cagi_table: TimingTable = vec![Vec::new(); section.variants.len()];

    // Variants are INTERLEAVED round-robin within each scenario so that GPU
    // clock/thermal drift over the run hits every variant equally — timing
    // them in sequential blocks showed up to ~10% cross-run drift on
    // identical shaders, the same order as the effects being measured.
    for scenario in &section.scenarios {
        // E4: every scenario has its own sun, so every light volume is re-flooded
        // to convergence for it BEFORE the shading pass is timed. Timing a
        // half-flooded volume would measure a different (mostly empty) memory
        // access pattern than the app's steady state.
        for (variant_index, resources) in variant_resources.iter_mut().enumerate() {
            resources.flood_to_convergence(
                device,
                queue,
                world_bindings,
                &section.variants[variant_index],
                scenario,
            );
        }
        let mut samples: Vec<Vec<f32>> =
            vec![Vec::with_capacity(BATCH_COUNT); variant_resources.len()];
        let mut cagi_samples: Vec<Vec<f32>> =
            vec![Vec::with_capacity(BATCH_COUNT); variant_resources.len()];
        for (variant_index, resources) in variant_resources.iter().enumerate() {
            for _ in 0..WARMUP_BATCHES {
                let variant = &section.variants[variant_index];
                time_one_batch(
                    device,
                    queue,
                    world_bindings,
                    resources,
                    variant,
                    scenario,
                    &scenario.lighting_uniform(&variant.quality),
                );
            }
        }
        for round in 0..BATCH_COUNT {
            // Rotate the starting variant each round: the slot a batch
            // occupies within a round measurably biases its timing (the
            // preceding batch's duration shapes the GPU clock state it
            // inherits), so every variant must sample every slot equally.
            for offset in 0..variant_resources.len() {
                let variant_index = (round + offset) % variant_resources.len();
                let variant = &section.variants[variant_index];
                samples[variant_index].push(time_one_batch(
                    device,
                    queue,
                    world_bindings,
                    &variant_resources[variant_index],
                    variant,
                    scenario,
                    &scenario.lighting_uniform(&variant.quality),
                ));
                if measures_light_volume {
                    cagi_samples[variant_index].push(time_one_light_volume_frame(
                        device,
                        queue,
                        world_bindings,
                        &mut variant_resources[variant_index],
                        &section.variants[variant_index],
                        scenario,
                    ));
                }
            }
        }
        for variant_index in 0..variant_resources.len() {
            let (median_milliseconds, p95_milliseconds) = summarize(&mut samples[variant_index]);
            println!(
                "{:<24} {:<28} median {:>7.3} ms   p95 {:>7.3} ms",
                section.variants[variant_index].label,
                scenario.label,
                median_milliseconds,
                p95_milliseconds
            );
            table[variant_index].push((median_milliseconds, p95_milliseconds));
            if measures_light_volume {
                cagi_table[variant_index].push(summarize(&mut cagi_samples[variant_index]));
            }
        }
    }

    for scenario in section
        .scenarios
        .iter()
        .filter(|scenario| scenario.capture_image)
    {
        let scenario_images: Vec<Vec<u8>> = variant_resources
            .iter_mut()
            .zip(&section.variants)
            .zip(&target_of_variant)
            .map(|((resources, variant), target_index)| {
                // Re-flood for THIS scenario's sun (the timing loop left the last
                // scenario's flood in place), then one un-timed dispatch so the
                // readback sees this variant's frame.
                resources.flood_to_convergence(device, queue, world_bindings, variant, scenario);
                let target = &targets[*target_index];
                render_once(device, queue, world_bindings, resources, variant, scenario);
                read_back_image(device, queue, target)
            })
            .collect();
        write_scenario_pngs(section, scenario, &scenario_images);
        compare_scenario_images(section, scenario, &scenario_images);
    }

    if measures_light_volume {
        print_light_volume_table(section, &cagi_table);
    }

    // Memory is read from the resources this section actually built, so the
    // footprint reported is the one that produced the timings above it.
    let footprints = section
        .variants
        .iter()
        .zip(&target_of_variant)
        .zip(&variant_resources)
        .map(|((variant, target_index), resources)| VariantFootprint {
            target_bytes: targets[*target_index].bytes(),
            light_volume_bytes: resources.light_volume.gpu_bytes(),
            _label: variant.label.clone(),
        })
        .collect();
    (table, footprints)
}

/// GPU bytes one variant holds, alongside its timings.
///
/// A variant's cost is not only time: a render-scale lever trades pixels for
/// memory, and a GI lever allocates a whole light volume. Reporting both in the
/// same section is what makes a Quest verdict judgeable — the headset can be short
/// of memory before it is short of milliseconds.
struct VariantFootprint {
    /// The storage/render target at this variant's resolution.
    target_bytes: u64,
    /// This variant's CAGI light volume: ping-pong buffers plus attributes.
    light_volume_bytes: u64,
    /// Kept for debugging a mismatched column order; the table takes its headings
    /// from `section.variants` like every other table here.
    _label: String,
}

/// Everything one variant needs on the GPU: the E4 light volume it samples, the
/// CA pass that floods it, and the shading pass itself.
struct VariantResources {
    light_volume: LightVolume,
    cagi_pass: CagiPass,
    dda_pass: DdaPass,
}

impl VariantResources {
    fn new(
        device: &wgpu::Device,
        world_bindings: &WorldBindings,
        brickmap: &Brickmap,
        variant: &Variant,
        output_view: &wgpu::TextureView,
    ) -> VariantResources {
        let light_volume = LightVolume::new(
            device,
            brickmap,
            &variant.quality.global_illumination,
            &voxel_rt::cagi::MaterialAttributes::compiled(),
        );
        VariantResources {
            cagi_pass: CagiPass::new_with_program(
                device,
                world_bindings,
                &light_volume,
                &variant.cagi_program(),
            ),
            dda_pass: DdaPass::new_with_program(
                device,
                world_bindings,
                &light_volume,
                output_view,
                &variant.dda_program(),
                // The bench measures the shipped 8-bit output path; output depth is
                // a display property and orthogonal to everything it sweeps.
                voxel_color::OutputFormat::default(),
            ),
            light_volume,
        }
    }

    /// Throw the volume away and flood it to convergence for `scenario`'s sun.
    /// [`CONVERGENCE_ITERATIONS`] is well past the point where the island's image
    /// stops changing (measured in the E4 convergence table).
    fn flood_to_convergence(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world_bindings: &WorldBindings,
        variant: &Variant,
        scenario: &Scenario,
    ) {
        if !variant.quality.global_illumination.enabled {
            return;
        }
        self.light_volume.mark_dirty();
        self.run_iterations(
            device,
            queue,
            world_bindings,
            &scenario.lighting_uniform(&variant.quality),
            CONVERGENCE_ITERATIONS,
        );
    }

    /// Encode `iterations` CA steps in one submit and block until they finish.
    fn run_iterations(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        world_bindings: &WorldBindings,
        lighting_uniform: &LightingUniform,
        iterations: u32,
    ) {
        world_bindings.write_lighting(queue, lighting_uniform);
        let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("bench cagi flood"),
        });
        self.cagi_pass
            .encode(&mut encoder, &mut self.light_volume, iterations, None);
        queue.submit([encoder.finish()]);
        device
            .poll(wgpu::PollType::Wait {
                submission_index: None,
                timeout: None,
            })
            .expect("device poll failed");
    }
}

// ---- GPU plumbing ------------------------------------------------------------

fn create_headless_device() -> (wgpu::Device, wgpu::Queue) {
    let instance = wgpu::Instance::default();
    let adapter = pollster::block_on(instance.request_adapter(&wgpu::RequestAdapterOptions {
        power_preference: wgpu::PowerPreference::HighPerformance,
        compatible_surface: None,
        force_fallback_adapter: false,
    }))
    .expect("no GPU adapter — the benchmark needs real hardware");
    let (device, queue) =
        pollster::block_on(adapter.request_device(&voxel_rt::gpu::device_descriptor(&adapter)))
            .expect("device creation failed");
    // Surface validation/device errors — without a handler wgpu only routes
    // them through `log`, and a silent device loss shows up here as a
    // baffling "no timestamps" panic instead of the real cause.
    device.on_uncaptured_error(std::sync::Arc::new(|error: wgpu::Error| {
        eprintln!("wgpu uncaptured error: {error}");
        // ALSO on stdout, and counted. A validation error does not stop the run:
        // the invalid bind group's dispatches are dropped and the section still
        // prints a timing table, so the column reads as very FAST rather than as
        // broken. That is how `gi-cells2` came back at 0.005 ms — a finer light
        // volume apparently 700x quicker than a coarser one — while stderr, which
        // nobody pastes into a document, held the reason. The numbers and the
        // warning have to travel together.
        if GPU_ERRORS.fetch_add(1, Ordering::Relaxed) == 0 {
            println!();
            println!("!! GPU VALIDATION ERROR — timings below this line are NOT TRUSTWORTHY");
            println!("!! {error}");
            println!();
        }
    }));
    println!("adapter: {}", adapter.get_info().name);
    (device, queue)
}

/// One timed batch: [`DISPATCHES_PER_BATCH`] back-to-back encodes of the
/// pass in a single command buffer, one submit, blocked to completion;
/// returns wall-clock milliseconds per dispatch.
///
/// Wall-clock instead of GPU timestamp spans: Metal (M3 Max) resolves
/// pass-boundary counter samples to all zeros as soon as a command buffer
/// holds more than one compute pass, and at 25 dispatches per submit the
/// fixed submit/poll overhead disappears into the average anyway — the
/// number matches what continuous app rendering pays per frame.
fn time_one_batch(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world_bindings: &WorldBindings,
    resources: &VariantResources,
    variant: &Variant,
    scenario: &Scenario,
    lighting_uniform: &LightingUniform,
) -> f32 {
    let (width, height) = variant.resolution();
    let camera_uniform = scenario.camera_uniform((width, height));
    // The lighting uniform is shared by both passes now, and variants differ in
    // their RUNTIME knobs (AO strength, fade ramp, GI strength), so it is written
    // per batch — one batch is one (variant, scenario).
    //
    // It arrives as an ARGUMENT rather than being derived from `(scenario, variant)`
    // here, because section 14 sweeps a lever that lives in the uniform and nowhere
    // else: the tonemap curve. Every other caller passes exactly the derivation this
    // line used to perform.
    world_bindings.write_lighting(queue, lighting_uniform);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench batch"),
    });
    for _ in 0..DISPATCHES_PER_BATCH {
        resources.dda_pass.encode(
            queue,
            &mut encoder,
            &camera_uniform,
            resources.light_volume.front(),
            width,
            height,
            None,
        );
    }
    let started = Instant::now();
    queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll failed");
    started.elapsed().as_secs_f32() * 1000.0 / DISPATCHES_PER_BATCH as f32
}

/// One FRAME's worth of CA iterations (`iterations_per_frame`), timed the same
/// wall-clock way — this is what the light volume adds to a frame at steady state,
/// on top of the shading pass's own table.
///
/// [`DISPATCHES_PER_BATCH`] frames go into one command buffer so the measurement
/// amortizes submit overhead exactly like the shading table does. The volume is
/// already converged, so these iterations reproduce the app's steady state (pinned
/// sun sources short-circuit, nothing re-traces).
fn time_one_light_volume_frame(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world_bindings: &WorldBindings,
    resources: &mut VariantResources,
    variant: &Variant,
    scenario: &Scenario,
) -> f32 {
    world_bindings.write_lighting(queue, &scenario.lighting_uniform(&variant.quality));
    let iterations = variant.quality.gi_iterations_per_frame();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench cagi batch"),
    });
    for _ in 0..DISPATCHES_PER_BATCH {
        resources
            .cagi_pass
            .encode(&mut encoder, &mut resources.light_volume, iterations, None);
    }
    let started = Instant::now();
    queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll failed");
    started.elapsed().as_secs_f32() * 1000.0 / DISPATCHES_PER_BATCH as f32
}

/// One un-timed dispatch so the output texture holds THIS variant's frame
/// before an image readback.
fn render_once(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    world_bindings: &WorldBindings,
    resources: &VariantResources,
    variant: &Variant,
    scenario: &Scenario,
) {
    world_bindings.write_lighting(queue, &scenario.lighting_uniform(&variant.quality));
    let (width, height) = variant.resolution();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench verify frame"),
    });
    resources.dda_pass.encode(
        queue,
        &mut encoder,
        &scenario.camera_uniform((width, height)),
        resources.light_volume.front(),
        width,
        height,
        None,
    );
    queue.submit([encoder.finish()]);
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll failed");
}

/// Sort the samples and return (median, p95) milliseconds.
fn summarize(samples: &mut [f32]) -> (f32, f32) {
    samples.sort_by(|a, b| a.partial_cmp(b).expect("NaN timing sample"));
    let median = samples[samples.len() / 2];
    let p95_index = ((samples.len() as f32 * 0.95) as usize).min(samples.len() - 1);
    let p95 = samples[p95_index];
    (median, p95)
}

/// Copy a render target to the CPU as tightly-packed RGBA bytes. Every
/// resolution the harness uses has a 256-byte-aligned row (2560/2048/1792 px x
/// 4 B), so the copy needs no row padding — asserted, because a preset with an
/// unaligned scale would silently read back garbage.
fn read_back_image(device: &wgpu::Device, queue: &wgpu::Queue, target: &RenderTarget) -> Vec<u8> {
    let bytes_per_row = target.width * 4;
    assert_eq!(
        bytes_per_row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT,
        0,
        "render width {} needs row padding for readback",
        target.width
    );
    let buffer_size = u64::from(bytes_per_row) * u64::from(target.height);
    let readback_buffer = device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("bench image readback"),
        size: buffer_size,
        usage: wgpu::BufferUsages::COPY_DST | wgpu::BufferUsages::MAP_READ,
        mapped_at_creation: false,
    });

    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench image readback encoder"),
    });
    encoder.copy_texture_to_buffer(
        target.texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: target.width,
            height: target.height,
            depth_or_array_layers: 1,
        },
    );
    queue.submit([encoder.finish()]);

    let slice = readback_buffer.slice(..);
    slice.map_async(wgpu::MapMode::Read, |result| {
        result.expect("image readback map failed")
    });
    device
        .poll(wgpu::PollType::Wait {
            submission_index: None,
            timeout: None,
        })
        .expect("device poll failed");
    let bytes = slice.get_mapped_range().to_vec();
    readback_buffer.unmap();
    bytes
}

// ---- Reporting ---------------------------------------------------------------

fn print_table(section: &Section, table: &[Vec<(f32, f32)>]) {
    println!();
    println!(
        "DDA pass, per-dispatch median / p95 ms over {} batches x {} dispatches \
         (base resolution {}x{}):",
        BATCH_COUNT, DISPATCHES_PER_BATCH, OUTPUT_WIDTH, OUTPUT_HEIGHT
    );
    print!("{:<28}", "scenario");
    for variant in &section.variants {
        print!(" | {:>23}", variant.label);
    }
    println!();
    for (scenario_index, scenario) in section.scenarios.iter().enumerate() {
        print!("{:<28}", scenario.label);
        for variant_row in table {
            let (median, p95) = variant_row[scenario_index];
            print!(" | {:>10.3} / {:>10.3}", median, p95);
        }
        println!();
    }
    println!();
}

/// Per-variant GPU memory, in the same column layout as [`print_table`].
///
/// Byte formatting comes from `atrium_profile::memory` — the SAME formatter the
/// live P panel uses, so a number read on screen and a number read from a gate run
/// are directly comparable rather than merely similar.
///
/// The world buffers are shared by every variant in a section, so they are printed
/// once below the table instead of repeated across every column.
fn print_memory_table(
    section: &Section,
    footprints: &[VariantFootprint],
    world_gpu_bytes: u64,
    world_cpu_bytes: u64,
) {
    use atrium_profile::memory::format_bytes;

    println!("GPU memory per variant (shared world buffers listed once below):");
    print!("{:<28}", "category");
    for variant in &section.variants {
        print!(" | {:>23}", variant.label);
    }
    println!();

    /// A named reader over one footprint field — the table's row definition.
    type MemoryRowReader = (&'static str, fn(&VariantFootprint) -> u64);

    let rows: [MemoryRowReader; 3] = [
        ("render target", |footprint| footprint.target_bytes),
        ("light volume (CAGI)", |footprint| {
            footprint.light_volume_bytes
        }),
        ("per-variant total", |footprint| {
            footprint.target_bytes + footprint.light_volume_bytes
        }),
    ];
    for (label, read) in rows {
        print!("{label:<28}");
        for footprint in footprints {
            print!(" | {:>23}", format_bytes(read(footprint)));
        }
        println!();
    }
    println!(
        "{:<28} | {} GPU, {} CPU",
        "world (shared)",
        format_bytes(world_gpu_bytes),
        format_bytes(world_cpu_bytes),
    );
    println!();
}

/// Lowercase letter slug of a scenario (`"A top-down, ..."` -> `"a"`).
fn scenario_slug(scenario: &Scenario) -> String {
    scenario
        .label
        .split(' ')
        .next()
        .expect("scenario label has a letter prefix")
        .to_lowercase()
}

/// Filename-safe slug of a variant label (`"Quest @2048x1152"` ->
/// `"quest_2048x1152"`).
fn variant_slug(variant: &Variant) -> String {
    variant
        .label
        .chars()
        .map(|character| match character {
            'A'..='Z' => character.to_ascii_lowercase(),
            'a'..='z' | '0'..='9' => character,
            _ => '_',
        })
        .collect()
}

/// The E4 compromise-checklist crops, in 2560x1440 render pixels. Each targets one
/// symptom from the dossier's "known compromises" list; the findings are recorded
/// in the bench doc's E4 section.
const CAGI_CROP_REGIONS: &[(&str, u32, u32, u32, u32)] = &[
    // Ground straight ahead of the camera: over-diffusion / glowing surfaces and
    // the volume's cell banding show here first.
    ("near-ground", 1024, 1024, 320, 180),
    // Mid-distance terrain + tree bases: anisotropy (axis-aligned fronts) and the
    // column-max sky test's tree-column artifact.
    ("mid-terrain", 1120, 640, 320, 180),
    // Canopy undersides and shadowed slopes: thin-geometry leaks and how far light
    // travels into cover (long-distance transport).
    ("canopy-shade", 1600, 480, 320, 180),
];

/// The L0 corridor crops, in 2560x1440 render pixels. Each one is a *claim*
/// about indirect light rather than a region of interest, which is the point of
/// an authored fixture: the crop can be aimed at a surface whose correct
/// appearance is known.
const LIGHT_CORRIDOR_CROP_REGIONS: &[(&str, u32, u32, u32, u32)] = &[
    // Upper middle of the frame: the white ceiling receding down the corridor.
    // It receives ZERO direct light, so every photon here bounced at least once
    // and the band colours are the evidence that the bounce carried albedo.
    // Grey here = the albedo bug.
    ("ceiling-bleed", 1120, 300, 320, 180),
    // Screen centre on the near poses: the first band boundary on the floor and
    // walls. Adjacent bands differ in exactly one RGB channel, so a per-channel
    // transport error reads as a hue break across this crop.
    ("band-boundary", 1120, 700, 320, 180),
    // Low and far: the end of the corridor, lit only by light that has travelled
    // the room's whole length. This is where CAGI's ~1-cell-per-tick speed of
    // light and its subtractive decay show up as a falloff, and where an
    // under-converged volume goes black.
    ("far-falloff", 1120, 980, 320, 180),
];

/// The E6 water crops, in 2560x1440 render pixels. Each targets one claim of the
/// E6 gate, so the PNGs are the evidence rather than an illustration.
const WATER_CROP_REGIONS: &[(&str, u32, u32, u32, u32)] = &[
    // Screen centre: the pool ahead on the shore scenarios, and the middle of
    // Snell's window on the underwater ones — the bright cone and, past the
    // critical angle, its rim.
    ("snells-window", 1120, 630, 320, 180),
    // The lower third: where a grazing water surface is closest to the camera, so
    // the mirror term and the depth gradient of the bed are both largest.
    ("water-near", 1120, 1080, 320, 180),
    // Upper-left of the water: the far side of a body of water, where extinction
    // has had the longest path to work over.
    ("water-far", 640, 560, 320, 180),
];

/// Zoom of the crops — nearest neighbour, so nothing is smoothed away.
const CROP_ZOOM: u32 = 3;

/// Write one scenario's per-variant renders to `target/bench_dda/` as
/// `scenario_{letter}_{variant}.png`, plus this section's zoomed crops.
fn write_scenario_pngs(section: &Section, scenario: &Scenario, images: &[Vec<u8>]) {
    let output_directory = std::path::Path::new("target/bench_dda");
    std::fs::create_dir_all(output_directory).expect("failed to create target/bench_dda");
    let slug = scenario_slug(scenario);
    for (variant, image) in section.variants.iter().zip(images) {
        let path = output_directory.join(format!("scenario_{slug}_{}.png", variant_slug(variant)));
        let (width, height) = variant.resolution();
        write_png(&path, image, width, height);
        for (crop_name, crop_x, crop_y, crop_width, crop_height) in section.crop_regions {
            if crop_x + crop_width > width || crop_y + crop_height > height {
                continue; // a lower-resolution tier cannot hold this crop
            }
            write_crop_png(
                &output_directory.join(format!(
                    "crop_{crop_name}_{slug}_{}.png",
                    variant_slug(variant)
                )),
                image,
                width,
                (*crop_x, *crop_y, *crop_width, *crop_height),
                CROP_ZOOM,
            );
        }
    }
    println!(
        "  PNGs for {} written to {}",
        scenario.label,
        output_directory.display()
    );
}

/// Pixel-compare one scenario's variant renders against the section's
/// reference. For the traversal section this is the shadow-correctness gate for
/// the column fast-forward (rays above a column max heading toward taller
/// terrain ahead must still occlude — long low-sun shadows must survive); for
/// the AO / E1b / preset sections it reports how much of the frame the variant
/// touches (the images differ by design). Variants rendering at another
/// resolution than the reference are skipped — there is no pixel
/// correspondence.
fn compare_scenario_images(section: &Section, scenario: &Scenario, images: &[Vec<u8>]) {
    let reference_index = section.reference_index();
    let reference_variant = &section.variants[reference_index];
    let reference_image = &images[reference_index];
    let reference_resolution = reference_variant.resolution();
    println!(
        "{}, {} (vs {}):",
        section.compare_heading, scenario.label, reference_variant.label
    );
    for (variant_index, variant) in section.variants.iter().enumerate() {
        if variant_index == reference_index {
            continue;
        }
        if variant.resolution() != reference_resolution {
            println!(
                "  {:<24} rendered at {:?} — no pixel compare against {:?}",
                variant.label,
                variant.resolution(),
                reference_resolution
            );
            continue;
        }
        let (differing_pixels, max_channel_delta) =
            compare_images(&images[variant_index], reference_image);
        let (width, height) = reference_resolution;
        let total_pixels = u64::from(width) * u64::from(height);
        println!(
            "  {:<24} differing pixels: {differing_pixels} / {total_pixels} \
             ({:.4}%), max channel delta {max_channel_delta}",
            variant.label,
            differing_pixels as f64 / total_pixels as f64 * 100.0,
        );
    }
}

/// Differing pixels and the largest channel delta between two RGBA images of the
/// same size.
fn compare_images(image: &[u8], reference_image: &[u8]) -> (u64, u8) {
    let mut differing_pixels = 0_u64;
    let mut max_channel_delta = 0_u8;
    for (pixel_bytes, reference_bytes) in image.chunks_exact(4).zip(reference_image.chunks_exact(4))
    {
        if pixel_bytes != reference_bytes {
            differing_pixels += 1;
            for channel in 0..4 {
                let delta = pixel_bytes[channel].abs_diff(reference_bytes[channel]);
                max_channel_delta = max_channel_delta.max(delta);
            }
        }
    }
    (differing_pixels, max_channel_delta)
}

/// The E4 second table: what the light volume's own pass costs per FRAME
/// (`iterations_per_frame` CA steps over a converged volume), next to the shading
/// pass's numbers. Printed only for sections whose variants have CAGI on — the
/// two passes are timed independently (isolation rule), and the frame total is
/// their sum.
fn print_light_volume_table(section: &Section, cagi_table: &[Vec<(f32, f32)>]) {
    println!();
    println!(
        "CAGI pass, per-FRAME median / p95 ms (iterations_per_frame CA steps over a \
         converged volume):"
    );
    print!("{:<28}", "scenario");
    for variant in &section.variants {
        print!(" | {:>23}", variant.label);
    }
    println!();
    for (scenario_index, scenario) in section.scenarios.iter().enumerate() {
        print!("{:<28}", scenario.label);
        for variant_row in cagi_table {
            match variant_row.get(scenario_index) {
                Some((median, p95)) => print!(" | {median:>10.3} / {p95:>10.3}"),
                None => print!(" | {:>23}", "-"),
            }
        }
        println!();
    }
    print!("{:<28}", "cell voxels x iterations");
    for variant in &section.variants {
        let configuration = if variant.quality.global_illumination.enabled {
            format!(
                "{} vox x {} it",
                variant.quality.global_illumination.cell_voxels,
                variant.quality.gi_iterations_per_frame()
            )
        } else {
            "off".to_string()
        };
        print!(" | {configuration:>23}");
    }
    println!();
    println!();
}

/// Write a zoomed crop of an image — the compromise-checklist evidence. Nearest
/// neighbour on purpose: the artifacts being judged (axis-aligned fronts, cell
/// banding, leaks) must not be smoothed by the zoom.
fn write_crop_png(
    path: &std::path::Path,
    rgba_bytes: &[u8],
    image_width: u32,
    crop: (u32, u32, u32, u32),
    zoom: u32,
) {
    let (crop_x, crop_y, crop_width, crop_height) = crop;
    let mut pixels = Vec::with_capacity((crop_width * zoom * crop_height * zoom * 4) as usize);
    for row in 0..crop_height * zoom {
        for column in 0..crop_width * zoom {
            let source_x = crop_x + column / zoom;
            let source_y = crop_y + row / zoom;
            let offset = ((source_y * image_width + source_x) * 4) as usize;
            pixels.extend_from_slice(&rgba_bytes[offset..offset + 4]);
        }
    }
    write_png(path, &pixels, crop_width * zoom, crop_height * zoom);
}

fn write_png(path: &std::path::Path, rgba_bytes: &[u8], width: u32, height: u32) {
    let file = std::fs::File::create(path).expect("failed to create PNG file");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), width, height);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("PNG header write failed");
    writer
        .write_image_data(rgba_bytes)
        .expect("PNG data write failed");
}

/// A centred square crop of a render, box-downsampled to [`SWATCH_THUMBNAIL`] —
/// what a side-by-side comparison sheet can actually hold.
///
/// Box-filtered rather than point-sampled, and that matters here specifically: these
/// patterns are piecewise constant on a texel grid, and point-sampling a grid at a
/// non-integer ratio produces moire that looks like a property of the generator
/// rather than of the downsample.
const SWATCH_THUMBNAIL: u32 = 220;

fn write_downsampled_crop(path: &std::path::Path, rgba_bytes: &[u8], width: u32, height: u32) {
    let side = height.min(width);
    let origin_x = (width - side) / 2;
    let origin_y = (height - side) / 2;
    let target = SWATCH_THUMBNAIL.min(side);
    let mut out = vec![0u8; (target * target * 4) as usize];
    for out_y in 0..target {
        for out_x in 0..target {
            // The source box this output texel averages.
            let x0 = origin_x + out_x * side / target;
            let x1 = (origin_x + (out_x + 1) * side / target).max(x0 + 1);
            let y0 = origin_y + out_y * side / target;
            let y1 = (origin_y + (out_y + 1) * side / target).max(y0 + 1);
            let mut sums = [0u32; 4];
            let mut count = 0u32;
            for y in y0..y1.min(height) {
                for x in x0..x1.min(width) {
                    let index = ((y * width + x) * 4) as usize;
                    for channel in 0..4 {
                        sums[channel] += rgba_bytes[index + channel] as u32;
                    }
                    count += 1;
                }
            }
            let out_index = ((out_y * target + out_x) * 4) as usize;
            for channel in 0..4 {
                out[out_index + channel] = (sums[channel] / count.max(1)) as u8;
            }
        }
    }
    write_png(path, &out, target, target);
}
