//! `material.pattern_worley_edge` — Worley Edge Pattern.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const PATTERN_WORLEY_EDGE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Bright on the boundary between two cells, 0..1.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_WORLEY_EDGE_FIELDS);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_worley_edge",
    NodeOperation::Material(MaterialNodeOperation::PatternWorleyEdge),
    "Worley Edge Pattern",
    "Cellular F2 minus F1 — cracked mud, dried paint, mortar.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_WORLEY_EDGE_OUT,
    PATTERN_WORLEY_EDGE_FIELDS,
    TemporalDependence::Inherited,
);
