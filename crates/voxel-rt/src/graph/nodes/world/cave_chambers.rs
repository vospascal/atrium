//! `world.cave_chambers` — the rounded volumes the cellular cave recipe carves.
//!
//! Its own node because chamber *scale* and chamber *density* are the two
//! decisions that set how much of the underground is open at all, and they are
//! read together: change the Worley scale and the openness cut needs revisiting.
//! Keeping them here, off `world.cave_carver`, also keeps that node about WHERE
//! caves may exist rather than what they look like — detail and fluids are
//! likewise their own nodes downstream.

use crate::graph::WorldNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const CAVE_CHAMBERS_IN: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The carved world whose cellular chambers this node shapes.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::REQUIRED_SINGLE
)];

const CAVE_CHAMBERS_OUT: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The world with cellular chamber volumes applied.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

const CAVE_CHAMBERS_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "cellular_worley_scale",
        "Worley Scale",
        "3D Worley frequency for the rounded chamber volumes. Larger values make \
         smaller, more numerous pockets; halving it roughly doubles chamber width.",
        FieldTarget::Property,
        FieldDefault::Scalar(0.05),
        POSITIVE,
        Some(NumericRange::new(0.001, 1.0)),
        Some(0.001),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "cellular_openness",
        "Openness",
        "How much of a cellular-open column actually opens. This is the cellular \
         recipe's own density cut, separate from Threshold, which belongs to the \
         value-noise shapes on Cave Carver — tuning one does nothing to the other.",
        FieldTarget::Property,
        FieldDefault::Scalar(0.72),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "world.cave_chambers",
    WorldNodeOperation::CaveChambers,
    "Cave Chambers",
    "Shapes the rounded chamber volumes of the cellular cave recipe.",
    NodeCategory::Features,
    NodePreview::None,
    WORLD,
    CAVE_CHAMBERS_IN,
    CAVE_CHAMBERS_OUT,
    CAVE_CHAMBERS_FIELDS,
    TemporalDependence::Inherited,
);
