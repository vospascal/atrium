//! `biome.output` — biome graph output.

use crate::graph::BiomeNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const INPUTS: &[SocketDeclarationStatic] = &[
    socket!(
        "biome",
        "Biome",
        "The authored biome definition consumed by world generation.",
        SocketType::BiomeDefinition,
        EvaluationRate::Uniform,
        Cardinality::REQUIRED_SINGLE
    ),
    socket!(
        "biomes",
        "Biomes",
        "Optional sampled influence field used by surface and cave routing.",
        SocketType::BiomeField,
        EvaluationRate::PerVoxel,
        Cardinality::OPTIONAL_SINGLE
    ),
];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "biome.output",
    BiomeNodeOperation::Output,
    "Biome Output",
    "Final biome definition exposed to the world generator.",
    NodeCategory::Render,
    NodePreview::None,
    BIOME,
    INPUTS,
    &[],
    &[],
    TemporalDependence::Inherited,
);
