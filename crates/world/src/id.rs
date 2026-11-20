//! The identity of an entity.

use serde::{Deserialize, Serialize};

use crate::WorldError;

/// What an entity is known by, for as long as it exists.
///
/// Everything that points at a sound points at this: the scene file, a
/// selection in an editor, an undo step. The engine's voice number is not an
/// identity; it changes when sounds are added and removed.
///
/// An id is 1 to 40 characters of `a-z`, `0-9` and `-`, so it reads as a word
/// in a scene file and never needs quoting there.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
#[serde(try_from = "String", into = "String")]
pub struct EntityId(String);

impl EntityId {
    /// The longest id accepted.
    pub const MAX_LENGTH: usize = 40;

    /// Check `id` and take it as an identity.
    pub fn new(id: impl Into<String>) -> Result<Self, WorldError> {
        let id = id.into();
        let usable = !id.is_empty()
            && id.len() <= Self::MAX_LENGTH
            && id
                .chars()
                .all(|character| matches!(character, 'a'..='z' | '0'..='9' | '-'));
        if usable {
            Ok(Self(id))
        } else {
            Err(WorldError::InvalidId(id))
        }
    }

    /// An id for a new entity that reads like `wanted` and is not among
    /// `taken`: `wanted` itself, cut down to what an id may hold, with `-2`,
    /// `-3`, … added while it is in use.
    pub fn unused<'taken>(wanted: &str, taken: impl IntoIterator<Item = &'taken str>) -> Self {
        let taken: Vec<&str> = taken.into_iter().collect();
        let mut base = String::new();
        for character in wanted.to_lowercase().chars() {
            let kept = matches!(character, 'a'..='z' | '0'..='9');
            if kept {
                base.push(character);
            } else if !base.is_empty() && !base.ends_with('-') {
                base.push('-');
            }
        }
        // Room for a numbering suffix.
        base.truncate(Self::MAX_LENGTH - 4);
        let base = base.trim_end_matches('-');
        let base = if base.is_empty() { "sound" } else { base };
        let mut candidate = base.to_string();
        let mut number = 2;
        while taken.contains(&candidate.as_str()) {
            candidate = format!("{base}-{number}");
            number += 1;
        }
        Self(candidate)
    }

    /// The id as written in a scene file.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

impl std::fmt::Display for EntityId {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl TryFrom<String> for EntityId {
    type Error = WorldError;

    fn try_from(id: String) -> Result<Self, Self::Error> {
        Self::new(id)
    }
}

impl From<EntityId> for String {
    fn from(id: EntityId) -> Self {
        id.0
    }
}

impl std::str::FromStr for EntityId {
    type Err = WorldError;

    fn from_str(id: &str) -> Result<Self, Self::Err> {
        Self::new(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_word_with_digits_and_hyphens_is_an_id() {
        for id in ["frogs", "field-wind", "rain-2", "a"] {
            assert_eq!(EntityId::new(id).expect("usable").as_str(), id);
        }
    }

    /// An id is written bare as a key in a scene file and will be pointed at
    /// from other files, so anything that needs quoting or changes meaning by
    /// case is refused instead of repaired.
    #[test]
    fn anything_else_is_refused_not_repaired() {
        let too_long = "a".repeat(EntityId::MAX_LENGTH + 1);
        for id in [
            "",
            "Frogs",
            "field wind",
            "rain_2",
            "étang",
            "a.b",
            &too_long,
        ] {
            assert_eq!(
                EntityId::new(id),
                Err(WorldError::InvalidId(id.to_string())),
                "`{id}` should be refused"
            );
        }
        assert!(EntityId::new("a".repeat(EntityId::MAX_LENGTH)).is_ok());
    }

    /// A sound added from a library gets its id from the preset's name. A
    /// second frog must not take the first frog's id, and whatever the name
    /// holds, the result is an id `new` would accept.
    #[test]
    fn a_new_sound_gets_a_free_id_that_reads_like_its_name() {
        assert_eq!(EntityId::unused("Frog", []).as_str(), "frog");
        assert_eq!(EntityId::unused("Frog", ["frog"]).as_str(), "frog-2");
        assert_eq!(
            EntityId::unused("Frog", ["frog", "frog-2", "birds"]).as_str(),
            "frog-3"
        );
        assert_eq!(EntityId::unused("Rain v2!", []).as_str(), "rain-v2");
        assert_eq!(
            EntityId::unused("  Étang  du  Nord ", []).as_str(),
            "tang-du-nord"
        );
        assert_eq!(EntityId::unused("???", ["sound"]).as_str(), "sound-2");
        let long = EntityId::unused(&"wind ".repeat(30), []);
        assert_eq!(EntityId::new(long.as_str()), Ok(long));
    }
}
