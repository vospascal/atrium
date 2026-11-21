//! The running engine: scene, audio output, and the two rings that connect a
//! UI to the audio thread.
//!
//! This file holds the object, how it starts, and what every edit shares.
//! The exported methods are grouped by what they change, one module each.
//!
//! Every edit is two halves. A `set_` method records what was asked in the
//! scene file and calls a `play_` helper; each `play_` helper changes what is
//! heard and what `scene()` reports, and leaves the scene file to its caller.
//! Undo puts back a whole earlier version of the file at once and calls the
//! same helpers.

mod audition;
mod device;
mod edits;
mod listener;
mod loudness;
mod rhythm;
mod shape;
mod sounds;
#[cfg(test)]
mod test_support;
#[cfg(test)]
mod tests;
mod transport;
mod undo;
mod weather;

use std::sync::{Arc, Mutex, MutexGuard};

use atrium::audio::distance::DistanceModel;
use atrium::config::{SceneConfig, SoundBuilder};
use atrium::head_tracking::{HeadTracking, Placement, Pose};
use atrium::plan::Planner;
use atrium_core::commands::Command;
use atrium_core::telemetry::{TelemetryFrame, MAX_SOURCES};
use atrium_world::{Entity, EntityId, Place};

use crate::api::{EngineError, Position, SceneDescription, SourceDescription};
use crate::history::History;
use crate::output::{playable_outputs, Output};

use device::OutputThread;
use edits::Edits;

/// Commands the audio thread can fall behind by before new ones are refused.
const COMMAND_CAPACITY: usize = 256;
/// Telemetry frames held for the UI; it only ever reads the newest.
const TELEMETRY_CAPACITY: usize = 4;
/// Sound changes the audio thread can fall behind by. Each is one press in a
/// UI, so a handful is plenty.
const EDIT_CAPACITY: usize = 8;
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
    /// Where the scene puts the listener and which way it faces. Head
    /// tracking turns from it, and it moves when the listener is moved.
    placement: Placement,
    /// The running tracker, if any.
    head_tracking: Mutex<Option<HeadTracking>>,
    /// Builds one more sound for this scene, as its own sounds were built.
    sounds: SoundBuilder,
    /// How the scene's sounds fade with distance; fixed for its lifetime.
    distance: DistanceModel,
    /// The session's plan: what every voice with takes will do, decided ahead.
    planner: Planner,
    /// Whether the scene was last paused; the audio thread keeps its own.
    paused: std::sync::atomic::AtomicBool,
    edits: Mutex<Edits>,
    /// Earlier and later versions of the scene, for undo and redo.
    history: Mutex<History<undo::Version>>,
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
        let distance = built.scene.distance_model;
        let placement = Placement::new(Pose {
            position: built.scene.listener.position,
            yaw: built.scene.listener.yaw,
        });
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
            placement,
            head_tracking: Mutex::new(None),
            sounds: built.sounds,
            distance,
            planner: built.planner,
            paused: std::sync::atomic::AtomicBool::new(false),
            edits: Mutex::new(Edits::new(to_audio, retired)),
            history: Mutex::new(History::default()),
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
}

impl Engine {
    /// Where `position` is in the scene file, to the centimetre: plenty for
    /// a place, and it keeps a saved file short.
    fn place_rounded(&self, position: Position) -> Place {
        let place = self.sounds.place_at(position.into());
        Place {
            x: rounded(place.x, 100.0),
            y: rounded(place.y, 100.0),
            z: rounded(place.z, 100.0),
        }
    }

    fn command(&self, command: Command) -> Result<(), EngineError> {
        lock(&self.commands)
            .push(command)
            .map_err(|_| EngineError::CommandQueueFull)
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

/// Whether every coordinate of `position` is a number.
fn finite(position: Position) -> bool {
    position.x.is_finite() && position.y.is_finite() && position.z.is_finite()
}

/// `value` rounded to steps of `1 / per_unit`.
fn rounded(value: f32, per_unit: f32) -> f32 {
    (value * per_unit).round() / per_unit
}

/// A poisoned lock only means another call panicked mid-push; the ring itself
/// is still consistent, so keep using it.
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}
