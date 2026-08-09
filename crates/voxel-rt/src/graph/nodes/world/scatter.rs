//! `world.scatter` — generation pass two: decoration over finished shape.
//!
//! The runtime implementation is [`voxel_core::scatter::ScatterProgram`]. This
//! declaration is the graph seam, and it sits after cave carving on purpose: the
//! pass reads the finished shape (is this a cave floor, a cave ceiling, the lit
//! surface) rather than re-deriving terrain, which is what keeps it cheap enough
//! to re-run on its own.
//!
//! What to place is not authored here. Each decoration class is its own
//! `world.scatter_rule` node, chained into the `rules` socket, so a world adds
//! and removes classes by adding and removing nodes and every class gets its own
//! rate, patch shape and height band. This node keeps only the two controls that
//! really are global: whether pass two runs, and how tightly every rule's
//! patches gather.

use crate::graph::WorldNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const SCATTER_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "world",
        "World",
        "The finished terrain and caves that decoration is placed onto.",
        SocketType::VoxelField,
        EvaluationRate::PerVoxel,
        Cardinality::REQUIRED_SINGLE
    ),
    socket!(
        "biomes",
        "Biomes",
        "Optional biome field used to vary decoration by region.",
        SocketType::BiomeField,
        EvaluationRate::PerVoxel,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "rules",
        "Rules",
        "The chain of `world.scatter_rule` nodes describing what to place. With \
         nothing connected the pass runs and places nothing.",
        SocketType::FeatureSet,
        EvaluationRate::Uniform,
        Cardinality::ANY
    ),
];

const SCATTER_OUT: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The world with decoration placed on its surfaces.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

const SCATTER_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "enabled",
        "Enabled",
        "Whether generation pass two runs at all.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "clumping",
        "Clumping",
        "How tightly decoration gathers into patches. 1.0 is the authored \
         coverage; higher values shrink patches so the same rate reads as \
         denser clumps with barer ground between them.",
        FieldTarget::Property,
        FieldDefault::Scalar(1.0),
        Some(NumericRange::new(0.0, 4.0)),
        Some(NumericRange::new(0.25, 3.0)),
        Some(0.05),
        EMPTY_CHOICES,
        false,
    ),
];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "world.scatter",
    WorldNodeOperation::Scatter,
    "Scatter",
    "Places vegetation and cave decoration on finished terrain (generation pass two).",
    NodeCategory::Features,
    NodePreview::None,
    WORLD,
    SCATTER_IN,
    SCATTER_OUT,
    SCATTER_FIELDS,
    TemporalDependence::Inherited,
);
