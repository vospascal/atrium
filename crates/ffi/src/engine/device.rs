//! The output device: which outputs it can play, its volume, what the audio
//! thread reports back, and the thread that holds the stream.

use std::sync::mpsc;
use std::thread::JoinHandle;

use atrium::audio::output::{AudioOutput, CpalOutput};
use atrium::engine::scene::AudioScene;
use atrium_core::commands::Command;

use super::{lock, Engine};
use crate::api::{EngineError, Probe, Telemetry};
use crate::head_tracking::HeadTrackingStatus;
use crate::output::Output;

#[uniffi::export]
impl Engine {
    /// The outputs this scene can play on the opened device.
    pub fn outputs(&self) -> Vec<Output> {
        self.outputs.clone()
    }

    /// Switch to `output` while playing. Both halves take effect in the same
    /// audio callback, so no buffer is rendered with a mismatched pair.
    pub fn set_output(&self, output: Output) -> Result<(), EngineError> {
        if !self.outputs.contains(&output) {
            return Err(EngineError::UnplayableOutput);
        }
        let mut commands = lock(&self.commands);
        if commands.slots() < 2 {
            return Err(EngineError::CommandQueueFull);
        }
        let _ = commands.push(Command::SetChannelMode {
            mode: output.channel_mode.into(),
        });
        let _ = commands.push(Command::SetRenderMode {
            mode: output.render_mode.into(),
        });
        Ok(())
    }

    /// Set the master gain; `volume` is clamped to 0–1.
    pub fn set_volume(&self, volume: f32) -> Result<(), EngineError> {
        let gain = if volume.is_finite() {
            volume.clamp(0.0, 1.0)
        } else {
            0.0
        };
        lock(&self.commands)
            .push(Command::SetMasterGain { gain })
            .map_err(|_| EngineError::CommandQueueFull)
    }

    /// Let only the parts of the render `probe` names reach the output, to
    /// listen to one at a time: the direct sound, the reflections, the late
    /// reverb or the ambient sounds, with single effects of the air and the
    /// ground on or off, from every output channel or one alone. Nothing
    /// else about the scene changes, so the parts heard alone add up to the
    /// whole. A hidden part fades out and back in within one audio callback.
    pub fn set_probe(&self, probe: Probe) -> Result<(), EngineError> {
        let mut scene = lock(&self.scene);
        let probe: atrium_core::probe::Probe = probe.into();
        lock(&self.commands)
            .push(Command::SetProbe { probe })
            .map_err(|_| EngineError::CommandQueueFull)?;
        scene.probe = probe;
        Ok(())
    }

    /// Which parts of the render reach the output now.
    pub fn probe(&self) -> Probe {
        lock(&self.scene).probe.into()
    }

    /// The newest telemetry since the last call, or nothing if no frame has
    /// arrived since.
    pub fn latest_telemetry(&self) -> Option<Telemetry> {
        let mut telemetry = lock(&self.telemetry);
        let mut latest = None;
        while let Ok(frame) = telemetry.pop() {
            latest = Some(frame);
        }
        let mut report = Telemetry::from(latest.as_ref()?);
        report.head_tracking = match lock(&self.head_tracking).as_ref() {
            None => HeadTrackingStatus::Off,
            Some(tracking) if tracking.is_receiving() => HeadTrackingStatus::Receiving,
            Some(_) => HeadTrackingStatus::Waiting,
        };
        Some(report)
    }
}

/// The thread that owns the audio stream.
///
/// `cpal::Stream` is not `Send` on Apple platforms, so it cannot live in the
/// `Send + Sync` engine object. It is created, held and dropped on this
/// thread, which waits for the engine to let go of `stop`.
pub(super) struct OutputThread {
    /// Channels the stream opened with.
    pub(super) device_channels: usize,
    /// The rate the stream opened at (Hz).
    pub(super) sample_rate: u32,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl OutputThread {
    pub(super) fn start(
        scene: AudioScene,
        commands: rtrb::Consumer<Command>,
    ) -> Result<Self, EngineError> {
        let (stop, stopped) = mpsc::channel::<()>();
        let (started, start_result) = mpsc::channel::<Result<(usize, u32), String>>();
        let thread = std::thread::Builder::new()
            .name("atrium-output".into())
            .spawn(move || match CpalOutput.start(scene, commands) {
                Ok(stream) => {
                    let _ = started.send(Ok((stream.channels() as usize, stream.sample_rate())));
                    // Returns when the engine drops `stop`.
                    let _ = stopped.recv();
                    drop(stream);
                }
                Err(error) => {
                    let _ = started.send(Err(error.to_string()));
                }
            })
            .map_err(|error| EngineError::Audio(error.to_string()))?;

        match start_result.recv() {
            Ok(Ok((device_channels, sample_rate))) => Ok(Self {
                device_channels,
                sample_rate,
                stop: Some(stop),
                thread: Some(thread),
            }),
            Ok(Err(reason)) => {
                let _ = thread.join();
                Err(EngineError::Audio(reason))
            }
            Err(_) => {
                let _ = thread.join();
                Err(EngineError::Audio(
                    "output thread exited while starting".into(),
                ))
            }
        }
    }
}

impl Drop for OutputThread {
    fn drop(&mut self) {
        drop(self.stop.take());
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::output::{ChannelMode, RenderMode};
    use std::time::{Duration, Instant};

    /// A probe is the listener's, like a mute: a scene starts whole, the
    /// probe set is the probe read, and undo leaves it alone. At zero volume.
    #[test]
    fn a_probe_is_the_listeners_and_outlives_undo() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine = Engine::new("scenes/frog.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");
        let whole: Probe = atrium_core::probe::Probe::WHOLE.into();
        assert_eq!(engine.probe(), whole);

        let reverb_from_one_ear = Probe {
            direct: false,
            reflections: false,
            ambient: false,
            moving_air: false,
            only_channel: Some(1),
            ..whole
        };
        engine.remember("Louder".into());
        engine
            .set_source_level(engine.scene().sources[0].slot, 71.0)
            .expect("level");
        engine.set_probe(reverb_from_one_ear).expect("probe");
        assert_eq!(engine.probe(), reverb_from_one_ear);
        assert!(engine.undo().expect("undo"));
        assert_eq!(
            engine.probe(),
            reverb_from_one_ear,
            "undo took the probe back"
        );
        engine.set_probe(whole).expect("whole again");
        assert_eq!(engine.probe(), whole);
    }

    /// Plays under a second of sound at a quarter volume on the default
    /// output device: `cargo test -p atrium-ffi -- --ignored`.
    #[test]
    #[ignore = "plays audio on the default output device"]
    fn output_and_volume_change_while_playing() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine = Engine::new("scenes/frog.yaml".into()).expect("scene should play");
        let outputs = engine.outputs();
        let target = *outputs
            .last()
            .expect("every device plays at least one output");
        engine.set_volume(0.25).expect("volume");
        engine.set_output(target).expect("a listed output plays");

        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(telemetry) = engine.latest_telemetry() {
                if telemetry.output == target && telemetry.volume == 0.25 {
                    break;
                }
            }
            assert!(
                Instant::now() < deadline,
                "engine never reported {target:?} at 0.25"
            );
            std::thread::sleep(Duration::from_millis(20));
        }

        let surround = Output {
            render_mode: RenderMode::Vbap,
            channel_mode: ChannelMode::Surround51,
        };
        if !outputs.contains(&surround) {
            assert!(matches!(
                engine.set_output(surround),
                Err(EngineError::UnplayableOutput)
            ));
        }
    }
}
