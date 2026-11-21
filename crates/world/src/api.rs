//! An entity and its parts, as plain data.
//!
//! The parts answer one question each, and an editor shows them in this
//! order: what is it ([`Sound`]), how does it fill the air ([`Shape`]), where
//! is it ([`Place`]), when does it sound ([`When`]), does it travel ([`Moves`]). A part left out of a scene
//! file means "nothing special": the preset's own shape, standing still.

use std::collections::BTreeMap;

use serde::{Deserialize, Serialize};

/// One sound in a scene.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Entity {
    /// The name a list shows. Without one, the id is shown.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
    /// UI color as `#rrggbb`. Without one, the engine picks from its palette.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub color: Option<String>,
    /// The family it belongs to in a list, such as "Weather" or "Creatures".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
    pub sound: Sound,
    #[serde(default, skip_serializing_if = "Shape::is_the_presets_own")]
    pub shape: Shape,
    pub place: Place,
    #[serde(default, skip_serializing_if = "When::is_the_presets_own")]
    pub when: When,
    #[serde(default, skip_serializing_if = "Moves::is_still")]
    pub moves: Moves,
}

/// What an entity sounds like: where its definition comes from, and what
/// this entity changes about it.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Sound {
    /// A file that defines the sound, such as `sources/frog.yaml`: a
    /// recording or a generator, with its level and shape.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub preset: Option<String>,
    /// A recording to play, such as `assets/frog.mp3`. With a preset it
    /// replaces the preset's recording; without one it is the whole
    /// definition.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<String>,
    /// Level in dB SPL at 1 m, in place of the preset's.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub level: Option<f32>,
}

/// How an entity fills the air, where it differs from its preset.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Shape {
    /// How wide its sound goes out, in degrees, in place of the preset's:
    /// 360 is all round, 180 a half circle, 60 a narrow beam. The width is
    /// where the level is halfway between the front and the back.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<f32>,
    /// How much is heard straight behind it, in dB below the front: 0 as
    /// loud, −12 duller and softer, −60 or lower nothing. In place of the
    /// preset's. It only matters when the width is less than all round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub behind: Option<f32>,
    /// How wide it sounds, from 0 (a point) to 1 (all around), in place of
    /// the preset's size.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub size: Option<f32>,
    /// Which way it faces, as a compass bearing in degrees: 0 is north (+y),
    /// 90 is east (+x), the way scene files give directions. It matters for
    /// a sound whose width is less than all round. Left out, the
    /// sound faces the way the engine would turn it by itself.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub facing: Option<f32>,
}

impl Shape {
    fn is_the_presets_own(&self) -> bool {
        *self == Self::default()
    }
}

/// When an entity sounds, where it differs from its preset.
///
/// A preset with takes utters one, rests, and utters another. These change
/// how long it rests and which take it is likely to choose. The same holds for
/// anything with takes: a bird, a bell, lines of a story, a tone.
///
/// ```yaml
/// when:
///   gap: [20, 45]
///   chance: { "Call 2": 3 }
/// ```
#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct When {
    /// Shortest and longest rest between two takes (s), in place of the
    /// preset's. `[0, 0]` is no rest at all: it sounds all the time. On a
    /// preset that plays all the time, a longer gap makes it come and go.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub gap: Option<[f32; 2]>,
    /// How often a take is chosen next to the others, by the take's name, in
    /// place of the preset's chance. A take with chance 3 is chosen three
    /// times as often as one with chance 1, and one with 0 never.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub chance: BTreeMap<String, f32>,
}

impl When {
    fn is_the_presets_own(&self) -> bool {
        *self == Self::default()
    }
}

/// Where an entity is, in metres. Written `[x, y, z]` in a scene file, in the
/// scene's own coordinates: the same ones the listener is placed in.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(from = "[f32; 3]", into = "[f32; 3]")]
pub struct Place {
    pub x: f32,
    pub y: f32,
    /// Height.
    pub z: f32,
}

impl From<[f32; 3]> for Place {
    fn from([x, y, z]: [f32; 3]) -> Self {
        Self { x, y, z }
    }
}

impl From<Place> for [f32; 3] {
    fn from(place: Place) -> Self {
        [place.x, place.y, place.z]
    }
}

/// Whether an entity travels, and how.
#[derive(Debug, Clone, Copy, PartialEq, Default, Serialize, Deserialize)]
#[serde(from = "MovesInFile", into = "MovesInFile")]
pub enum Moves {
    /// It stays where it was placed.
    #[default]
    Still,
    /// It circles its place.
    Orbit {
        /// Metres from its place.
        radius: f32,
        /// Radians per second; one turn takes 2π / speed seconds.
        speed: f32,
    },
}

impl Moves {
    fn is_still(&self) -> bool {
        *self == Self::Still
    }
}

/// `moves:` as written: one key per way of moving, of which at most one is
/// present. No key at all is standing still.
///
/// A plain struct instead of a tagged enum keeps the file free of YAML tags,
/// and a new way of moving is one more optional key.
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct MovesInFile {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    orbit: Option<OrbitInFile>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct OrbitInFile {
    radius: f32,
    speed: f32,
}

impl From<MovesInFile> for Moves {
    fn from(written: MovesInFile) -> Self {
        match written.orbit {
            Some(OrbitInFile { radius, speed }) => Self::Orbit { radius, speed },
            None => Self::Still,
        }
    }
}

impl From<Moves> for MovesInFile {
    fn from(moves: Moves) -> Self {
        Self {
            orbit: match moves {
                Moves::Still => None,
                Moves::Orbit { radius, speed } => Some(OrbitInFile { radius, speed }),
            },
        }
    }
}
