//! What an ambient field sounds like through the decorrelator, as WAV files to
//! judge by ear. The numbers in the tests say whether an attack is heard once;
//! only a listener can say whether it sounds right.
//!
//! `AMBIENT_OUT=target/listening/ambient cargo test --release --lib write_ambient_listening_samples`
//! writes four 48 kHz, 16-bit stereo files: the first two channels of the
//! field, as headphones play it. Without `AMBIENT_OUT` it writes nothing.

use std::path::Path;

use atrium_core::source::SoundSource;
use atrium_core::speaker::SpeakerLayout;
use atrium_core::types::Vec3;

use super::{FieldEncoding, FieldRenderer};
use crate::audio::decode::decode_file;
use crate::synth::noise::Rng;

const SAMPLE_RATE: f32 = 48_000.0;
/// Frames per render call, a typical audio callback.
const BLOCK_FRAMES: usize = 512;
/// Peak of every input signal, so that each file is at the same level before
/// and after a change to the filter and nothing clips.
const INPUT_PEAK: f32 = 0.5;

/// Plays a mono signal once at the rate it was recorded at, interpolating
/// linearly between samples as the engine's own players do, then silence.
struct OneShot {
    samples: Vec<f32>,
    step: f64,
    position: f64,
}

impl OneShot {
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

impl SoundSource for OneShot {
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

/// The field's first two channels for `source`, plus `tail_seconds` of the
/// filter ringing out after the source stops.
fn render_stereo(mut source: OneShot, tail_seconds: f32) -> Vec<f32> {
    let channels = 2;
    let layout = SpeakerLayout::stereo(Vec3::new(-1.0, 1.0, 0.0), Vec3::new(1.0, 1.0, 0.0));
    let mut renderer = FieldRenderer::default();
    renderer.ensure_topology(1, SAMPLE_RATE);
    let total_frames = source.frames() + (tail_seconds * SAMPLE_RATE) as usize;
    let mut output = vec![0.0f32; total_frames * channels];
    for block in output.chunks_mut(BLOCK_FRAMES * channels) {
        let frames = block.len() / channels;
        renderer.render_source(
            0,
            &mut source,
            FieldEncoding::Speaker,
            &layout,
            SAMPLE_RATE,
            channels,
            channels,
            frames,
            (1.0, 1.0),
            block,
        );
    }
    output
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
    for sample in interleaved {
        bytes.extend_from_slice(&((sample.clamp(-1.0, 1.0) * 32_767.0) as i16).to_le_bytes());
    }
    std::fs::write(path, bytes).expect("write wav");
}

#[test]
fn write_ambient_listening_samples() {
    let Ok(directory) = std::env::var("AMBIENT_OUT") else {
        return;
    };
    let directory = Path::new(&directory);
    std::fs::create_dir_all(directory).expect("directory");
    let assets = Path::new(env!("CARGO_MANIFEST_DIR")).join("assets");

    // (a) A single-sample click every 400 ms: each should be heard once.
    let click_spacing = (0.4 * SAMPLE_RATE) as usize;
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
        &directory.join("1-clicks.wav"),
        &render_stereo(OneShot::new(clicks, SAMPLE_RATE), 0.2),
    );

    // (b) The first 8 s of the frog chorus.
    let frog = decode_file(&assets.join("frog.mp3")).expect("decode frog");
    let frog_samples = frog
        .samples
        .len()
        .min((8.0 * frog.sample_rate as f32) as usize);
    write_stereo_wav(
        &directory.join("2-frog.wav"),
        &render_stereo(
            OneShot::new(
                scaled_to_input_peak(&frog.samples[..frog_samples]),
                frog.sample_rate as f32,
            ),
            0.2,
        ),
    );

    // (c) One robin phrase.
    let robin =
        decode_file(&assets.join("birds/roodborst/roodborst-zang_1.mp3")).expect("decode robin");
    write_stereo_wav(
        &directory.join("3-robin.wav"),
        &render_stereo(
            OneShot::new(
                scaled_to_input_peak(&robin.samples),
                robin.sample_rate as f32,
            ),
            0.5,
        ),
    );

    // (d) 4 s of white noise, the case every decorrelator handles well.
    let mut generator = Rng::new(1);
    let noise: Vec<f32> = (0..(4.0 * SAMPLE_RATE) as usize)
        .map(|_| generator.next_bipolar())
        .collect();
    write_stereo_wav(
        &directory.join("4-white-noise.wav"),
        &render_stereo(OneShot::new(scaled_to_input_peak(&noise), SAMPLE_RATE), 0.2),
    );
}
