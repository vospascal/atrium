//! `material.reroute_vector` — Vector Reroute.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const REROUTE_VECTOR_IN: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "The vector whose wire is being redirected.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::OPTIONAL_SINGLE
)];

const REROUTE_VECTOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "The same vector; only the wire's path through the editor differs.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

const VECTOR_REROUTE_FIELDS: &[FieldDeclarationStatic] = &[field(
    "vector",
    "Vector",
    "Input vector.",
    FieldTarget::InputSocket,
    FieldDefault::Vector3([0.0; 3]),
    NONE,
    NONE,
    Some(0.01),
    EMPTY_CHOICES,
    false,
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.reroute_vector",
    MaterialNodeOperation::RerouteVector,
    "Vector Reroute",
    "Reroutes a vector connection.",
    NodeCategory::Utilities,
    NodePreview::None,
    MATERIAL,
    REROUTE_VECTOR_IN,
    REROUTE_VECTOR_OUT,
    VECTOR_REROUTE_FIELDS,
    TemporalDependence::Inherited,
);
