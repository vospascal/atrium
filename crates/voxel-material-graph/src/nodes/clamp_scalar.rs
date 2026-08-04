//! `material.clamp_scalar` — Clamp.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const SCALAR_CLAMP_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "value",
        "Value",
        "The scalar to hold inside the bounds.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "minimum",
        "Minimum",
        "Lowest value the result may take.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "maximum",
        "Maximum",
        "Highest value the result may take.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const CLAMP_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "Value pulled back inside Minimum..Maximum.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

const CLAMP_SCALAR_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "value",
        "Value",
        "Value to clamp.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "minimum",
        "Minimum",
        "Lower bound.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "maximum",
        "Maximum",
        "Upper bound.",
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
    "material.clamp_scalar",
    MaterialNodeOperation::ClampScalar,
    "Clamp",
    "Clamps a scalar between two bounds.",
    NodeCategory::Utilities,
    NodePreview::Value,
    MATERIAL,
    SCALAR_CLAMP_IN,
    CLAMP_SCALAR_OUT,
    CLAMP_SCALAR_FIELDS,
    TemporalDependence::Inherited,
);
