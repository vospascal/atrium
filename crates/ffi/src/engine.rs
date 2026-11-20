//! The running engine: scene, audio output, and the two rings that connect a
//! UI to the audio thread.

use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use atrium::audio::output::{AudioOutput, CpalOutput};
use atrium::config::SceneConfig;
use atrium::engine::scene::AudioScene;
use atrium_core::commands::Command;
use atrium_core::telemetry::TelemetryFrame;

use crate::api::{EngineError, Position, SceneDescription, Telemetry};
use crate::output::{playable_outputs, Output};

/// Commands the audio thread can fall behind by before new ones are refused.
const COMMAND_CAPACITY: usize = 256;
/// Telemetry frames held for the UI; it only ever reads the newest.
const TELEMETRY_CAPACITY: usize = 4;

/// A loaded scene playing on the default output device.
///
/// Audio plays from construction until the object is destroyed (Kotlin
/// `close()`). Telemetry is pulled, not pushed: the UI asks for the newest
/// frame when it draws, so nothing calls into Kotlin from an engine thread.
#[derive(uniffi::Object)]
pub struct Engine {
    description: SceneDescription,
    /// What this scene can play on the opened device; fixed for its lifetime.
    outputs: Vec<Output>,
    commands: Mutex<rtrb::Producer<Command>>,
    telemetry: Mutex<rtrb::Consumer<TelemetryFrame>>,
    /// Dropped last; dropping it stops the audio.
    _output: OutputThread,
}

#[uniffi::export]
impl Engine {
    /// Load the scene at `scene_path` and start playing it. Paths inside the
    /// scene file resolve against the process working directory.
    #[uniffi::constructor]
    pub fn new(scene_path: String) -> Result<Arc<Self>, EngineError> {
        let mut built = SceneConfig::load(&scene_path)
            .and_then(SceneConfig::build)
            .map_err(|error| EngineError::Scene(error.to_string()))?;

        let (telemetry_producer, telemetry) = rtrb::RingBuffer::new(TELEMETRY_CAPACITY);
        built.scene.telemetry_out = Some(telemetry_producer);
        let (commands, command_consumer) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
        let layout_channels = built.scene.speaker_layout.total_channels();
        let output = OutputThread::start(built.scene, command_consumer)?;

        Ok(Arc::new(Self {
            description: built.description.into(),
            outputs: playable_outputs(layout_channels, output.device_channels),
            commands: Mutex::new(commands),
            telemetry: Mutex::new(telemetry),
            _output: output,
        }))
    }

    /// The scene as built.
    pub fn scene(&self) -> SceneDescription {
        self.description.clone()
    }

    /// Move the source in `slot` to `position`.
    pub fn set_source_position(&self, slot: u16, position: Position) -> Result<(), EngineError> {
        lock(&self.commands)
            .push(Command::SetSourcePosition {
                index: slot,
                position: position.into(),
            })
            .map_err(|_| EngineError::CommandQueueFull)
    }

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

    /// The newest telemetry since the last call, or nothing if no frame has
    /// arrived since.
    pub fn latest_telemetry(&self) -> Option<Telemetry> {
        let mut telemetry = lock(&self.telemetry);
        let mut latest = None;
        while let Ok(frame) = telemetry.pop() {
            latest = Some(frame);
        }
        latest.as_ref().map(Telemetry::from)
    }
}

/// A poisoned lock only means another call panicked mid-push; the ring itself
/// is still consistent, so keep using it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// The thread that owns the audio stream.
///
/// `cpal::Stream` is not `Send` on Apple platforms, so it cannot live in the
/// `Send + Sync` engine object. It is created, held and dropped on this
/// thread, which waits for the engine to let go of `stop`.
struct OutputThread {
    /// Channels the stream opened with.
    device_channels: usize,
    stop: Option<mpsc::Sender<()>>,
    thread: Option<JoinHandle<()>>,
}

impl OutputThread {
    fn start(scene: AudioScene, commands: rtrb::Consumer<Command>) -> Result<Self, EngineError> {
        let (stop, stopped) = mpsc::channel::<()>();
        let (started, start_result) = mpsc::channel::<Result<usize, String>>();
        let thread = std::thread::Builder::new()
            .name("atrium-output".into())
            .spawn(move || match CpalOutput.start(scene, commands) {
                Ok(stream) => {
                    let _ = started.send(Ok(stream.channels() as usize));
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
            Ok(Ok(device_channels)) => Ok(Self {
                device_channels,
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

    /// Plays under a second of sound at a quarter volume on the default
    /// output device: `cargo test -p atrium-ffi -- --ignored`.
    #[test]
    #[ignore = "plays audio on the default output device"]
    fn output_and_volume_change_while_playing() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine = Engine::new("scenes/frog-headphones.yaml".into()).expect("scene should play");
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

    #[test]
    fn a_missing_scene_fails_before_any_audio_starts() {
        match Engine::new("scenes/does-not-exist.yaml".into()) {
            Err(EngineError::Scene(reason)) => assert!(reason.contains("does-not-exist")),
            Err(other) => panic!("expected a scene error, got {other}"),
            Ok(_) => panic!("a missing scene must not start an engine"),
        }
    }
}
