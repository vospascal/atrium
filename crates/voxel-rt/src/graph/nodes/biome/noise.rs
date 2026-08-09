//! `biome.noise` — a deterministic scalar field used by biome selectors.

use crate::graph::BiomeNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const INPUTS: &[SocketDeclarationStatic] = &[
    socket!(
        "position",
        "Position",
        "World-space position sampled by the biome field.",
        SocketType::Vector3,
        EvaluationRate::PerVoxel,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "scale",
        "Scale",
        "Noise frequency in world voxels.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "octaves",
        "Octaves",
        "Number of fractal noise octaves.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "roughness",
        "Roughness",
        "Amplitude retained by each octave.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const OUTPUTS: &[SocketDeclarationStatic] = &[socket!(
    "factor",
    "Factor",
    "Deterministic 0..1 noise value used to select and blend biomes.",
    SocketType::Scalar,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

const FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "position",
        "Position",
        "Default world-space sample position.",
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
        "Noise frequency in world voxels.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1280.0),
        POSITIVE,
        Some(NumericRange::new(0.01, 4096.0)),
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "octaves",
        "Octaves",
        "Fractal octave count.",
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
        "Amplitude retained by each octave.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.5),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "biome.noise",
    BiomeNodeOperation::Noise,
    "Biome Noise",
    "Samples a deterministic fractal field for biome and cave selection.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    BIOME,
    INPUTS,
    OUTPUTS,
    FIELDS,
    TemporalDependence::Inherited,
);
