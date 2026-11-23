//! Undo and redo for the playing scene.
//!
//! A UI calls `remember` just before each step it lets the user take, under
//! the step's name. Going back puts an earlier version in place while it plays:
//! the world crate says, sound by sound, the smallest changes that get there
//! ([`atrium_world::changes`]), and each is played the way the edit that made
//! it was. A step that changed the place builds the scene again there
//! instead, as `set_place` does. Only what the scene file holds and the
//! editor changes is kept: the scene's kind and place, the sounds and their
//! layers, the weather, the air and where the listener stands. Mutes,
//! volume, output and head tracking belong to the listener, and stay as they
//! are.

use atrium::config::{
    name_and_color, yaw_for_bearing, Atmosphere, ListenerConfig, SceneKind, WeatherConfig,
};
use atrium::head_tracking::Pose;
use atrium_world::{changes, Change, Entities};

use super::edits::Edits;
use super::{lock, sound_in, Engine, Live};
use crate::api::{EditHistory, EngineError};

/// What undo puts back.
#[derive(Debug, Clone, PartialEq)]
pub(super) struct Version {
    kind: SceneKind,
    /// The environment file it plays in; none for the dry space.
    space: Option<String>,
    sounds: Entities,
    weather: Option<WeatherConfig>,
    listener: ListenerConfig,
    /// The air as the scene file names it.
    atmosphere: Option<Atmosphere>,
}

impl Version {
    fn of(scene: &Live) -> Self {
        Self {
            kind: scene.document.kind,
            space: scene.document.environment.clone(),
            sounds: scene.document.sounds.clone(),
            weather: scene.document.weather.clone(),
            listener: scene.document.listener.clone(),
            atmosphere: scene.document.atmosphere.clone(),
        }
    }
}

#[uniffi::export]
impl Engine {
    /// Keep the scene as it is now, just before the step `label` ("Move
    /// robin"), so it can be undone. Call it once per step: at the start of a
    /// drag, not at every point of it. A step that changes nothing is passed
    /// over by undo.
    pub fn remember(&self, label: String) {
        let mut history = lock(&self.history);
        let version = Version::of(&lock(&self.scene));
        history.remember(&label, version);
    }

    /// Put back the scene as it was before the last step, while it plays.
    /// Returns whether there was a step to undo. Bringing back a removed
    /// sound builds it again, which takes a moment, like `add_sound`.
    pub fn undo(&self) -> Result<bool, EngineError> {
        let mut history = lock(&self.history);
        let current = Version::of(&lock(&self.scene));
        match history.undo(current) {
            Some(version) => self.restore(version).map(|()| true),
            None => Ok(false),
        }
    }

    /// Take again the last step undone. Returns whether there was one.
    pub fn redo(&self) -> Result<bool, EngineError> {
        let mut history = lock(&self.history);
        let current = Version::of(&lock(&self.scene));
        match history.redo(current) {
            Some(version) => self.restore(version).map(|()| true),
            None => Ok(false),
        }
    }

    /// The names of the steps that can be undone and redone.
    pub fn history(&self) -> EditHistory {
        let history = lock(&self.history);
        EditHistory {
            undoable: history.undoable(),
            redoable: history.redoable(),
        }
    }
}

impl Engine {
    /// Make `version` the scene, changing only what differs.
    fn restore(&self, version: Version) -> Result<(), EngineError> {
        let mut edits = lock(&self.edits);
        self.end_audition(&mut edits)?;
        let mut scene = lock(&self.scene);
        if scene.document.environment != version.space {
            let mut document = scene.document.clone();
            document.kind = version.kind;
            document.environment = version.space;
            document.sounds = version.sounds;
            document.weather = version.weather;
            document.atmosphere = version.atmosphere;
            document.listener = version.listener;
            return self.play_place(&mut edits, &mut scene, document);
        }
        let found = changes(&scene.document.sounds, &version.sounds);
        // First, so a sound built again is built as it was.
        scene.document.sounds = version.sounds;
        let new_kind = scene.document.kind != version.kind;
        let new_weather = scene.document.weather != version.weather;
        let new_air = scene.document.atmosphere != version.atmosphere;
        scene.document.weather = version.weather;
        scene.document.atmosphere = version.atmosphere;
        if new_kind {
            self.play_kind(&mut scene, version.kind)?;
        } else {
            if new_weather {
                self.play_weather(&scene.document)?;
            }
            if new_air {
                let air = scene
                    .document
                    .air()
                    .map_err(|error| EngineError::Scene(error.to_string()))?;
                self.play_air(&mut scene, air.temperature_c, air.humidity_pct)?;
            }
        }
        if scene.document.listener != version.listener {
            let place = atrium_world::Place {
                x: version.listener.position[0],
                y: version.listener.position[1],
                z: version.listener.position[2],
            };
            let pose = Pose {
                position: self.sounds().world_at(place),
                yaw: version.listener.yaw_degrees.to_radians(),
            };
            self.play_listener(&mut scene, pose)?;
            scene.document.listener = version.listener;
        }
        for change in found {
            self.play_change(&mut edits, &mut scene, change)?;
        }
        let order: Vec<String> = scene
            .document
            .sounds
            .iter()
            .map(|(id, _)| id.to_string())
            .collect();
        scene
            .description
            .sources
            .sort_by_key(|source| order.iter().position(|id| *id == source.id));
        Ok(())
    }

    fn play_change(
        &self,
        edits: &mut Edits,
        scene: &mut Live,
        change: Change,
    ) -> Result<(), EngineError> {
        let slot_of = |scene: &Live, id: &atrium_world::EntityId| {
            scene
                .description
                .sources
                .iter()
                .find(|source| source.id == id.as_str())
                .map(|source| source.slot)
        };
        match change {
            Change::Added { id, entity } => {
                self.play_added(edits, scene, &id, &entity)?;
            }
            Change::Removed { id } => {
                if let Some(slot) = slot_of(scene, &id) {
                    self.play_removed(edits, scene, slot)?;
                }
            }
            Change::Rebuilt { id, .. } => {
                if let Some(slot) = slot_of(scene, &id) {
                    self.rebuild(edits, scene, slot)?;
                }
            }
            Change::Moved { id, place } => {
                if let Some(slot) = slot_of(scene, &id) {
                    let position = self.sounds().world_at(place).into();
                    self.play_position(scene, slot, position)?;
                }
            }
            Change::Level { id, level } => {
                if let Some(slot) = slot_of(scene, &id) {
                    self.play_level(scene, slot, level)?;
                }
            }
            Change::Layers { id, layers } => {
                if let Some(slot) = slot_of(scene, &id) {
                    self.play_layers(scene, slot, &layers)?;
                }
            }
            Change::Beam { id, width, behind } => {
                if let Some(slot) = slot_of(scene, &id) {
                    self.play_beam(scene, slot, width, behind)?;
                }
            }
            Change::Size { id, size } => {
                if let Some(slot) = slot_of(scene, &id) {
                    self.play_size(scene, slot, size)?;
                }
            }
            Change::Arc { id, degrees } => {
                if let Some(slot) = slot_of(scene, &id) {
                    self.play_arc(scene, slot, degrees)?;
                }
            }
            Change::Facing { id, facing } => {
                if let Some(slot) = slot_of(scene, &id) {
                    self.play_facing(scene, slot, yaw_for_bearing(facing))?;
                }
            }
            // The scene file already holds the wanted name; it is only shown.
            Change::Identity { id, .. } => {
                let (Some(slot), Some(entity)) =
                    (slot_of(scene, &id), scene.document.sounds.get(&id))
                else {
                    return Ok(());
                };
                let (name, color) = name_and_color(&id, entity, slot as usize);
                let group = entity.group.clone();
                if let Some(source) = sound_in(&mut scene.description, slot) {
                    source.name = name;
                    source.color = color;
                    source.group = group;
                }
            }
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::{Band, Position, Story, Weather};

    /// Every kind of edit, undone one by one back to the scene as loaded and
    /// redone to the end, leaves the scene exactly as it was at each step.
    #[test]
    fn every_edit_undoes_and_redoes() {
        std::env::set_current_dir(concat!(env!("CARGO_MANIFEST_DIR"), "/../.."))
            .expect("repository root");
        let engine = Engine::new("scenes/frog.yaml".into()).expect("scene should play");
        engine.set_volume(0.0).expect("volume");
        let loaded = engine.scene();
        let frogs = loaded.sources[0].slot;
        let mut seen = vec![(engine.scene(), engine.weather())];
        let mut step = |label: &str, edit: &dyn Fn(&Engine)| {
            engine.remember(label.into());
            edit(&engine);
            seen.push((engine.scene(), engine.weather()));
        };
        step("Move", &|engine| {
            engine
                .set_source_position(
                    frogs,
                    Position {
                        x: 3.0,
                        y: 4.0,
                        z: 0.0,
                    },
                )
                .expect("move");
        });
        step("Louder", &|engine| {
            engine.set_source_level(frogs, 71.5).expect("level")
        });
        step("Narrow", &|engine| {
            engine.set_source_beam(frogs, 90.0, -20.0).expect("beam")
        });
        step("Turn", &|engine| {
            engine
                .set_source_facing(
                    frogs,
                    Position {
                        x: 1.0,
                        y: 0.0,
                        z: 0.0,
                    },
                )
                .expect("facing")
        });
        step("Wide", &|engine| {
            engine.set_source_size(frogs, 0.4).expect("size")
        });
        step("Air", &|engine| {
            engine.set_atmosphere(8.0, 85.0).expect("air");
            assert_eq!(engine.scene().temperature_c, 8.0);
            assert_eq!(engine.scene().humidity_pct, 85.0);
        });
        step("Ambient", &|engine| {
            engine.set_source_ambient(frogs, true).expect("ambient");
            let frog = engine.scene().sources[0].clone();
            assert_eq!(frog.emitter_kind, crate::api::EmitterKind::Field);
            assert_eq!(frog.beam_half_angle, None);
            assert_eq!(frog.size, 0.4, "it keeps what was set before");
            assert_eq!(frog.arc, 360.0, "all round until narrowed");
        });
        step("Arc", &|engine| {
            engine.set_source_arc(frogs, 89.6).expect("arc");
            assert_eq!(engine.scene().sources[0].arc, 90.0, "whole degrees");
        });
        step("Add", &|engine| {
            engine
                .add_sound("sources/creatures/cricket.yaml".into(), None)
                .expect("add");
        });
        step("Rename", &|engine| {
            engine
                .set_source_identity(frogs, "Pond".into(), Some("#336699".into()), None)
                .expect("identity")
        });
        step("Duplicate", &|engine| {
            let copy = engine.duplicate_sound(frogs, None).expect("duplicate");
            assert_eq!(copy.name, "Pond copy");
            assert_eq!(copy.color, "#336699");
            assert_eq!(copy.spl, 71.5);
            let original = engine.scene().sources[0].clone();
            assert!((copy.position.x - original.position.x - 1.0).abs() < 1e-3);
        });
        step("Drop", &|engine| {
            let dropped = engine
                .add_sound(
                    "sources/creatures/frog.yaml".into(),
                    Some(Position {
                        x: -5.0,
                        y: 6.0,
                        z: 1.2,
                    }),
                )
                .expect("drop");
            assert_eq!((dropped.position.x, dropped.position.y), (-5.0, 6.0));
        });
        step("Remove", &|engine| {
            engine.remove_sound(frogs).expect("remove")
        });
        step("Listener", &|engine| {
            engine
                .set_listener_position(Position {
                    x: 2.0,
                    y: -3.0,
                    z: 1.6,
                })
                .expect("listener moves");
            engine
                .set_listener_facing(Position {
                    x: 0.0,
                    y: -1.0,
                    z: 0.0,
                })
                .expect("listener turns");
        });
        step("Storm", &|engine| {
            let band = |low, high| Band { low, high };
            engine
                .set_weather(Weather {
                    story: Story::PassingStorm,
                    wind: band(0.1, 0.6),
                    rain: band(0.0, 0.8),
                    storm: band(0.0, 1.0),
                    daylight: band(0.5, 0.5),
                    wind_from: 300.0,
                })
                .expect("weather");
        });
        // Telemetry shows facings and slots; what a UI shows is compared
        // through `scene()`, by id, since a sound put back may take another
        // voice.
        let by_id = |scene: &crate::api::SceneDescription| {
            let mut sources = scene.sources.clone();
            for source in &mut sources {
                source.slot = 0;
                source.color = String::new();
            }
            sources
        };
        assert_eq!(engine.history().undoable.len(), seen.len() - 1);
        for expected in seen.iter().rev().skip(1) {
            assert!(engine.undo().expect("undo"));
            assert_eq!(by_id(&engine.scene()), by_id(&expected.0));
            assert_eq!(engine.scene().listener, expected.0.listener);
            assert_eq!(
                (engine.scene().temperature_c, engine.scene().humidity_pct),
                (expected.0.temperature_c, expected.0.humidity_pct)
            );
            assert_eq!(engine.weather(), expected.1);
        }
        assert!(!engine.undo().expect("nothing more"));
        for expected in seen.iter().skip(1) {
            assert!(engine.redo().expect("redo"));
            assert_eq!(by_id(&engine.scene()), by_id(&expected.0));
            assert_eq!(engine.scene().listener, expected.0.listener);
            assert_eq!(engine.weather(), expected.1);
        }
        assert!(!engine.redo().expect("nothing more"));
    }
}
