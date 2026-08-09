//! `biome.cellular_smooth` — step two: neighbour-count smoothing turns seeded
//! noise into rooms.
//!
//! Its own node because this is where the cave's *character* is decided — the
//! two cutoffs and the iteration count are a rule, and a rule is worth reading
//! on its own card. Fill lives upstream on the seed, cleanup and connection
//! downstream: an author sweeping cutoffs is not touching any of those, and the
//! node boundary makes that independence visible in the graph.

use crate::graph::BiomeNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const CELLULAR_SMOOTH_IN: &[SocketDeclarationStatic] = &[socket!(
    "biomes",
    "Biomes",
    "The biome field carrying a seeded cellular grid to smooth.",
    SocketType::BiomeField,
    EvaluationRate::PerVoxel,
    Cardinality::REQUIRED_SINGLE
)];

const CELLULAR_SMOOTH_OUT: &[SocketDeclarationStatic] = &[socket!(
    "biomes",
    "Biomes",
    "The biome field after neighbour-count smoothing has formed rooms.",
    SocketType::BiomeField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

const CELLULAR_SMOOTH_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "iterations",
        "Iterations",
        "How many smoothing passes run. Rooms stop changing shape after about \
         six passes; more only costs generation time.",
        FieldTarget::Property,
        FieldDefault::Integer(4),
        Some(NumericRange::new(0.0, 32.0)),
        Some(NumericRange::new(0.0, 16.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "r1_cutoff",
        "3x3 Wall Cutoff",
        "A cell stays solid when at least this many of its eight neighbours are \
         wall. This is the main room-size control: 5 is the classic rule, 4 eats \
         walls away, 6 or more closes the grid up.",
        FieldTarget::Property,
        FieldDefault::Integer(5),
        Some(NumericRange::new(0.0, 9.0)),
        Some(NumericRange::new(0.0, 9.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "r2_cutoff",
        "5x5 Open Cutoff",
        "Wide-neighbour rule that reopens a cell when its 5x5 wall count falls \
         below this value, which is what carves large halls rather than a maze. \
         Use -1 to disable the rule entirely.",
        FieldTarget::Property,
        FieldDefault::Integer(2),
        Some(NumericRange::new(-1.0, 25.0)),
        Some(NumericRange::new(-1.0, 25.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "biome.cellular_smooth",
    BiomeNodeOperation::CellularSmooth,
    "Cellular Smoothing",
    "Smooths the seeded cellular grid with neighbour-count rules until rooms form.",
    NodeCategory::Biomes,
    NodePreview::Noise,
    BIOME,
    CELLULAR_SMOOTH_IN,
    CELLULAR_SMOOTH_OUT,
    CELLULAR_SMOOTH_FIELDS,
    TemporalDependence::Inherited,
);
