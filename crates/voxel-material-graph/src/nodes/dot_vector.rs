//! `material.dot_vector` — Dot Product.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const VECTOR_DOT_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "a",
        "A",
        "First vector of the product.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "b",
        "B",
        "Second vector of the product; wire the surface normal here to test which \
         way a face points.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const VECTOR_DOT_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The dot product; for unit-length inputs this is the cosine of the angle \
     between them, -1..1.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.dot_vector",
    MaterialNodeOperation::DotVector,
    "Dot Product",
    "Computes a vector dot product.",
    NodeCategory::Coordinates,
    NodePreview::Value,
    MATERIAL,
    VECTOR_DOT_IN,
    VECTOR_DOT_OUT,
    VECTOR_BINARY_FIELDS,
    TemporalDependence::Inherited,
);
