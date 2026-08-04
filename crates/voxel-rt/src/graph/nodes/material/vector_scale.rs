//! `material.vector_scale` — Vector Scale.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const VECTOR_SCALE_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "vector",
        "Vector",
        "The vector to lengthen or shorten.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "scale",
        "Scale",
        "Multiplier applied to every component; a negative value reverses the \
         direction.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const VECTOR_SCALE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "Vector times Scale.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

const VECTOR_SCALE_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "vector",
        "Vector",
        "Vector to scale.",
        FieldTarget::InputSocket,
        FieldDefault::Vector3([0.0; 3]),
        NONE,
        NONE,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "scale",
        "Scale",
        "Scalar multiplier.",
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
    "material.vector_scale",
    NodeOperation::Material(MaterialNodeOperation::VectorScale),
    "Vector Scale",
    "Scales a vector.",
    NodeCategory::Coordinates,
    NodePreview::Value,
    MATERIAL,
    VECTOR_SCALE_IN,
    VECTOR_SCALE_OUT,
    VECTOR_SCALE_FIELDS,
    TemporalDependence::Inherited,
);
