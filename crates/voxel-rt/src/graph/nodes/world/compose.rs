//! `world.compose` — Surface Composer.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from `voxel_material_graph::declare`.

use crate::graph::WorldNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const WORLD_COMPOSE_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "world",
        "World",
        "The base terrain the registered programs are applied to.",
        SocketType::VoxelField,
        EvaluationRate::PerVoxel,
        Cardinality::REQUIRED_SINGLE
    ),
    socket!(
        "environment",
        "Environment",
        "Climate and lighting context the surface rules read; omitted, the profile's \
         own defaults apply.",
        SocketType::Environment,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "biomes",
        "Biomes",
        "Per-sample biome weights selecting which surface profile wins where.",
        SocketType::BiomeField,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const WORLD_COMPOSE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The terrain with environment, biome and surface programs applied.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

pub const DECLARATION: NodeDeclaration = node!(
    "world.compose",
    WorldNodeOperation::Compose,
    "Surface Composer",
    "Applies registered environment, biome, and surface programs to terrain.",
    NodeCategory::Surface,
    NodePreview::None,
    WORLD,
    WORLD_COMPOSE_IN,
    WORLD_COMPOSE_OUT,
    &[],
    TemporalDependence::Inherited,
);
