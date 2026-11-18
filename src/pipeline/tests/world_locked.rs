//! WorldLocked: speaker feeds that do not follow the listener.

use super::{
    default_distance_model, default_wall_materials, layout_5_1, test_environment, ConstSource,
};
use crate::pipeline::builders;
use crate::pipeline::{render_pipeline, PipelineParams, RenderParams};
use atrium_core::listener::Listener;
use atrium_core::source::SoundSource;
use atrium_core::types::Vec3;

// ── WorldLocked: gains independent of listener position ──────────────

#[test]
fn world_locked_gains_independent_of_listener() {
    let layout = layout_5_1();
    let dm = default_distance_model();
    let source_pos = Vec3::new(1.0, 1.0, 0.0);

    // Build a minimal WorldLocked pipeline (only DistanceDirectivity path stage)
    let params = PipelineParams {
        ..PipelineParams::default()
    };
    let mut pipeline = builders::world_locked::build(&params);

    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
        pos: source_pos,
        val: 1.0,
    })];

    let channels = 6;
    let frames = 512;
    let mut buf_a = vec![0.0f32; frames * channels];
    let mut buf_b = vec![0.0f32; frames * channels];

    let environment_min = Vec3::new(0.0, 0.0, 0.0);
    let environment_max = Vec3::new(6.0, 4.0, 3.0);

    // Listener at center of room
    let listener_a = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);
    let environment = test_environment(environment_min, environment_max, default_wall_materials());
    let rp_a = RenderParams {
        listener: &listener_a,
        channels,
        sample_rate: 48000.0,
        master_gain: 1.0,
        distance_model: &dm,
        layout: &layout,
        environment: &environment,
        measurement_mode: false,
    };
    render_pipeline(&mut pipeline, &mut sources, &rp_a, &mut buf_a);

    // Reset for second pass (clear gain ramp state)
    pipeline.reset();

    // Listener at a completely different position
    let listener_b = Listener::new(Vec3::new(5.0, 0.5, 0.0), 0.0);
    let environment = test_environment(environment_min, environment_max, default_wall_materials());
    let rp_b = RenderParams {
        listener: &listener_b,
        channels,
        sample_rate: 48000.0,
        master_gain: 1.0,
        distance_model: &dm,
        layout: &layout,
        environment: &environment,
        measurement_mode: false,
    };
    render_pipeline(&mut pipeline, &mut sources, &rp_b, &mut buf_b);

    // Gains should be identical (WorldLocked ignores listener position).
    // We compare per-channel energy (RMS) — the first buffer ramps from 0,
    // so compare the second half where ramps have settled.
    let half = frames / 2;
    for ch in 0..channels {
        let rms_a: f32 = (half..frames)
            .map(|f| buf_a[f * channels + ch].powi(2))
            .sum::<f32>()
            / half as f32;
        let rms_b: f32 = (half..frames)
            .map(|f| buf_b[f * channels + ch].powi(2))
            .sum::<f32>()
            / half as f32;
        assert!(
            (rms_a - rms_b).abs() < 1e-6,
            "WorldLocked ch{} RMS differs: {:.6} vs {:.6}",
            ch,
            rms_a,
            rms_b,
        );
    }
}

// ── WorldLocked: nearest speaker gets highest gain ───────────────────

#[test]
fn world_locked_nearest_speaker_highest_gain() {
    let layout = layout_5_1();
    let dm = default_distance_model();

    // Source very close to rear-left speaker (0,0,0)
    let source_pos = Vec3::new(0.1, 0.1, 0.0);

    let params = PipelineParams::default();
    let mut pipeline = builders::world_locked::build(&params);

    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
        pos: source_pos,
        val: 1.0,
    })];

    let channels = 6;
    let frames = 1024;
    let mut buffer = vec![0.0f32; frames * channels];

    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
    let environment = test_environment(
        Vec3::ZERO,
        Vec3::new(6.0, 4.0, 3.0),
        default_wall_materials(),
    );
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

    // Measure RMS of last quarter (gain ramp settled)
    let quarter = frames * 3 / 4;
    let mut ch_rms = [0.0f32; 6];
    for ch in 0..channels {
        ch_rms[ch] = (quarter..frames)
            .map(|f| buffer[f * channels + ch].powi(2))
            .sum::<f32>()
            / (frames - quarter) as f32;
    }

    // RL is channel 4 — it should have the highest gain
    let rl_rms = ch_rms[4];
    for (ch, &rms) in ch_rms.iter().enumerate() {
        if ch == 4 || ch == 3 {
            continue;
        } // skip RL itself and LFE (no speaker)
        assert!(
            rl_rms >= rms,
            "WorldLocked: RL (ch4) RMS {:.6} should be >= ch{} RMS {:.6}",
            rl_rms,
            ch,
            rms,
        );
    }
}

// ── WorldLocked: per-speaker air absorption differs with distance ─────

#[test]
fn world_locked_air_absorption_differs_by_distance() {
    // Two speakers at very different distances from source.
    // The nearer speaker should have higher high-frequency content
    // (less air absorption) than the far speaker.
    use atrium_core::speaker::{Speaker, SpeakerLayout};

    let near_pos = Vec3::new(1.0, 0.0, 0.0); // 1m from source
    let far_pos = Vec3::new(15.0, 0.0, 0.0); // 15m from source
    let layout = SpeakerLayout::new(
        &[
            Speaker {
                position: near_pos,
                channel: 0,
            },
            Speaker {
                position: far_pos,
                channel: 1,
            },
        ],
        None,
        2,
    );

    let dm = default_distance_model();
    let source_pos = Vec3::ZERO;

    let params = PipelineParams::default();
    let mut pipeline = builders::world_locked::build(&params);
    pipeline.ensure_topology(1, &layout, 48000.0);

    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
        pos: source_pos,
        val: 1.0,
    })];

    let channels = 2;
    let frames = 2048; // enough for filter to settle
    let mut buffer = vec![0.0f32; frames * channels];

    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), 0.0);
    let environment = test_environment(
        Vec3::new(-20.0, -20.0, -5.0),
        Vec3::new(20.0, 20.0, 5.0),
        default_wall_materials(),
    );
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
    // The far speaker is 15 m away: the sound needs 2099 samples to arrive
    // there, so the first 2048-frame buffer is still silent on that channel.
    render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);
    render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

    // Compare RMS of last quarter (filters settled)
    let quarter = frames * 3 / 4;
    let rms = |ch: usize| -> f32 {
        (quarter..frames)
            .map(|f| buffer[f * channels + ch].powi(2))
            .sum::<f32>()
            / (frames - quarter) as f32
    };

    let near_rms = rms(0);
    let far_rms = rms(1);

    // Near speaker should have significantly more signal than far
    // (less distance attenuation + less air absorption filtering)
    assert!(
        near_rms > far_rms * 2.0,
        "Near speaker RMS ({near_rms:.6}) should be significantly louder than far ({far_rms:.6})"
    );

    // Both should have nonzero signal (WorldLocked renders to both)
    assert!(near_rms > 1e-10, "Near speaker should have signal");
    assert!(far_rms > 1e-10, "Far speaker should have signal");
}
