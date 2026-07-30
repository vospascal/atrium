//! Headless DDA-pass benchmark — the PERMANENT perf harness for voxel-rt.
//!
//! Baseline numbers, how to read the output, and the regression protocol
//! for new features live in `docs/voxel-rt-bench.md`. Run with:
//! ```text
//! cargo run -p voxel-rt --example bench_dda --release
//! ```
//!
//! No window, no surface: instance → adapter → device, the real island
//! world (seed 1, season 0.0) + brickmap, and the real
//! [`voxel_rt::passes::dda::DdaPass`] dispatched at exactly 2560x1440 (the
//! Retina 2x resolution the app renders at on the dev machine). Each
//! (variant, scenario) pair times [`BATCH_COUNT`] batches of
//! [`DISPATCHES_PER_BATCH`] back-to-back dispatches (wall-clock per batch /
//! batch size — Metal resolves pass-boundary timestamp counters to zero when
//! several passes share a command buffer, and the batch amortizes
//! submit/poll overhead the way continuous rendering does) and reports
//! median + p95 per-dispatch milliseconds.
//!
//! Scenarios (fixed, deterministic — poses documented at the definitions):
//!   A  top-down over the island center from 60 m altitude, default sun
//!      (azimuth 32.5°, elevation 50.8°)
//!   B  same view, low sun (elevation 5° — worst case for shadow rays)
//!   C  ground-level at the spawn point looking across the island, default sun
//!   D  same view, low sun
//!
//! A/B variants: extra pipelines built from patched copies of the shader
//! source, flipping the "A/B benchmark levers" consts at the top of
//! `shaders/dda.wgsl` — each traversal optimization measured in isolation,
//! plus the all-off Stage 2 baseline.
//!
//! Correctness evidence: the low-sun scenarios (B, D) are rendered once per
//! variant and compared pixel-by-pixel against the no-fast-path reference
//! (`stage2-baseline`); PNGs land in `target/bench_dda/`. The column
//! fast-forward must never terminate a ray that is heading toward taller
//! terrain ahead — long low-sun shadows across water/valleys must survive.

use std::time::Instant;

use voxel_rt::brickmap::Brickmap;
use voxel_rt::camera::{CameraPose, CameraUniform, DEFAULT_VERTICAL_FOV_RADIANS};
use voxel_rt::lighting::{LightingUniform, SunSettings};
use voxel_rt::passes::dda::{DdaPass, SHADER_SOURCE};

use glam::Vec3;
use voxel_core::world::{VoxelWorld, VOXEL_SIZE, WATER_LEVEL, WORLD_SIZE_X, WORLD_SIZE_Z};

/// Output resolution: the dev machine's physical Retina size (2560x1440,
/// reported as 1280x720 logical). All historical numbers were taken here.
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

/// One fixed camera + sun combination.
struct Scenario {
    label: &'static str,
    camera_uniform: CameraUniform,
    lighting_uniform: LightingUniform,
    /// Low-sun scenarios get rendered + compared for shadow correctness.
    verify_image: bool,
}

/// One shader build to measure.
struct Variant {
    label: &'static str,
    shader_source: String,
}

fn main() {
    let world_start = Instant::now();
    let world = VoxelWorld::generate(WORLD_SEED, WORLD_SEASON);
    let brickmap = Brickmap::build(&world);
    println!(
        "world + brickmap ready in {:.2?} ({} occupied bricks)",
        world_start.elapsed(),
        brickmap.occupied_brick_count()
    );

    let (device, queue) = create_headless_device();

    let output_texture = device.create_texture(&wgpu::TextureDescriptor {
        label: Some("bench output texture"),
        size: wgpu::Extent3d {
            width: OUTPUT_WIDTH,
            height: OUTPUT_HEIGHT,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::STORAGE_BINDING | wgpu::TextureUsages::COPY_SRC,
        view_formats: &[],
    });
    let output_view = output_texture.create_view(&wgpu::TextureViewDescriptor::default());

    let scenarios = build_scenarios();
    let variants = build_variants();

    let passes: Vec<DdaPass> = variants
        .iter()
        .map(|variant| {
            DdaPass::new_with_shader_source(
                &device,
                &brickmap,
                &output_view,
                &variant.shader_source,
            )
        })
        .collect();

    // Timing table: variants x scenarios, median/p95 ms.
    let mut table: Vec<Vec<(f32, f32)>> = vec![Vec::new(); variants.len()];
    // Rendered images of the verify scenarios, per variant, for the shadow
    // correctness comparison: image_rows[variant][verify_index].
    let mut image_rows: Vec<Vec<Vec<u8>>> = vec![Vec::new(); variants.len()];

    // Variants are INTERLEAVED round-robin within each scenario so that GPU
    // clock/thermal drift over the run hits every variant equally — timing
    // them in sequential blocks showed up to ~10% cross-run drift on
    // identical shaders, the same order as the effects being measured.
    for scenario in &scenarios {
        let mut samples: Vec<Vec<f32>> = vec![Vec::with_capacity(BATCH_COUNT); passes.len()];
        for pass in &passes {
            for _ in 0..WARMUP_BATCHES {
                time_one_batch(&device, &queue, pass, scenario);
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
                    &device,
                    &queue,
                    &passes[variant_index],
                    scenario,
                ));
            }
        }
        for (variant_index, pass) in passes.iter().enumerate() {
            let (median_milliseconds, p95_milliseconds) = summarize(&mut samples[variant_index]);
            println!(
                "{:<18} {:<28} median {:>7.3} ms   p95 {:>7.3} ms",
                variants[variant_index].label,
                scenario.label,
                median_milliseconds,
                p95_milliseconds
            );
            table[variant_index].push((median_milliseconds, p95_milliseconds));
            if scenario.verify_image {
                // One extra dispatch so the readback sees THIS variant's frame.
                render_once(&device, &queue, pass, scenario);
                image_rows[variant_index].push(read_back_image(&device, &queue, &output_texture));
            }
        }
    }

    print_table(&scenarios, &variants, &table);
    verify_shadow_images(&scenarios, &variants, &image_rows);
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
fn build_scenarios() -> Vec<Scenario> {
    let world_x_meters = WORLD_SIZE_X as f32 * VOXEL_SIZE;
    let world_z_meters = WORLD_SIZE_Z as f32 * VOXEL_SIZE;
    let water_meters = WATER_LEVEL as f32 * VOXEL_SIZE;
    let resolution = (OUTPUT_WIDTH, OUTPUT_HEIGHT);

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

    vec![
        Scenario {
            label: "A top-down, default sun",
            camera_uniform: top_down_pose.gpu_uniform(DEFAULT_VERTICAL_FOV_RADIANS, resolution),
            lighting_uniform: default_sun.lighting_uniform(),
            verify_image: false,
        },
        Scenario {
            label: "B top-down, low sun 5 deg",
            camera_uniform: top_down_pose.gpu_uniform(DEFAULT_VERTICAL_FOV_RADIANS, resolution),
            lighting_uniform: low_sun.lighting_uniform(),
            verify_image: true,
        },
        Scenario {
            label: "C ground, default sun",
            camera_uniform: ground_pose.gpu_uniform(DEFAULT_VERTICAL_FOV_RADIANS, resolution),
            lighting_uniform: default_sun.lighting_uniform(),
            verify_image: false,
        },
        Scenario {
            label: "D ground, low sun 5 deg",
            camera_uniform: ground_pose.gpu_uniform(DEFAULT_VERTICAL_FOV_RADIANS, resolution),
            lighting_uniform: low_sun.lighting_uniform(),
            verify_image: true,
        },
    ]
}

// ---- Variants ----------------------------------------------------------------

/// Set one `const NAME: bool` lever in a shader source copy to `value`.
/// Panics when the lever is missing or already has that value — the patch
/// must never silently no-op.
fn patch_flag(shader_source: &str, flag_name: &str, value: bool) -> String {
    let needle = format!("const {flag_name}: bool = {};", !value);
    let replacement = format!("const {flag_name}: bool = {value};");
    assert!(
        shader_source.contains(&needle),
        "A/B lever `{flag_name} = {}` not found in dda.wgsl — benchmark and shader drifted apart",
        !value
    );
    shader_source.replacen(&needle, &replacement, 1)
}

/// Variants around the shipped defaults (`current` = distance skip +
/// global-max terminate only): each default-off lever flipped ON in
/// isolation, the distance skip flipped OFF, and the all-off Stage 2
/// baseline (which doubles as the image-compare reference — it must stay
/// LAST).
fn build_variants() -> Vec<Variant> {
    let current = SHADER_SOURCE.to_string();
    let with_column_fast_forward = patch_flag(&current, "ENABLE_COLUMN_FAST_FORWARD", true);
    let with_descend_fast_forward = patch_flag(&current, "ENABLE_DESCEND_FAST_FORWARD", true);
    let with_any_hit_shadow = patch_flag(&current, "ENABLE_ANY_HIT_SHADOW", true);
    let with_bit_grid = patch_flag(&current, "ENABLE_BRICK_BIT_GRID", true);
    let no_distance_skip = patch_flag(&current, "ENABLE_DISTANCE_SKIP", false);
    let mut stage2_baseline = current.clone();
    for flag_name in ["ENABLE_GLOBAL_MAX_TERMINATE", "ENABLE_DISTANCE_SKIP"] {
        stage2_baseline = patch_flag(&stage2_baseline, flag_name, false);
    }
    vec![
        Variant {
            label: "current",
            shader_source: current,
        },
        Variant {
            label: "with-column-ff",
            shader_source: with_column_fast_forward,
        },
        Variant {
            label: "with-descend-ff",
            shader_source: with_descend_fast_forward,
        },
        Variant {
            label: "with-anyhit-shadow",
            shader_source: with_any_hit_shadow,
        },
        Variant {
            label: "with-bit-grid",
            shader_source: with_bit_grid,
        },
        Variant {
            label: "no-dist-skip",
            shader_source: no_distance_skip,
        },
        Variant {
            label: "stage2-baseline",
            shader_source: stage2_baseline,
        },
    ]
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
    scenario: &Scenario,
) -> f32 {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench batch"),
    });
    for _ in 0..DISPATCHES_PER_BATCH {
        pass.encode(
            queue,
            &mut encoder,
            &scenario.camera_uniform,
            &scenario.lighting_uniform,
            OUTPUT_WIDTH,
            OUTPUT_HEIGHT,
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
fn render_once(device: &wgpu::Device, queue: &wgpu::Queue, pass: &DdaPass, scenario: &Scenario) {
    let mut encoder = device.create_command_encoder(&wgpu::CommandEncoderDescriptor {
        label: Some("bench verify frame"),
    });
    pass.encode(
        queue,
        &mut encoder,
        &scenario.camera_uniform,
        &scenario.lighting_uniform,
        OUTPUT_WIDTH,
        OUTPUT_HEIGHT,
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

/// Copy the output texture to the CPU as tightly-packed RGBA bytes.
/// 2560 * 4 = 10240 bytes/row is already a multiple of 256, so the copy
/// needs no row padding.
fn read_back_image(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    output_texture: &wgpu::Texture,
) -> Vec<u8> {
    let bytes_per_row = OUTPUT_WIDTH * 4;
    assert_eq!(bytes_per_row % wgpu::COPY_BYTES_PER_ROW_ALIGNMENT, 0);
    let buffer_size = u64::from(bytes_per_row) * u64::from(OUTPUT_HEIGHT);
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
        output_texture.as_image_copy(),
        wgpu::TexelCopyBufferInfo {
            buffer: &readback_buffer,
            layout: wgpu::TexelCopyBufferLayout {
                offset: 0,
                bytes_per_row: Some(bytes_per_row),
                rows_per_image: None,
            },
        },
        wgpu::Extent3d {
            width: OUTPUT_WIDTH,
            height: OUTPUT_HEIGHT,
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

fn print_table(scenarios: &[Scenario], variants: &[Variant], table: &[Vec<(f32, f32)>]) {
    println!();
    println!(
        "DDA pass, {}x{}, per-dispatch median / p95 ms over {} batches x {} dispatches:",
        OUTPUT_WIDTH, OUTPUT_HEIGHT, BATCH_COUNT, DISPATCHES_PER_BATCH
    );
    print!("{:<28}", "scenario");
    for variant in variants {
        print!(" | {:>21}", variant.label);
    }
    println!();
    for (scenario_index, scenario) in scenarios.iter().enumerate() {
        print!("{:<28}", scenario.label);
        for variant_row in table {
            let (median, p95) = variant_row[scenario_index];
            print!(" | {:>9.3} / {:>9.3}", median, p95);
        }
        println!();
    }
    println!();
}

/// Compare every variant's low-sun renders against the no-fast-path
/// reference (`stage2-baseline`, the last variant) and write all PNGs to
/// `target/bench_dda/`. The comparison is the correctness gate for the
/// column fast-forward: rays above a column max heading toward taller
/// terrain ahead must still occlude — long low-sun shadows must survive.
fn verify_shadow_images(scenarios: &[Scenario], variants: &[Variant], image_rows: &[Vec<Vec<u8>>]) {
    let output_directory = std::path::Path::new("target/bench_dda");
    std::fs::create_dir_all(output_directory).expect("failed to create target/bench_dda");

    let verify_labels: Vec<&str> = scenarios
        .iter()
        .filter(|scenario| scenario.verify_image)
        .map(|scenario| scenario.label)
        .collect();
    let reference_index = variants.len() - 1;
    assert_eq!(variants[reference_index].label, "stage2-baseline");

    for (verify_index, verify_label) in verify_labels.iter().enumerate() {
        let scenario_slug = verify_label
            .split(' ')
            .next()
            .expect("scenario label has a letter prefix")
            .to_lowercase();
        for (variant_index, variant) in variants.iter().enumerate() {
            let path = output_directory.join(format!(
                "scenario_{scenario_slug}_{}.png",
                variant.label.replace('-', "_")
            ));
            write_png(&path, &image_rows[variant_index][verify_index]);
        }

        let reference_image = &image_rows[reference_index][verify_index];
        println!("shadow correctness, {verify_label} (vs stage2-baseline):");
        for (variant_index, variant) in variants.iter().enumerate() {
            if variant_index == reference_index {
                continue;
            }
            let image = &image_rows[variant_index][verify_index];
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
            let total_pixels = u64::from(OUTPUT_WIDTH) * u64::from(OUTPUT_HEIGHT);
            println!(
                "  {:<18} differing pixels: {differing_pixels} / {total_pixels} \
                 ({:.4}%), max channel delta {max_channel_delta}",
                variant.label,
                differing_pixels as f64 / total_pixels as f64 * 100.0,
            );
        }
    }
    println!("PNGs written to {}", output_directory.display());
}

fn write_png(path: &std::path::Path, rgba_bytes: &[u8]) {
    let file = std::fs::File::create(path).expect("failed to create PNG file");
    let mut encoder = png::Encoder::new(std::io::BufWriter::new(file), OUTPUT_WIDTH, OUTPUT_HEIGHT);
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header().expect("PNG header write failed");
    writer
        .write_image_data(rgba_bytes)
        .expect("PNG data write failed");
}
