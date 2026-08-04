//! Node-graph documents, independent of what any graph means.
//!
//! This is the "picture" half of the node system: boxes, wires, whether the wiring is
//! legal, undo/redo, and save/load. It does not evaluate anything. Turning a picture into
//! something runnable is a separate crate per domain — `voxel-material-graph` for
//! materials, and the same shape later for textures, audio and animation.
//!
//! ```text
//! picture:  [noise] ──┐                  recipe:  1. noise
//!                     ├─→ [mix] → [out]           2. red
//!           [red]  ───┘                           3. mix step 1 and step 2
//!                                                 4. output step 3
//!    ^ this crate                                    ^ a domain crate
//! ```
//!
//! # Where things are
//!
//! | module | holds |
//! |---|---|
//! | [`id`] | the four persisted identities |
//! | [`socket`] | value types, evaluation rate, cardinality, the feed rule |
//! | [`field`] | authored values on a node, and `const fn` builders for declaring them |
//! | [`declaration`] | what one node type is — schema, not behaviour |
//! | [`contract`] | per-kind rules a graph must satisfy |
//! | [`registry`] | a catalogue: declarations + contracts, both supplied by the caller |
//! | [`document`] | [`GraphAsset`] — the editable picture |
//! | [`validate`] | resolving a document against a catalogue, and [`Diagnostic`]s |
//! | [`history`] | edits as values, so undo is a stack |
//! | [`asset`] | durable identity for saved documents |
//!
//! There is no single `api.rs`, unusually for this workspace. Here the contracts *are* the
//! whole crate — there is no implementation for them to be a facade over — so one `api.rs`
//! would be the catch-all file the convention forbids rather than a boundary.
//!
//! # The crate knows no domains, and that is enforced
//!
//! `serde` is the only dependency. A node's *kind* is an [`OperationTag`], an opaque label,
//! precisely so the enum naming `MixColor` and `PatternLayer` can live with the catalogue
//! that owns those nodes. `cargo tree -p voxel-graph` is the test.

pub mod asset;
pub mod contract;
pub mod declaration;
pub mod document;
pub mod field;
pub mod history;
pub mod id;
pub mod registry;
pub mod socket;
pub mod validate;

pub use asset::{AssetId, STUDIO_ASSET_SCHEMA_VERSION};
pub use contract::{FlowConstraintStatic, GraphContractStatic, NodeConstraintStatic};
pub use declaration::{GraphKind, NodeCategory, NodeDeclaration, NodePreview, OperationTag};
pub use document::{
    ConnectionError, ConnectionPlan, GraphAsset, GraphInterface, GraphLayout, InputPin, LinkRecord,
    NodeRecord, OutputPin,
};
pub use field::{
    choice, field, ChoiceDeclaration, FieldDeclarationStatic, FieldDefault, FieldTarget,
    NumericRange, PropertyValue, EMPTY_CHOICES, NONE, POSITIVE, SIGNED, UNIT, WIDE,
};
pub use history::{AppliedCommand, EditImpact, GraphCommand, GraphCommandError, GraphHistory};
pub use id::{LinkId, NodeId, NodeTypeId, SocketKey};
pub use registry::NodeRegistry;
pub use socket::{
    Cardinality, EvaluationRate, Separable, SocketDeclaration, SocketDeclarationStatic, SocketType,
    TemporalDependence,
};
pub use validate::{
    node_reachability, Diagnostic, DiagnosticSeverity, GraphHashes, ResolvedGraph, ResolvedLink,
    ResolvedNode,
};
