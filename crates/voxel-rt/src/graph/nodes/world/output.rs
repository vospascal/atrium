//! `world.output` — World Output.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from `voxel_material_graph::declare`.

use crate::graph::WorldNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const VOXEL_FIELD_IN: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The finished voxel world the engine streams and renders; every world graph \
     must terminate here.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::REQUIRED_SINGLE
)];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "world.output",
    WorldNodeOperation::Output,
    "World Output",
    "Final voxel world consumed by the engine.",
    NodeCategory::Render,
    NodePreview::None,
    WORLD,
    VOXEL_FIELD_IN,
    &[],
    &[],
    TemporalDependence::Inherited,
);
