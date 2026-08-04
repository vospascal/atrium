//! `material.normalize_vector` — Normalize.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const NORMALIZE_VECTOR_IN: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "The vector to rescale to unit length; its direction is kept.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::OPTIONAL_SINGLE
)];

const NORMALIZE_VECTOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "The input pointing the same way but exactly one unit long; a zero-length \
     input stays zero.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

const VECTOR_INPUT_FIELDS: &[FieldDeclarationStatic] = &[field(
    "vector",
    "Vector",
    "Input vector.",
    FieldTarget::InputSocket,
    FieldDefault::Vector3([0.0, 1.0, 0.0]),
    NONE,
    NONE,
    Some(0.01),
    EMPTY_CHOICES,
    false,
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.normalize_vector",
    MaterialNodeOperation::NormalizeVector,
    "Normalize",
    "Normalizes a vector.",
    NodeCategory::Coordinates,
    NodePreview::Value,
    MATERIAL,
    NORMALIZE_VECTOR_IN,
    NORMALIZE_VECTOR_OUT,
    VECTOR_INPUT_FIELDS,
    TemporalDependence::Inherited,
);
