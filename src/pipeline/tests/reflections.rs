//! Image-source reflections: energy bounds, timing and the floor counted once.

use super::{
    default_atmosphere, default_distance_model, default_wall_materials, energy,
    first_non_zero_frame, layout_5_1, quad_around, test_environment, ConstSource, ImpulseSource,
    Rig, BLOCK_FRAMES, SAMPLE_RATES,
};
use crate::pipeline::builders;
use crate::pipeline::wall_material;
use crate::pipeline::{render_pipeline, PipelineParams, RenderParams};
use atrium_core::listener::Listener;
use atrium_core::source::SoundSource;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

// ── Energy probe: reflections should never exceed direct ──────────────

/// For each mode with image-source reflections (VBAP, HRTF, DBAP, Ambisonics),
/// render a constant source and verify that the total reflection energy does
/// not exceed the direct signal energy. This guards against the reflection
/// gain formula producing too-hot wall bounces.
#[test]
fn reflection_energy_bounded_by_direct() {
    let layout = layout_5_1();
    let dm = default_distance_model();
    let wall_materials = default_wall_materials();

    let source_pos = Vec3::new(4.5, 2.0, 0.0); // 1.5m from listener
    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
    let channels = 6;
    let frames = 4096;

    // Use the actual atrium room bounds
    let environment_min = Vec3::new(0.0, 0.0, 0.0);
    let environment_max = Vec3::new(6.0, 4.0, 3.0);

    let render_mode = |mode: RenderMode, reflectivity: f32| -> Vec<f32> {
        let params = PipelineParams {
            er_wall_reflectivity: reflectivity,
            ..PipelineParams::default()
        };
        let mut pipeline = match mode {
            RenderMode::Vbap => builders::vbap::build(&params),
            RenderMode::Hrtf => builders::hrtf::build(&params),
            RenderMode::Dbap => builders::dbap::build(&params),
            RenderMode::Ambisonics => builders::ambisonics::build(&params),
            _ => return vec![],
        };
        pipeline.ensure_topology(1, &layout, 48000.0);
        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: source_pos,
            val: 1.0,
        })];
        let mut buffer = vec![0.0f32; frames * channels];
        let environment =
            test_environment(environment_min, environment_max, wall_materials.clone());
        let rp = RenderParams {
            listener: &listener,
            channels,
            sample_rate: 48000.0,
            master_gain: 1.0,
            distance_model: &dm,
            layout: &layout,
            environment: &environment,
            measurement_mode: false,
        };
        render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);
        buffer
    };

    // Measure RMS over second half (after transients settle)
    let half = frames / 2;
    let rms = |buf: &[f32]| -> f32 {
        let sum: f32 = (half..frames)
            .flat_map(|f| (0..channels).map(move |ch| buf[f * channels + ch].powi(2)))
            .sum();
        (sum / ((frames - half) * channels) as f32).sqrt()
    };

    eprintln!();
    eprintln!("═══ Pipeline energy probe: 6×4×3m room ═══");
    eprintln!(
        "{:>12}  {:>12}  {:>12}  {:>12}  {:>8}",
        "mode", "dead(r=0.1)", "mid(r=0.5)", "live(r=0.9)", "refl/dry"
    );
    eprintln!("{}", "─".repeat(65));

    for mode in [
        RenderMode::Vbap,
        RenderMode::Hrtf,
        RenderMode::Dbap,
        RenderMode::Ambisonics,
    ] {
        let dead_buf = render_mode(mode, 0.1); // nearly anechoic
        let mid_buf = render_mode(mode, 0.5); // moderate reflections
        let live_buf = render_mode(mode, 0.9); // highly reflective

        let dead_rms = rms(&dead_buf);
        let mid_rms = rms(&mid_buf);
        let live_rms = rms(&live_buf);

        // Reflection contribution relative to dead room
        let refl_rms = (live_rms * live_rms - dead_rms * dead_rms).max(0.0).sqrt();
        let ratio = if dead_rms > 1e-10 {
            refl_rms / dead_rms
        } else {
            0.0
        };

        eprintln!(
            "{:>12}  {:>12.6}  {:>12.6}  {:>12.6}  {:>8.3}",
            format!("{mode:?}"),
            dead_rms,
            mid_rms,
            live_rms,
            ratio
        );

        // 1. Increasing reflectivity should monotonically increase total energy.
        assert!(
            dead_rms <= mid_rms,
            "{mode:?}: r=0.1 ({dead_rms:.6}) should produce less energy \
             than r=0.5 ({mid_rms:.6})"
        );
        assert!(
            mid_rms <= live_rms,
            "{mode:?}: r=0.5 ({mid_rms:.6}) should produce less energy \
             than r=0.9 ({live_rms:.6})"
        );

        // 2. Reflection energy should stay bounded (< 5× direct for a small room).
        assert!(
            ratio < 5.0,
            "{mode:?}: reflection energy ({refl_rms:.4}) exceeds 5× \
             direct energy ({dead_rms:.4}), got ratio {ratio:.2}"
        );
    }
    eprintln!();
}

/// The listener-centric modes do not model the absolute flight time of
/// the direct sound: `PathContribution::delay_seconds` is 0 for the
/// direct path by definition (path.rs) and only reflections are delayed,
/// by (image − direct) / c. Room: only the +X wall (x = 5 m) reflects.
///   source (0, 3, 0), listener at the origin → direct 3 m
///   image (10, 3, 0) → √109 = 10.4403 m → extra 7.4403 m
///   → 21.665 ms = 955.4 / 1039.9 / 2079.9 samples at 44.1 / 48 / 96 kHz.
/// The reflection is isolated by subtracting an identical render with the
/// +X wall open: until the reflection arrives both renders are bit-equal.
#[test]
fn listener_centric_modes_delay_reflections_relative_to_an_undelayed_direct_path() {
    let source_position = Vec3::new(0.0, 3.0, 0.0);
    let emit_at = 4 * BLOCK_FRAMES;
    let mut arrivals_seconds = Vec::new();
    for mode in [
        RenderMode::Vbap,
        RenderMode::Hrtf,
        RenderMode::Dbap,
        RenderMode::Ambisonics,
    ] {
        for sample_rate in SAMPLE_RATES {
            let mut rig = Rig::free_field(sample_rate);
            rig.environment.min = Vec3::new(-5.0, -20.0, -20.0);
            rig.environment.max = Vec3::new(5.0, 20.0, 20.0);
            let extra_metres = 109.0_f32.sqrt() - 3.0;
            let delay_samples =
                extra_metres / rig.environment.atmosphere.speed_of_sound() * sample_rate;
            let total_frames = emit_at + delay_samples as usize + 256;

            let open = {
                let mut pipeline = rig.build(mode);
                let mut sources = vec![ImpulseSource::boxed(source_position, emit_at)];
                rig.render(&mut pipeline, &mut sources, total_frames)
            };
            rig.environment.wall_materials[1] = wall_material::WallMaterial::hard_wall();
            let reflective = {
                let mut pipeline = rig.build(mode);
                let mut sources = vec![ImpulseSource::boxed(source_position, emit_at)];
                rig.render(&mut pipeline, &mut sources, total_frames)
            };

            let direct =
                first_non_zero_frame(&open, rig.channels).expect("direct sound never arrives");
            if mode == RenderMode::Hrtf {
                // HRIR onset + SOFA ITD; still no propagation delay.
                assert!(
                    (emit_at..emit_at + 64).contains(&direct),
                    "{mode:?} {sample_rate} Hz: direct onset at {direct}, emitted at {emit_at}"
                );
            } else {
                assert_eq!(
                    direct, emit_at,
                    "{mode:?} {sample_rate} Hz: direct path must not be delayed"
                );
            }

            let reflection_only: Vec<f32> = reflective
                .iter()
                .zip(&open)
                .map(|(with_wall, without_wall)| with_wall - without_wall)
                .collect();
            let reflection = first_non_zero_frame(&reflection_only, rig.channels)
                .expect("reflection never arrives")
                - emit_at;
            assert!(
                (reflection as f32 - delay_samples.floor()).abs() <= 1.0,
                "{mode:?} {sample_rate} Hz: reflection {reflection} samples after the direct, \
                 expected floor({delay_samples:.2})"
            );
            arrivals_seconds.push((mode, reflection as f32 / sample_rate));
        }
    }
    let expected_seconds = (109.0_f32.sqrt() - 3.0) / default_atmosphere().speed_of_sound();
    for (mode, seconds) in arrivals_seconds {
        assert!(
            (seconds - expected_seconds).abs() <= 1.0 / 44_100.0,
            "{mode:?}: reflection delay {seconds:.6} s vs {expected_seconds:.6} s"
        );
    }
}

/// Delay lines are sized when the device rate arrives, not at build time
/// (they used to be sized for a guessed 48 kHz). Cathedral (40 × 25 × 20 m) at 96 kHz, only the
/// far +X wall reflective, listener (1, 12.5, 0), source (2, 12.5, 0):
///   direct 1 m, image (78, 12.5, 0) → 77 m → extra 76 m
///   → 0.22130 s = 21 245 samples at 96 kHz.
/// Sized for 48 kHz the ring was next_pow2(86.2 m / c · 48 kHz) = 16 384
/// samples and clamped this reflection to 16 382 samples (0.1706 s).
#[test]
fn long_reflection_arrives_on_time_at_96_khz() {
    let sample_rate = 96_000.0;
    let listener_position = Vec3::new(1.0, 12.5, 0.0);
    let mut rig = Rig::free_field(sample_rate);
    rig.layout = quad_around(listener_position);
    rig.listener = Listener::new(listener_position, std::f32::consts::FRAC_PI_2);
    rig.environment.min = Vec3::ZERO;
    rig.environment.max = Vec3::new(40.0, 25.0, 20.0);
    let delay_samples = 76.0 / rig.environment.atmosphere.speed_of_sound() * sample_rate;
    let emit_at = 4 * BLOCK_FRAMES;
    let total_frames = emit_at + delay_samples as usize + 512;
    let source_position = Vec3::new(2.0, 12.5, 0.0);
    let open = {
        let mut pipeline = rig.build(RenderMode::Vbap);
        let mut sources = vec![ImpulseSource::boxed(source_position, emit_at)];
        rig.render(&mut pipeline, &mut sources, total_frames)
    };
    rig.environment.wall_materials[1] = wall_material::WallMaterial::stone();
    let reflective = {
        let mut pipeline = rig.build(RenderMode::Vbap);
        let mut sources = vec![ImpulseSource::boxed(source_position, emit_at)];
        rig.render(&mut pipeline, &mut sources, total_frames)
    };
    let reflection_only: Vec<f32> = reflective.iter().zip(&open).map(|(a, b)| a - b).collect();
    let arrival = first_non_zero_frame(&reflection_only, rig.channels)
        .expect("reflection never arrives")
        - emit_at;
    assert!(
        (arrival as f32 - delay_samples.floor()).abs() <= 1.0,
        "far-wall reflection after {arrival} samples, expected floor({delay_samples:.1})"
    );
}

/// The floor is modelled twice in the image-source modes: ISO 9613-2
/// A_gr (hard ground G = 0 → −3 dB, i.e. +3 dB = the coherent direct +
/// ground-reflected energy) is applied to every path, *and* the floor is a
/// reflective image-source wall. Listener and source 1.2 m up, 5 m apart:
/// floor image √(25 + 2.4²) = 5.55 m, stone gain √(1 − 0.034) = 0.98;
/// measured +1.6 dB extra energy on top of ISO's +3 dB. Counting the floor
/// once means adding the explicit floor image must change the energy by
/// less than 1 dB.
#[test]
fn floor_is_counted_once_between_ground_effect_and_image_source() {
    let mut rig = Rig::free_field(48_000.0);
    let height = 1.2;
    let centre = Vec3::new(0.0, 0.0, height);
    rig.layout = quad_around(centre);
    rig.listener = Listener::new(centre, std::f32::consts::FRAC_PI_2);
    rig.environment.min = Vec3::new(-30.0, -30.0, 0.0);
    rig.environment.max = Vec3::new(30.0, 30.0, 30.0);
    let emit_at = 4 * BLOCK_FRAMES;
    let source_position = Vec3::new(0.0, 5.0, height);
    let total_energy = |rig: &Rig| -> f64 {
        let mut pipeline = rig.build(RenderMode::Vbap);
        let mut sources = vec![ImpulseSource::boxed(source_position, emit_at)];
        let output = rig.render(&mut pipeline, &mut sources, emit_at + 4096);
        let all_channels: Vec<usize> = (0..rig.channels).collect();
        energy(&output, rig.channels, &all_channels, 0)
    };
    let without_floor = total_energy(&rig);
    rig.environment.wall_materials[4] = wall_material::WallMaterial::stone();
    let with_floor = total_energy(&rig);
    let added_db = 10.0 * (with_floor / without_floor).log10();
    assert!(
        added_db < 1.0,
        "explicit floor image added {added_db:.2} dB on top of the ISO hard-ground +3 dB"
    );
}
