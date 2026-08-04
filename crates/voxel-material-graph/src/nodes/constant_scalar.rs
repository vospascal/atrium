//! `material.constant_scalar` — Scalar.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const CONSTANT_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The authored constant, unchanged.",
    SocketType::Scalar,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];

const CONSTANT_SCALAR_FIELDS: &[FieldDeclarationStatic] = &[field(
    "value",
    "Value",
    "Constant scalar value.",
    FieldTarget::Property,
    FieldDefault::Scalar(0.5),
    WIDE,
    SIGNED,
    Some(0.01),
    EMPTY_CHOICES,
    false,
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.constant_scalar",
    MaterialNodeOperation::ConstantScalar,
    "Scalar",
    "A constant scalar value.",
    NodeCategory::Inputs,
    NodePreview::Value,
    MATERIAL,
    &[],
    CONSTANT_SCALAR_OUT,
    CONSTANT_SCALAR_FIELDS,
    TemporalDependence::Inherited,
);
