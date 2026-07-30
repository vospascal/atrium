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
//! Four sections, each its own variant table (isolation rule):
//!
//! 1. **Traversal levers, AO off** — the Stage 2 regression gate. Every column
//!    has `AO_MODE = AO_MODE_OFF` so the medians stay comparable with the
//!    recorded pre-E1 baseline. Correctness evidence: the low-sun scenarios
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
//! All PNGs land in `target/bench_dda/`.

use std::time::Instant;

use voxel_rt::ao::{AoMode, AoSettings};
use voxel_rt::brickmap::Brickmap;
use voxel_rt::camera::{CameraPose, CameraUniform, DEFAULT_VERTICAL_FOV_RADIANS};
use voxel_rt::lighting::{LightingUniform, SunSettings};
use voxel_rt::passes::dda::{build_shader_source, DdaPass, SHADER_SOURCE};
use voxel_rt::variants::{
    bench_points_of, BenchSection, LeverId, LeverValue, QualityPreset, RenderQuality,
    QUALITY_PRESETS,
};

use glam::Vec3;
use voxel_core::world::{VoxelWorld, VOXEL_SIZE, WATER_LEVEL, WORLD_SIZE_X, WORLD_SIZE_Z};

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
    /// penumbra scale, fade ramp) — the levers that need no pipeline rebuild
    /// are swept exactly the way the app applies them.
    fn lighting_uniform(&self, quality: &RenderQuality) -> LightingUniform {
        self.sun.lighting_uniform(quality.shading_params())
    }
}

/// One shader build to measure: a full [`RenderQuality`] (so the runtime knobs
/// and the render scale ride along, not just the compile-time consts) plus the
/// shader source it compiles to.
struct Variant {
    label: String,
    quality: RenderQuality,
    shader_source: String,
}

impl Variant {
    /// The normal case: the shader source IS what this quality compiles to.
    fn new(label: String, quality: RenderQuality) -> Variant {
        Variant {
            shader_source: build_shader_source(&quality),
            label,
            quality,
        }
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

fn main() {
    // Optional section filter: `-- 1 3` runs sections 1 and 3 only. No
    // argument = the full run (the documented default). Sections are
    // independent by the isolation rule, so running one in isolation is
    // exactly equivalent to reading its rows out of a full run — and it keeps
    // a single section inside a shell timeout.
    let selected_sections: Vec<usize> = std::env::args()
        .skip(1)
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
    let brickmap = Brickmap::build(&world);
    println!(
        "world + brickmap ready in {:.2?} ({} occupied bricks)",
        world_start.elapsed(),
        brickmap.occupied_brick_count()
    );

    let (device, queue) = create_headless_device();

    if runs_section(1) {
        run_section(&device, &queue, &brickmap, traversal_section());
    }
    if runs_section(2) {
        run_section(&device, &queue, &brickmap, ray_traced_ao_section());
    }
    if runs_section(3) {
        run_section(&device, &queue, &brickmap, cheap_occlusion_section());
    }
    if runs_section(4) {
        report_preset_pipeline_cache(&device, &brickmap);
        run_section(&device, &queue, &brickmap, preset_section());
    }
}

fn run_section(device: &wgpu::Device, queue: &wgpu::Queue, brickmap: &Brickmap, section: Section) {
    println!();
    println!("== {} ==", section.heading);
    let table = measure_section(device, queue, brickmap, &section);
    print_table(&section, &table);
}

// ---- Sections ----------------------------------------------------------------

/// Section 1: traversal levers around the shipped defaults, ALL with AO forced
/// off so the table stays comparable with the recorded pre-E1 baseline. The
/// per-lever columns come from the registry; the anchors are `current` (the
/// shipped shader with AO off) and `stage2-baseline` (every traversal aid off),
/// which doubles as the pixel-compare reference.
fn traversal_section() -> Section {
    let baseline = ao_off(RenderQuality::default());
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
    }
}

/// Section 2: E1's ray-traced-AO ladder. Baseline = the GRID CENTER (2 rays,
/// 16 voxels, cosine-weighted, distance falloff); the registry's one-factor
/// columns vary a single knob around it. Anchors: the center itself and the
/// cheap-combo interaction the one-factor grid misses (fewest rays x shortest
/// distance).
fn ray_traced_ao_section() -> Section {
    let center = RenderQuality {
        ambient_occlusion: AoSettings {
            mode: AoMode::RayTraced,
            max_distance_voxels: 16,
            ..AoSettings::default()
        },
        ..RenderQuality::default()
    };
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
    }
}

/// Section 3: E1b's cheap-occlusion / soft-shadow shootout. Baseline = E1's
/// shipped default (2 rays / 8 voxels / cosine / falloff), which is also the
/// row every cheap contender is judged against. Anchors: that baseline, and
/// E1c's const-vs-uniform A/B for the fade distances.
fn cheap_occlusion_section() -> Section {
    let e1_default = RenderQuality {
        ambient_occlusion: AoSettings {
            mode: AoMode::RayTraced,
            ..AoSettings::default()
        },
        ..RenderQuality::default()
    };
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
    }
}

/// AO forced off — spelled once, used by section 1.
fn ao_off(mut quality: RenderQuality) -> RenderQuality {
    quality.ambient_occlusion.mode = AoMode::Off;
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
        shader_source: shader_source.replacen(uniform_read, folded_literals, 1),
    }
}

/// Startup cost of the preset pipeline cache: what the app pays in
/// `AppState::new` so that switching preset in-app is a hash lookup instead of
/// a shader compile.
fn report_preset_pipeline_cache(device: &wgpu::Device, brickmap: &Brickmap) {
    let target = create_render_target(device, OUTPUT_WIDTH, OUTPUT_HEIGHT);
    let mut pass = DdaPass::new(device, brickmap, &target.view);
    println!();
    println!("== preset pipeline cache ==");
    let mut shader_sources = Vec::new();
    for spec in QUALITY_PRESETS
        .iter()
        .filter(|spec| spec.preset != QualityPreset::Custom)
    {
        let shader_source = build_shader_source(&spec.resolve());
        let compile_start = Instant::now();
        let cached = pass.prewarm_pipelines(device, std::slice::from_ref(&shader_source));
        println!(
            "  {:<10} {:>8.2?}  (cache holds {cached})",
            spec.label,
            compile_start.elapsed()
        );
        shader_sources.push(shader_source);
    }
    let total_start = Instant::now();
    let cached = pass.prewarm_pipelines(device, &shader_sources);
    println!(
        "  re-prewarm of all {} presets: {:.2?} (cache holds {cached} distinct pipelines, \
         {} WGSL sources of ~{} KB)",
        shader_sources.len(),
        total_start.elapsed(),
        shader_sources.len(),
        SHADER_SOURCE.len() / 1024
    );
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
        azimuth_degrees: default_sun.azimuth_degrees,
        elevation_degrees: 5.0,
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
    brickmap: &Brickmap,
    section: &Section,
) -> TimingTable {
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

    let passes: Vec<DdaPass> = section
        .variants
        .iter()
        .zip(&target_of_variant)
        .map(|(variant, target_index)| {
            DdaPass::new_with_shader_source(
                device,
                brickmap,
                &targets[*target_index].view,
                &variant.shader_source,
            )
        })
        .collect();

    let mut table: TimingTable = vec![Vec::new(); section.variants.len()];

    // Variants are INTERLEAVED round-robin within each scenario so that GPU
    // clock/thermal drift over the run hits every variant equally — timing
    // them in sequential blocks showed up to ~10% cross-run drift on
    // identical shaders, the same order as the effects being measured.
    for scenario in &section.scenarios {
        let mut samples: Vec<Vec<f32>> = vec![Vec::with_capacity(BATCH_COUNT); passes.len()];
        for (variant_index, pass) in passes.iter().enumerate() {
            for _ in 0..WARMUP_BATCHES {
                time_one_batch(
                    device,
                    queue,
                    pass,
                    &section.variants[variant_index],
                    scenario,
                );
            }
        }
        for round in 0..BATCH_COUNT {
            // Rotate the starting variant each round: the slot a batch
            // occupies within a round measurably biases its timing (the
            // preceding batch's duration shapes the GPU clock state it
            // inherits), so every variant must sample every slot equally.
            for offset in 0..passes.len() {
                let variant_index = (round + offset) % passes.len();
                samples[variant_index].push(time_one_batch(
                    device,
                    queue,
                    &passes[variant_index],
                    &section.variants[variant_index],
                    scenario,
                ));
            }
        }
        for variant_index in 0..passes.len() {
            let (median_milliseconds, p95_milliseconds) = summarize(&mut samples[variant_index]);
            println!(
                "{:<24} {:<28} median {:>7.3} ms   p95 {:>7.3} ms",
                section.variants[variant_index].label,
                scenario.label,
                median_milliseconds,
                p95_milliseconds
            );
            table[variant_index].push((median_milliseconds, p95_milliseconds));
        }
    }

    for scenario in section
        .scenarios
        .iter()
        .filter(|scenario| scenario.capture_image)
    {
        let scenario_images: Vec<Vec<u8>> = passes
            .iter()
            .zip(&section.variants)
            .zip(&target_of_variant)
            .map(|((pass, variant), target_index)| {
                // One un-timed dispatch so the readback sees THIS variant's frame.
                let target = &targets[*target_index];
                render_once(device, queue, pass, variant, scenario);
                read_back_image(device, queue, target)
            })
            .collect();
        write_scenario_pngs(section, scenario, &scenario_images);
        compare_scenario_images(section, scenario, &scenario_images);
    }
    table
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
    let (device, queue) = pollster::block_on(adapter.request_device(&wgpu::DeviceDescriptor {
        label: Some("bench_dda device"),
        ..Default::default()
    }))
    .expect("device creation failed");
    // Surface validation/device errors — without a handler wgpu only routes
    // them through `log`, and a silent device loss shows up here as a
    // baffling "no timestamps" panic instead of the real cause.
    device.on_uncaptured_error(std::sync::Arc::new(|error: wgpu::Error| {
        eprintln!("wgpu uncaptured error: {error}");
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
    pass: &DdaPass,
    variant: &Variant,
    scenario: &Scenario,
) -> f32 {
    let (width, height) = variant.resolution();
    let camera_uniform = scenario.camera_uniform((width, height));
    let lighting_uniform = scenario.lighting_uniform(&variant.quality);
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench batch"),
    });
    for _ in 0..DISPATCHES_PER_BATCH {
        pass.encode(
            queue,
            &mut encoder,
            &camera_uniform,
            &lighting_uniform,
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

/// One un-timed dispatch so the output texture holds THIS variant's frame
/// before an image readback.
fn render_once(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    pass: &DdaPass,
    variant: &Variant,
    scenario: &Scenario,
) {
    let (width, height) = variant.resolution();
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench verify frame"),
    });
    pass.encode(
        queue,
        &mut encoder,
        &scenario.camera_uniform((width, height)),
        &scenario.lighting_uniform(&variant.quality),
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

/// Write one scenario's per-variant renders to `target/bench_dda/` as
/// `scenario_{letter}_{variant}.png`.
fn write_scenario_pngs(section: &Section, scenario: &Scenario, images: &[Vec<u8>]) {
    let output_directory = std::path::Path::new("target/bench_dda");
    std::fs::create_dir_all(output_directory).expect("failed to create target/bench_dda");
    let slug = scenario_slug(scenario);
    for (variant, image) in section.variants.iter().zip(images) {
        let path = output_directory.join(format!("scenario_{slug}_{}.png", variant_slug(variant)));
        let (width, height) = variant.resolution();
        write_png(&path, image, width, height);
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
        let image = &images[variant_index];
        let mut differing_pixels = 0_u64;
        let mut max_channel_delta = 0_u8;
        for (pixel_bytes, reference_bytes) in
            image.chunks_exact(4).zip(reference_image.chunks_exact(4))
        {
            if pixel_bytes != reference_bytes {
                differing_pixels += 1;
                for channel in 0..4 {
                    let delta = pixel_bytes[channel].abs_diff(reference_bytes[channel]);
                    max_channel_delta = max_channel_delta.max(delta);
                }
            }
        }
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
