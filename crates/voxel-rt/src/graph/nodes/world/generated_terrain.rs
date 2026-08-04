//! `world.generated_terrain` — Generated Terrain.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from `voxel_material_graph::declare`.

use crate::graph::WorldNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const GENERATED_TERRAIN_OUT: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The deterministic base terrain, before any environment, biome or surface \
     program runs.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "world.generated_terrain",
    WorldNodeOperation::GeneratedTerrain,
    "Generated Terrain",
    "Creates the deterministic base voxel terrain.",
    NodeCategory::Environment,
    NodePreview::None,
    WORLD,
    &[],
    GENERATED_TERRAIN_OUT,
    &[],
    TemporalDependence::Inherited,
);
