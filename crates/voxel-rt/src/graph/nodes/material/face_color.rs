//! `material.face_color` — Face Color.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const FACE_COLOR_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "base",
        "Base",
        "Color used by any face whose own input is left at its default.",
        SocketType::Color,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "top",
        "Top",
        "Color for up-facing voxel faces — the grass cap of a turf block.",
        SocketType::Color,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "side",
        "Side",
        "Color for the four vertical voxel faces.",
        SocketType::Color,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "bottom",
        "Bottom",
        "Color for down-facing voxel faces.",
        SocketType::Color,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const FACE_COLOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "The color belonging to the face currently being shaded.",
    SocketType::Color,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

const FACE_COLOR_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "base",
        "Base",
        "Fallback face color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.8, 0.8, 0.8, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "top",
        "Top",
        "Up-facing color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.8, 0.8, 0.8, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "side",
        "Side",
        "Side-facing color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.8, 0.8, 0.8, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "bottom",
        "Bottom",
        "Down-facing color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.8, 0.8, 0.8, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
];

pub const DECLARATION: NodeDeclaration = node!(
    "material.face_color",
    NodeOperation::Material(MaterialNodeOperation::FaceColor),
    "Face Color",
    "Selects color by voxel face orientation.",
    NodeCategory::MaterialOutput,
    NodePreview::Value,
    MATERIAL,
    FACE_COLOR_IN,
    FACE_COLOR_OUT,
    FACE_COLOR_FIELDS,
    TemporalDependence::Inherited,
);
