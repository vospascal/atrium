//! `material.vector_add` — Vector Add.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const VECTOR_ADD_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "a",
        "A",
        "First vector of the sum.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "b",
        "B",
        "Second vector of the sum, added component by component.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const VECTOR_ADD_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "A plus B, component by component.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.vector_add",
    MaterialNodeOperation::VectorAdd,
    "Vector Add",
    "Adds two vectors.",
    NodeCategory::Coordinates,
    NodePreview::Value,
    MATERIAL,
    VECTOR_ADD_IN,
    VECTOR_ADD_OUT,
    VECTOR_BINARY_FIELDS,
    TemporalDependence::Inherited,
);
