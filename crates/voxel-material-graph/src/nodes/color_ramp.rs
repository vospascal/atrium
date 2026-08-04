//! `material.color_ramp` — Color Ramp.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const COLOR_RAMP_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "factor",
        "Factor",
        "Where to read the ramp; values at or below Position A give Color A and at \
         or above Position B give Color B.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "color_a",
        "Color A",
        "Color at the first stop.",
        SocketType::Color,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "color_b",
        "Color B",
        "Color at the second stop.",
        SocketType::Color,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "position_a",
        "Position A",
        "Where the first stop sits along the ramp, 0..1.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "position_b",
        "Position B",
        "Where the second stop sits along the ramp, 0..1.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const COLOR_RAMP_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "The ramp sampled at Factor.",
    SocketType::Color,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

const COLOR_RAMP_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "factor",
        "Factor",
        "Ramp coordinate.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.5),
        WIDE,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "color_a",
        "Color A",
        "First stop color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.08, 0.2, 0.03, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "color_b",
        "Color B",
        "Second stop color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.55, 0.8, 0.12, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "position_a",
        "Position A",
        "First stop position.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.25),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "position_b",
        "Position B",
        "Second stop position.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.75),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];

pub const DECLARATION: NodeDeclaration = node!(
    "material.color_ramp",
    MaterialNodeOperation::ColorRamp,
    "Color Ramp",
    "Maps a scalar through two color stops.",
    NodeCategory::Procedural,
    NodePreview::ColorRamp,
    MATERIAL,
    COLOR_RAMP_IN,
    COLOR_RAMP_OUT,
    COLOR_RAMP_FIELDS,
    TemporalDependence::Inherited,
);
