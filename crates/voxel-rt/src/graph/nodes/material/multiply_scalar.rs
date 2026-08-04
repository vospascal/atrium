//! `material.multiply_scalar` — Multiply.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const MULTIPLY_SCALAR_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "a",
        "A",
        "First factor — usually the thing being gated.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "b",
        "B",
        "Second factor — wire a sensor signal here to gate A, since 0 mutes it and \
         1 passes it through.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const MULTIPLY_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "A times B.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

const MULTIPLY_SCALAR_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "a",
        "A",
        "First operand.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
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
        FieldDefault::Scalar(1.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];

pub const DECLARATION: NodeDeclaration = node!(
    "material.multiply_scalar",
    NodeOperation::Material(MaterialNodeOperation::MultiplyScalar),
    "Multiply",
    "Multiplies two scalar values. This is how a trigger GATES something: \
         an event sensor's signal times an oscillator is a pulse that only runs \
         while the sensor is firing.",
    NodeCategory::Utilities,
    NodePreview::Value,
    MATERIAL,
    MULTIPLY_SCALAR_IN,
    MULTIPLY_SCALAR_OUT,
    MULTIPLY_SCALAR_FIELDS,
    TemporalDependence::Inherited,
);
