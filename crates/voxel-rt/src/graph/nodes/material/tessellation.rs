//! `material.tessellation` — Tessellation.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const TESSELLATION_FIELDS: &[FieldDeclarationStatic] =
    &[TILE_ASPECT_FIELD, TILE_BOND_FIELD, TILE_GAP_FIELD];

const TESSELLATION_OUT: &[SocketDeclarationStatic] = &[socket!(
    "tessellation",
    "Tessellation",
    "Where the tiles are. Wire it into every generator that should share this \
     tiling — they cannot disagree if they read the same node.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.tessellation",
    NodeOperation::Material(MaterialNodeOperation::Tessellation),
    "Tessellation",
    "Divides a wall into bonded tiles. Share it between every layer that should \
         agree about where the tiles are.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    &[],
    TESSELLATION_OUT,
    TESSELLATION_FIELDS,
    TemporalDependence::Inherited,
);
