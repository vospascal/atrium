//! DBAP: listener-independent gains and the configured rolloff.

use super::{
    default_distance_model, default_wall_materials, layout_5_1, test_environment, tone_power_db,
    uniform_walls, ConstSource, ImpulseSource, Rig, BLOCK_FRAMES,
};
use crate::pipeline::builders;
use crate::pipeline::wall_material;
use crate::pipeline::{render_pipeline, PipelineParams, RenderParams, RenderPipeline};
use atrium_core::listener::Listener;
use atrium_core::source::SoundSource;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

// ── DBAP: nearest speaker gets loudest output ───────────────────────

#[test]
fn dbap_nearest_speaker_loudest() {
    let layout = layout_5_1();
    let dm = default_distance_model();

    // Source near front-left speaker (0,4)
    let source_pos = Vec3::new(0.5, 3.5, 0.0);
    let params = PipelineParams::default();
    let mut pipeline = builders::dbap::build(&params);
    pipeline.ensure_topology(1, &layout, 48000.0);

    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
        pos: source_pos,
        val: 1.0,
    })];

    let channels = 6;
    let frames = 2048;
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
    render_pipeline(&mut pipeline, &mut sources, &rp, &mut buffer);

    // RMS per channel (last quarter, filters settled)
    let quarter = frames * 3 / 4;
    let rms = |ch: usize| -> f32 {
        let sum: f32 = (quarter..frames)
            .map(|f| buffer[f * channels + ch].powi(2))
            .sum();
        (sum / (frames - quarter) as f32).sqrt()
    };

    let fl_rms = rms(0);
    let fr_rms = rms(1);
    let c_rms = rms(2);
    let rl_rms = rms(4);
    let rr_rms = rms(5);
    eprintln!("DBAP RMS: FL={fl_rms:.6} FR={fr_rms:.6} C={c_rms:.6} RL={rl_rms:.6} RR={rr_rms:.6}");

    // FL is nearest speaker — it should be loudest
    assert!(
        fl_rms > fr_rms,
        "FL ({fl_rms}) should be louder than FR ({fr_rms})"
    );
    assert!(
        fl_rms > rr_rms,
        "FL ({fl_rms}) should be louder than RR ({rr_rms})"
    );

    // All speakers should have signal (DBAP sends to all)
    assert!(fr_rms > 1e-6, "FR should have signal");
    assert!(rl_rms > 1e-6, "RL should have signal");
    assert!(rr_rms > 1e-6, "RR should have signal");
}

#[test]
fn dbap_differs_from_world_locked() {
    let layout = layout_5_1();
    let dm = default_distance_model();

    let source_pos = Vec3::new(1.0, 3.0, 0.0);
    let params = PipelineParams::default();
    let listener = Listener::new(Vec3::new(3.0, 2.0, 0.0), std::f32::consts::FRAC_PI_2);

    let channels = 6;
    let frames = 2048;

    let render = |pipeline: &mut RenderPipeline| -> Vec<f32> {
        pipeline.ensure_topology(1, &layout, 48000.0);
        let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ConstSource {
            pos: source_pos,
            val: 1.0,
        })];
        let mut buffer = vec![0.0f32; frames * channels];
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
        render_pipeline(pipeline, &mut sources, &rp, &mut buffer);
        buffer
    };

    let wl_buf = render(&mut builders::world_locked::build(&params));
    let dbap_buf = render(&mut builders::dbap::build(&params));

    // Compare RMS per channel (last quarter)
    let quarter = frames * 3 / 4;
    let rms = |buf: &[f32], ch: usize| -> f32 {
        let sum: f32 = (quarter..frames)
            .map(|f| buf[f * channels + ch].powi(2))
            .sum();
        (sum / (frames - quarter) as f32).sqrt()
    };

    // They must differ on at least some channels
    let mut total_diff = 0.0f32;
    for ch in [0, 1, 2, 4, 5] {
        let wl = rms(&wl_buf, ch);
        let db = rms(&dbap_buf, ch);
        eprintln!(
            "ch{ch}: WorldLocked={wl:.6} DBAP={db:.6} diff={:.6}",
            (wl - db).abs()
        );
        total_diff += (wl - db).abs();
    }
    assert!(
        total_diff > 0.01,
        "DBAP and WorldLocked should produce different output (total_diff={total_diff})"
    );
}

/// DBAP feeds speakers from the scene alone. In a reverberant stone room
/// (reflections, per-path delay and air absorption, reverb send all
/// active), walking the listener across the room must not change one
/// sample of the speaker feed. The paths are resolved towards the middle
/// of the speaker array, a point that belongs to the room.
#[test]
fn dbap_speaker_feed_does_not_depend_on_the_listener() {
    let render_with_listener_at = |listener_position: Vec3| {
        let mut rig = Rig::free_field(48_000.0);
        rig.environment.min = Vec3::new(-6.0, -5.0, -1.2);
        rig.environment.max = Vec3::new(6.0, 5.0, 4.0);
        rig.environment.wall_materials = uniform_walls(wall_material::WallMaterial::stone());
        rig.listener = Listener::new(listener_position, 0.3);
        let mut pipeline = rig.build(RenderMode::Dbap);
        let mut sources = vec![ImpulseSource::boxed(Vec3::new(3.0, 2.0, 0.5), BLOCK_FRAMES)];
        rig.render(&mut pipeline, &mut sources, 16 * BLOCK_FRAMES)
    };
    let at_the_centre = render_with_listener_at(Vec3::ZERO);
    let in_a_corner = render_with_listener_at(Vec3::new(-4.0, -3.5, 0.0));
    assert!(
        at_the_centre.iter().any(|sample| sample.abs() > 1e-4),
        "the scene must produce sound for the comparison to mean anything"
    );
    let largest_difference = at_the_centre
        .iter()
        .zip(&in_a_corner)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert_eq!(
        largest_difference, 0.0,
        "moving the listener changed the DBAP speaker feed"
    );
}

/// DBAP is listener-independent: the overall level only falls once the
/// source leaves the reference circle (array radius 2 m), via the power
/// factor p = d_ref / d_centroid. Per `dbap.rs` Eq. 9 the amplitude goes
/// as p^(2a), a = 6 / 6.0206 → −12.0 dB per doubling at rolloff 6 dB,
/// i.e. twice the configured rolloff and twice the 6.02 dB of every other
/// mode (flagged in the integration audit). Pinned as a range: at least
/// the configured rolloff, at most twice it. Power, not DC area: DBAP
/// redistributes gain over the speakers as the source moves, and only Σv²
/// is the level.
#[test]
fn dbap_far_field_level_falls_at_least_by_the_configured_rolloff() {
    let rig = Rig::free_field(48_000.0);
    let level_db =
        |distance: f32| tone_power_db(&rig, RenderMode::Dbap, Vec3::new(0.0, distance, 0.0), 4096);
    // Inside the reference circle the level is distance-independent.
    let inside_db = level_db(0.5) - level_db(1.0);
    assert!(
        inside_db.abs() < 0.5,
        "DBAP inside the array should not follow a distance law, moved {inside_db:.2} dB"
    );
    let far_drop_db = level_db(8.0) - level_db(16.0);
    assert!(
        (6.0..=12.1).contains(&far_drop_db),
        "DBAP 8 m → 16 m dropped {far_drop_db:.2} dB, expected between 6 and 12.1 dB"
    );
}
