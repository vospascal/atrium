//! `material.roughness` — Roughness.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const ROUGHNESS_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Roughness",
    "The intrinsic microsurface roughness, 0 mirror-smooth to 1 fully diffuse.",
    SocketType::Scalar,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];

const ROUGHNESS_FIELDS: &[FieldDeclarationStatic] = &[field(
    "value",
    "Roughness",
    "Microsurface roughness.",
    FieldTarget::Property,
    FieldDefault::Scalar(0.6),
    UNIT,
    UNIT,
    Some(0.01),
    EMPTY_CHOICES,
    false,
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.roughness",
    MaterialNodeOperation::ConstantScalar,
    "Roughness",
    "Intrinsic microsurface roughness.",
    NodeCategory::MaterialOutput,
    NodePreview::Value,
    MATERIAL,
    &[],
    ROUGHNESS_OUT,
    ROUGHNESS_FIELDS,
    TemporalDependence::Inherited,
);
