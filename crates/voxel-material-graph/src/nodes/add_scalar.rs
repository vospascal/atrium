//! `material.add_scalar` — Add.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const ADD_SCALAR_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "a",
        "A",
        "First term of the sum.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "b",
        "B",
        "Second term of the sum.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const ADD_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "A plus B.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

const ADD_SCALAR_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "a",
        "A",
        "First operand.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "b",
        "B",
        "Second operand.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];

pub const DECLARATION: NodeDeclaration = node!(
    "material.add_scalar",
    MaterialNodeOperation::AddScalar,
    "Add",
    "Adds two scalar values.",
    NodeCategory::Utilities,
    NodePreview::Value,
    MATERIAL,
    ADD_SCALAR_IN,
    ADD_SCALAR_OUT,
    ADD_SCALAR_FIELDS,
    TemporalDependence::Inherited,
);
