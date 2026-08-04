//! `material.pattern_checker` — Checker Pattern.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const PATTERN_CHECKER_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Alternating lattice cells: 1 and 0.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_CHECKER_FIELDS);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_checker",
    NodeOperation::Material(MaterialNodeOperation::PatternChecker),
    "Checker Pattern",
    "Alternating lattice cells — tiles and boards.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_CHECKER_OUT,
    PATTERN_CHECKER_FIELDS,
    TemporalDependence::Inherited,
);
