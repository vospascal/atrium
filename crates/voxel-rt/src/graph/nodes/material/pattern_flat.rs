//! `material.pattern_flat` — Flat Pattern.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const PATTERN_FLAT_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "One stable value per sampling cell, 0..1.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_FLAT_FIELDS);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_flat",
    NodeOperation::Material(MaterialNodeOperation::PatternFlat),
    "Flat Pattern",
    "One stable value per sampling cell.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_FLAT_OUT,
    PATTERN_FLAT_FIELDS,
    TemporalDependence::Inherited,
);
