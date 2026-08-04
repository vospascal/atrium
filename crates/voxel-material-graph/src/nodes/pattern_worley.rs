//! `material.pattern_worley` — Worley Pattern.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const PATTERN_WORLEY_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Distance to the nearest feature point, 0..1.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_WORLEY_FIELDS);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_worley",
    MaterialNodeOperation::PatternWorley,
    "Worley Pattern",
    "Cellular F1 — pebbles, cells, lichen colonies.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_WORLEY_OUT,
    PATTERN_WORLEY_FIELDS,
    TemporalDependence::Inherited,
);
