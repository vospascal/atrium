//! `material.base_color` — Base Color.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const BASE_COLOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "The intrinsic base color, ready for the surface's Base Color input.",
    SocketType::Color,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];

const BASE_COLOR_FIELDS: &[FieldDeclarationStatic] = &[field(
    "value",
    "Color",
    "Intrinsic base color of the material.",
    FieldTarget::Property,
    FieldDefault::Color([0.4, 0.7, 0.25, 1.0]),
    NONE,
    NONE,
    None,
    EMPTY_CHOICES,
    false,
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.base_color",
    MaterialNodeOperation::ConstantColor,
    "Base Color",
    "Intrinsic base color input.",
    NodeCategory::MaterialOutput,
    NodePreview::ColorWheel,
    MATERIAL,
    &[],
    BASE_COLOR_OUT,
    BASE_COLOR_FIELDS,
    TemporalDependence::Inherited,
);
