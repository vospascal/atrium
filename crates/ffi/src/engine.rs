//! The running engine: scene, audio output, and the two rings that connect a
//! UI to the audio thread.

use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use atrium::audio::output::{AudioOutput, CpalOutput};
use atrium::config::{bearing_for_heading, radiation_of, read_preset, SceneConfig, SoundBuilder};
use atrium::engine::edit::{Retired, SceneEdit};
use atrium::engine::scene::AudioScene;
use atrium::head_tracking::HeadTracking;
use atrium_core::commands::Command;
use atrium_core::telemetry::{TelemetryFrame, MAX_SOURCES};
use atrium_core::types::Vec3;
use atrium_world::{Entity, EntityId, Moves, Place, Shape, Sound, When};

use crate::api::{
    EmitterKind, EngineError, Gap, Pattern, Position, SceneDescription, SourceDescription,
    Telemetry,
};
use crate::head_tracking::{head_trackers, HeadTracker, HeadTrackingStatus};
use crate::output::{playable_outputs, Output};

/// Commands the audio thread can fall behind by before new ones are refused.
const COMMAND_CAPACITY: usize = 256;
/// Telemetry frames held for the UI; it only ever reads the newest.
const TELEMETRY_CAPACITY: usize = 4;
/// Sound changes the audio thread can fall behind by. Each is one press in a
/// UI, so a handful is plenty.
const EDIT_CAPACITY: usize = 8;
/// How far ahead of the listener an auditioned or newly added sound stands (m).
const AHEAD_OF_LISTENER: f32 = 2.0;
/// The voice an auditioned preset plays from: the last one, so a scene's own
/// sounds, which fill voices from the first, reach it last.
const AUDITION_SLOT: usize = MAX_SOURCES - 1;

/// A loaded scene playing on the default output device.
///
/// Audio plays from construction until the object is destroyed (Kotlin
/// `close()`). Telemetry is pulled, not pushed: the UI asks for the newest
/// frame when it draws, so nothing calls into Kotlin from an engine thread.
#[derive(uniffi::Object)]
pub struct Engine {
    /// The scene as it is now, as a UI shows it and as a file saves it.
    scene: Mutex<Live>,
    /// What this scene can play on the opened device; fixed for its lifetime.
    outputs: Vec<Output>,
    /// Shared with head tracking, which steers the listener from its own
    /// thread.
    commands: Arc<Mutex<rtrb::Producer<Command>>>,
    telemetry: Mutex<rtrb::Consumer<TelemetryFrame>>,
    /// Where the scene puts the listener and which way it faces (radians).
    /// Head turns are added to this heading.
    listener_position: Vec3,
    listener_yaw: f32,
    /// The running tracker, if any.
    head_tracking: Mutex<Option<HeadTracking>>,
    /// Builds one more sound for this scene, as its own sounds were built.
    sounds: SoundBuilder,
    edits: Mutex<Edits>,
    /// Dropped last; dropping it stops the audio.
    _output: OutputThread,
}

#[uniffi::export]
impl Engine {
    /// Load the scene at `scene_path` and start playing it. Paths inside the
    /// scene file resolve against the process working directory.
    #[uniffi::constructor]
    pub fn new(scene_path: String) -> Result<Arc<Self>, EngineError> {
        let document = SceneConfig::load(&scene_path)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        let mut built = document
            .clone()
            .build()
            .map_err(|error| EngineError::Scene(error.to_string()))?;

        let (telemetry_producer, telemetry) = rtrb::RingBuffer::new(TELEMETRY_CAPACITY);
        built.scene.telemetry_out = Some(telemetry_producer);
        let (commands, command_consumer) = rtrb::RingBuffer::new(COMMAND_CAPACITY);
        let (to_audio, edit_consumer) = rtrb::RingBuffer::new(EDIT_CAPACITY);
        let (retired_producer, retired) = rtrb::RingBuffer::new(EDIT_CAPACITY);
        built.scene.scene_edits = Some(edit_consumer);
        built.scene.retired_out = Some(retired_producer);
        let layout_channels = built.scene.speaker_layout.total_channels();
        let listener_position = built.scene.listener.position;
        let listener_yaw = built.scene.listener.yaw;
        let output = OutputThread::start(built.scene, command_consumer)?;

        Ok(Arc::new(Self {
            scene: Mutex::new(Live {
                description: built.description.into(),
                document,
                muted: Default::default(),
            }),
            outputs: playable_outputs(layout_channels, output.device_channels),
            commands: Arc::new(Mutex::new(commands)),
            telemetry: Mutex::new(telemetry),
            listener_position,
            listener_yaw,
            head_tracking: Mutex::new(None),
            sounds: built.sounds,
            edits: Mutex::new(Edits {
                to_audio,
                retired,
                auditioned: false,
            }),
            _output: output,
        }))
    }

    /// The scene as it is now: as built, with every level set since.
    pub fn scene(&self) -> SceneDescription {
        lock(&self.scene).description.clone()
    }

    /// Write the scene as it is now to `path`, under `title`. Every change
    /// made through this object is in it: levels, places, shapes, facings,
    /// sounds added and removed. What belongs to the listener is not: mutes,
    /// volume, output and head tracking.
    ///
    /// A file already at `path` is replaced only if the editor wrote it. A
    /// hand-written scene keeps its comments and is refused, so it is saved
    /// under a new name.
    pub fn save_scene(&self, path: String, title: String) -> Result<(), EngineError> {
        let mut scene = lock(&self.scene);
        let title = title.trim();
        if !title.is_empty() {
            scene.document.title = Some(title.to_owned());
        }
        scene
            .document
            .save(&path)
            .map_err(|error| EngineError::Scene(error.to_string()))
    }

    /// Silence the source in `slot`, or let it sound again. It keeps running
    /// while muted, so it comes back where it would have been.
    pub fn set_source_muted(&self, slot: u16, muted: bool) -> Result<(), EngineError> {
        let mut scene = lock(&self.scene);
        lock(&self.commands)
            .push(Command::SetSourceMuted { index: slot, muted })
            .map_err(|_| EngineError::CommandQueueFull)?;
        if muted {
            scene.muted.insert(slot);
        } else {
            scene.muted.remove(&slot);
        }
        Ok(())
    }

    /// Let the sound in `slot` rest between `shortest` and `longest` seconds
    /// between two takes. With both 0 it plays all the time. Anything with
    /// takes can rest, including a recording that loops: it then comes and
    /// goes. `scene()` reports the new gap from then on.
    ///
    /// Within the same way of playing this is immediate. Changing between
    /// looping and resting builds the sound again in its voice, which takes a
    /// moment, like `add_sound`. A generator has no takes and is left alone.
    pub fn set_source_gap(
        &self,
        slot: u16,
        shortest: f32,
        longest: f32,
    ) -> Result<(), EngineError> {
        if !(shortest.is_finite() && longest.is_finite() && shortest >= 0.0 && longest >= shortest)
        {
            return Ok(());
        }
        let (shortest, longest) = (rounded(shortest, 10.0), rounded(longest, 10.0));
        let mut edits = lock(&self.edits);
        let mut scene = lock(&self.scene);
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        if source.takes.is_empty() {
            return Ok(());
        }
        let loops_now = source.gap.is_none();
        let loops_after = longest == 0.0 && source.takes.len() == 1;
        if let Some(entity) = scene.entity_in(slot) {
            entity.when.gap = Some([shortest, longest]);
        }
        if loops_now != loops_after {
            return self.rebuild(&mut edits, &mut scene, slot);
        }
        if loops_after {
            return Ok(());
        }
        lock(&self.commands)
            .push(Command::SetSourceGap {
                index: slot,
                shortest,
                longest,
            })
            .map_err(|_| EngineError::CommandQueueFull)?;
        if let Some(source) = sound_in(&mut scene.description, slot) {
            source.gap = Some(Gap { shortest, longest });
        }
        Ok(())
    }

    /// Choose take `take` of the sound in `slot` with `chance` next to its
    /// others: 3 three times as often as 1, and 0 never. The last take that
    /// can still be chosen cannot be set to 0.
    pub fn set_take_chance(&self, slot: u16, take: u16, chance: f32) -> Result<(), EngineError> {
        if !(chance.is_finite() && chance >= 0.0) {
            return Ok(());
        }
        let chance = rounded(chance, 10.0);
        let mut scene = lock(&self.scene);
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        let Some(name) = source
            .takes
            .get(take as usize)
            .map(|take| take.name.clone())
        else {
            return Ok(());
        };
        let others_can_be_chosen = source
            .takes
            .iter()
            .enumerate()
            .any(|(index, other)| index != take as usize && other.chance > 0.0);
        if chance == 0.0 && !others_can_be_chosen {
            return Ok(());
        }
        lock(&self.commands)
            .push(Command::SetTakeChance {
                index: slot,
                take: take as u8,
                chance,
            })
            .map_err(|_| EngineError::CommandQueueFull)?;
        source.takes[take as usize].chance = chance;
        if let Some(entity) = scene.entity_in(slot) {
            entity.when.chance.insert(name, chance);
        }
        Ok(())
    }

    /// Set the level of the source in `slot`, in dB SPL at its reference
    /// distance. `scene()` reports the new level from then on. A slot that
    /// holds no source, or a level that is not a number, changes nothing.
    pub fn set_source_level(&self, slot: u16, level: f32) -> Result<(), EngineError> {
        let mut scene = lock(&self.scene);
        if !level.is_finite() {
            return Ok(());
        }
        let Some(entity) = scene.entity_in(slot) else {
            return Ok(());
        };
        entity.sound.level = Some(rounded(level, 10.0));
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        lock(&self.commands)
            .push(Command::SetSourceSpl {
                index: slot,
                spl: level,
            })
            .map_err(|_| EngineError::CommandQueueFull)?;
        source.spl = level;
        Ok(())
    }

    /// Move the source in `slot` to `position`. `scene()` reports it there
    /// from then on, as where the source was placed; telemetry has where it
    /// is, which differs for a source that moves by itself.
    pub fn set_source_position(&self, slot: u16, position: Position) -> Result<(), EngineError> {
        let mut scene = lock(&self.scene);
        lock(&self.commands)
            .push(Command::SetSourcePosition {
                index: slot,
                position: position.into(),
            })
            .map_err(|_| EngineError::CommandQueueFull)?;
        if let Some(source) = sound_in(&mut scene.description, slot) {
            source.position = position;
        }
        let place = self.sounds.place_at(position.into());
        if let Some(entity) = scene.entity_in(slot) {
            // Centimetres are plenty for a place, and keep a saved file short.
            entity.place = Place {
                x: rounded(place.x, 100.0),
                y: rounded(place.y, 100.0),
                z: rounded(place.z, 100.0),
            };
        }
        Ok(())
    }

    /// Change how the source in `slot` radiates. `scene()` reports the
    /// pattern and the beam it gives from then on. A field has no beam,
    /// whatever its pattern.
    pub fn set_source_pattern(&self, slot: u16, pattern: Pattern) -> Result<(), EngineError> {
        let mut scene = lock(&self.scene);
        if let Some(entity) = scene.entity_in(slot) {
            entity.shape.pattern = Some(pattern.into());
        }
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        let (radiation, beam_half_angle) = radiation_of(pattern.into());
        lock(&self.commands)
            .push(Command::SetSourceDirectivity {
                index: slot,
                pattern: radiation,
            })
            .map_err(|_| EngineError::CommandQueueFull)?;
        source.pattern = pattern;
        source.beam_half_angle = match source.emitter_kind {
            EmitterKind::Object => beam_half_angle,
            EmitterKind::Field => None,
        };
        Ok(())
    }

    /// Turn the source in `slot` to face along `facing`, a direction over
    /// the ground; its length and height are ignored. A direction with no
    /// length changes nothing.
    pub fn set_source_facing(&self, slot: u16, facing: Position) -> Result<(), EngineError> {
        let length = facing.x.hypot(facing.y);
        if !length.is_finite() || length < 1e-6 {
            return Ok(());
        }
        let mut scene = lock(&self.scene);
        let yaw = facing.y.atan2(facing.x);
        if let Some(entity) = scene.entity_in(slot) {
            entity.shape.facing = Some(rounded(bearing_for_heading(yaw), 1.0) % 360.0);
        }
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        lock(&self.commands)
            .push(Command::SetSourceFacing { index: slot, yaw })
            .map_err(|_| EngineError::CommandQueueFull)?;
        source.facing = Position {
            x: facing.x / length,
            y: facing.y / length,
            z: 0.0,
        };
        Ok(())
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

    /// Play `preset` just ahead of the listener, to hear it before placing
    /// it, or stop with `None`. One preset is auditioned at a time, and it is
    /// not part of the scene: `scene()` does not list it.
    ///
    /// Building the sound decodes its recording or measures its generator, so
    /// this call takes a moment. Keep it off a UI thread.
    pub fn audition(&self, preset: Option<String>) -> Result<(), EngineError> {
        let mut edits = lock(&self.edits);
        edits.stop_audition()?;
        let Some(preset) = preset else {
            return Ok(());
        };
        let taken = lock(&self.scene)
            .description
            .sources
            .iter()
            .any(|source| source.slot as usize == AUDITION_SLOT);
        if taken {
            return Err(EngineError::NoFreeVoice);
        }
        let id = EntityId::unused("audition", []);
        let built = self
            .sounds
            .build(&id, &self.sound_ahead(preset, None, None), AUDITION_SLOT)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        edits.send(SceneEdit::AddSource {
            slot: AUDITION_SLOT as u16,
            source: built.source,
            bands: built.bands,
            amplitude: built.amplitude,
        })?;
        edits.auditioned = true;
        Ok(())
    }

    /// Add a sound made from `preset` to the scene, just ahead of the
    /// listener, and report it. It takes the lowest free voice and an id made
    /// from the preset's name. An audition in progress is stopped. Like
    /// `audition`, this takes a moment.
    pub fn add_sound(&self, preset: String) -> Result<SourceDescription, EngineError> {
        let mut edits = lock(&self.edits);
        edits.stop_audition()?;
        let mut scene = lock(&self.scene);
        let slot = (0..MAX_SOURCES)
            .find(|slot| {
                scene
                    .description
                    .sources
                    .iter()
                    .all(|source| source.slot as usize != *slot)
            })
            .ok_or(EngineError::NoFreeVoice)?;
        let summary =
            read_preset(&preset).map_err(|error| EngineError::Scene(error.to_string()))?;
        let id = EntityId::unused(
            &summary.name,
            scene
                .description
                .sources
                .iter()
                .map(|source| source.id.as_str()),
        );
        let entity = self.sound_ahead(preset, Some(summary.name), summary.group);
        let built = self
            .sounds
            .build(&id, &entity, slot)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        edits.send(SceneEdit::AddSource {
            slot: slot as u16,
            source: built.source,
            bands: built.bands,
            amplitude: built.amplitude,
        })?;
        let added: SourceDescription = built.description.into();
        scene.description.sources.push(added.clone());
        scene
            .document
            .sounds
            .push(id, entity)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        Ok(added)
    }

    /// Take the sound in `slot` out of the scene. Its voice falls silent and
    /// is free for the next sound added; `scene()` no longer lists it. A slot
    /// that holds no sound changes nothing.
    pub fn remove_sound(&self, slot: u16) -> Result<(), EngineError> {
        let mut edits = lock(&self.edits);
        let mut scene = lock(&self.scene);
        let Some(id) = sound_in(&mut scene.description, slot).map(|source| source.id.clone())
        else {
            return Ok(());
        };
        edits.send(SceneEdit::RemoveSource {
            slot,
            filler: SoundBuilder::silence(),
        })?;
        scene
            .description
            .sources
            .retain(|source| source.slot != slot);
        scene.muted.remove(&slot);
        if let Ok(id) = EntityId::new(&id) {
            scene.document.sounds.remove(&id);
        }
        Ok(())
    }

    /// Let `tracker` steer the listener, or stop with `None`. Stopping turns
    /// the listener back to the way the scene faces it. Any tracker already
    /// running is stopped first, and has let go of its port when this returns.
    pub fn set_head_tracker(&self, tracker: Option<HeadTracker>) -> Result<(), EngineError> {
        let mut head_tracking = lock(&self.head_tracking);
        // Stop before starting: both listen on the same port.
        *head_tracking = None;
        let Some(tracker) = tracker else {
            return Ok(());
        };
        if !head_trackers().contains(&tracker) {
            return Err(EngineError::HeadTracking(
                "that tracker does not run on this platform".into(),
            ));
        }
        let started = atrium::head_tracking::start(
            tracker.into(),
            self.commands.clone(),
            self.listener_position,
            self.listener_yaw,
        )
        .map_err(|error| EngineError::HeadTracking(error.to_string()))?;
        *head_tracking = Some(started);
        Ok(())
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

impl Engine {
    /// Build the sound in `slot` again from the scene file and put it in its
    /// voice in place of the one playing, keeping it muted if it was.
    fn rebuild(&self, edits: &mut Edits, scene: &mut Live, slot: u16) -> Result<(), EngineError> {
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        let id =
            EntityId::new(&source.id).map_err(|error| EngineError::Scene(error.to_string()))?;
        let Some(entity) = scene
            .document
            .sounds
            .iter()
            .find(|(own, _)| **own == id)
            .map(|(_, entity)| entity.clone())
        else {
            return Ok(());
        };
        let mut built = self
            .sounds
            .build(&id, &entity, slot as usize)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        if scene.muted.contains(&slot) {
            built.source.set_muted(true);
        }
        edits.send(SceneEdit::AddSource {
            slot,
            source: built.source,
            bands: built.bands,
            amplitude: built.amplitude,
        })?;
        if let Some(source) = sound_in(&mut scene.description, slot) {
            *source = built.description.into();
        }
        Ok(())
    }

    /// A sound made from `preset`, standing a short way ahead of where the
    /// scene puts the listener, at ear height.
    fn sound_ahead(&self, preset: String, name: Option<String>, group: Option<String>) -> Entity {
        let ahead = Vec3::new(self.listener_yaw.cos(), self.listener_yaw.sin(), 0.0);
        Entity {
            name,
            color: None,
            group,
            sound: Sound {
                preset: Some(preset),
                audio: None,
                level: None,
            },
            shape: Shape::default(),
            place: self
                .sounds
                .place_at(self.listener_position + ahead * AHEAD_OF_LISTENER),
            when: When::default(),
            moves: Moves::Still,
        }
    }
}

/// The sound playing from `slot`, if the scene has one there.
fn sound_in(description: &mut SceneDescription, slot: u16) -> Option<&mut SourceDescription> {
    description
        .sources
        .iter_mut()
        .find(|source| source.slot == slot)
}

/// The scene twice over: as a UI shows it, and as a scene file saves it.
/// Both change together, under one lock, so they cannot disagree.
struct Live {
    description: SceneDescription,
    /// The scene file as loaded, with every edit since.
    document: SceneConfig,
    /// The voices the listener silenced, so a sound built again stays silent.
    /// They belong to the listener, not the scene, and are never saved.
    muted: std::collections::BTreeSet<u16>,
}

impl Live {
    /// The entity playing from `slot`, to record a change to it.
    fn entity_in(&mut self, slot: u16) -> Option<&mut Entity> {
        let id = self
            .description
            .sources
            .iter()
            .find(|source| source.slot == slot)?
            .id
            .clone();
        self.document.sounds.get_mut(&EntityId::new(&id).ok()?)
    }
}

/// `value` rounded to steps of `1 / per_unit`.
fn rounded(value: f32, per_unit: f32) -> f32 {
    (value * per_unit).round() / per_unit
}

/// The way sounds reach the audio thread while it runs, and come back.
struct Edits {
    to_audio: rtrb::Producer<SceneEdit>,
    /// Sounds the audio thread took out. Freeing memory is not safe there,
    /// so it sends them back to be dropped on the calling thread.
    retired: rtrb::Consumer<Retired>,
    /// Whether a preset is playing from the audition voice.
    auditioned: bool,
}

impl Edits {
    fn send(&mut self, edit: SceneEdit) -> Result<(), EngineError> {
        while self.retired.pop().is_ok() {}
        self.to_audio
            .push(edit)
            .map_err(|_| EngineError::CommandQueueFull)
    }

    fn stop_audition(&mut self) -> Result<(), EngineError> {
        if self.auditioned {
            self.send(SceneEdit::RemoveSource {
                slot: AUDITION_SLOT as u16,
                filler: SoundBuilder::silence(),
            })?;
            self.auditioned = false;
        }
        Ok(())
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

    /// Opens the default output device at zero volume and binds the tracker
    /// port, so nothing else may be listening on it:
    /// `cargo test -p atrium-ffi -- --ignored`.
    #[test]
    #[ignore = "opens the default output device and the head tracking port"]
    fn head_tracking_holds_its_port_only_while_it_is_on() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        // OpenTrack's default port, which the engine listens on.
        let port_is_free = || std::net::UdpSocket::bind(("127.0.0.1", 4242)).is_ok();
        assert!(port_is_free(), "stop the running head tracker first");

        let engine = Engine::new("scenes/frog.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");

        engine
            .set_head_tracker(Some(HeadTracker::External))
            .expect("the listener starts");
        assert!(!port_is_free(), "the tracker listens while it is on");

        // Asking again restarts it on the same port instead of failing.
        engine
            .set_head_tracker(Some(HeadTracker::External))
            .expect("a running tracker can be chosen again");

        engine.set_head_tracker(None).expect("the listener stops");
        assert!(port_is_free(), "switching off frees the port at once");

        // A scene change closes the engine with tracking still on. The next
        // scene's engine binds the same port straight away.
        engine
            .set_head_tracker(Some(HeadTracker::External))
            .expect("the listener starts again");
        drop(engine);
        assert!(port_is_free(), "closing the engine stops head tracking");
    }

    /// The newest telemetry for which `ready` holds, waiting up to two seconds.
    fn telemetry_when(engine: &Engine, ready: impl Fn(&Telemetry) -> bool) -> Telemetry {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(telemetry) = engine.latest_telemetry() {
                if ready(&telemetry) {
                    return telemetry;
                }
            }
            assert!(Instant::now() < deadline, "the engine never reported it");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    /// What the app's map and Settings read: whether a tracker is chosen,
    /// whether it is sending, and where the head points. A stand-in tracker
    /// sends one OpenTrack packet over loopback. Opens the default output
    /// device at zero volume: `cargo test -p atrium-ffi -- --ignored`.
    #[test]
    #[ignore = "opens the default output device and the head tracking port"]
    fn telemetry_shows_whether_a_tracker_is_sending_and_where_the_head_points() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        // The frog scene faces the listener north: +Y.
        let engine = Engine::new("scenes/frog.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");

        let at_rest = telemetry_when(&engine, |_| true);
        assert_eq!(at_rest.head_tracking, HeadTrackingStatus::Off);
        assert!(at_rest.listener_facing.y > 0.99, "the scene faces north");

        engine
            .set_head_tracker(Some(HeadTracker::External))
            .expect("the listener starts");
        let listening = telemetry_when(&engine, |telemetry| {
            telemetry.head_tracking != HeadTrackingStatus::Off
        });
        assert_eq!(
            listening.head_tracking,
            HeadTrackingStatus::Waiting,
            "chosen, but nothing has been sent"
        );

        // A 90° left turn, which a tracker reports as −90°. North turns west.
        let left_turn: Vec<u8> = [0.0f64, 0.0, 0.0, -90.0, 0.0, 0.0]
            .iter()
            .flat_map(|value| value.to_le_bytes())
            .collect();
        std::net::UdpSocket::bind(("127.0.0.1", 0))
            .and_then(|tracker| tracker.send_to(&left_turn, ("127.0.0.1", 4242)))
            .expect("the stand-in tracker sends");
        let turned = telemetry_when(&engine, |telemetry| {
            telemetry.head_tracking == HeadTrackingStatus::Receiving
                && telemetry.listener_facing.x < -0.99
        });
        assert!(turned.listener_facing.y.abs() < 0.1, "the head points west");

        engine.set_head_tracker(None).expect("the listener stops");
        telemetry_when(&engine, |telemetry| {
            telemetry.head_tracking == HeadTrackingStatus::Off && telemetry.listener_facing.y > 0.99
        });
    }

    /// What the editor's layers and library do to a playing scene, at zero
    /// volume: `cargo test -p atrium-ffi -- --ignored --test-threads=1`.
    #[test]
    #[ignore = "opens the default output device"]
    fn layers_and_library_change_the_playing_scene() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine = Engine::new("scenes/frog.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");
        let playing = |telemetry: &Telemetry, slot: usize| {
            telemetry
                .sources
                .iter()
                .any(|source| source.slot as usize == slot)
        };
        let scene = engine.scene();
        assert_eq!(scene.sources.len(), 1);
        assert_eq!(scene.sources[0].id, "frogs");
        assert_eq!(scene.sources[0].group.as_deref(), Some("Creatures"));

        // Hearing a preset before placing it: it plays, and is not in the scene.
        engine
            .audition(Some("sources/cricket.yaml".into()))
            .expect("the cricket builds");
        telemetry_when(&engine, |telemetry| playing(telemetry, AUDITION_SLOT));
        assert_eq!(
            engine.scene().sources.len(),
            1,
            "an audition is not a layer"
        );
        engine.audition(None).expect("the audition stops");
        telemetry_when(&engine, |telemetry| !playing(telemetry, AUDITION_SLOT));

        // Adding one: the next voice, an id from the preset's name, its group,
        // and a place 2 m ahead of a listener who faces north.
        let added = engine
            .add_sound("sources/frog.yaml".into())
            .expect("the frog builds");
        assert_eq!((added.id.as_str(), added.name.as_str()), ("frog", "Frog"));
        assert_eq!(added.group.as_deref(), Some("Creatures"));
        assert!((added.position.y - scene.listener.y - 2.0).abs() < 1e-3);
        assert!((added.position.x - scene.listener.x).abs() < 1e-3);
        let second = engine
            .add_sound("sources/frog.yaml".into())
            .expect("a second frog builds");
        assert_eq!(second.id, "frog-2", "ids stay unique");
        assert_eq!(engine.scene().sources.len(), 3);
        telemetry_when(&engine, |telemetry| {
            playing(telemetry, 1) && playing(telemetry, 2)
        });

        // A layer's mute and level.
        engine.set_source_muted(1, true).expect("mute");
        telemetry_when(&engine, |telemetry| {
            telemetry
                .sources
                .iter()
                .any(|source| source.slot == 1 && source.muted)
        });
        let level_of = |slot: u16| {
            engine
                .scene()
                .sources
                .iter()
                .find(|source| source.slot == slot)
                .map(|source| source.spl)
        };
        engine.set_source_level(1, 50.0).expect("level");
        assert_eq!(level_of(1), Some(50.0));

        // Removing a layer: its voice falls silent, the scene forgets it, and
        // the next sound added takes the freed voice and the freed id.
        engine.remove_sound(1).expect("remove");
        telemetry_when(&engine, |telemetry| !playing(telemetry, 1));
        let left: Vec<(String, u16)> = engine
            .scene()
            .sources
            .iter()
            .map(|source| (source.id.clone(), source.slot))
            .collect();
        assert_eq!(left, [("frogs".to_string(), 0), ("frog-2".to_string(), 2)]);
        engine
            .remove_sound(1)
            .expect("an empty voice is left alone");
        let again = engine
            .add_sound("sources/frog.yaml".into())
            .expect("a frog builds again");
        assert_eq!((again.id.as_str(), again.slot), ("frog", 1));
        telemetry_when(&engine, |telemetry| playing(telemetry, 1));
        assert_eq!(engine.scene().sources.len(), 3);
        assert_eq!(engine.scene().sources[2].id, "frog", "added ones go last");

        // A shape and a direction: a cardioid's beam is 90° to each side, and
        // a sound turned north faces +Y on the audio thread too.
        assert_eq!(engine.scene().sources[0].pattern, Pattern::Omni);
        assert_eq!(engine.scene().sources[0].beam_half_angle, None);
        engine
            .set_source_pattern(0, Pattern::Cardioid)
            .expect("pattern");
        let shaped = engine.scene().sources[0].clone();
        assert_eq!(shaped.pattern, Pattern::Cardioid);
        let beam = shaped.beam_half_angle.expect("a cardioid has a beam");
        assert!((beam - std::f32::consts::FRAC_PI_2).abs() < 1e-5);
        let north = Position {
            x: 0.0,
            y: 3.0,
            z: 9.0,
        };
        engine.set_source_facing(0, north).expect("facing");
        let turned = engine.scene().sources[0].facing;
        assert!(turned.x.abs() < 1e-6 && (turned.y - 1.0).abs() < 1e-6 && turned.z == 0.0);
        telemetry_when(&engine, |telemetry| {
            telemetry.sources.iter().any(|source| {
                source.slot == 0 && source.facing.x.abs() < 1e-3 && source.facing.y > 0.999
            })
        });

        // A sound dragged on the map: the scene remembers where it was put.
        let moved = Position {
            x: added.position.x + 4.0,
            y: added.position.y - 1.0,
            z: added.position.z,
        };
        engine.set_source_position(1, moved).expect("move");
        let placed = engine
            .scene()
            .sources
            .iter()
            .find(|source| source.slot == 1)
            .map(|source| source.position);
        assert_eq!(placed, Some(moved));
        telemetry_when(&engine, |telemetry| {
            telemetry
                .sources
                .iter()
                .any(|source| source.slot == 1 && source.position == moved)
        });
        assert_eq!(level_of(2), Some(65.0), "only that layer");

        // Saving: every change above is in the file, and a hand-written scene
        // is never saved over.
        let saved = std::env::temp_dir().join(format!("atrium-saved-{}.yaml", std::process::id()));
        let saved = saved.to_str().expect("utf-8").to_owned();
        let _ = std::fs::remove_file(&saved);
        assert!(engine
            .save_scene("scenes/frog.yaml".into(), "Frog".into())
            .is_err());
        engine
            .save_scene(saved.clone(), "Three frogs".into())
            .expect("a new file");
        let written = SceneConfig::load(&saved).expect("the saved scene reads");
        assert_eq!(written.title.as_deref(), Some("Three frogs"));
        let ids: Vec<&str> = written.sounds.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["frogs", "frog-2", "frog"]);
        let (_, frogs) = written.sounds.iter().next().expect("frogs");
        assert_eq!(frogs.shape.pattern, Some(atrium_world::Pattern::Cardioid));
        assert_eq!(frogs.shape.facing, Some(0.0), "north");
        let (_, frog) = written.sounds.iter().nth(2).expect("frog");
        let expected = engine.sounds.place_at(moved.into());
        assert!(
            (frog.place.x - expected.x).abs() < 0.01 && (frog.place.y - expected.y).abs() < 0.01
        );
        let rebuilt = written.build().expect("the saved scene builds");
        assert_eq!(rebuilt.description.sources.len(), 3);
        let _ = std::fs::remove_file(&saved);
    }

    /// When a sound plays: its rest and its takes' chances change while it
    /// plays, a loop can be made to come and go and back, and a muted sound
    /// stays muted through that. At zero volume:
    /// `cargo test -p atrium-ffi -- --ignored --test-threads=1`.
    #[test]
    #[ignore = "opens the default output device"]
    fn rests_and_chances_change_while_playing() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine = Engine::new("scenes/frog.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");
        let gap_of = |slot: u16| {
            engine
                .scene()
                .sources
                .iter()
                .find(|source| source.slot == slot)
                .expect("the sound")
                .gap
        };
        assert!(gap_of(0).is_some(), "a frog croaks now and then");

        engine.set_source_muted(0, true).expect("mute");
        engine.set_source_gap(0, 0.0, 0.0).expect("no rest");
        assert_eq!(gap_of(0), None, "one take with no rest loops");
        telemetry_when(&engine, |telemetry| {
            telemetry
                .sources
                .iter()
                .any(|source| source.slot == 0 && source.muted)
        });
        engine.set_source_gap(0, 1.0, 2.04).expect("rests again");
        assert_eq!(
            gap_of(0),
            Some(Gap {
                shortest: 1.0,
                longest: 2.0
            }),
            "kept to a tenth of a second"
        );
        engine
            .set_source_gap(0, 3.0, 1.0)
            .expect("a backwards gap is ignored");
        assert_eq!(gap_of(0).map(|gap| gap.longest), Some(2.0));

        let cricket = engine
            .add_sound("sources/cricket.yaml".into())
            .expect("a cricket builds");
        assert!(cricket.takes.len() > 1);
        for take in 0..cricket.takes.len() as u16 {
            engine
                .set_take_chance(cricket.slot, take, 0.0)
                .expect("chance");
        }
        let chances: Vec<f32> = engine
            .scene()
            .sources
            .iter()
            .find(|source| source.slot == cricket.slot)
            .expect("cricket")
            .takes
            .iter()
            .map(|take| take.chance)
            .collect();
        assert_eq!(
            chances.iter().filter(|chance| **chance > 0.0).count(),
            1,
            "the last take that can be chosen keeps its chance: {chances:?}"
        );

        let saved = std::env::temp_dir().join(format!("atrium-rests-{}.yaml", std::process::id()));
        let saved = saved.to_str().expect("utf-8").to_owned();
        let _ = std::fs::remove_file(&saved);
        engine
            .save_scene(saved.clone(), String::new())
            .expect("saves");
        let written = SceneConfig::load(&saved).expect("reads");
        let (_, frogs) = written.sounds.iter().next().expect("frogs");
        assert_eq!(frogs.when.gap, Some([1.0, 2.0]));
        let (_, cricket) = written.sounds.iter().nth(1).expect("cricket");
        assert_eq!(
            cricket
                .when
                .chance
                .values()
                .filter(|chance| **chance == 0.0)
                .count(),
            chances.len() - 1
        );
        written.build().expect("the saved scene builds");
        let _ = std::fs::remove_file(&saved);
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
