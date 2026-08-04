//! `material.pattern_worley_smooth` — Smooth Worley Pattern.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const PATTERN_WORLEY_SMOOTH_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Worley through a smooth minimum, 0..1. No hard creases.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_WORLEY_SMOOTH_FIELDS);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_worley_smooth",
    MaterialNodeOperation::PatternWorleySmooth,
    "Smooth Worley Pattern",
    "Cellular F1 through a smooth minimum — merged, blobby cells.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_WORLEY_SMOOTH_OUT,
    PATTERN_WORLEY_SMOOTH_FIELDS,
    TemporalDependence::Inherited,
);
