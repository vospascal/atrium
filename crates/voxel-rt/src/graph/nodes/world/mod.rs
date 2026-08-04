//! The `world` node family — one file per node.

use voxel_graph::NodeDeclaration;

pub mod compose;
pub mod generated_terrain;
pub mod output;
pub mod studio_preview;

/// Every `world` node, in catalogue order.
pub const NODES: &[NodeDeclaration] = &[
    generated_terrain::DECLARATION,
    compose::DECLARATION,
    output::DECLARATION,
    studio_preview::DECLARATION,
];
