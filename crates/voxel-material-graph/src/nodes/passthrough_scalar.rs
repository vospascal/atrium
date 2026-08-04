//! `material.passthrough_scalar` — Scalar Passthrough.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const PASSTHROUGH_SCALAR_IN: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The scalar to pass through untouched.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::OPTIONAL_SINGLE
)];

const PASSTHROUGH_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The incoming scalar, unchanged.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.passthrough_scalar",
    MaterialNodeOperation::PassthroughScalar,
    "Scalar Passthrough",
    "Passes a scalar unchanged.",
    NodeCategory::Utilities,
    NodePreview::Value,
    MATERIAL,
    PASSTHROUGH_SCALAR_IN,
    PASSTHROUGH_SCALAR_OUT,
    SCALAR_INPUT_FIELDS,
    TemporalDependence::Inherited,
);
