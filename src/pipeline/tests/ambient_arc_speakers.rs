//! Ambient sound from an arc on loudspeakers: where its energy comes from,
//! and that all of it arrives.
//!
//! Gerzon's energy vector ("General metatheory of auditory localisation",
//! AES 92nd Convention, 1992) is the mean of the speakers' directions, seen
//! from where the mode renders for, each weighted by its share of the power.
//! For an arc it should point at the arc's middle, and grow shorter as the
//! arc widens: a wider arc is heard from more directions at once.

use super::ambient_arc_headphones::{place_towards, ArcSound};
use super::Rig;
use crate::config::SceneConfig;
use crate::synth::noise::Rng;
use atrium_core::source::SoundSource;
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

const SAMPLE_RATE: f32 = 48_000.0;
const FRAMES: usize = 96_000;

const MODES: [RenderMode; 4] = [
    RenderMode::Vbap,
    RenderMode::Dbap,
    RenderMode::WorldLocked,
    RenderMode::Ambisonics,
];

/// The 5.1 layout and the listener of `scenes/nature.yaml`, in the free
/// field.
fn nature_rig() -> Rig {
    let scene = SceneConfig::load("scenes/nature.yaml")
        .expect("scene")
        .build()
        .expect("build")
        .scene;
    let mut rig = Rig::free_field(SAMPLE_RATE);
    rig.layout = scene.speaker_layout.clone();
    rig.channels = rig.layout.total_channels();
    rig.listener = scene.listener;
    rig
}

fn noise() -> Vec<f32> {
    let mut generator = Rng::new(23);
    (0..FRAMES)
        .map(|_| 0.25 * generator.next_bipolar())
        .collect()
}

/// Each channel's power of `width` degrees of arc towards `centre` degrees
/// anticlockwise from where the listener faces, rendered in `mode`.
fn channel_powers(rig: &Rig, mode: RenderMode, input: &[f32], width: f32, centre: f64) -> Vec<f64> {
    let mut pipeline = rig.build(mode);
    let facing = rig.listener.yaw as f64 - std::f64::consts::FRAC_PI_2;
    let towards = place_towards(centre);
    let (cos, sin) = (facing.cos() as f32, facing.sin() as f32);
    let place = rig.listener.position
        + Vec3::new(
            towards.x * cos - towards.y * sin,
            towards.x * sin + towards.y * cos,
            0.0,
        );
    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(ArcSound {
        samples: input.to_vec(),
        index: 0,
        degrees: width,
        place,
        looped: false,
    })];
    pipeline.prepare_sources(&sources);
    let output = rig.render(&mut pipeline, &mut sources, FRAMES);
    (0..rig.channels)
        .map(|channel| {
            output
                .iter()
                .skip(channel)
                .step_by(rig.channels)
                .map(|&sample| (sample as f64).powi(2))
                .sum()
        })
        .collect()
}

/// The energy vector of `powers`: its direction, degrees anticlockwise from
/// where the listener faces, and its length.
fn energy_vector(rig: &Rig, powers: &[f64]) -> (f64, f64) {
    let receiver = rig.listener.position;
    let (mut x, mut y, mut total) = (0.0, 0.0, 0.0);
    for speaker in rig.layout.speakers() {
        let across = speaker.position - receiver;
        let heading = (across.y as f64).atan2(across.x as f64) - rig.listener.yaw as f64;
        let power = powers[speaker.channel];
        x += power * heading.cos();
        y += power * heading.sin();
        total += power;
    }
    (y.atan2(x).to_degrees(), (x * x + y * y).sqrt() / total)
}

/// In the 5.1 layout of `scenes/nature.yaml`, in every speaker mode, the
/// energy vector of white noise from an arc of 60° or 120°, centred ahead,
/// to the left, behind or to the right, points within 10° of the arc's
/// middle, and is longer for 60° than for 120° and for 120° than all round.
///
/// One exception, said here and not hidden: behind, the two rear speakers
/// are 112° apart, and an arc up to that wide lies between them alone.
/// Panning shares it between those two equally whatever its width, so 60°
/// and 120° there have the same energy vector (0.554 in VBAP); the 120° arc
/// spills 4° each side into the side pairs, too little to tell. Behind,
/// 120° is held to be no longer than 60°.
#[test]
fn the_energy_vector_points_at_the_arc_and_shortens_as_it_widens() {
    let rig = nature_rig();
    let input = noise();
    let mut report = String::new();
    let mut failed = false;
    for mode in MODES {
        for centre in [0.0, 90.0, 180.0, 270.0] {
            let mut line = format!("{mode:?} at {centre}°:");
            let mut lengths = Vec::new();
            for width in [60.0f32, 120.0, 360.0] {
                let powers = channel_powers(&rig, mode, &input, width, centre);
                let (direction, length) = energy_vector(&rig, &powers);
                let off = (direction - centre + 540.0).rem_euclid(360.0) - 180.0;
                line.push_str(&format!("  {width}°: {off:+5.1}° off, length {length:.3}"));
                if width < 360.0 && off.abs() > 10.0 {
                    failed = true;
                }
                lengths.push(length);
            }
            let narrower_longer = if centre == 180.0 {
                lengths[0] >= lengths[1] - 0.005
            } else {
                lengths[0] > lengths[1]
            };
            if !narrower_longer || lengths[1] <= lengths[2] {
                failed = true;
            }
            report.push_str(&line);
            report.push('\n');
        }
    }
    eprintln!("{report}");
    assert!(!failed, "{report}");
}
