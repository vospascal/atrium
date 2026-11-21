//! The running engine: scene, audio output, and the two rings that connect a
//! UI to the audio thread.

mod undo;

use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::thread::JoinHandle;

use atrium::audio::distance::DistanceModel;
use atrium::audio::loudness::{level_map, Emitter, Grid};
use atrium::audio::output::{AudioOutput, CpalOutput};
use atrium::config::{
    bearing_for_heading, radiation_of, read_preset, SceneConfig, SoundBuilder, WeatherConfig,
};
use atrium::engine::edit::{Retired, SceneEdit};
use atrium::engine::scene::AudioScene;
use atrium::head_tracking::{HeadTracking, Placement, Pose};
use atrium::plan::{Condition, Planner};
use atrium_core::commands::Command;
use atrium_core::directivity::SILENT_BEHIND_DB;
use atrium_core::telemetry::{TelemetryFrame, MAX_SOURCES};
use atrium_core::types::Vec3;
use atrium_world::{Entity, EntityId, Moves, Place, Shape, Sound, When};

use crate::api::{
    Band, Bout, Conditions, EmitterKind, EngineError, Gap, LoudnessMap, PlannedTake, Position,
    SceneDescription, SourceDescription, Telemetry, Weather,
};
use crate::head_tracking::{head_trackers, HeadTracker, HeadTrackingStatus};
use crate::history::History;
use crate::output::{playable_outputs, Output};

/// Commands the audio thread can fall behind by before new ones are refused.
const COMMAND_CAPACITY: usize = 256;
/// Telemetry frames held for the UI; it only ever reads the newest.
const TELEMETRY_CAPACITY: usize = 4;
/// Sound changes the audio thread can fall behind by. Each is one press in a
/// UI, so a handful is plenty.
const EDIT_CAPACITY: usize = 8;
/// How long, and how often, a full edit ring is waited on: some 200 ms, many
/// audio callbacks.
const EDIT_WAIT: std::time::Duration = std::time::Duration::from_millis(5);
const EDIT_WAITS: usize = 40;
/// How far east of its original a copy stands (m).
const COPY_OFFSET: f32 = 1.0;
/// How far ahead of the listener an auditioned or newly added sound stands (m).
const AHEAD_OF_LISTENER: f32 = 2.0;
/// The loudest a preview of a kind plays (dB SPL at 1 m), however loud the
/// kind itself is.
const PREVIEW_LEVEL_LIMIT: f32 = 80.0;
/// The most moments one `conditions` call returns.
const MAX_CONDITION_STEPS: u32 = 2048;
/// How far ahead `planned` looks at most (s).
const MAX_PLANNED_SECONDS: f32 = 60.0;
/// The narrowest beam a sound can be given, and all round (degrees).
const MIN_WIDTH: f32 = 10.0;
const ALL_ROUND: f32 = 360.0;
/// The coldest and warmest air a scene can be given (°C).
const COLDEST: f32 = -20.0;
const WARMEST: f32 = 50.0;
/// The most points a loudness map has along one side.
const MAX_MAP_SIDE: u32 = 160;
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
            edits: Mutex::new(Edits {
                to_audio,
                retired,
                auditioned: false,
            }),
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
        self.planner.set_gap(slot as usize, shortest, longest);
        if let Some(source) = sound_in(&mut scene.description, slot) {
            source.gap = Some(Gap { shortest, longest });
        }
        Ok(())
    }

    /// The scene's weather: its story and the band of each condition.
    pub fn weather(&self) -> Weather {
        let weather = self.planner.weather();
        let band = |condition| {
            let (low, high) = weather.band(condition);
            Band { low, high }
        };
        Weather {
            story: weather.story.into(),
            wind: band(Condition::Wind),
            rain: band(Condition::Rain),
            storm: band(Condition::Storm),
            daylight: band(Condition::Daylight),
        }
    }

    /// Let `weather` move the conditions from now on, and keep it with the
    /// scene. Every sound that follows a condition follows the new story;
    /// what is already handed to the audio thread still plays.
    pub fn set_weather(&self, weather: Weather) -> Result<(), EngineError> {
        let bands = [weather.wind, weather.rain, weather.storm, weather.daylight];
        if bands
            .iter()
            .any(|band| !(band.low.is_finite() && band.high.is_finite()))
        {
            return Ok(());
        }
        let story: atrium::plan::Story = weather.story.into();
        let in_file = |band: Band| {
            let (low, high) = (band.low.clamp(0.0, 1.0), band.high.clamp(0.0, 1.0));
            let (low, high) = (rounded(low.min(high), 100.0), rounded(low.max(high), 100.0));
            if low == high {
                atrium::config::Band::At(low)
            } else {
                atrium::config::Band::Between([low, high])
            }
        };
        let weather = WeatherConfig {
            story: story.name().to_string(),
            wind: in_file(weather.wind),
            rain: in_file(weather.rain),
            storm: in_file(weather.storm),
            daylight: in_file(weather.daylight),
        };
        let mut scene = lock(&self.scene);
        self.play_weather(Some(&weather))?;
        scene.document.weather = Some(weather);
        Ok(())
    }

    /// The conditions from `from` to `to` seconds around now (negative is
    /// the past), at `steps` evenly spaced moments. The weather is a pure
    /// function of time, so the past is what was, and the future what will be
    /// unless the weather is changed.
    pub fn conditions(&self, from: f32, to: f32, steps: u32) -> Vec<Conditions> {
        let steps = steps.clamp(1, MAX_CONDITION_STEPS);
        (0..steps)
            .map(|step| {
                let along = if steps == 1 {
                    0.0
                } else {
                    step as f32 / (steps - 1) as f32
                };
                self.planner
                    .conditions((from + (to - from) * along) as f64)
                    .into()
            })
            .collect()
    }

    /// Pause the scene, or play it on. Paused, the session's clock stops:
    /// every sound, the plan and the weather hold where they are, and resume
    /// from there. It fades out and in, so nothing clicks.
    pub fn set_paused(&self, paused: bool) -> Result<(), EngineError> {
        lock(&self.commands)
            .push(Command::SetPaused { paused })
            .map_err(|_| EngineError::CommandQueueFull)?;
        self.paused
            .store(paused, std::sync::atomic::Ordering::Relaxed);
        Ok(())
    }

    /// Whether the scene is paused.
    pub fn paused(&self) -> bool {
        self.paused.load(std::sync::atomic::Ordering::Relaxed)
    }

    /// Let the weather run `speed` times as fast as the scene: 60 runs an hour
    /// of weather in a minute, a time-lapse, while every sound still plays
    /// at its own pace.
    pub fn set_weather_speed(&self, speed: f32) {
        self.planner.set_weather_speed(speed as f64);
    }

    pub fn weather_speed(&self) -> f32 {
        self.planner.weather_speed() as f32
    }

    /// Seconds into the scene: what a timeline's playhead shows. It stands
    /// still while the scene is paused, and jumps when it is moved.
    pub fn scene_time(&self) -> f32 {
        self.planner.now() as f32
    }

    /// Move the scene to `seconds`, as a playhead is dragged along its
    /// timeline: every sound and the weather are as they would be there, a
    /// take under way starts partway, and it plays on from that moment.
    pub fn seek(&self, seconds: f32) {
        self.planner.seek(seconds as f64);
    }

    /// The takes the plan has decided for the next `seconds`, at most a
    /// minute ahead.
    pub fn planned(&self, seconds: f32) -> Vec<PlannedTake> {
        self.planner
            .upcoming(seconds.clamp(0.0, MAX_PLANNED_SECONDS) as f64)
            .into_iter()
            .map(|(slot, planned)| PlannedTake {
                slot: slot as u16,
                take: planned.take,
                starts_in: planned.at as f32,
                seconds: planned.seconds as f32,
            })
            .collect()
    }

    /// Let the sound in `slot` utter one take `fewest` to `most` times
    /// running before it chooses another: 1 and 1 changes every time.
    /// Immediate; a sound that loops or has no takes is left alone.
    pub fn set_source_bout(&self, slot: u16, fewest: u32, most: u32) -> Result<(), EngineError> {
        if fewest < 1 || most < fewest {
            return Ok(());
        }
        let mut scene = lock(&self.scene);
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        if source.gap.is_none() {
            return Ok(());
        }
        self.planner.set_bout(slot as usize, fewest, most);
        source.bout = Bout { fewest, most };
        if let Some(entity) = scene.entity_in(slot) {
            entity.when.bout = Some([fewest, most]);
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
        self.planner
            .set_chance(slot as usize, take as usize, chance);
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
        let level = rounded(level, 10.0);
        entity.sound.level = Some(level);
        self.play_level(&mut scene, slot, level)
    }

    /// Move the source in `slot` to `position`. `scene()` reports it there
    /// from then on, as where the source was placed; telemetry has where it
    /// is, which differs for a source that moves by itself.
    pub fn set_source_position(&self, slot: u16, position: Position) -> Result<(), EngineError> {
        let mut scene = lock(&self.scene);
        self.play_position(&mut scene, slot, position)?;
        let place = self.place_rounded(position);
        if let Some(entity) = scene.entity_in(slot) {
            entity.place = place;
        }
        Ok(())
    }

    /// Change how the source in `slot` radiates: `width` degrees wide, from
    /// a narrow beam up to 360 (all round), with the sound straight behind
    /// it `behind` dB below the front, −60 or lower for nothing. `scene()`
    /// reports both and the beam they give from then on. A field has no
    /// beam, whatever its width.
    pub fn set_source_beam(&self, slot: u16, width: f32, behind: f32) -> Result<(), EngineError> {
        if !(width.is_finite() && behind.is_finite()) {
            return Ok(());
        }
        let width = rounded(width.clamp(MIN_WIDTH, ALL_ROUND), 1.0);
        let behind = rounded(behind.clamp(SILENT_BEHIND_DB, 0.0), 1.0);
        let mut scene = lock(&self.scene);
        if let Some(entity) = scene.entity_in(slot) {
            entity.shape.width = Some(width);
            entity.shape.behind = Some(behind);
        }
        self.play_beam(&mut scene, slot, width, behind)
    }

    /// Make the source in `slot` sound `size` wide, from 0 (a point) to 1
    /// (all around). A wider sound spreads over more loudspeakers and feeds
    /// less of the room's reverb; from 0.75 it feeds none. `scene()` reports
    /// it from then on. A field ignores its size.
    pub fn set_source_size(&self, slot: u16, size: f32) -> Result<(), EngineError> {
        if !size.is_finite() {
            return Ok(());
        }
        let size = rounded(size.clamp(0.0, 1.0), 100.0);
        let mut scene = lock(&self.scene);
        if let Some(entity) = scene.entity_in(slot) {
            entity.shape.size = Some(size);
        }
        self.play_size(&mut scene, slot, size)
    }

    /// Make the source in `slot` ambient, heard everywhere as the air of the
    /// place, or a point again. An ambient sound has no distance, direction,
    /// beam or echo: its level is what is heard wherever the listener
    /// stands, so one that stood far off becomes much louder. This builds
    /// the sound again in its voice, which takes a moment, like `add_sound`,
    /// and a recording starts over.
    pub fn set_source_ambient(&self, slot: u16, ambient: bool) -> Result<(), EngineError> {
        let mut edits = lock(&self.edits);
        let mut scene = lock(&self.scene);
        let Some(entity) = scene.entity_in(slot) else {
            return Ok(());
        };
        let before = entity.shape.ambient;
        entity.shape.ambient = Some(ambient);
        let rebuilt = self.rebuild(&mut edits, &mut scene, slot);
        if rebuilt.is_err() {
            if let Some(entity) = scene.entity_in(slot) {
                entity.shape.ambient = before;
            }
        }
        rebuilt
    }

    /// Set the air sound travels through: `temperature` in °C, from −20 to
    /// 50, and relative `humidity` in %. They decide how fast sound travels
    /// and how much treble the air takes from a distant sound. `scene()`
    /// reports them from then on, and the scene file keeps them as its own.
    pub fn set_atmosphere(&self, temperature: f32, humidity: f32) -> Result<(), EngineError> {
        if !(temperature.is_finite() && humidity.is_finite()) {
            return Ok(());
        }
        let temperature = rounded(temperature.clamp(COLDEST, WARMEST), 2.0);
        let humidity = rounded(humidity.clamp(0.0, 100.0), 1.0);
        let mut scene = lock(&self.scene);
        scene
            .document
            .set_air(temperature, humidity)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        self.play_air(&mut scene, temperature, humidity)
    }

    /// How loud the scene is over the ground between `from` and `to`, at
    /// `from`'s height, on `columns` by `rows` points: all its sounds that
    /// are not silenced, or only the one in `only`. Each point adds up the
    /// direct sound of every source, from its level as `scene()` reports it,
    /// the scene's distance law and the source's beam.
    pub fn loudness(
        &self,
        from: Position,
        to: Position,
        columns: u32,
        rows: u32,
        only: Option<u16>,
    ) -> LoudnessMap {
        let columns = columns.clamp(1, MAX_MAP_SIDE);
        let rows = rows.clamp(1, MAX_MAP_SIDE);
        let (west, east) = (from.x.min(to.x), from.x.max(to.x));
        let (south, north) = (from.y.min(to.y), from.y.max(to.y));
        let corner = Position {
            x: west,
            y: south,
            z: from.z,
        };
        if !(finite(from) && finite(to)) {
            return LoudnessMap {
                corner,
                cell: 0.0,
                columns: 0,
                rows: 0,
                levels_db: Vec::new(),
            };
        }
        // Square cells that cover the wider side.
        let cell = ((east - west) / columns as f32).max((north - south) / rows as f32);
        let grid = Grid {
            // Each point stands in the middle of its cell.
            corner: Vec3::new(west + cell / 2.0, south + cell / 2.0, from.z),
            cell,
            columns: columns as usize,
            rows: rows as usize,
        };
        let scene = lock(&self.scene);
        let emitters: Vec<Emitter> = scene
            .description
            .sources
            .iter()
            .filter(|source| match only {
                Some(slot) => source.slot == slot,
                None => !scene.muted.contains(&source.slot),
            })
            .map(|source| Emitter {
                position: source.position.into(),
                level_db: source.spl,
                reference_distance: source.reference_distance,
                pattern: radiation_of(source.width, source.behind).0,
                facing: source.facing.into(),
                ambient: source.emitter_kind == EmitterKind::Field,
            })
            .collect();
        LoudnessMap {
            corner,
            cell,
            columns,
            rows,
            levels_db: level_map(&emitters, &self.distance, &grid),
        }
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
        self.play_facing(&mut scene, slot, yaw)
    }

    /// Move the listener to `position`, facing the way it faced. Every sound
    /// is heard from there at once; head tracking turns from there; the
    /// scene file keeps it. A position that is not a number changes nothing.
    pub fn set_listener_position(&self, position: Position) -> Result<(), EngineError> {
        if !finite(position) {
            return Ok(());
        }
        let mut scene = lock(&self.scene);
        let pose = Pose {
            position: position.into(),
            ..self.placement.get()
        };
        self.play_listener(&mut scene, pose)?;
        scene.document.listener = self.listener_in_file(pose);
        Ok(())
    }

    /// Turn the listener to face along `facing`, a direction over the ground,
    /// as `set_source_facing` turns a sound. Head tracking turns from there.
    pub fn set_listener_facing(&self, facing: Position) -> Result<(), EngineError> {
        let length = facing.x.hypot(facing.y);
        if !length.is_finite() || length < 1e-6 {
            return Ok(());
        }
        let mut scene = lock(&self.scene);
        let pose = Pose {
            yaw: facing.y.atan2(facing.x),
            ..self.placement.get()
        };
        self.play_listener(&mut scene, pose)?;
        scene.document.listener = self.listener_in_file(pose);
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
        self.planner.remove_voice(AUDITION_SLOT);
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
        // Heard two metres away, a kind as loud as thunder would hurt: a
        // preview is never louder than a raised voice.
        let level = read_preset(&preset)
            .map_err(|error| EngineError::Scene(error.to_string()))?
            .level;
        let mut previewed = self.sound_ahead(preset, None, None);
        previewed.sound.level = Some(level.min(PREVIEW_LEVEL_LIMIT));
        let built = self
            .sounds
            .build(&id, &previewed, AUDITION_SLOT)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        edits.send(SceneEdit::AddSource {
            slot: AUDITION_SLOT as u16,
            source: built.source,
            bands: built.bands,
            amplitude: built.amplitude,
        })?;
        // A kind that rests is told by the plan when to speak; heard on its
        // own it speaks at once.
        self.planner.preview_voice(AUDITION_SLOT, built.rhythm);
        edits.auditioned = true;
        Ok(())
    }

    /// Play take `take` of the sound in `slot` once, just ahead of the
    /// listener and all round, to hear it on its own; or stop with `None`.
    /// It plays from the audition voice, like `audition`, and stops any
    /// audition in progress. Telemetry reports the audition voice's take while
    /// it sounds and nothing once it has ended.
    ///
    /// Building it decodes the recording, so this takes a moment. Keep it off
    /// a UI thread.
    pub fn audition_take(&self, slot: u16, take: Option<u16>) -> Result<(), EngineError> {
        let mut edits = lock(&self.edits);
        edits.stop_audition()?;
        self.planner.remove_voice(AUDITION_SLOT);
        let Some(take) = take else {
            return Ok(());
        };
        let scene = lock(&self.scene);
        let Some(source) = scene
            .description
            .sources
            .iter()
            .find(|source| source.slot == slot)
        else {
            return Ok(());
        };
        let id =
            EntityId::new(&source.id).map_err(|error| EngineError::Scene(error.to_string()))?;
        let Some(mut entity) = scene
            .document
            .sounds
            .iter()
            .find(|(own, _)| **own == id)
            .map(|(_, entity)| entity.clone())
        else {
            return Ok(());
        };
        drop(scene);
        entity.place = self.place_ahead();
        entity.shape.width = Some(ALL_ROUND);
        entity.moves = Moves::Still;
        let built = self
            .sounds
            .build_take(&id, &entity, take as usize, AUDITION_SLOT)
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

    /// Add a sound made from `preset` to the scene and report it: at
    /// `position`, where it was dropped, or else just ahead of the listener.
    /// It takes the lowest free voice and an id made from the preset's name.
    /// An audition in progress is stopped. Like `audition`, this takes a
    /// moment.
    pub fn add_sound(
        &self,
        preset: String,
        position: Option<Position>,
    ) -> Result<SourceDescription, EngineError> {
        let summary =
            read_preset(&preset).map_err(|error| EngineError::Scene(error.to_string()))?;
        let mut entity = self.sound_ahead(preset, Some(summary.name.clone()), summary.group);
        if let Some(position) = position.filter(|position| finite(*position)) {
            entity.place = self.place_rounded(position);
        }
        self.add_entity(&summary.name, entity)
    }

    /// Add a copy of the sound in `slot`, named as a copy, and report it: at
    /// `position`, where it was dropped, or else a metre east of the
    /// original. Everything else about it is the same: its kind, level,
    /// shape, rests and weather. Like `add_sound`, this takes a moment.
    pub fn duplicate_sound(
        &self,
        slot: u16,
        position: Option<Position>,
    ) -> Result<SourceDescription, EngineError> {
        let (id, mut entity) = {
            let mut scene = lock(&self.scene);
            let id = sound_in(&mut scene.description, slot)
                .map(|source| (source.id.clone(), source.name.clone()));
            let entity = scene.entity_in(slot).cloned();
            match (id, entity) {
                (Some(id), Some(entity)) => (id, entity),
                _ => {
                    return Err(EngineError::Scene(format!(
                        "no sound plays from voice {slot}"
                    )))
                }
            }
        };
        let (id, name) = id;
        entity.name = Some(format!("{name} copy"));
        match position.filter(|position| finite(*position)) {
            Some(position) => entity.place = self.place_rounded(position),
            None => entity.place.x += COPY_OFFSET,
        }
        self.add_entity(&id, entity)
    }

    /// Show the sound in `slot` as `name`, in `color` (`#rrggbb`) and under
    /// `group` in a list. An empty name goes back to its id, no colour to its
    /// voice's own, and no group to none. Nothing heard changes. A colour
    /// that is not `#rrggbb` changes nothing.
    pub fn set_source_identity(
        &self,
        slot: u16,
        name: String,
        color: Option<String>,
        group: Option<String>,
    ) -> Result<(), EngineError> {
        if color.as_deref().is_some_and(|color| !is_color(color)) {
            return Ok(());
        }
        let wanted = |text: Option<String>| {
            text.map(|text| text.trim().to_owned())
                .filter(|text| !text.is_empty())
        };
        let mut scene = lock(&self.scene);
        let Some(id) = sound_in(&mut scene.description, slot).map(|source| source.id.clone())
        else {
            return Ok(());
        };
        let Some(entity) = scene.entity_in(slot) else {
            return Ok(());
        };
        entity.name = wanted(Some(name));
        entity.color = wanted(color);
        entity.group = wanted(group);
        let entity = entity.clone();
        let id = EntityId::new(&id).map_err(|error| EngineError::Scene(error.to_string()))?;
        let (name, color) = atrium::config::name_and_color(&id, &entity, slot as usize);
        if let Some(source) = sound_in(&mut scene.description, slot) {
            source.name = name;
            source.color = color;
            source.group = entity.group;
        }
        Ok(())
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
        self.play_removed(&mut edits, &mut scene, slot)?;
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
            self.placement.clone(),
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
        let rhythm = built.rhythm.take();
        let follows = std::mem::take(&mut built.follows);
        edits.send(SceneEdit::AddSource {
            slot,
            source: built.source,
            bands: built.bands,
            amplitude: built.amplitude,
        })?;
        // After the new voice is on its way, so no utterance reaches the old one.
        self.planner
            .add_voice(slot as usize, id.as_str(), rhythm, follows);
        if let Some(source) = sound_in(&mut scene.description, slot) {
            *source = built.description.into();
        }
        Ok(())
    }

    /// A short way ahead of the listener, at ear height, in the scene file's
    /// coordinates.
    fn place_ahead(&self) -> Place {
        let pose = self.placement.get();
        let ahead = Vec3::new(pose.yaw.cos(), pose.yaw.sin(), 0.0);
        self.sounds
            .place_at(pose.position + ahead * AHEAD_OF_LISTENER)
    }

    /// Put the listener at `pose`: heard from there at once, head tracking
    /// turns from it, and `scene()` reports it.
    fn play_listener(&self, scene: &mut Live, pose: Pose) -> Result<(), EngineError> {
        self.command(Command::SetListenerPose {
            position: pose.position,
            yaw: pose.yaw,
        })?;
        self.placement.set(pose);
        scene.description.listener = pose.position.into();
        Ok(())
    }

    /// The listener as the scene file writes it, for `pose`.
    fn listener_in_file(&self, pose: Pose) -> atrium::config::ListenerConfig {
        let place = self.place_rounded(pose.position.into());
        atrium::config::ListenerConfig {
            position: [place.x, place.y, place.z],
            yaw_degrees: rounded(pose.yaw.to_degrees().rem_euclid(360.0), 1.0),
        }
    }

    /// Add `entity` to the scene under an id like `wanted`, in the lowest
    /// free voice, stopping any audition first.
    fn add_entity(&self, wanted: &str, entity: Entity) -> Result<SourceDescription, EngineError> {
        let mut edits = lock(&self.edits);
        edits.stop_audition()?;
        self.planner.remove_voice(AUDITION_SLOT);
        let mut scene = lock(&self.scene);
        let id = EntityId::unused(
            wanted,
            scene
                .description
                .sources
                .iter()
                .map(|source| source.id.as_str()),
        );
        let added = self.play_added(&mut edits, &mut scene, &id, &entity)?;
        scene
            .document
            .sounds
            .push(id, entity)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        Ok(added)
    }

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

    // Each `play_` call below changes what is heard and what `scene()`
    // reports, and leaves the scene file to its caller: an edit records what
    // was asked, and undo puts back a whole earlier version at once.

    /// Set the level of the sound in `slot`.
    fn play_level(&self, scene: &mut Live, slot: u16, level: f32) -> Result<(), EngineError> {
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        self.command(Command::SetSourceSpl {
            index: slot,
            spl: level,
        })?;
        source.spl = level;
        Ok(())
    }

    /// Move the sound in `slot` to `position`, in the world.
    fn play_position(
        &self,
        scene: &mut Live,
        slot: u16,
        position: Position,
    ) -> Result<(), EngineError> {
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        self.command(Command::SetSourcePosition {
            index: slot,
            position: position.into(),
        })?;
        source.position = position;
        Ok(())
    }

    /// Give the sound in `slot` a beam `width` degrees wide, `behind` dB
    /// down straight behind it.
    fn play_beam(
        &self,
        scene: &mut Live,
        slot: u16,
        width: f32,
        behind: f32,
    ) -> Result<(), EngineError> {
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        let (radiation, beam_half_angle) = radiation_of(width, behind);
        self.command(Command::SetSourceDirectivity {
            index: slot,
            pattern: radiation,
        })?;
        source.width = width;
        source.behind = behind;
        source.beam_half_angle = match source.emitter_kind {
            EmitterKind::Object => beam_half_angle,
            EmitterKind::Field => None,
        };
        Ok(())
    }

    /// Make the sound in `slot` sound `size` wide.
    fn play_size(&self, scene: &mut Live, slot: u16, size: f32) -> Result<(), EngineError> {
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        self.command(Command::SetSourceSpread {
            index: slot,
            spread: size,
        })?;
        source.size = size;
        Ok(())
    }

    /// Give the place air at `temperature` °C and `humidity` %.
    fn play_air(
        &self,
        scene: &mut Live,
        temperature: f32,
        humidity: f32,
    ) -> Result<(), EngineError> {
        self.command(Command::SetAtmosphere {
            temperature_c: temperature,
            humidity_pct: humidity,
        })?;
        scene.description.temperature_c = temperature;
        scene.description.humidity_pct = humidity;
        Ok(())
    }

    /// Turn the sound in `slot` to the heading `yaw` (radians from +x).
    fn play_facing(&self, scene: &mut Live, slot: u16, yaw: f32) -> Result<(), EngineError> {
        let Some(source) = sound_in(&mut scene.description, slot) else {
            return Ok(());
        };
        self.command(Command::SetSourceFacing { index: slot, yaw })?;
        source.facing = Position {
            x: yaw.cos(),
            y: yaw.sin(),
            z: 0.0,
        };
        Ok(())
    }

    /// Let `weather` move the conditions from now on; none is a calm day.
    fn play_weather(&self, weather: Option<&WeatherConfig>) -> Result<(), EngineError> {
        let seed = self.planner.seed();
        let weather = match weather {
            Some(weather) => weather
                .story(seed)
                .map_err(|error| EngineError::Scene(error.to_string()))?,
            None => atrium::plan::Weather::calm(seed),
        };
        self.planner.set_weather(weather);
        Ok(())
    }

    /// Build `entity` as `id` in the lowest free voice and start it.
    fn play_added(
        &self,
        edits: &mut Edits,
        scene: &mut Live,
        id: &EntityId,
        entity: &Entity,
    ) -> Result<SourceDescription, EngineError> {
        let slot = (0..MAX_SOURCES)
            .find(|slot| {
                scene
                    .description
                    .sources
                    .iter()
                    .all(|source| source.slot as usize != *slot)
            })
            .ok_or(EngineError::NoFreeVoice)?;
        let built = self
            .sounds
            .build(id, entity, slot)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        edits.send(SceneEdit::AddSource {
            slot: slot as u16,
            source: built.source,
            bands: built.bands,
            amplitude: built.amplitude,
        })?;
        self.planner
            .add_voice(slot, id.as_str(), built.rhythm, built.follows);
        let added: SourceDescription = built.description.into();
        scene.description.sources.push(added.clone());
        Ok(added)
    }

    /// Silence the voice in `slot` and free it.
    fn play_removed(
        &self,
        edits: &mut Edits,
        scene: &mut Live,
        slot: u16,
    ) -> Result<(), EngineError> {
        edits.send(SceneEdit::RemoveSource {
            slot,
            filler: SoundBuilder::silence(),
        })?;
        self.planner.remove_voice(slot as usize);
        scene
            .description
            .sources
            .retain(|source| source.slot != slot);
        scene.muted.remove(&slot);
        Ok(())
    }

    fn command(&self, command: Command) -> Result<(), EngineError> {
        lock(&self.commands)
            .push(command)
            .map_err(|_| EngineError::CommandQueueFull)
    }

    /// A sound made from `preset`, standing a short way ahead of where the
    /// scene puts the listener, at ear height.
    fn sound_ahead(&self, preset: String, name: Option<String>, group: Option<String>) -> Entity {
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
            place: self.place_ahead(),
            when: When::default(),
            follows: None,
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

/// Whether every coordinate of `position` is a number.
fn finite(position: Position) -> bool {
    position.x.is_finite() && position.y.is_finite() && position.z.is_finite()
}

/// Whether `color` is written `#rrggbb`.
fn is_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color[1..].chars().all(|digit| digit.is_ascii_hexdigit())
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
    /// Hand `edit` to the audio thread. Undo can send several at once, more
    /// than the ring holds, so a full ring is waited on for a few audio
    /// callbacks before the edit is refused.
    fn send(&mut self, edit: SceneEdit) -> Result<(), EngineError> {
        let mut edit = edit;
        for _ in 0..EDIT_WAITS {
            while self.retired.pop().is_ok() {}
            match self.to_audio.push(edit) {
                Ok(()) => return Ok(()),
                Err(rtrb::PushError::Full(returned)) => edit = returned,
            }
            std::thread::sleep(EDIT_WAIT);
        }
        Err(EngineError::CommandQueueFull)
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

    use crate::api::Story;
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
            .add_sound("sources/frog.yaml".into(), None)
            .expect("the frog builds");
        assert_eq!((added.id.as_str(), added.name.as_str()), ("frog", "Frog"));
        assert_eq!(added.group.as_deref(), Some("Creatures"));
        assert!((added.position.y - scene.listener.y - 2.0).abs() < 1e-3);
        assert!((added.position.x - scene.listener.x).abs() < 1e-3);
        let second = engine
            .add_sound("sources/frog.yaml".into(), None)
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
            .add_sound("sources/frog.yaml".into(), None)
            .expect("a frog builds again");
        assert_eq!((again.id.as_str(), again.slot), ("frog", 1));
        telemetry_when(&engine, |telemetry| playing(telemetry, 1));
        assert_eq!(engine.scene().sources.len(), 3);
        assert_eq!(engine.scene().sources[2].id, "frog", "added ones go last");

        // A shape and a direction: a half circle's beam is 90° to each side, and
        // a sound turned north faces +Y on the audio thread too.
        assert_eq!(engine.scene().sources[0].width, 360.0);
        assert_eq!(engine.scene().sources[0].beam_half_angle, None);
        engine.set_source_beam(0, 180.0, -60.0).expect("width");
        let shaped = engine.scene().sources[0].clone();
        assert_eq!((shaped.width, shaped.behind), (180.0, -60.0));
        let beam = shaped.beam_half_angle.expect("a half circle has a beam");
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
        assert_eq!(frogs.shape.width, Some(180.0));
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
            .add_sound("sources/cricket.yaml".into(), None)
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

        // Bouts, and one take heard on its own from the audition voice.
        engine.set_source_bout(cricket.slot, 2, 3).expect("bout");
        engine
            .set_source_bout(cricket.slot, 3, 1)
            .expect("a backwards bout is ignored");
        let bout = engine
            .scene()
            .sources
            .iter()
            .find(|source| source.slot == cricket.slot)
            .expect("cricket")
            .bout;
        assert_eq!(bout, Bout { fewest: 2, most: 3 });
        engine
            .audition_take(cricket.slot, Some(0))
            .expect("one take on its own");
        telemetry_when(&engine, |telemetry| {
            telemetry
                .sources
                .iter()
                .any(|source| source.slot as usize == AUDITION_SLOT && source.take == Some(0))
        });
        assert_eq!(
            engine.scene().sources.len(),
            2,
            "an audition is not a layer"
        );
        engine.audition_take(cricket.slot, None).expect("stops");

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
        assert_eq!(cricket.when.bout, Some([2, 3]));
        written.build().expect("the saved scene builds");
        let _ = std::fs::remove_file(&saved);
    }

    /// The session's plan reaches the audio thread: birds whose takes are
    /// decided ahead are heard uttering them, and a rest made short is heard
    /// at once. At zero volume:
    /// `cargo test -p atrium-ffi -- --ignored --test-threads=1`.
    #[test]
    #[ignore = "opens the default output device"]
    fn planned_takes_are_heard_and_a_shorter_rest_at_once() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine = Engine::new("scenes/meadow-birds.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");
        let blackbird = engine
            .scene()
            .sources
            .iter()
            .find(|source| source.id == "blackbird")
            .expect("a blackbird")
            .slot;
        engine
            .set_source_gap(blackbird, 0.1, 0.2)
            .expect("a short rest");
        let deadline = Instant::now() + Duration::from_secs(3);
        let mut heard = std::collections::BTreeSet::new();
        while Instant::now() < deadline {
            if let Some(telemetry) = engine.latest_telemetry() {
                for source in &telemetry.sources {
                    if source.slot == blackbird {
                        heard.extend(source.take);
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            !heard.is_empty(),
            "the blackbird was planned but never heard"
        );

        // Paused, the scene's clock stands still: what is planned does not
        // come closer. Scrubbing the weather moves its story at once.
        engine.set_paused(true).expect("pause");
        std::thread::sleep(Duration::from_millis(300));
        let before = engine.planned(30.0).first().map(|take| take.starts_in);
        std::thread::sleep(Duration::from_millis(500));
        let after = engine.planned(30.0).first().map(|take| take.starts_in);
        assert!(engine.paused());
        assert_eq!(before, after, "a paused plan stands still");
        let paused_at = engine.scene_time();
        std::thread::sleep(Duration::from_millis(300));
        assert_eq!(
            engine.scene_time(),
            paused_at,
            "the scene's time stands still"
        );
        engine.set_paused(false).expect("play");
        engine.seek(1234.0);
        assert!(
            (engine.scene_time() - 1234.0).abs() < 0.5,
            "moved to 1234 s"
        );
        std::thread::sleep(Duration::from_millis(300));
        assert!(engine.scene_time() > 1234.0, "and plays on from there");
        engine.set_weather_speed(60.0);
        assert_eq!(engine.weather_speed(), 60.0);

        // What is planned is there to read, and the weather can be changed and
        // saved; conditions are known for the past and the future alike.
        assert!(engine
            .planned(30.0)
            .iter()
            .any(|take| take.slot == blackbird));
        engine
            .set_weather(Weather {
                story: Story::PassingStorm,
                wind: Band {
                    low: 0.1,
                    high: 0.3,
                },
                rain: Band {
                    low: 0.0,
                    high: 0.0,
                },
                storm: Band {
                    low: 0.0,
                    high: 0.0,
                },
                daylight: Band {
                    low: 1.0,
                    high: 1.0,
                },
            })
            .expect("weather");
        assert_eq!(engine.weather().story, Story::PassingStorm);
        let hour = engine.conditions(-3600.0, 3600.0, 721);
        assert_eq!(hour.len(), 721);
        assert!(hour.iter().all(|now| (0.0..=1.0).contains(&now.wind)));
        let saved =
            std::env::temp_dir().join(format!("atrium-weather-{}.yaml", std::process::id()));
        let saved = saved.to_str().expect("utf-8").to_owned();
        let _ = std::fs::remove_file(&saved);
        engine
            .save_scene(saved.clone(), String::new())
            .expect("saves");
        let written = SceneConfig::load(&saved).expect("reads");
        let weather = written.weather.clone().expect("the weather is saved");
        assert_eq!(weather.story, "passing storm");
        assert_eq!(weather.wind, atrium::config::Band::Between([0.1, 0.3]));
        written.build().expect("the saved scene builds");
        let _ = std::fs::remove_file(&saved);
    }

    /// Hearing a kind before placing it, and one of a sound's takes on its
    /// own: both are heard within a moment, even for a kind that rests
    /// between takes. At zero volume:
    /// `cargo test -p atrium-ffi -- --ignored --test-threads=1`.
    #[test]
    #[ignore = "opens the default output device"]
    fn a_kind_and_a_take_are_heard_at_once_on_their_own() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine = Engine::new("scenes/meadow-birds.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");
        let heard_within = |seconds: u64| {
            let deadline = Instant::now() + Duration::from_secs(seconds);
            while Instant::now() < deadline {
                if let Some(telemetry) = engine.latest_telemetry() {
                    if telemetry.sources.iter().any(|source| {
                        source.slot as usize == AUDITION_SLOT && source.take.is_some()
                    }) {
                        return true;
                    }
                }
                std::thread::sleep(Duration::from_millis(20));
            }
            false
        };
        engine
            .audition(Some("sources/robin.yaml".into()))
            .expect("the robin builds");
        assert!(
            heard_within(2),
            "a kind that rests is heard at once when previewed"
        );
        engine.audition(None).expect("stops");

        let robin = engine
            .scene()
            .sources
            .iter()
            .find(|source| source.id == "robin")
            .expect("a robin")
            .slot;
        engine.audition_take(robin, Some(2)).expect("one take");
        assert!(heard_within(2), "one take on its own is heard at once");
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
