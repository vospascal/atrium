//! `world.output` — World Output.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{NodeOperation, WorldNodeOperation};

const VOXEL_FIELD_IN: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The finished voxel world the engine streams and renders; every world graph \
     must terminate here.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::REQUIRED_SINGLE
)];

pub const DECLARATION: NodeDeclaration = node!(
    "world.output",
    NodeOperation::World(WorldNodeOperation::Output),
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
