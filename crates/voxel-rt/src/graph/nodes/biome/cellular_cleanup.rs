//! `biome.cellular_cleanup` — step three: strict passes that remove speckle.
//!
//! One field, and still its own node. Cleanup is a different *intent* from
//! smoothing: smoothing decides room shape, cleanup only deletes the isolated
//! wall and floor pixels that shape left behind. Folding it back into smoothing
//! is what produced the original fourteen-field node, where two iteration counts
//! sat side by side and nobody could say which one to reach for.

use crate::graph::BiomeNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const CELLULAR_CLEANUP_IN: &[SocketDeclarationStatic] = &[socket!(
    "biomes",
    "Biomes",
    "The biome field carrying a smoothed cellular grid to clean up.",
    SocketType::BiomeField,
    EvaluationRate::PerVoxel,
    Cardinality::REQUIRED_SINGLE
)];

const CELLULAR_CLEANUP_OUT: &[SocketDeclarationStatic] = &[socket!(
    "biomes",
    "Biomes",
    "The biome field with isolated wall and floor speckle removed.",
    SocketType::BiomeField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

const CELLULAR_CLEANUP_FIELDS: &[FieldDeclarationStatic] = &[field(
    "cleanup_iterations",
    "Cleanup Iterations",
    "Strict 3x3 passes that delete cells with almost no support, removing lone \
     pillars and single-cell pockets. High counts also round off deliberate \
     narrow connectors, so raise this only until the speckle is gone.",
    FieldTarget::Property,
    FieldDefault::Integer(3),
    Some(NumericRange::new(0.0, 32.0)),
    Some(NumericRange::new(0.0, 16.0)),
    Some(1.0),
    EMPTY_CHOICES,
    false,
)];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "biome.cellular_cleanup",
    BiomeNodeOperation::CellularCleanup,
    "Cellular Cleanup",
    "Removes isolated speckle left over from cellular smoothing.",
    NodeCategory::Biomes,
    NodePreview::None,
    BIOME,
    CELLULAR_CLEANUP_IN,
    CELLULAR_CLEANUP_OUT,
    CELLULAR_CLEANUP_FIELDS,
    TemporalDependence::Inherited,
);
