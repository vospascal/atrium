//! A render is what the engine plays: the same as driving `AudioScene` by
//! hand, block by block, as the audio callback and the planner do.

use atrium_core::commands::Command;
use atrium_core::probe::Probe;

use super::{identical, on_headphones, peak, render, MEADOW};
use crate::config::SceneConfig;
use crate::offline::RenderSettings;
use crate::plan::LEAD_SECONDS;

const SAMPLE_RATE: u32 = 48_000;
const BLOCK_FRAMES: usize = 512;
const SECONDS: f64 = 10.0;
const SEED: u64 = 5;
const TURN_DEGREES_PER_SECOND: f64 = 30.0;
const PROBE: Probe = Probe {
    air_absorption: false,
    ..Probe::WHOLE
};

/// The meadow on headphones, driven by hand the way
/// `tests/audio_thread_allocations.rs` drives a scene and the way the output
/// device prepares one: commands drained, then a block rendered, with the
/// plan's takes handed over a lead ahead and the head turning.
fn by_hand() -> Vec<f32> {
    let mut config = SceneConfig::load(MEADOW)
        .expect("scene loads")
        .for_headphones();
    config.seed = Some(SEED);
    let built = config.build_unstarted().expect("scene builds");
    let (mut scene, mut plan, mut plan_out) = (built.scene, built.plan, built.plan_out);
    let channels = scene.speaker_layout.total_channels();
    scene.sample_rate = SAMPLE_RATE as f32;
    scene.fit_to_output_device(channels);
    scene.init_pipelines();

    let (mut commands, mut commands_out) = rtrb::RingBuffer::<Command>::new(16);
    commands
        .push(Command::SetProbe { probe: PROBE })
        .expect("room");
    let (position, yaw) = (scene.listener.position, scene.listener.yaw);
    let frames = (SECONDS * SAMPLE_RATE as f64) as usize;
    let mut output = Vec::new();
    let mut block = vec![0.0f32; BLOCK_FRAMES * channels];
    let mut rendered = 0;
    while rendered < frames {
        let frames_now = BLOCK_FRAMES.min(frames - rendered);
        let seconds = rendered as f64 / SAMPLE_RATE as f64;
        let turned = yaw as f64 + TURN_DEGREES_PER_SECOND.to_radians() * seconds;
        commands
            .push(Command::SetListenerPose {
                position,
                yaw: turned.rem_euclid(std::f64::consts::TAU) as f32,
            })
            .expect("room");
        let block_end = (rendered + frames_now) as f64 / SAMPLE_RATE as f64;
        for due in plan.take_due(block_end + LEAD_SECONDS) {
            plan_out
                .push(due.scheduled(SAMPLE_RATE as f32, 0.0))
                .expect("room");
        }
        let buffer = &mut block[..frames_now * channels];
        scene.process_commands(&mut commands_out);
        scene.render(buffer, channels);
        output.extend_from_slice(buffer);
        rendered += frames_now;
    }
    output
}

/// Sample for sample, the render is the scene driven by hand with the same
/// commands and the same schedule. Nothing in the renderer may quietly
/// render otherwise than the audio callback does.
#[test]
fn a_render_is_what_the_engine_plays() {
    let rendered = render(
        MEADOW,
        &RenderSettings {
            seed: Some(SEED),
            probe: PROBE,
            turn_degrees_per_second: TURN_DEGREES_PER_SECOND,
            sample_rate: SAMPLE_RATE,
            block_frames: BLOCK_FRAMES,
            ..on_headphones(SECONDS)
        },
    );
    let by_hand = by_hand();
    assert!(peak(&by_hand) > 1e-4, "the meadow is silent");
    assert!(
        identical(rendered.frames(), &by_hand),
        "the render is not what the engine plays"
    );
}
