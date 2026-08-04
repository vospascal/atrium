//! `material.noise` — Noise.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const PROCEDURAL_SCALAR_IN: &[SocketDeclarationStatic] = &[
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
        "Noise frequency: larger values pack more features into the same metre.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "detail",
        "Detail",
        "How many fractal octaves are summed; higher adds finer grain.",
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

const NOISE_OUT: &[SocketDeclarationStatic] = &[
    socket!(
        "factor",
        "Factor",
        "Noise amplitude at Position, 0..1.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::ANY
    ),
    socket!(
        "color",
        "Color",
        "Three decorrelated noise channels packed as an RGB color, each 0..1.",
        SocketType::Color,
        EvaluationRate::PerSample,
        Cardinality::ANY
    ),
];

const NOISE_FIELDS: &[FieldDeclarationStatic] = &[
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
        "detail",
        "Detail",
        "Fractal octave detail.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(3.0),
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
    "material.noise",
    NodeOperation::Material(MaterialNodeOperation::Noise),
    "Noise",
    "Procedural multi-octave noise.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    PROCEDURAL_SCALAR_IN,
    NOISE_OUT,
    NOISE_FIELDS,
    TemporalDependence::Inherited,
);
