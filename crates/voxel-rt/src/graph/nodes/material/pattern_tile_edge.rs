//! `material.pattern_tile_edge` — Tile Edge.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const PATTERN_TILE_EDGE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "0 in the joint, 1 at the tile's centre.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_TILE_EDGE_FIELDS, PATTERN_SHARPNESS_FIELD);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_tile_edge",
    NodeOperation::Material(MaterialNodeOperation::PatternTileEdge),
    "Tile Edge",
    "Distance to the nearest tile edge — grout, and the bevel of a raised block.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_TILE_EDGE_OUT,
    PATTERN_TILE_EDGE_FIELDS,
    TemporalDependence::Inherited,
);
