//! What an ambient sound from an arc sounds like, as WAV files to judge by
//! ear. The tests say the ears get what the measured head gets; only a
//! listener can say whether a shore on one side sounds like one.
//!
//! `AMBIENT_ARC_OUT=target/listening/ambient-arc cargo test --release --lib
//! write_ambient_arc_listening_samples` renders six samples through the
//! whole pipeline at 48 kHz, 16-bit. Without `AMBIENT_ARC_OUT` it writes
//! nothing. The listener stands at the origin facing +y (north).
//!
//! - `a-field-wind-90-left`: 10 s of the field wind as a 90° arc on the
//!   left, on headphones.
//! - `b-field-wind-all-round`: the same wind all round, for comparison, at
//!   the same scale as `a`.
//! - `c-frog-60-ahead-right`: 8 s of the frog chorus made ambient, a 60° arc
//!   ahead and to the right.
//! - `d-noise-60-ahead-turning`: 12 s of white noise as a 60° arc straight
//!   ahead while the listener turns a full circle to the left: the arc goes
//!   round the head.
//! - `e-running-water-120-behind`: 10 s of running water as a 120° arc
//!   behind.
//! - `f-field-wind-90-left-vbap-5.1`: the wind of `a` in VBAP to the 5.1
//!   layout of `scenes/nature.yaml`, six channels (L, R, C, LFE, Ls, Rs).

use std::path::{Path, PathBuf};

use super::ambient_arc_headphones::place_towards;
use super::headphone_equalisation_listening::recording;
use super::Rig;
use crate::config::SceneConfig;
use crate::synth::field_wind::FieldWindSource;
use crate::synth::noise::Rng;
use atrium_core::source::{EmitterKind, SoundSource};
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

const SAMPLE_RATE: f32 = 48_000.0;

/// Any sound made ambient, from an arc `degrees` wide towards `place`.
struct Arced {
    sound: Box<dyn FnMut() -> f32 + Send>,
    degrees: f32,
    place: Vec3,
}

impl SoundSource for Arced {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        (self.sound)()
    }
    fn position(&self) -> Vec3 {
        self.place
    }
    fn emitter_kind(&self) -> EmitterKind {
        EmitterKind::Field
    }
    fn arc_degrees(&self) -> f32 {
        self.degrees
    }
    fn tick(&mut self, _delta_seconds: f32) {}
}

fn wind() -> Box<dyn FnMut() -> f32 + Send> {
    let mut generator = FieldWindSource::new(Vec3::ZERO, 1.0, 8.0, 42);
    Box::new(move || generator.next_sample(SAMPLE_RATE))
}

fn played(samples: Vec<f32>) -> Box<dyn FnMut() -> f32 + Send> {
    let mut index = 0;
    Box::new(move || {
        index += 1;
        samples.get(index - 1).copied().unwrap_or(0.0)
    })
}

/// `seconds` of `sound` from an arc `degrees` wide towards `centre` degrees
/// anticlockwise from north, rendered in `mode` by `rig`, the listener
/// turned by `yaw_at(frame)` before each callback.
fn render(
    mut rig: Rig,
    mode: RenderMode,
    sound: Box<dyn FnMut() -> f32 + Send>,
    degrees: f32,
    centre: f64,
    seconds: f32,
    yaw_at: impl Fn(usize) -> f32,
) -> Vec<f32> {
    let mut pipeline = rig.build(mode);
    let mut sources: Vec<Box<dyn SoundSource>> = vec![Box::new(Arced {
        sound,
        degrees,
        place: place_towards(centre),
    })];
    pipeline.prepare_sources(&sources);
    let frames = (seconds * SAMPLE_RATE) as usize;
    let mut output = Vec::with_capacity(frames * rig.channels);
    let mut rendered = 0;
    while rendered < frames {
        let block = super::BLOCK_FRAMES.min(frames - rendered);
        rig.listener.yaw = yaw_at(rendered);
        output.extend(rig.render(&mut pipeline, &mut sources, block));
        rendered += block;
    }
    output
}

fn facing_north(_frame: usize) -> f32 {
    std::f32::consts::FRAC_PI_2
}

fn write_wav(path: &Path, interleaved: &[f32], channels: u16, scale: f32) {
    let data_length = (interleaved.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_length as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_length).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&(48_000u32 * 2 * channels as u32).to_le_bytes());
    bytes.extend_from_slice(&(2 * channels).to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_length.to_le_bytes());
    for sample in interleaved {
        let scaled = (sample * scale).clamp(-1.0, 1.0);
        bytes.extend_from_slice(&((scaled * 32_767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).expect("write wav");
}

fn peak(samples: &[f32]) -> f32 {
    samples
        .iter()
        .fold(0f32, |peak, sample| peak.max(sample.abs()))
        .max(1e-9)
}

#[test]
fn write_ambient_arc_listening_samples() {
    let Ok(directory) = std::env::var("AMBIENT_ARC_OUT") else {
        return;
    };
    let directory = PathBuf::from(directory);
    std::fs::create_dir_all(&directory).expect("directory");
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let headphones = || Rig::headphones(SAMPLE_RATE);
    let write = |name: &str, output: &[f32], channels: u16, scale: f32| {
        let path = directory.join(format!("{name}.wav"));
        write_wav(&path, output, channels, scale);
        eprintln!("{}", path.display());
    };

    let left = render(
        headphones(),
        RenderMode::Hrtf,
        wind(),
        90.0,
        90.0,
        10.0,
        facing_north,
    );
    let all_round = render(
        headphones(),
        RenderMode::Hrtf,
        wind(),
        360.0,
        90.0,
        10.0,
        facing_north,
    );
    let scale = 0.8 / peak(&left).max(peak(&all_round));
    write("a-field-wind-90-left", &left, 2, scale);
    write("b-field-wind-all-round", &all_round, 2, scale);

    let frog = recording(&assets.join("frog.mp3"));
    let frog = frog[..frog.len().min((8.0 * SAMPLE_RATE) as usize)].to_vec();
    let output = render(
        headphones(),
        RenderMode::Hrtf,
        played(frog),
        60.0,
        -45.0,
        8.0,
        facing_north,
    );
    write("c-frog-60-ahead-right", &output, 2, 0.8 / peak(&output));

    let mut generator = Rng::new(31);
    let noise = Box::new(move || 0.25 * generator.next_bipolar());
    let turning = |frame: usize| {
        std::f32::consts::FRAC_PI_2 + std::f32::consts::TAU * frame as f32 / (12.0 * SAMPLE_RATE)
    };
    let output = render(
        headphones(),
        RenderMode::Hrtf,
        noise,
        60.0,
        0.0,
        12.0,
        turning,
    );
    write("d-noise-60-ahead-turning", &output, 2, 0.8 / peak(&output));

    let water = recording(&assets.join("running_water.ogg"));
    let water = water[..water.len().min((10.0 * SAMPLE_RATE) as usize)].to_vec();
    let output = render(
        headphones(),
        RenderMode::Hrtf,
        played(water),
        120.0,
        180.0,
        10.0,
        facing_north,
    );
    write(
        "e-running-water-120-behind",
        &output,
        2,
        0.8 / peak(&output),
    );

    let scene = SceneConfig::load("scenes/nature.yaml")
        .expect("scene")
        .build()
        .expect("build")
        .scene;
    let mut speakers = Rig::free_field(SAMPLE_RATE);
    speakers.layout = scene.speaker_layout.clone();
    speakers.channels = speakers.layout.total_channels();
    speakers.listener.position = Vec3::ZERO;
    let layout_centre = speakers.layout.bounding_box_centre();
    speakers.layout = atrium_core::speaker::SpeakerLayout::surround_5_1(
        speaker_at(&scene.speaker_layout, 0) - layout_centre,
        speaker_at(&scene.speaker_layout, 1) - layout_centre,
        speaker_at(&scene.speaker_layout, 2) - layout_centre,
        speaker_at(&scene.speaker_layout, 4) - layout_centre,
        speaker_at(&scene.speaker_layout, 5) - layout_centre,
    );
    let output = render(
        speakers,
        RenderMode::Vbap,
        wind(),
        90.0,
        90.0,
        10.0,
        facing_north,
    );
    write(
        "f-field-wind-90-left-vbap-5.1",
        &output,
        6,
        0.8 / peak(&output),
    );
}

/// Where the speaker on `channel` stands, at ear height.
fn speaker_at(layout: &atrium_core::speaker::SpeakerLayout, channel: usize) -> Vec3 {
    let position = layout
        .speakers()
        .iter()
        .find(|speaker| speaker.channel == channel)
        .expect("a 5.1 speaker")
        .position;
    Vec3::new(position.x, position.y, 0.0)
}
