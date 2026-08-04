//! `material.pattern_ridged` — Ridged Pattern.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const PATTERN_RIDGED_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Ridged multifractal, 0..1. Creases at each octave's midline.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_RIDGED_FIELDS, PATTERN_OCTAVES_FIELD);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_ridged",
    MaterialNodeOperation::PatternRidged,
    "Ridged Pattern",
    "Ridged multifractal: veins, erosion channels, rock strata.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_RIDGED_OUT,
    PATTERN_RIDGED_FIELDS,
    TemporalDependence::Inherited,
);
