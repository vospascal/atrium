//! `material.reroute_scalar` — Scalar Reroute.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const REROUTE_SCALAR_IN: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The scalar whose wire is being redirected.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::OPTIONAL_SINGLE
)];

const REROUTE_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The same scalar; only the wire's path through the editor differs.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.reroute_scalar",
    MaterialNodeOperation::RerouteScalar,
    "Scalar Reroute",
    "Reroutes a scalar connection.",
    NodeCategory::Utilities,
    NodePreview::None,
    MATERIAL,
    REROUTE_SCALAR_IN,
    REROUTE_SCALAR_OUT,
    SCALAR_INPUT_FIELDS,
    TemporalDependence::Inherited,
);
