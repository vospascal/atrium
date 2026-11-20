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
}
