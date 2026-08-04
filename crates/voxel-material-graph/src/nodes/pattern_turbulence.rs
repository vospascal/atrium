//! `material.pattern_turbulence` — Turbulence Pattern.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const PATTERN_TURBULENCE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Turbulence, 0..1. Creases at each octave's zero crossing.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_TURBULENCE_FIELDS, PATTERN_OCTAVES_FIELD);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_turbulence",
    MaterialNodeOperation::PatternTurbulence,
    "Turbulence Pattern",
    "Turbulence: marble veining, smoke, weathering streaks.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_TURBULENCE_OUT,
    PATTERN_TURBULENCE_FIELDS,
    TemporalDependence::Inherited,
);
