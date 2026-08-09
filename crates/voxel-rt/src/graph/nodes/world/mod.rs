//! The `world` node family — one file per node.

use voxel_graph::NodeDeclaration;

pub(crate) mod cave_carver;
pub(crate) mod cave_chambers;
pub(crate) mod cave_detail;
pub(crate) mod cave_fluids;
pub mod compose;
pub(crate) mod generated_terrain;
pub mod output;
pub(crate) mod scatter;
pub(crate) mod scatter_rule;
pub(crate) mod studio_preview;

/// Every `world` node, in pipeline order: terrain, cave shape, cave fluids,
/// decoration, then composition and output.
pub const NODES: &[NodeDeclaration] = &[
    generated_terrain::DECLARATION,
    cave_carver::DECLARATION,
    cave_chambers::DECLARATION,
    cave_detail::DECLARATION,
    cave_fluids::DECLARATION,
    scatter_rule::DECLARATION,
    scatter::DECLARATION,
    compose::DECLARATION,
    output::DECLARATION,
    studio_preview::DECLARATION,
];
