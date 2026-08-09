//! `world.cave_carver` — deterministic cave mask application.
//!
//! The runtime implementation is [`voxel_core::cave::CaveProgram`]. This
//! declaration is the graph seam: it keeps cave carving between base terrain
//! and surface composition, where a future biome input can select a different
//! cave program without changing the world output contract.
//!
//! What this node owns is deliberately narrow: WHERE caves may exist (the Y
//! window and the region the mask applies to) and the shape of the value-noise
//! family — cheese, spaghetti, noodle — which is scale, threshold and salt. The
//! cellular recipe is authored in the BIOME graph, as a chain of
//! `biome.cellular_seed` → `smooth` → `cleanup` → `connect`, and the cellular
//! look in the world graph belongs to `world.cave_chambers`,
//! `world.cave_detail` and `world.cave_fluids`.
//!
//! That split also ends a real trap: every cellular parameter used to be
//! authored twice, once here and once on `biome.cellular_automata`, with nothing
//! saying which copy generation actually read.

use crate::graph::WorldNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const CAVE_CARVER_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "world",
        "World",
        "The base terrain whose solid voxels may be carved.",
        SocketType::VoxelField,
        EvaluationRate::PerVoxel,
        Cardinality::REQUIRED_SINGLE
    ),
    socket!(
        "biomes",
        "Biomes",
        "Optional biome field used to select cave programs by region.",
        SocketType::BiomeField,
        EvaluationRate::PerVoxel,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const CAVE_CARVER_OUT: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The terrain after the deterministic cave mask has been applied.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

const SHAPE_CHOICES: &[ChoiceDeclaration] = &[
    choice(
        "cheese",
        "Cheese",
        "Broad chambers with soft, open volumes.",
    ),
    choice(
        "spaghetti",
        "Spaghetti",
        "Winding tunnels with a denser strand field.",
    ),
    choice(
        "noodle",
        "Noodle",
        "Thin, long connectors between larger cave spaces.",
    ),
    choice(
        "cellular_automata",
        "Cellular Automata",
        "Seeded walls smoothed by neighbour-count rules for connected cave-like regions.",
    ),
];

const CAVE_CARVER_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "shape",
        "Shape",
        "The deterministic cave density family to evaluate.",
        FieldTarget::Property,
        FieldDefault::Text("cheese"),
        NONE,
        NONE,
        None,
        SHAPE_CHOICES,
        false,
    ),
    field(
        "min_y",
        "Minimum Y",
        "Lowest world voxel where caves may form.",
        FieldTarget::Property,
        // 8/56 rather than 4/24: these must match `CaveProgram::default()`, which
        // is what the runtime falls back to for a graph that omits them. When they
        // disagreed, the node card showed 4..24 while generation used 8..56.
        FieldDefault::Integer(8),
        Some(NumericRange::new(-4096.0, 4096.0)),
        Some(NumericRange::new(-4096.0, 4096.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "max_y",
        "Maximum Y",
        "Highest world voxel where caves may form.",
        FieldTarget::Property,
        FieldDefault::Integer(56),
        Some(NumericRange::new(-4096.0, 4096.0)),
        Some(NumericRange::new(-4096.0, 4096.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "horizontal_scale",
        "Horizontal Scale",
        "Horizontal size of cave features in world voxels.",
        FieldTarget::Property,
        FieldDefault::Scalar(40.0),
        POSITIVE,
        Some(NumericRange::new(0.01, 4096.0)),
        Some(0.1),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "vertical_scale",
        "Vertical Scale",
        "Vertical size of cave features in world voxels.",
        FieldTarget::Property,
        FieldDefault::Scalar(20.0),
        POSITIVE,
        Some(NumericRange::new(0.01, 4096.0)),
        Some(0.1),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "threshold",
        "Threshold",
        "Density at which a solid voxel becomes cave air.",
        FieldTarget::Property,
        FieldDefault::Scalar(0.70),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "salt",
        "Salt",
        "Stable per-node offset that gives this cave program its own field.",
        FieldTarget::Property,
        FieldDefault::Integer(0xCAFE_0001),
        Some(NumericRange::new(0.0, 4_294_967_295.0)),
        Some(NumericRange::new(0.0, 4_294_967_295.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "world.cave_carver",
    WorldNodeOperation::CaveCarver,
    "Cave Carver",
    "Applies a deterministic cave program before surface and feature composition.",
    NodeCategory::Features,
    NodePreview::None,
    WORLD,
    CAVE_CARVER_IN,
    CAVE_CARVER_OUT,
    CAVE_CARVER_FIELDS,
    TemporalDependence::Inherited,
);
