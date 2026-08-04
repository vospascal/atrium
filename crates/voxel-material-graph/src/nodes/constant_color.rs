//! `material.constant_color` — Color.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const CONSTANT_COLOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "The authored constant, as linear RGBA.",
    SocketType::Color,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];

const CONSTANT_COLOR_FIELDS: &[FieldDeclarationStatic] = &[field(
    "value",
    "Color",
    "Constant linear RGBA color. The swatch authors CHROMATICITY in 0-1 only — every \
     colour picker does, and clamping is the widget's, not ours. Magnitude above white \
     comes from a scale downstream (Emission Strength for emitters), which is also the \
     only thing HDR float output can show.",
    FieldTarget::Property,
    FieldDefault::Color([0.8, 0.8, 0.8, 1.0]),
    NONE,
    NONE,
    None,
    EMPTY_CHOICES,
    false,
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.constant_color",
    MaterialNodeOperation::ConstantColor,
    "Color",
    "A constant linear color.",
    NodeCategory::Inputs,
    NodePreview::ColorWheel,
    MATERIAL,
    &[],
    CONSTANT_COLOR_OUT,
    CONSTANT_COLOR_FIELDS,
    TemporalDependence::Inherited,
);
