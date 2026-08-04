//! `material.fbm` — Fractal Noise.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const FBM_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "position",
        "Position",
        "World-space point to sample the noise at, in metres.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "scale",
        "Scale",
        "Frequency of the first octave: larger values pack more features into the \
         same metre.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "octaves",
        "Octaves",
        "How many doublings of frequency are summed.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "roughness",
        "Roughness",
        "How much amplitude each octave keeps, 0..1; higher makes the noise grittier.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const FBM_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The summed fractal noise at Position, 0..1.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

const FBM_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "position",
        "Position",
        "Sampling position.",
        FieldTarget::InputSocket,
        FieldDefault::Vector3([0.0; 3]),
        NONE,
        NONE,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "scale",
        "Scale",
        "Noise frequency.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        POSITIVE,
        Some(NumericRange::new(0.01, 32.0)),
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "octaves",
        "Octaves",
        "Fractal octave count.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(5.0),
        Some(NumericRange::new(1.0, 16.0)),
        Some(NumericRange::new(1.0, 8.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "roughness",
        "Roughness",
        "Fractal amplitude decay.",
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
    "material.fbm",
    NodeOperation::Material(MaterialNodeOperation::Fbm),
    "Fractal Noise",
    "Fractal Brownian motion scalar noise.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    FBM_IN,
    FBM_OUT,
    FBM_FIELDS,
    TemporalDependence::Inherited,
);
