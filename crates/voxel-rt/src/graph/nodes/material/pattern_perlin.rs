//! `material.pattern_perlin` — Perlin Pattern.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const PATTERN_PERLIN_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Fractal Perlin gradient noise, 0..1. No axis bias.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_PERLIN_FIELDS, PATTERN_OCTAVES_FIELD);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_perlin",
    NodeOperation::Material(MaterialNodeOperation::PatternPerlin),
    "Perlin Pattern",
    "Fractal Perlin gradient noise on the cubic lattice — no axis bias.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_PERLIN_OUT,
    PATTERN_PERLIN_FIELDS,
    TemporalDependence::Inherited,
);
