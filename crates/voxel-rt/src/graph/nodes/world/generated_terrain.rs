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

const GENERATED_TERRAIN_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "surface_headroom",
        "Surface Headroom",
        "Empty air voxels reserved above the highest generated terrain surface.",
        FieldTarget::Property,
        FieldDefault::Integer(10),
        // The hard range stays generous, and generation clamps rather than
        // panicking on a value the current world height cannot honour — the debug
        // window reports the value actually used.
        Some(NumericRange::new(0.0, 4096.0)),
        Some(NumericRange::new(0.0, 256.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "sea_level",
        "Sea Level",
        "Highest world voxel that open water fills. Terrain below it floods.",
        FieldTarget::Property,
        FieldDefault::Integer(32),
        Some(NumericRange::new(0.0, 4096.0)),
        Some(NumericRange::new(0.0, 64.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "base_height",
        "Base Height",
        "Terrain height at the middle of the relief range. Put it above Sea Level \
         for mostly land, below it for mostly ocean.",
        FieldTarget::Property,
        FieldDefault::Integer(34),
        Some(NumericRange::new(0.0, 4096.0)),
        Some(NumericRange::new(0.0, 64.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "relief",
        "Relief",
        "Peak-to-trough terrain height range. Half sits below Base Height, which \
         is what puts coastline on both sides of the sea level.",
        FieldTarget::Property,
        FieldDefault::Integer(20),
        Some(NumericRange::new(0.0, 4096.0)),
        Some(NumericRange::new(0.0, 64.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
];

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
    GENERATED_TERRAIN_FIELDS,
    TemporalDependence::Inherited,
);
