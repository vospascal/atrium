//! The entities of one scene: unique ids, in the order the scene lists them.

use std::fmt;

use serde::de::{MapAccess, Visitor};
use serde::ser::SerializeMap;
use serde::{Deserialize, Deserializer, Serialize, Serializer};

use crate::{Entity, EntityId, WorldError};

/// The sounds of a scene, each under its own id.
///
/// The order is the order of the scene file. It is kept because the engine
/// gives out its voices in that order, so a scene that is loaded twice plays
/// the same sound from the same voice.
///
/// In a scene file this is a mapping from id to entity:
///
/// ```yaml
/// sounds:
///   frogs:
///     name: Frogs
///     sound: { preset: sources/frog.yaml }
///     place: [10.1, 10.1, 0.0]
/// ```
#[derive(Debug, Clone, PartialEq, Default)]
pub struct Entities {
    entries: Vec<(EntityId, Entity)>,
}

impl Entities {
    /// Take `entries` as a scene's sounds, in that order. Refused when two
    /// share an id or one names nothing to play.
    pub fn new(entries: Vec<(EntityId, Entity)>) -> Result<Self, WorldError> {
        for (position, (id, entity)) in entries.iter().enumerate() {
            if entries[..position].iter().any(|(earlier, _)| earlier == id) {
                return Err(WorldError::DuplicateId(id.clone()));
            }
            if entity.sound.preset.is_none() && entity.sound.audio.is_none() {
                return Err(WorldError::NothingToPlay(id.clone()));
            }
        }
        Ok(Self { entries })
    }

    /// How many sounds the scene holds.
    pub fn len(&self) -> usize {
        self.entries.len()
    }

    /// Whether the scene holds no sound at all.
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    /// Every sound with its id, in scene order.
    pub fn iter(&self) -> impl Iterator<Item = (&EntityId, &Entity)> {
        self.entries.iter().map(|(id, entity)| (id, entity))
    }

    /// The sound with `id`, to change it in place.
    pub fn get_mut(&mut self, id: &EntityId) -> Option<&mut Entity> {
        self.entries
            .iter_mut()
            .find(|(own, _)| own == id)
            .map(|(_, entity)| entity)
    }

    /// Add `entity` after the others, under `id`. Refused for the same
    /// reasons a scene file is: an id already taken, or nothing to play.
    pub fn push(&mut self, id: EntityId, entity: Entity) -> Result<(), WorldError> {
        if self.entries.iter().any(|(own, _)| *own == id) {
            return Err(WorldError::DuplicateId(id));
        }
        if entity.sound.preset.is_none() && entity.sound.audio.is_none() {
            return Err(WorldError::NothingToPlay(id));
        }
        self.entries.push((id, entity));
        Ok(())
    }

    /// Take the sound with `id` out, keeping the others in their order.
    pub fn remove(&mut self, id: &EntityId) -> Option<Entity> {
        let position = self.entries.iter().position(|(own, _)| own == id)?;
        Some(self.entries.remove(position).1)
    }
}

impl Serialize for Entities {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        let mut map = serializer.serialize_map(Some(self.entries.len()))?;
        for (id, entity) in &self.entries {
            map.serialize_entry(id, entity)?;
        }
        map.end()
    }
}

impl<'de> Deserialize<'de> for Entities {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        struct InFileOrder;

        impl<'de> Visitor<'de> for InFileOrder {
            type Value = Entities;

            fn expecting(&self, formatter: &mut fmt::Formatter) -> fmt::Result {
                formatter.write_str("a mapping from sound id to sound")
            }

            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Self::Value, A::Error> {
                let mut entries = Vec::with_capacity(map.size_hint().unwrap_or(0));
                while let Some(entry) = map.next_entry::<EntityId, Entity>()? {
                    entries.push(entry);
                }
                Entities::new(entries).map_err(serde::de::Error::custom)
            }
        }

        deserializer.deserialize_map(InFileOrder)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{Moves, Pattern, Place, Shape, Sound};

    const MEADOW: &str = "\
frogs:
  name: Frogs
  color: \"#6db33f\"
  group: Creatures
  sound: { preset: sources/frog.yaml, level: 68 }
  shape: { pattern: cardioid, size: 0.2, facing: 225 }
  place: [10.1, 10.1, 0.0]
drum:
  sound: { audio: assets/djembe.mp3 }
  place: [0.0, 0.0, 0.0]
  moves:
    orbit: { radius: 1.5, speed: 1.0 }
";

    fn read(yaml: &str) -> Result<Entities, String> {
        serde_yaml::from_str(yaml).map_err(|error| error.to_string())
    }

    #[test]
    fn a_scenes_sounds_read_in_file_order_with_every_part() {
        let sounds = read(MEADOW).expect("the meadow reads");
        let ids: Vec<&str> = sounds.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["frogs", "drum"], "file order, not alphabetical");

        let (_, frogs) = sounds.iter().next().expect("frogs");
        assert_eq!(
            *frogs,
            Entity {
                name: Some("Frogs".into()),
                color: Some("#6db33f".into()),
                group: Some("Creatures".into()),
                sound: Sound {
                    preset: Some("sources/frog.yaml".into()),
                    audio: None,
                    level: Some(68.0),
                },
                shape: Shape {
                    pattern: Some(Pattern::Cardioid),
                    size: Some(0.2),
                    facing: Some(225.0),
                },
                place: Place {
                    x: 10.1,
                    y: 10.1,
                    z: 0.0
                },
                moves: Moves::Still,
            }
        );
    }

    /// A part left out means "nothing special": the preset's own shape,
    /// standing still, no name of its own.
    #[test]
    fn a_missing_part_is_the_plain_case() {
        let sounds = read(MEADOW).expect("the meadow reads");
        let (_, drum) = sounds.iter().nth(1).expect("drum");
        assert_eq!(drum.name, None);
        assert_eq!(drum.shape, Shape::default());
        assert_eq!(
            drum.moves,
            Moves::Orbit {
                radius: 1.5,
                speed: 1.0
            }
        );
    }

    /// What is written is what was read: the same ids in the same order, and
    /// nothing added for parts that were left out. A scene saved by an editor
    /// stays as short as the one it opened.
    #[test]
    fn writing_gives_back_what_was_read() {
        let sounds = read(MEADOW).expect("the meadow reads");
        let written = serde_yaml::to_string(&sounds).expect("the meadow writes");
        assert_eq!(read(&written).expect("it reads again"), sounds);
        assert!(
            !written.contains('!'),
            "no YAML tags in a scene file:\n{written}"
        );

        let drum = written.split("drum:").nth(1).expect("the drum is written");
        for absent in ["name", "color", "group", "shape", "level", "preset"] {
            assert!(
                !drum.contains(absent),
                "`{absent}` was not in the file and must not appear:\n{written}"
            );
        }
    }

    #[test]
    fn two_sounds_with_one_id_are_refused() {
        let twice = "\
frogs:
  sound: { preset: sources/frog.yaml }
  place: [0, 0, 0]
frogs:
  sound: { preset: sources/frog.yaml }
  place: [1, 0, 0]
";
        let refusal = read(twice).expect_err("a second `frogs` is refused");
        assert!(refusal.contains("frogs"), "{refusal}");
    }

    #[test]
    fn a_sound_with_nothing_to_play_is_refused_by_name() {
        let silent = "\
frogs:
  sound: { level: 60 }
  place: [0, 0, 0]
";
        let refusal = read(silent).expect_err("no preset and no audio");
        assert!(
            refusal.contains("sound `frogs` needs a `preset` or an `audio` file"),
            "{refusal}"
        );
        assert_eq!(
            Entities::new(vec![(
                EntityId::new("frogs").expect("usable"),
                Entity {
                    name: None,
                    color: None,
                    group: None,
                    sound: Sound {
                        preset: None,
                        audio: None,
                        level: Some(60.0)
                    },
                    shape: Shape::default(),
                    place: Place {
                        x: 0.0,
                        y: 0.0,
                        z: 0.0
                    },
                    moves: Moves::Still,
                }
            )]),
            Err(WorldError::NothingToPlay(
                EntityId::new("frogs").expect("usable")
            ))
        );
    }

    /// A misspelt or outdated key is an error, not a setting that is silently
    /// ignored. `position` was the key before entities.
    #[test]
    fn an_unknown_key_is_an_error_not_ignored() {
        for stray in ["position: [0, 0, 0]", "orbit_radius: 1.5", "colour: red"] {
            let yaml = format!(
                "frogs:\n  sound: {{ preset: sources/frog.yaml }}\n  place: [0, 0, 0]\n  {stray}\n"
            );
            assert!(read(&yaml).is_err(), "`{stray}` should be refused");
        }
        let two_ways = "\
frogs:
  sound: { preset: sources/frog.yaml }
  place: [0, 0, 0]
  moves: { path: [] }
";
        assert!(read(two_ways).is_err(), "an unknown way of moving");
    }

    #[test]
    fn an_id_that_is_not_a_plain_word_is_refused() {
        let yaml = "Frog Pond:\n  sound: { preset: sources/frog.yaml }\n  place: [0, 0, 0]\n";
        let refusal = read(yaml).expect_err("a capitalised id with a space");
        assert!(refusal.contains("Frog Pond"), "{refusal}");
    }

    /// An editor adds, changes and removes sounds; the rules of a scene file
    /// still hold, and the others keep their order.
    #[test]
    fn sounds_are_added_changed_and_removed_by_id() {
        let mut sounds = read(MEADOW).expect("the meadow reads");
        let frogs = EntityId::new("frogs").expect("usable");
        let (_, template) = sounds.iter().next().expect("frogs");
        let template = template.clone();

        assert_eq!(
            sounds.push(frogs.clone(), template.clone()),
            Err(WorldError::DuplicateId(frogs.clone()))
        );
        let bell = EntityId::new("bell").expect("usable");
        sounds.push(bell.clone(), template).expect("a new id");
        sounds.get_mut(&bell).expect("added").sound.level = Some(80.0);

        assert!(sounds.remove(&frogs).is_some());
        assert!(sounds.remove(&frogs).is_none(), "already gone");
        let ids: Vec<&str> = sounds.iter().map(|(id, _)| id.as_str()).collect();
        assert_eq!(ids, ["drum", "bell"]);
        let (_, bell) = sounds.iter().nth(1).expect("bell");
        assert_eq!(bell.sound.level, Some(80.0));
    }

    #[test]
    fn an_empty_scene_has_no_sounds() {
        let sounds = read("{}").expect("an empty mapping reads");
        assert!(sounds.is_empty());
        assert_eq!(sounds.len(), 0);
    }
}
