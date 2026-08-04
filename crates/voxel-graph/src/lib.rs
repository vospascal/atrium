//! Node-graph documents, independent of what any graph means.
//!
//! This is the "picture" half of the node system: boxes, wires, whether the wiring is
//! legal, undo/redo, and save/load. It does not evaluate anything. Turning a picture into
//! something runnable is a separate crate per domain — `voxel-material-graph` for
//! materials, and later the same shape for textures, audio and animation.
//!
//! # Contents
//!
//! - [`AssetId`], [`STUDIO_ASSET_SCHEMA_VERSION`] — durable identity for saved documents.
//!
//! The graph mechanics themselves (documents, sockets, validation, history, the node
//! registry) land here next; they are still in `voxel-rt` at the time of writing.

pub mod asset;

pub use asset::{AssetId, STUDIO_ASSET_SCHEMA_VERSION};
