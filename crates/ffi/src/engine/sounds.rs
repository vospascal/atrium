//! The scene's sounds: adding, copying, naming, silencing and removing one,
//! and building one again in its voice.

use atrium::config::{name_and_color, read_preset, SoundBuilder};
use atrium::engine::edit::SceneEdit;
use atrium_core::commands::Command;
use atrium_core::telemetry::MAX_SOURCES;
use atrium_world::{Entity, EntityId, Moves, Shape, Sound, When};

use super::edits::Edits;
use super::{finite, lock, sound_in, Engine, Live};
use crate::api::{EngineError, Position, SourceDescription};

/// How far east of its original a copy stands (m).
const COPY_OFFSET: f32 = 1.0;

#[uniffi::export]
impl Engine {
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
        let (name, color) = name_and_color(&id, &entity, slot as usize);
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
}

impl Engine {
    /// Build the sound in `slot` again from the scene file and put it in its
    /// voice in place of the one playing, keeping it muted if it was.
    pub(super) fn rebuild(
        &self,
        edits: &mut Edits,
        scene: &mut Live,
        slot: u16,
    ) -> Result<(), EngineError> {
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
            .sounds()
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
        self.planner()
            .add_voice(slot as usize, id.as_str(), rhythm, follows);
        if let Some(source) = sound_in(&mut scene.description, slot) {
            *source = built.description.into();
        }
        Ok(())
    }

    /// Add `entity` to the scene under an id like `wanted`, in the lowest
    /// free voice, stopping any audition first.
    fn add_entity(&self, wanted: &str, entity: Entity) -> Result<SourceDescription, EngineError> {
        let mut edits = lock(&self.edits);
        self.end_audition(&mut edits)?;
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

    /// Build `entity` as `id` in the lowest free voice and start it.
    pub(super) fn play_added(
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
            .sounds()
            .build(id, entity, slot)
            .map_err(|error| EngineError::Scene(error.to_string()))?;
        edits.send(SceneEdit::AddSource {
            slot: slot as u16,
            source: built.source,
            bands: built.bands,
            amplitude: built.amplitude,
        })?;
        self.planner()
            .add_voice(slot, id.as_str(), built.rhythm, built.follows);
        let added: SourceDescription = built.description.into();
        scene.description.sources.push(added.clone());
        Ok(added)
    }

    /// Silence the voice in `slot` and free it.
    pub(super) fn play_removed(
        &self,
        edits: &mut Edits,
        scene: &mut Live,
        slot: u16,
    ) -> Result<(), EngineError> {
        edits.send(SceneEdit::RemoveSource {
            slot,
            filler: SoundBuilder::silence(),
        })?;
        self.planner().remove_voice(slot as usize);
        scene
            .description
            .sources
            .retain(|source| source.slot != slot);
        scene.muted.remove(&slot);
        Ok(())
    }

    /// A sound made from `preset`, standing a short way ahead of where the
    /// scene puts the listener, at ear height.
    pub(super) fn sound_ahead(
        &self,
        preset: String,
        name: Option<String>,
        group: Option<String>,
    ) -> Entity {
        Entity {
            name,
            color: None,
            group,
            sound: Sound {
                preset: Some(preset),
                audio: None,
                level: None,
                layers: Default::default(),
            },
            shape: Shape::default(),
            place: self.place_ahead(),
            when: When::default(),
            follows: None,
            moves: Moves::Still,
        }
    }
}

/// Whether `color` is written `#rrggbb`.
fn is_color(color: &str) -> bool {
    color.len() == 7
        && color.starts_with('#')
        && color[1..].chars().all(|digit| digit.is_ascii_hexdigit())
}
