//! `biome.cellular_seed` — step one of the cellular cave recipe: the noise the
//! rest of the chain refines.
//!
//! This is its own node because seeding answers one question — how much of the
//! grid starts as wall, and at what cell resolution — and that answer is the one
//! an author changes when rooms come out too cramped or too soupy. Smoothing,
//! cleanup and connection each answer a different question, so they are separate
//! nodes downstream; keeping the seed apart means retuning fill does not mean
//! scrolling past a dozen unrelated smoothing knobs.

use crate::graph::BiomeNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const CELLULAR_SEED_IN: &[SocketDeclarationStatic] = &[socket!(
    "biomes",
    "Biomes",
    "The biome field whose cave regions this seed pass fills with initial walls.",
    SocketType::BiomeField,
    EvaluationRate::PerVoxel,
    Cardinality::REQUIRED_SINGLE
)];

const CELLULAR_SEED_OUT: &[SocketDeclarationStatic] = &[socket!(
    "biomes",
    "Biomes",
    "The biome field carrying the seeded, not yet smoothed cellular grid.",
    SocketType::BiomeField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

const CELLULAR_SEED_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "cellular_fill",
        "Initial Wall Fill",
        "Fraction of cells seeded as wall before any smoothing. Lower values open \
         larger rooms; above roughly 0.55 smoothing closes the grid solid and no \
         cave survives.",
        FieldTarget::Property,
        FieldDefault::Scalar(0.40),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "cellular_cell_size",
        "Cell Size",
        "World voxels represented by one cellular cell. Larger cells make broader \
         rooms and fewer one-voxel trenches, at the cost of blockier walls.",
        FieldTarget::Property,
        FieldDefault::Integer(2),
        Some(NumericRange::new(1.0, 8.0)),
        Some(NumericRange::new(1.0, 8.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "biome.cellular_seed",
    BiomeNodeOperation::CellularSeed,
    "Cellular Seed",
    "Seeds the cellular cave grid with an initial wall fill at a chosen cell size.",
    NodeCategory::Biomes,
    NodePreview::Noise,
    BIOME,
    CELLULAR_SEED_IN,
    CELLULAR_SEED_OUT,
    CELLULAR_SEED_FIELDS,
    TemporalDependence::Inherited,
);
