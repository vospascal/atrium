//! `material.pattern_simplex` — Simplex Pattern.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const PATTERN_SIMPLEX_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Fractal simplex gradient noise, 0..1. Four corners per octave.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_SIMPLEX_FIELDS, PATTERN_OCTAVES_FIELD);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_simplex",
    NodeOperation::Material(MaterialNodeOperation::PatternSimplex),
    "Simplex Pattern",
    "Fractal gradient noise on the tetrahedral lattice — four corners per octave.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_SIMPLEX_OUT,
    PATTERN_SIMPLEX_FIELDS,
    TemporalDependence::Inherited,
);
