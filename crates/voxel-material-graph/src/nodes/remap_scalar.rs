//! `material.remap_scalar` — Remap.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const REMAP_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "value",
        "Value",
        "The scalar to rescale, read against the From interval.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "from_min",
        "From Min",
        "Input value that maps onto To Min.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "from_max",
        "From Max",
        "Input value that maps onto To Max.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "to_min",
        "To Min",
        "Result produced at From Min.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "to_max",
        "To Max",
        "Result produced at From Max.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const REMAP_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "Value rescaled into the To Min..To Max interval.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

const REMAP_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "value",
        "Value",
        "Value to remap.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "from_min",
        "From Min",
        "Input lower bound.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "from_max",
        "From Max",
        "Input upper bound.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "to_min",
        "To Min",
        "Output lower bound.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "to_max",
        "To Max",
        "Output upper bound.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "clamp",
        "Clamp",
        "Clamp the normalized factor.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
];

pub const DECLARATION: NodeDeclaration = node!(
    "material.remap_scalar",
    MaterialNodeOperation::RemapScalar,
    "Remap",
    "Remaps one scalar interval to another.",
    NodeCategory::Procedural,
    NodePreview::Value,
    MATERIAL,
    REMAP_IN,
    REMAP_SCALAR_OUT,
    REMAP_FIELDS,
    TemporalDependence::Inherited,
);
