//! `material.mix_color` — Mix Color.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const COLOR_MIX_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "a",
        "A",
        "Color returned when Factor is 0.",
        SocketType::Color,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "b",
        "B",
        "Color returned when Factor is 1.",
        SocketType::Color,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "factor",
        "Factor",
        "Blend position between the two colors, 0..1.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const MIX_COLOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "A and B blended at Factor.",
    SocketType::Color,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];

const MIX_COLOR_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "a",
        "A",
        "First color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.0, 0.0, 0.0, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "b",
        "B",
        "Second color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([1.0, 1.0, 1.0, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "factor",
        "Factor",
        "Blend weight.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.5),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];

pub const DECLARATION: NodeDeclaration = node!(
    "material.mix_color",
    NodeOperation::Material(MaterialNodeOperation::MixColor),
    "Mix Color",
    "Blends two colors.",
    NodeCategory::Utilities,
    NodePreview::Value,
    MATERIAL,
    COLOR_MIX_IN,
    MIX_COLOR_OUT,
    MIX_COLOR_FIELDS,
    TemporalDependence::Inherited,
);
