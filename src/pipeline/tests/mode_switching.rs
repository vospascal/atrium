//! Mode switching, reset, and state that must not linger or blow up.

use super::{layout_5_1, uniform_walls, Rig, ToneSource, BLOCK_FRAMES};
use crate::pipeline::builders;
use crate::pipeline::wall_material;
use crate::pipeline::{build_all_pipelines, PipelineParams};
use atrium_core::listener::Listener;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

// ── RenderMode index ──────────────────────────────────────────────────

#[test]
fn render_mode_index_roundtrip() {
    for mode in RenderMode::ALL {
        assert_eq!(RenderMode::ALL[mode.index()], mode);
    }
}

// ── Pipeline reset clears gain ramps ─────────────────────────────────

#[test]
fn pipeline_reset_clears_state() {
    let params = PipelineParams::default();
    let mut pipeline = builders::vbap::build(&params);
    let layout = layout_5_1();

    pipeline.ensure_topology(2, &layout, 48000.0);

    // After reset, renderer should have zeroed prev_gains
    // (can't inspect directly, but calling reset should not panic)
    pipeline.reset();
}

// ── Build all pipelines without panic ────────────────────────────────

#[test]
fn build_all_pipelines_smoke() {
    let params = PipelineParams::default();
    let pipelines = build_all_pipelines(&params);
    assert_eq!(pipelines.len(), 5);
    assert_eq!(
        pipelines[RenderMode::WorldLocked.index()].renderer.name(),
        "world_locked"
    );
    assert_eq!(
        pipelines[RenderMode::Vbap.index()].renderer.name(),
        "multichannel"
    );
    assert_eq!(pipelines[RenderMode::Hrtf.index()].renderer.name(), "hrtf");
    assert_eq!(pipelines[RenderMode::Dbap.index()].renderer.name(), "dbap");
    assert_eq!(
        pipelines[RenderMode::Ambisonics.index()].renderer.name(),
        "ambisonics"
    );
}

/// `Command::SetRenderMode` resets the pipeline it activates. Play a tone
/// in a reflective room (RT60 1 s: every stateful stage holds energy),
/// switch away, switch back: the returning pipeline must be exactly
/// silent (no stale delay line, FDN, filter, overlap or ramp state), and
/// no sample anywhere may be non-finite or exceed the soft-clip bound 1.0.
#[test]
fn mode_switch_leaves_no_stale_state_and_stays_bounded() {
    let mut rig = Rig::free_field(48_000.0);
    rig.layout = layout_5_1();
    rig.channels = rig.layout.total_channels();
    rig.listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
    rig.environment.min = Vec3::ZERO;
    rig.environment.max = Vec3::new(8.0, 6.0, 4.0);
    rig.environment.wall_materials = uniform_walls(wall_material::WallMaterial::hard_wall());
    rig.wall_reflectivity = 0.9;
    rig.reverb_rt60_seconds = Some(1.0);
    rig.measurement_mode = false;
    for (index, mode) in RenderMode::ALL.into_iter().enumerate() {
        let other = RenderMode::ALL[(index + 1) % RenderMode::ALL.len()];
        let mut pipelines = build_all_pipelines(&rig.pipeline_params());
        for pipeline in pipelines.iter_mut() {
            rig.init(pipeline);
        }
        let mut sources = vec![ToneSource::boxed(
            Vec3::new(4.0, 4.5, 0.0),
            1000.0,
            4 * BLOCK_FRAMES,
        )];
        let playing = rig.render(&mut pipelines[mode.index()], &mut sources, 8 * BLOCK_FRAMES);
        pipelines[other.index()].reset();
        let elsewhere = rig.render(
            &mut pipelines[other.index()],
            &mut sources,
            2 * BLOCK_FRAMES,
        );
        pipelines[mode.index()].reset();
        let back = rig.render(&mut pipelines[mode.index()], &mut sources, BLOCK_FRAMES);

        assert!(
            playing.iter().any(|sample| *sample != 0.0),
            "{mode:?}: produced no sound"
        );
        for sample in playing.iter().chain(&elsewhere).chain(&back) {
            assert!(
                sample.is_finite() && sample.abs() <= 1.0,
                "{mode:?} → {other:?}: sample {sample} outside the soft-clip bound"
            );
        }
        let stale_peak = back
            .iter()
            .map(|sample| sample.abs())
            .fold(0.0f32, f32::max);
        assert_eq!(
            stale_peak, 0.0,
            "{mode:?}: stale state survived the mode-switch reset"
        );
    }
}

/// Silence in → exact silence out from a cold start in every mode (no DC,
/// dither or noise injection by any stage), and a burst's tail decays to
/// below −180 dBFS without growing again. RT60 0.2 s: 0.85 s after the
/// burst stops the FDN is ≥ 250 dB down.
#[test]
fn silence_stays_exact_and_tails_decay_without_denormal_growth() {
    let mut rig = Rig::free_field(48_000.0);
    rig.layout = layout_5_1();
    rig.channels = rig.layout.total_channels();
    rig.listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
    rig.environment.min = Vec3::ZERO;
    rig.environment.max = Vec3::new(8.0, 6.0, 4.0);
    rig.environment.wall_materials = uniform_walls(wall_material::WallMaterial::hard_wall());
    rig.wall_reflectivity = 0.9;
    rig.reverb_rt60_seconds = Some(0.2);
    rig.measurement_mode = false;
    let source_position = Vec3::new(4.0, 4.5, 0.0);
    for mode in RenderMode::ALL {
        let mut pipeline = rig.build(mode);
        let mut silent = vec![ToneSource::boxed(source_position, 1000.0, 0)];
        let cold = rig.render(&mut pipeline, &mut silent, 4 * BLOCK_FRAMES);
        assert!(
            cold.iter().all(|sample| *sample == 0.0),
            "{mode:?}: silent input produced output"
        );

        let mut pipeline = rig.build(mode);
        let mut burst = vec![ToneSource::boxed(source_position, 1000.0, 2 * BLOCK_FRAMES)];
        let blocks = 84; // 0.9 s
        let output = rig.render(&mut pipeline, &mut burst, blocks * BLOCK_FRAMES);
        let block_peak = |block: usize| -> f32 {
            output[block * BLOCK_FRAMES * rig.channels..(block + 1) * BLOCK_FRAMES * rig.channels]
                .iter()
                .map(|sample| sample.abs())
                .fold(0.0f32, f32::max)
        };
        assert!(output.iter().all(|sample| sample.is_finite()));
        let last = block_peak(blocks - 1);
        assert!(
            last < 1e-9,
            "{mode:?}: tail still at {:.1} dBFS 0.85 s after the burst",
            20.0 * last.max(1e-30).log10()
        );
        assert!(
            last <= block_peak(blocks - 21),
            "{mode:?}: tail grew again ({last:e} > {:e})",
            block_peak(blocks - 21)
        );
    }
}
