//! `world.studio_preview` — Studio Preview.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from `voxel_material_graph::declare`.

use crate::graph::WorldNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const STUDIO_PREVIEW_OUT: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The isolated preview plate and subject used to look at a single material.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "world.studio_preview",
    WorldNodeOperation::StudioPreview,
    "Studio Preview",
    "Builds the isolated material preview plate and subject.",
    NodeCategory::Inputs,
    NodePreview::None,
    WORLD,
    &[],
    STUDIO_PREVIEW_OUT,
    &[],
    TemporalDependence::Inherited,
);
