// Audio output abstraction.
//
// Currently: cpal (https://github.com/RustAudio/cpal)
// Alternative backends for future:
//   - cubeb-rs (https://github.com/mozilla/cubeb-rs) — Mozilla's Firefox audio backend,
//     pure-Rust CoreAudio + PulseAudio implementations, potentially lower latency
//   - web-audio-api-rs (https://github.com/orottier/web-audio-api-rs) — uses cpal or cubeb internally
//
// See REFERENCES.md for full list.

use cpal::traits::{DeviceTrait, HostTrait, StreamTrait};
use cpal::{SampleFormat, Stream, StreamConfig};

use crate::engine::commands::Command;
use crate::engine::scene::AudioScene;

/// Abstraction over audio output backends.
/// Future: implement for cubeb-rs or other backends.
pub trait AudioOutput {
    fn start(
        self,
        scene: AudioScene,
        commands: rtrb::Consumer<Command>,
    ) -> Result<Box<dyn StreamHandle>, Box<dyn std::error::Error>>;
}

/// Handle that keeps the audio stream alive. Drop it to stop playback.
/// Not Send — cpal::Stream contains platform-specific non-Send types (e.g. CoreAudio pointers).
/// Keep this on the main thread.
pub trait StreamHandle {
    fn sample_rate(&self) -> u32;
    fn channels(&self) -> u16;
}

/// cpal-based audio output.
pub struct CpalOutput;

struct CpalStreamHandle {
    _stream: Stream,
    sample_rate: u32,
    channels: u16,
}

impl StreamHandle for CpalStreamHandle {
    fn sample_rate(&self) -> u32 {
        self.sample_rate
    }
    fn channels(&self) -> u16 {
        self.channels
    }
}

impl AudioOutput for CpalOutput {
    fn start(
        self,
        mut scene: AudioScene,
        mut commands: rtrb::Consumer<Command>,
    ) -> Result<Box<dyn StreamHandle>, Box<dyn std::error::Error>> {
        let host = cpal::default_host();
        let device = host
            .default_output_device()
            .ok_or("no output device found")?;

        println!("Audio device: {}", device.name().unwrap_or_default());

        let supported = device.default_output_config()?;
        let sample_rate = supported.sample_rate().0;
        let channels = supported.channels();

        println!(
            "Output config: {}Hz, {} channels, {:?}",
            sample_rate,
            channels,
            supported.sample_format()
        );

        scene.sample_rate = sample_rate as f32;
        scene.init_processors();

        let config = StreamConfig {
            channels,
            sample_rate: cpal::SampleRate(sample_rate),
            buffer_size: cpal::BufferSize::Default,
        };

        let stream = match supported.sample_format() {
            SampleFormat::F32 => device.build_output_stream(
                &config,
                move |data: &mut [f32], _: &cpal::OutputCallbackInfo| {
                    scene.process_commands(&mut commands);
                    scene.render(data, channels as usize);
                },
                |err| eprintln!("audio stream error: {err}"),
                None,
            )?,
            SampleFormat::I16 => device.build_output_stream(
                &config,
                move |data: &mut [i16], _: &cpal::OutputCallbackInfo| {
                    // Convert: render to f32 internally, then convert to i16
                    let len = data.len();
                    let mut float_buf = vec![0.0f32; len]; // TODO: pre-allocate
                    scene.process_commands(&mut commands);
                    scene.render(&mut float_buf, channels as usize);
                    for (out, &sample) in data.iter_mut().zip(float_buf.iter()) {
                        *out = (sample * i16::MAX as f32) as i16;
                    }
                },
                |err| eprintln!("audio stream error: {err}"),
                None,
            )?,
            format => return Err(format!("unsupported sample format: {format:?}").into()),
        };

        stream.play()?;

        Ok(Box::new(CpalStreamHandle {
            _stream: stream,
            sample_rate,
            channels,
        }))
    }
}
