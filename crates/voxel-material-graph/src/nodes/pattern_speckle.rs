//! `material.pattern_speckle` — Speckle Pattern.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const PATTERN_SPECKLE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "1 inside a speck and 0 everywhere else.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_SPECKLE_FIELDS, PATTERN_DENSITY_FIELD);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_speckle",
    MaterialNodeOperation::PatternSpeckle,
    "Speckle Pattern",
    "Scattered specks controlled by cell density.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_SPECKLE_OUT,
    PATTERN_SPECKLE_FIELDS,
    TemporalDependence::Inherited,
);
