//! What ambient sound sounds like on headphones, as WAV files to judge by ear.
//! The numbers in the tests say whether two ears hear a diffuse field and
//! whether an attack is heard once; only a listener can say whether it
//! sounds right.
//!
//! `AMBIENT_OUT=target/listening/ambient-headphones/after cargo test --release --lib write_ambient_listening_samples`
//! renders five ambient sounds through the whole headphone pipeline and
//! writes them as 48 kHz, 16-bit stereo files. Without `AMBIENT_OUT` it
//! writes nothing.

use std::path::Path;

use super::Rig;
use crate::audio::decode::decode_file;
use crate::synth::field_wind::FieldWindSource;
use crate::synth::noise::Rng;
use atrium_core::source::{EmitterKind, SoundSource};
use atrium_core::speaker::RenderMode;
use atrium_core::types::Vec3;

const SAMPLE_RATE: f32 = 48_000.0;
/// Peak of every recorded or generated input, so each file is at the same
/// level before and after a change and nothing clips.
const INPUT_PEAK: f32 = 0.5;
/// What every file is multiplied by on its way to 16 bits. Fixed, so a file
/// written before a change and one written after are on the same scale.
const OUTPUT_SCALE: f32 = 0.5;

/// A mono signal played once as an ambient sound, at the rate it was
/// recorded at, interpolating linearly between samples as the engine's own
/// players do; then silence.
struct AmbientOneShot {
    samples: Vec<f32>,
    step: f64,
    position: f64,
}

impl AmbientOneShot {
    fn new(samples: Vec<f32>, recorded_rate: f32) -> Self {
        Self {
            samples,
            step: recorded_rate as f64 / SAMPLE_RATE as f64,
            position: 0.0,
        }
    }

    /// Frames it lasts at the output rate.
    fn frames(&self) -> usize {
        (self.samples.len() as f64 / self.step).ceil() as usize
    }
}

impl SoundSource for AmbientOneShot {
    fn next_sample(&mut self, _sample_rate: f32) -> f32 {
        let position = self.position;
        self.position += self.step;
        let index = position as usize;
        let Some(&current) = self.samples.get(index) else {
            return 0.0;
        };
        let next = self.samples.get(index + 1).copied().unwrap_or(0.0);
        let fraction = (position - index as f64) as f32;
        current + (next - current) * fraction
    }

    fn position(&self) -> Vec3 {
        Vec3::ZERO
    }

    fn emitter_kind(&self) -> EmitterKind {
        EmitterKind::Field
    }

    fn tick(&mut self, _delta_seconds: f32) {}
}

fn scaled_to_input_peak(samples: &[f32]) -> Vec<f32> {
    let peak = samples
        .iter()
        .fold(0f32, |peak, sample| peak.max(sample.abs()))
        .max(1e-9);
    samples
        .iter()
        .map(|sample| sample * INPUT_PEAK / peak)
        .collect()
}

/// `frames` of `source` through the whole headphone pipeline, the listener
/// turned by `yaw_at(frame)` before each callback.
fn render_headphones(
    source: Box<dyn SoundSource>,
    frames: usize,
    yaw_at: impl Fn(usize) -> f32,
) -> Vec<f32> {
    let mut rig = Rig::headphones(SAMPLE_RATE);
    let mut pipeline = rig.build(RenderMode::Hrtf);
    let mut sources = vec![source];
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

fn facing_ahead(_frame: usize) -> f32 {
    std::f32::consts::FRAC_PI_2
}

fn write_stereo_wav(path: &Path, interleaved: &[f32]) {
    let channels = 2u16;
    let sample_rate = SAMPLE_RATE as u32;
    let data_length = (interleaved.len() * 2) as u32;
    let mut bytes = Vec::with_capacity(44 + interleaved.len() * 2);
    bytes.extend_from_slice(b"RIFF");
    bytes.extend_from_slice(&(36 + data_length).to_le_bytes());
    bytes.extend_from_slice(b"WAVEfmt ");
    bytes.extend_from_slice(&16u32.to_le_bytes());
    bytes.extend_from_slice(&1u16.to_le_bytes());
    bytes.extend_from_slice(&channels.to_le_bytes());
    bytes.extend_from_slice(&sample_rate.to_le_bytes());
    bytes.extend_from_slice(&(sample_rate * channels as u32 * 2).to_le_bytes());
    bytes.extend_from_slice(&(channels * 2).to_le_bytes());
    bytes.extend_from_slice(&16u16.to_le_bytes());
    bytes.extend_from_slice(b"data");
    bytes.extend_from_slice(&data_length.to_le_bytes());
    let mut clipped = 0;
    let mut peak = 0f32;
    for sample in interleaved {
        let scaled = sample * OUTPUT_SCALE;
        peak = peak.max(scaled.abs());
        if scaled.abs() > 1.0 {
            clipped += 1;
        }
        bytes.extend_from_slice(&((scaled.clamp(-1.0, 1.0) * 32_767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).expect("write wav");
    eprintln!(
        "{}: peak {:.3}, {clipped} samples clipped",
        path.display(),
        peak
    );
}

#[test]
fn write_ambient_listening_samples() {
    let Ok(directory) = std::env::var("AMBIENT_OUT") else {
        return;
    };
    let directory = Path::new(&directory);
    std::fs::create_dir_all(directory).expect("directory");
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");
    let seconds = |seconds: f32| (seconds * SAMPLE_RATE) as usize;

    // (a) Ten seconds of the field wind, as the shipped preset bounds it.
    write_stereo_wav(
        &directory.join("1-field-wind.wav"),
        &render_headphones(
            Box::new(FieldWindSource::new(Vec3::ZERO, 1.0, 8.0, 42)),
            seconds(10.0),
            facing_ahead,
        ),
    );

    // (b) The first 8 s of the frog chorus, made ambient.
    let frog = decode_file(&assets.join("frog.mp3")).expect("decode frog");
    let frog_samples = frog
        .samples
        .len()
        .min((8.0 * frog.sample_rate as f32) as usize);
    let frog = AmbientOneShot::new(
        scaled_to_input_peak(&frog.samples[..frog_samples]),
        frog.sample_rate as f32,
    );
    let frog_frames = frog.frames() + seconds(0.3);
    write_stereo_wav(
        &directory.join("2-frog.wav"),
        &render_headphones(Box::new(frog), frog_frames, facing_ahead),
    );

    // (c) One robin phrase, made ambient.
    let robin =
        decode_file(&assets.join("birds/roodborst/roodborst-zang_1.mp3")).expect("decode robin");
    let robin = AmbientOneShot::new(
        scaled_to_input_peak(&robin.samples),
        robin.sample_rate as f32,
    );
    let robin_frames = robin.frames() + seconds(0.5);
    write_stereo_wav(
        &directory.join("3-robin.wav"),
        &render_headphones(Box::new(robin), robin_frames, facing_ahead),
    );

    // (d) A single-sample click every 400 ms: each should be heard once.
    let click_spacing = seconds(0.4);
    let clicks: Vec<f32> = (0..10 * click_spacing)
        .map(|index| {
            if index % click_spacing == 0 {
                INPUT_PEAK
            } else {
                0.0
            }
        })
        .collect();
    write_stereo_wav(
        &directory.join("4-clicks.wav"),
        &render_headphones(
            Box::new(AmbientOneShot::new(clicks, SAMPLE_RATE)),
            10 * click_spacing + seconds(0.2),
            facing_ahead,
        ),
    );

    // (e) Six seconds of white noise while the listener turns a full circle:
    // an ambient sound should not move or change as the head turns.
    let mut generator = Rng::new(1);
    let noise: Vec<f32> = (0..seconds(6.0))
        .map(|_| generator.next_bipolar())
        .collect();
    let sweep_frames = seconds(6.0);
    write_stereo_wav(
        &directory.join("5-white-noise-turning.wav"),
        &render_headphones(
            Box::new(AmbientOneShot::new(
                scaled_to_input_peak(&noise),
                SAMPLE_RATE,
            )),
            sweep_frames,
            |frame| {
                std::f32::consts::FRAC_PI_2
                    + std::f32::consts::TAU * frame as f32 / sweep_frames as f32
            },
        ),
    );
}
