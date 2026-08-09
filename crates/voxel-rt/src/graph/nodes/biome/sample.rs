//! `biome.sample` — deterministic biome field sampling.

use crate::graph::BiomeNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const INPUTS: &[SocketDeclarationStatic] = &[
    socket!(
        "environment",
        "Environment",
        "Global and spatial fields used to classify the current world position.",
        SocketType::Environment,
        EvaluationRate::PerVoxel,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "noise",
        "Noise",
        "Optional authored scalar field used to modulate biome selector weights.",
        SocketType::Scalar,
        EvaluationRate::PerVoxel,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const OUTPUTS: &[SocketDeclarationStatic] = &[socket!(
    "biomes",
    "Biomes",
    "Weighted biome influences at the current world position.",
    SocketType::BiomeField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

const FIELDS: &[FieldDeclarationStatic] = &[field(
    "noise",
    "Noise",
    "Default scalar used when no authored noise node is connected.",
    FieldTarget::InputSocket,
    FieldDefault::Scalar(0.5),
    UNIT,
    UNIT,
    Some(0.01),
    EMPTY_CHOICES,
    false,
)];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "biome.sample",
    BiomeNodeOperation::Sample,
    "Biome Sampler",
    "Samples authored biome selectors into a deterministic biome field.",
    NodeCategory::Biomes,
    NodePreview::None,
    BIOME,
    INPUTS,
    OUTPUTS,
    FIELDS,
    TemporalDependence::Inherited,
);
