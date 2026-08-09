//! `biome.cellular_connect` — step four: discard the rooms nobody can reach and
//! tunnel between the ones that stay.
//!
//! Its own node because connection is the only step that reasons about the grid
//! *globally*: every earlier pass is a local neighbour rule, while this one
//! floods rooms, measures them and links them. That difference in scope is worth
//! a node boundary — it is also the step you turn off first when a cave should
//! read as disjoint pockets rather than one network.

use crate::graph::BiomeNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const CELLULAR_CONNECT_IN: &[SocketDeclarationStatic] = &[socket!(
    "biomes",
    "Biomes",
    "The biome field carrying a cleaned cellular grid whose rooms may be isolated.",
    SocketType::BiomeField,
    EvaluationRate::PerVoxel,
    Cardinality::REQUIRED_SINGLE
)];

const CELLULAR_CONNECT_OUT: &[SocketDeclarationStatic] = &[socket!(
    "biomes",
    "Biomes",
    "The biome field with tiny rooms discarded and the survivors joined by corridors.",
    SocketType::BiomeField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

const CELLULAR_CONNECT_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "min_room_cells",
        "Minimum Room Cells",
        "Rooms smaller than this many cells are filled back in as solid, which \
         is what stops the cave being littered with unreachable pockets. Set 0 \
         to keep every room.",
        FieldTarget::Property,
        FieldDefault::Integer(12),
        Some(NumericRange::new(0.0, 4096.0)),
        Some(NumericRange::new(0.0, 256.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "corridor_radius",
        "Corridor Radius",
        "Half-width in cells of the tunnels bored between surviving rooms. 0 \
         leaves the rooms unconnected; 1 is a crawlspace, 3 and up reads as an \
         excavated passage rather than a natural one.",
        FieldTarget::Property,
        FieldDefault::Integer(1),
        Some(NumericRange::new(0.0, 8.0)),
        Some(NumericRange::new(0.0, 8.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "biome.cellular_connect",
    BiomeNodeOperation::CellularConnect,
    "Cellular Connect",
    "Discards undersized cellular rooms and bores corridors between the rest.",
    NodeCategory::Biomes,
    NodePreview::None,
    BIOME,
    CELLULAR_CONNECT_IN,
    CELLULAR_CONNECT_OUT,
    CELLULAR_CONNECT_FIELDS,
    TemporalDependence::Inherited,
);
