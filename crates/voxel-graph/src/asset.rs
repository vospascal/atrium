//! Durable asset identity and the on-disk schema version.
//!
//! These live in this crate rather than in the asset *store* that reads and writes the
//! files, and that placement is the whole point — see the crate README. An id is more
//! primitive than a store: a graph document carries one, and so does every other kind of
//! saved asset, so putting it next to the store made the store a dependency of everything
//! that merely needs to name something.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use serde::{Deserialize, Serialize};

/// The on-disk asset format. Readers migrate old documents before resolving
/// them into runtime objects. History: 3 = the material look/light split
/// (2026-08-07) — older saves inherit the compiled `light` defaults on load,
/// because their `None` means "field did not exist", not "authored no cast".
pub const STUDIO_ASSET_SCHEMA_VERSION: u32 = 3;

static NEXT_ASSET_ID: AtomicU64 = AtomicU64::new(1);

/// A durable asset identity. It is deliberately opaque: runtime material-table
/// indices are not safe project identities because a project may later reorder
/// or replace its material assignments.
#[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct AssetId(pub String);

impl AssetId {
    /// Create a locally unique, portable text identity without making UUIDs a
    /// rendering dependency. Asset files retain this value forever after first
    /// save, so uniqueness only matters at creation time.
    pub fn new() -> AssetId {
        let sequence = NEXT_ASSET_ID.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap_or_default()
            .as_nanos();
        AssetId(format!("vx-{nanos:032x}-{sequence:016x}"))
    }
}

impl Default for AssetId {
    fn default() -> Self {
        Self::new()
    }
}

impl fmt::Display for AssetId {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The counter and the timestamp are both in the id because neither alone is enough:
    /// two ids minted in the same nanosecond tick would collide on the timestamp, and a
    /// fresh process restarts the counter at 1.
    #[test]
    fn ids_are_unique_within_a_process() {
        let ids: Vec<AssetId> = (0..1000).map(|_| AssetId::new()).collect();
        let unique: std::collections::BTreeSet<&AssetId> = ids.iter().collect();
        assert_eq!(unique.len(), ids.len());
    }

    /// `serde(transparent)` — the id must serialise as a bare string, not `{"0": "..."}`.
    /// Every asset file on disk already depends on this, so it is a compatibility contract
    /// rather than a formatting preference.
    #[test]
    fn serialises_as_a_bare_string() {
        let id = AssetId("vx-test".to_string());
        let json = serde_json::to_string(&id).expect("serialise");
        assert_eq!(json, "\"vx-test\"");
        let round_tripped: AssetId = serde_json::from_str(&json).expect("deserialise");
        assert_eq!(round_tripped, id);
    }

    #[test]
    fn displays_without_the_wrapper() {
        assert_eq!(AssetId("vx-test".to_string()).to_string(), "vx-test");
    }
}
