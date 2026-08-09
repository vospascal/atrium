//! `world.cave_detail` — the organic bumps on cave floors and ceilings.
//!
//! Separate from `world.cave_chambers` because it is a look pass, not a volume
//! pass: chambers decide how much cave there is, detail decides whether its
//! surfaces read as carved rock or as smooth noise. Splitting it also means a
//! low-end quality preset can dial detail to zero without touching the shape a
//! world's layout depends on.

use crate::graph::WorldNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const CAVE_DETAIL_IN: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The world whose cave surfaces receive organic detail.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::REQUIRED_SINGLE
)];

const CAVE_DETAIL_OUT: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The world with cave floor and ceiling detail applied.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

const CAVE_DETAIL_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "cellular_billow_scale",
        "Billow Scale",
        "3D Billow frequency for floor and ceiling relief. Larger values add \
         finer bumps; past roughly 0.3 the detail is smaller than a voxel and \
         only reads as noise.",
        FieldTarget::Property,
        FieldDefault::Scalar(0.08),
        POSITIVE,
        Some(NumericRange::new(0.001, 1.0)),
        Some(0.001),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "cellular_billow_strength",
        "Billow Strength",
        "How strongly the Billow detail is subtracted from the chamber volume. \
         High values eat into chambers hard enough to pinch off passages, so \
         check connectivity after raising it.",
        FieldTarget::Property,
        FieldDefault::Scalar(0.40),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "cellular_billow_octaves",
        "Billow Octaves",
        "Number of fractal layers in the detail. 0 disables detail outright; \
         each extra layer costs generation time for progressively finer relief.",
        FieldTarget::Property,
        FieldDefault::Integer(3),
        Some(NumericRange::new(0.0, 8.0)),
        Some(NumericRange::new(0.0, 8.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "world.cave_detail",
    WorldNodeOperation::CaveDetail,
    "Cave Detail",
    "Adds organic Billow relief to cave floors and ceilings.",
    NodeCategory::Features,
    NodePreview::None,
    WORLD,
    CAVE_DETAIL_IN,
    CAVE_DETAIL_OUT,
    CAVE_DETAIL_FIELDS,
    TemporalDependence::Inherited,
);
