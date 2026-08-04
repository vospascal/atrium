//! `material.pattern_noise` — Noise Pattern.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const PATTERN_NOISE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Fractal value noise across the sampling cells, 0..1.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_NOISE_FIELDS, PATTERN_OCTAVES_FIELD);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_noise",
    NodeOperation::Material(MaterialNodeOperation::PatternNoise),
    "Noise Pattern",
    "Fractal value-noise pattern.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_NOISE_OUT,
    PATTERN_NOISE_FIELDS,
    TemporalDependence::Inherited,
);
