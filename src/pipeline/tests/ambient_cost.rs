//! What headphone rendering costs: ambient sound against point sources,
//! and a whole scene with the headphone equalisation and without.
//!
//! `cargo test --release --lib ambient_sound_cost -- --ignored --nocapture`
//! and `... headphone_scene_cost ...` print the time one 512-frame callback
//! at 48 kHz takes. They assert nothing: timings depend on the machine, so
//! they are read, not tested.

use std::time::Instant;

use super::{FieldTextureSource, Rig, ToneSource, BLOCK_FRAMES};
use atrium_core::commands::Command;
use atrium_core::probe::Probe;
use atrium_core::source::SoundSource;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

const CALLBACKS: usize = 2_000;

fn microseconds_per_callback(sources: Vec<Box<dyn SoundSource>>) -> f64 {
    let rig = Rig::headphones(48_000.0);
    let mut pipeline = rig.build(RenderMode::Hrtf);
    pipeline.ensure_topology(sources.len(), &rig.layout, rig.sample_rate);
    let mut sources = sources;
    let mut buffer = vec![0.0f32; BLOCK_FRAMES * rig.channels];
    let params = rig.render_params();
    for _ in 0..100 {
        crate::pipeline::render_pipeline(&mut pipeline, &mut sources, &params, &mut buffer);
    }
    let start = Instant::now();
    for _ in 0..CALLBACKS {
        crate::pipeline::render_pipeline(&mut pipeline, &mut sources, &params, &mut buffer);
    }
    start.elapsed().as_secs_f64() * 1e6 / CALLBACKS as f64
}

fn ambient(count: usize) -> Vec<Box<dyn SoundSource>> {
    (0..count)
        .map(|index| {
            Box::new(FieldTextureSource {
                pos: Vec3::ZERO,
                rng: crate::synth::noise::Rng::new(index as u64 + 1),
            }) as Box<dyn SoundSource>
        })
        .collect()
}

fn points(count: usize) -> Vec<Box<dyn SoundSource>> {
    (0..count)
        .map(|index| {
            ToneSource::boxed(
                Vec3::new(3.0 * (index as f32).cos(), 3.0 * (index as f32).sin(), 0.0),
                440.0,
                usize::MAX,
            )
        })
        .collect()
}

#[test]
#[ignore = "a timing, read by hand: --ignored --nocapture"]
fn ambient_sound_cost() {
    for (name, sources) in [
        ("nothing", Vec::new()),
        ("one ambient sound", ambient(1)),
        ("four ambient sounds", ambient(4)),
        ("one point source", points(1)),
        ("four point sources", points(4)),
    ] {
        eprintln!(
            "headphones, {name}: {:.1} µs per 512-frame callback at 48 kHz",
            microseconds_per_callback(sources)
        );
    }
}

/// The time of one callback of `scenes/nature.yaml` on headphones, offline,
/// with `probe`: the best of three runs of 2,000 callbacks.
fn scene_microseconds_per_callback(probe: Probe) -> f64 {
    let mut config = crate::config::SceneConfig::load("scenes/nature.yaml")
        .expect("scene")
        .for_headphones();
    config.seed = Some(7);
    let mut scene = config.build().expect("build").scene;
    scene.sample_rate = 48_000.0;
    scene.init_pipelines();
    let (mut producer, mut consumer) = rtrb::RingBuffer::<Command>::new(4);
    producer
        .push(Command::SetProbe { probe })
        .expect("room for the probe");
    scene.process_commands(&mut consumer);
    let channels = scene.speaker_layout.total_channels();
    let mut buffer = vec![0.0f32; BLOCK_FRAMES * channels];
    for _ in 0..200 {
        scene.render(&mut buffer, channels);
    }
    (0..3)
        .map(|_| {
            let start = Instant::now();
            for _ in 0..CALLBACKS {
                scene.render(&mut buffer, channels);
            }
            start.elapsed().as_secs_f64() * 1e6 / CALLBACKS as f64
        })
        .fold(f64::MAX, f64::min)
}

#[test]
#[ignore = "a timing, read by hand: --ignored --nocapture"]
fn headphone_scene_cost() {
    for (name, probe) in [
        ("equalised", Probe::WHOLE),
        (
            "as measured",
            Probe {
                headphone_equalisation: false,
                ..Probe::WHOLE
            },
        ),
    ] {
        eprintln!(
            "scenes/nature.yaml on headphones, {name}: {:.1} µs per 512-frame callback at 48 kHz",
            scene_microseconds_per_callback(probe)
        );
    }
}
