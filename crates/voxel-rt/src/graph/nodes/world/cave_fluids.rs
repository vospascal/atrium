//! `world.cave_fluids` — water and lava in the voids the cave passes carved.
//!
//! Its own node because it is a different subject from cave shape: three world
//! heights and a switch, none of which change where a cave is, all of which
//! change what is standing in it. It sits after the shape nodes on purpose —
//! fluids need finished voids to fill — and being separate is what lets a world
//! be authored dry by simply not placing this node.

use crate::graph::WorldNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const CAVE_FLUIDS_IN: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The world whose finished cave voids may fill with fluid.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::REQUIRED_SINGLE
)];

const CAVE_FLUIDS_OUT: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The world with underground water and lava placed.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

/// One world-height field. Every level control here shares the same shape — a
/// wide hard range so extreme worlds stay authorable, and a narrow soft range
/// that keeps the slider useful for ordinary ones — so they are built from one
/// helper rather than four near-identical literal blocks.
const fn level(
    key: &'static str,
    label: &'static str,
    description: &'static str,
    default: i64,
) -> FieldDeclarationStatic {
    field(
        key,
        label,
        description,
        FieldTarget::Property,
        FieldDefault::Integer(default),
        Some(NumericRange::new(-4096.0, 4096.0)),
        Some(NumericRange::new(0.0, 64.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    )
}

const CAVE_FLUIDS_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "fluids_enabled",
        "Enabled",
        "Whether underground fluids are placed at all.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    level(
        "water_level",
        "Water Level",
        "Highest world voxel where a carved void fills with water.",
        20,
    ),
    level(
        "lava_level",
        "Lava Level",
        "Highest world voxel where a carved void fills with lava. Lava wins where \
         it overlaps the water level, so a lava level above the water level makes \
         the water disappear rather than float.",
        12,
    ),
    level(
        "fluid_min_y",
        "Fluid Floor",
        "Lowest world voxel any underground fluid may occupy, so the deepest \
         strata stay dry.",
        2,
    ),
];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "world.cave_fluids",
    WorldNodeOperation::CaveFluids,
    "Cave Fluids",
    "Fills finished cave voids with water and lava below the authored levels.",
    NodeCategory::Features,
    NodePreview::None,
    WORLD,
    CAVE_FLUIDS_IN,
    CAVE_FLUIDS_OUT,
    CAVE_FLUIDS_FIELDS,
    TemporalDependence::Inherited,
);
