//! The sounds of a scene as entities.
//!
//! An [`Entity`] is one sound: an identity and the parts that describe it
//! ([`Sound`], [`Shape`], [`Place`], [`Moves`]). [`Entities`] is the set a
//! scene holds, keyed by [`EntityId`], and is what a scene file's `sounds:`
//! section reads into and writes from.
//!
//! This crate describes; it does not play. It opens no file, decodes no audio
//! and knows nothing about voices or speakers. The engine turns each entity
//! into a playing sound.

mod api;
mod changes;
mod entities;
mod id;

pub use api::{Entity, Follow, Moves, Place, Shape, Sound, When};
pub use changes::{changes, Change};
pub use entities::Entities;
pub use id::EntityId;

/// Why a set of entities was refused.
#[derive(Debug, Clone, PartialEq)]
#[non_exhaustive]
pub enum WorldError {
    /// The id is empty, too long, or has a character other than `a-z`, `0-9`
    /// and `-`.
    InvalidId(String),
    /// Two entities share this id.
    DuplicateId(EntityId),
    /// The entity names neither a preset nor a recording, so nothing says
    /// what it sounds like.
    NothingToPlay(EntityId),
}

impl std::fmt::Display for WorldError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::InvalidId(id) => write!(
                formatter,
                "`{id}` is not a usable id: use 1 to {} of a-z, 0-9 and -",
                EntityId::MAX_LENGTH
            ),
            Self::DuplicateId(id) => write!(formatter, "two sounds are called `{id}`"),
            Self::NothingToPlay(id) => write!(
                formatter,
                "sound `{id}` needs a `preset` or an `audio` file under `sound`"
            ),
        }
    }
}

impl std::error::Error for WorldError {}
