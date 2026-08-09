//! `biome.blend` — spatial biome influence blending.

use crate::graph::BiomeNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const INPUTS: &[SocketDeclarationStatic] = &[socket!(
    "biomes",
    "Biomes",
    "Biome influences to blend by the world classifier.",
    SocketType::BiomeField,
    EvaluationRate::PerVoxel,
    Cardinality::OPTIONAL_SINGLE
)];

const OUTPUTS: &[SocketDeclarationStatic] = &[socket!(
    "biomes",
    "Biomes",
    "The blended biome field passed to world and cave programs.",
    SocketType::BiomeField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "biome.blend",
    BiomeNodeOperation::Blend,
    "Biome Blend",
    "Blends overlapping biome influences into one deterministic field.",
    NodeCategory::Biomes,
    NodePreview::None,
    BIOME,
    INPUTS,
    OUTPUTS,
    &[],
    TemporalDependence::Inherited,
);
