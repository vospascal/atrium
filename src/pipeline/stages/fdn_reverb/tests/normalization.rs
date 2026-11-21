use super::*;

/// Output normalization adapts to wall materials: absorptive rooms get
/// less attenuation (higher output_normalization) because the Jot feedback
/// gains are lower, causing less steady-state energy buildup in the network.
#[test]
fn output_normalization_scales_with_materials() {
    let channels = 2;
    let render_channels = 2;
    let (layout, listener) = make_ctx(channels, render_channels);

    // Dead room: acoustic tile on all walls → high absorption → short RT60
    let dead_environment = Environment::uniform(
        TEST_ENVIRONMENT.min,
        TEST_ENVIRONMENT.max,
        WallMaterial::ceiling_tile(),
    );
    let ctx_dead = MixContext {
        environment: &dead_environment,
        ..mix_ctx(&layout, &listener, channels, render_channels)
    };
    let mut fdn_dead = FdnReverbStage::new();
    fdn_dead.init(&ctx_dead);

    // Live room: hard walls → low absorption → long RT60
    let ctx_live = mix_ctx(&layout, &listener, channels, render_channels);
    let mut fdn_live = FdnReverbStage::new();
    fdn_live.init(&ctx_live);

    assert!(
        fdn_dead.output_normalization > fdn_live.output_normalization,
        "dead room norm ({}) should exceed live room norm ({})",
        fdn_dead.output_normalization,
        fdn_live.output_normalization
    );
}

/// Verify the actual output normalization for the default atrium room (6×4×3m, hard walls).
/// This is the generic FDN used by VBAP, HRTF, and DBAP modes.
#[test]
fn output_normalization_atrium_room() {
    let channels = 6;
    let render_channels = 6;
    let (layout, listener) = make_ctx(channels, render_channels);

    // Actual atrium: 6×4×3m room, hard walls
    let atrium_environment = Environment::uniform(
        Vec3::new(0.0, 0.0, 0.0),
        Vec3::new(6.0, 4.0, 3.0),
        WallMaterial::HARD_WALL,
    );
    let ctx = MixContext {
        environment: &atrium_environment,
        ..mix_ctx(&layout, &listener, channels, render_channels)
    };

    let mut fdn = FdnReverbStage::new();
    fdn.init(&ctx);

    // Hard walls produce long RT60 → high feedback gains → low output normalization.
    // The exact value depends on material-derived RT60 at 500 Hz (3.39 s) and
    // 4 kHz (1.57 s). Measured 0.691. It was 0.49 while the 1.57 s decay sat
    // at Nyquist: the damping filters now reach it at 4 kHz and damp 4–24 kHz
    // harder, so the network builds up less energy (its measured wet/send
    // level moved only +0.5 dB, −15.5 → −15.0 dB, so the estimate tracks it).
    eprintln!(
        "Generic FDN output_normalization = {:.4} (atrium 6x4x3, hard walls)",
        fdn.output_normalization
    );
    assert!(
        fdn.output_normalization > 0.05 && fdn.output_normalization < 0.8,
        "atrium room norm should be between 0.05 and 0.8, got {}",
        fdn.output_normalization
    );
}

/// Sweep wall_reflectivity and RT60 high-frequency ratio to show how each
/// affects normalization and actual wet tail energy.
///
/// Feeds a single impulse through the FDN and measures:
/// - output_normalization (structural gain compensation)
/// - RT60 low (from Sabine)
/// - wet tail RMS (actual energy in the reverb tail after the impulse)
#[test]
fn parameter_sweep_reflectivity_and_hf_ratio() {
    use crate::pipeline::room_acoustics;

    let channels = 2;
    let render_channels = 2;
    let (layout, listener) = make_ctx(channels, render_channels);
    let sample_rate = 48000.0;

    let environment_min = Vec3::new(0.0, 0.0, 0.0);
    let environment_max = Vec3::new(6.0, 4.0, 3.0);
    let (volume, surface_area) = room_acoustics::room_geometry(environment_min, environment_max);
    let environment =
        Environment::uniform(environment_min, environment_max, WallMaterial::HARD_WALL);

    let reflectivities = [0.70, 0.75, 0.80, 0.85, 0.90, 0.95];
    let hf_ratios = [0.3, 0.4, 0.5, 0.6];

    eprintln!();
    eprintln!("═══ Parameter sweep: 6×4×3m atrium ═══");
    eprintln!(
        "{:>6}  {:>8}  {:>8}  {:>8}  {:>12}",
        "refl", "hf_ratio", "RT60_lo", "norm", "tail_rms_dB"
    );
    eprintln!("{}", "─".repeat(50));

    for &refl in &reflectivities {
        let rt60_low = room_acoustics::sabine_rt60(volume, surface_area, refl);

        for &hf_ratio in &hf_ratios {
            let rt60_high = rt60_low * hf_ratio;

            let mut fdn = FdnReverbStage::new();
            fdn.compute_delays(sample_rate);
            fdn.compute_pre_delay(sample_rate, volume, surface_area, 343.42);
            fdn.compute_damping(sample_rate, rt60_low, rt60_high);
            fdn.initialized = true;

            // Feed a single impulse and measure the tail energy.
            // Use reverb_input to control exactly what goes in (mono bus on ch0).
            let frames = 48000; // 1 second
            let mut buffer = vec![0.0f32; frames * channels];
            let mut reverb_input = vec![0.0f32; frames * channels];
            reverb_input[0] = 1.0; // single impulse on ch0

            let ctx = MixContext {
                environment: &environment,
                reverb_input: Some(&reverb_input),
                ..mix_ctx(&layout, &listener, channels, render_channels)
            };

            fdn.process(&mut buffer, &ctx);

            // Measure wet tail RMS (skip first 50ms to avoid the direct impulse).
            let skip_samples = (0.05 * sample_rate) as usize;
            let tail_energy: f32 = buffer[skip_samples * channels..]
                .iter()
                .step_by(channels) // ch0 only
                .map(|s| s * s)
                .sum();
            let tail_samples = (frames - skip_samples) as f32;
            let tail_rms = (tail_energy / tail_samples).sqrt();
            let tail_rms_db = if tail_rms > 1e-10 {
                20.0 * tail_rms.log10()
            } else {
                -100.0
            };

            eprintln!(
                "{:>6.2}  {:>8.1}  {:>8.3}  {:>8.4}  {:>12.1}",
                refl, hf_ratio, rt60_low, fdn.output_normalization, tail_rms_db
            );
        }
    }
    eprintln!();
}

/// Break down the energy budget between early reflections and FDN late tail
/// for the default atrium room. Shows which stage dominates the perceived echo.
#[test]
fn energy_breakdown_early_vs_late() {
    use crate::pipeline::path::{PathResolver, PathSet, ResolveContext};
    use crate::pipeline::path_resolvers::ImageSourceResolver;
    use crate::pipeline::room_acoustics;

    let channels = 2;
    let render_channels = 2;
    let (layout, listener) = make_ctx(channels, render_channels);
    let sample_rate = 48000.0;

    let environment_min = Vec3::new(0.0, 0.0, 0.0);
    let environment_max = Vec3::new(6.0, 4.0, 3.0);
    let (volume, surface_area) = room_acoustics::room_geometry(environment_min, environment_max);
    let hard_environment =
        Environment::uniform(environment_min, environment_max, WallMaterial::HARD_WALL);

    let listener_pos = Vec3::new(3.0, 2.0, 0.0); // center of room
    let source_positions = [
        ("djembe (center, 1.5m orbit)", Vec3::new(4.5, 2.0, 0.0)),
        ("campfire (corner)", Vec3::new(1.0, 1.0, 0.0)),
        ("purring (front-right)", Vec3::new(5.0, 3.0, 0.0)),
    ];

    let reflectivities = [0.75, 0.90];

    for &refl in &reflectivities {
        let rt60 = room_acoustics::sabine_rt60(volume, surface_area, refl);
        let d_c = room_acoustics::critical_distance(volume, rt60, 1.0);

        eprintln!();
        eprintln!("═══ Energy breakdown: r={refl}, RT60={rt60:.3}s, d_c={d_c:.3}m ═══");
        eprintln!(
            "{:>25}  {:>6}  {:>10}  {:>12}  {:>12}  {:>10}",
            "source", "dist", "send (d/dc)", "refl_energy", "fdn_tail_dB", "ratio E/L"
        );
        eprintln!("{}", "─".repeat(85));

        let resolver = ImageSourceResolver;
        let resolver_environment = Environment::uniform(
            environment_min,
            environment_max,
            WallMaterial::with_reflectivity(refl),
        );

        for (name, source_pos) in &source_positions {
            let dist = source_pos.distance_to(listener_pos);
            let send = room_acoustics::reverb_send(dist, d_c);

            // ── Early reflections: path resolver gives us the gains ──
            let mut paths = PathSet::new();
            let resolve_ctx = ResolveContext {
                source_pos: *source_pos,
                target_pos: listener_pos,
                environment: &resolver_environment,
            };
            resolver.resolve(&resolve_ctx, &mut paths);

            let mut early_energy = 0.0f32;
            for path in paths.as_slice() {
                if path.kind == crate::pipeline::path::PathKind::Reflection {
                    // Each reflection has gain = reflectivity / image_dist
                    early_energy += path.gain * path.gain;
                }
            }

            // ── FDN late tail: feed impulse × send, measure tail ──
            // Use band-specific RT60 from wall materials (hard walls for this test).
            let wall_areas = room_acoustics::wall_surface_areas(environment_min, environment_max);
            let atmosphere = AtmosphericParams::default();
            let rt60_high = room_acoustics::sabine_rt60_at_band(
                volume,
                &wall_areas,
                &TEST_ENVIRONMENT.wall_materials,
                &atmosphere,
                RT60_HIGH_BAND,
            );
            let mut fdn = FdnReverbStage::new();
            fdn.compute_delays(sample_rate);
            fdn.compute_pre_delay(sample_rate, volume, surface_area, 343.42);
            fdn.compute_damping(sample_rate, rt60, rt60_high);
            fdn.initialized = true;

            let frames = 48000; // 1 second
            let mut buffer = vec![0.0f32; frames * channels];
            let mut reverb_input = vec![0.0f32; frames * channels];
            reverb_input[0] = send; // impulse scaled by d/d_c send law

            let ctx = MixContext {
                environment: &hard_environment,
                reverb_input: Some(&reverb_input),
                ..mix_ctx(&layout, &listener, channels, render_channels)
            };

            fdn.process(&mut buffer, &ctx);

            // Measure FDN tail energy (skip first 50ms)
            let skip = (0.05 * sample_rate) as usize;
            let tail_energy: f32 = buffer[skip * channels..]
                .iter()
                .step_by(channels)
                .map(|s| s * s)
                .sum();
            let tail_rms = (tail_energy / (frames - skip) as f32).sqrt();
            let tail_db = if tail_rms > 1e-10 {
                20.0 * tail_rms.log10()
            } else {
                -100.0
            };

            let ratio = if tail_energy > 1e-20 {
                early_energy / tail_energy
            } else {
                f32::INFINITY
            };

            eprintln!(
                "{:>25}  {:>6.2}  {:>10.4}  {:>12.6}  {:>12.1}  {:>10.1}",
                name, dist, send, early_energy, tail_db, ratio
            );
        }
    }
    eprintln!();
}
