//! What the headphone equalisation sounds like, as WAV files to judge by
//! ear. The tests say a diffuse field reaches the ears flat and the cues
//! between them survive; only a listener can say whether the head now
//! sounds like a head and not like a dummy's ear canal.
//!
//! `HEADPHONE_EQ_OUT=target/listening/headphone-eq cargo test --release --lib
//! write_headphone_equalisation_listening_samples` renders five samples
//! through the whole headphone pipeline twice, equalised into `on/` and as
//! measured into `off/`, under the same names, as 48 kHz 16-bit stereo.
//! Each pair shares one scale factor, so the equalisation's change in level
//! is heard as it is. Without `HEADPHONE_EQ_OUT` it writes nothing.
//!
//! - `1-robin-ahead-3m`: a robin's phrase straight ahead at 3 m.
//! - `2-robin-circling-3m`: the phrase repeated while it circles the head at
//!   3 m, once every 6 s, for 12 s.
//! - `3-frog-ambient`: the first 8 s of the frog chorus, made ambient.
//! - `4-field-wind-ambient`: 10 s of the field wind, an ambient source.
//! - `5-nature-scene`: 15 s of `scenes/nature.yaml` on headphones, as it
//!   plays, from a fixed seed.

use std::path::{Path, PathBuf};

use super::Rig;
use crate::audio::decode::decode_file;
use crate::config::SceneConfig;
use crate::synth::field_wind::FieldWindSource;
use atrium_core::commands::Command;
use atrium_core::probe::Probe;
use atrium_core::source::{EmitterKind, SoundSource};
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

const SAMPLE_RATE: f32 = 48_000.0;

const AS_MEASURED: Probe = Probe {
    headphone_equalisation: false,
    ..Probe::WHOLE
};

/// A mono recording at 48 kHz, by linear interpolation, peaking at 0.5.
fn recording(path: &Path) -> Vec<f32> {
    let decoded = decode_file(path).expect("decode");
    let step = decoded.sample_rate as f64 / SAMPLE_RATE as f64;
    let resampled: Vec<f32> = (0..((decoded.samples.len() - 1) as f64 / step) as usize)
        .map(|index| {
            let position = index as f64 * step;
            let (whole, part) = (position as usize, position.fract() as f32);
            decoded.samples[whole] * (1.0 - part) + decoded.samples[whole + 1] * part
        })
        .collect();
    let peak = resampled
        .iter()
        .fold(0f32, |peak, sample| peak.max(sample.abs()))
        .max(1e-9);
    resampled.iter().map(|sample| sample * 0.5 / peak).collect()
}

/// A recording played as a point source following `path` (seconds to
/// position), or as an ambient sound; `looped` repeats it.
struct Played {
    samples: Vec<f32>,
    index: usize,
    looped: bool,
    kind: EmitterKind,
    path: fn(f32) -> Vec3,
}

impl SoundSource for Played {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        let index = self.index;
        self.index += 1;
        if self.looped {
            self.samples[index % self.samples.len()]
        } else {
            self.samples.get(index).copied().unwrap_or(0.0)
        }
    }
    fn position(&self) -> Vec3 {
        (self.path)(self.index as f32 / SAMPLE_RATE)
    }
    fn emitter_kind(&self) -> EmitterKind {
        self.kind
    }
    fn tick(&mut self, _delta_seconds: f32) {}
}

/// `frames` of `source` through the headphone pipeline in the free field,
/// the listener at the origin facing +y.
fn render_rig(source: Box<dyn SoundSource>, probe: Probe, frames: usize) -> Vec<f32> {
    let rig = Rig::headphones(SAMPLE_RATE);
    let mut pipeline = rig.build(RenderMode::Hrtf);
    pipeline.probe = probe;
    rig.render(&mut pipeline, &mut [source], frames)
}

/// `frames` of `scenes/nature.yaml` on headphones, offline, from a fixed
/// seed so both renders play the same.
fn render_nature(probe: Probe, frames: usize) -> Vec<f32> {
    let mut config = SceneConfig::load("scenes/nature.yaml")
        .expect("scene")
        .for_headphones();
    config.seed = Some(7);
    let mut scene = config.build().expect("build").scene;
    scene.sample_rate = SAMPLE_RATE;
    scene.init_pipelines();
    let (mut producer, mut consumer) = rtrb::RingBuffer::<Command>::new(4);
    producer
        .push(Command::SetProbe { probe })
        .expect("room for the probe");
    scene.process_commands(&mut consumer);
    let channels = scene.speaker_layout.total_channels();
    assert_eq!(channels, 2, "headphones");
    let mut output = Vec::with_capacity(frames * channels);
    let mut block = vec![0.0f32; 512 * channels];
    while output.len() < frames * channels {
        scene.render(&mut block, channels);
        output.extend_from_slice(&block);
    }
    output.truncate(frames * channels);
    output
}

fn write_stereo_wav(path: &Path, interleaved: &[f32], scale: f32) {
    let data_length = (interleaved.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + data_length as usize);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_length).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&2u16.to_le_bytes());
    bytes.extend_from_slice(&48_000u32.to_le_bytes());
    bytes.extend_from_slice(&(48_000u32 * 4).to_le_bytes());
    bytes.extend_from_slice(&4u16.to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_length.to_le_bytes());
    for sample in interleaved {
        let scaled = (sample * scale).clamp(-1.0, 1.0);
        bytes.extend_from_slice(&((scaled * 32_767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).expect("write wav");
}

#[test]
fn write_headphone_equalisation_listening_samples() {
    let Ok(directory) = std::env::var("HEADPHONE_EQ_OUT") else {
        return;
    };
    let directory = PathBuf::from(directory);
    for side in ["on", "off"] {
        std::fs::create_dir_all(directory.join(side)).expect("directory");
    }
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let seconds = |seconds: f32| (seconds * SAMPLE_RATE) as usize;
    let robin = recording(&assets.join("birds/roodborst/roodborst-zang_1.mp3"));
    let frog = recording(&assets.join("frog.mp3"));
    let frog = frog[..frog.len().min(seconds(8.0))].to_vec();

    type Render = Box<dyn Fn(Probe) -> Vec<f32>>;
    let samples: Vec<(&str, Render)> = vec![
        ("1-robin-ahead-3m", {
            let robin = robin.clone();
            Box::new(move |probe| {
                let frames = robin.len() + seconds(0.5);
                let source = Played {
                    samples: robin.clone(),
                    index: 0,
                    looped: false,
                    kind: EmitterKind::Object,
                    path: |_| Vec3::new(0.0, 3.0, 0.0),
                };
                render_rig(Box::new(source), probe, frames)
            })
        }),
        ("2-robin-circling-3m", {
            let robin = robin.clone();
            Box::new(move |probe| {
                let source = Played {
                    samples: robin.clone(),
                    index: 0,
                    looped: true,
                    kind: EmitterKind::Object,
                    path: |seconds| {
                        let angle = std::f32::consts::TAU * seconds / 6.0;
                        Vec3::new(3.0 * angle.sin(), 3.0 * angle.cos(), 0.0)
                    },
                };
                render_rig(Box::new(source), probe, seconds(12.0))
            })
        }),
        ("3-frog-ambient", {
            Box::new(move |probe| {
                let frames = frog.len() + seconds(0.3);
                let source = Played {
                    samples: frog.clone(),
                    index: 0,
                    looped: false,
                    kind: EmitterKind::Field,
                    path: |_| Vec3::ZERO,
                };
                render_rig(Box::new(source), probe, frames)
            })
        }),
        (
            "4-field-wind-ambient",
            Box::new(move |probe| {
                render_rig(
                    Box::new(FieldWindSource::new(Vec3::ZERO, 1.0, 8.0, 42)),
                    probe,
                    seconds(10.0),
                )
            }),
        ),
        (
            "5-nature-scene",
            Box::new(move |probe| render_nature(probe, seconds(15.0))),
        ),
    ];

    for (name, render) in samples {
        let rendered = [render(Probe::WHOLE), render(AS_MEASURED)];
        let peak = rendered
            .iter()
            .flatten()
            .fold(0f32, |peak, sample| peak.max(sample.abs()))
            .max(1e-9);
        let scale = 0.8 / peak;
        for (output, side) in rendered.iter().zip(["on", "off"]) {
            let path = directory.join(side).join(format!("{name}.wav"));
            write_stereo_wav(&path, output, scale);
            let side_peak = output.iter().fold(0f32, |peak, s| peak.max(s.abs()));
            let rms = (output.iter().map(|&s| (s as f64).powi(2)).sum::<f64>()
                / output.len() as f64)
                .sqrt();
            eprintln!(
                "{}: peak {:.3}, rms {:.1} dBFS after scaling",
                path.display(),
                side_peak * scale,
                20.0 * (rms * scale as f64).log10()
            );
        }
    }
}
