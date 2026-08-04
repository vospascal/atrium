//! Stable identities for the pieces of a graph.
//!
//! All four are newtyped strings because they are PERSISTED: a graph saved today must
//! still resolve tomorrow, and an index into a Vec would not survive a reorder. The
//! generated form is a process-local counter, which is enough because uniqueness only has
//! to hold at creation time — after the first save the file carries the value forever.

use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

static NEXT_GRAPH_ID: AtomicU64 = AtomicU64::new(1);

macro_rules! graph_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        #[allow(clippy::new_without_default)]
        impl $name {
            pub fn new() -> Self {
                let sequence = NEXT_GRAPH_ID.fetch_add(1, Ordering::Relaxed);
                Self(format!("g-{sequence:016x}"))
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

graph_id!(NodeId);
graph_id!(LinkId);
graph_id!(SocketKey);
graph_id!(NodeTypeId);
