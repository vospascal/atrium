//! `world.studio_preview` — Studio Preview.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{NodeOperation, WorldNodeOperation};

const STUDIO_PREVIEW_OUT: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The isolated preview plate and subject used to look at a single material.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];

pub const DECLARATION: NodeDeclaration = node!(
    "world.studio_preview",
    NodeOperation::World(WorldNodeOperation::StudioPreview),
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
