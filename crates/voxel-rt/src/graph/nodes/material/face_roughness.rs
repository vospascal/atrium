//! `material.face_roughness` — Face Roughness.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const FACE_SCALAR_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "base",
        "Base",
        "Roughness used by any face whose own input is left at its default.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "top",
        "Top",
        "Roughness for up-facing voxel faces, 0..1.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "side",
        "Side",
        "Roughness for the four vertical voxel faces, 0..1.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "bottom",
        "Bottom",
        "Roughness for down-facing voxel faces, 0..1.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const FACE_ROUGHNESS_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Roughness",
    "The roughness belonging to the face currently being shaded, 0..1.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

const FACE_ROUGHNESS_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "base",
        "Base",
        "Fallback roughness.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.6),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "top",
        "Top",
        "Up-facing roughness.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.6),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "side",
        "Side",
        "Side-facing roughness.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.6),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "bottom",
        "Bottom",
        "Down-facing roughness.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.6),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];

pub const DECLARATION: NodeDeclaration = node!(
    "material.face_roughness",
    NodeOperation::Material(MaterialNodeOperation::FaceRoughness),
    "Face Roughness",
    "Selects roughness by voxel face orientation.",
    NodeCategory::MaterialOutput,
    NodePreview::Value,
    MATERIAL,
    FACE_SCALAR_IN,
    FACE_ROUGHNESS_OUT,
    FACE_ROUGHNESS_FIELDS,
    TemporalDependence::Inherited,
);
