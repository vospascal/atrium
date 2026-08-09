//! The `biome` node family — classification, the cellular cave recipe, and biome output.

use voxel_graph::NodeDeclaration;

pub(crate) mod blend;
pub(crate) mod cellular_cleanup;
pub(crate) mod cellular_connect;
pub(crate) mod cellular_seed;
pub(crate) mod cellular_smooth;
pub(crate) mod definition;
pub(crate) mod noise;
pub(crate) mod output;
pub(crate) mod sample;

/// Every `biome` node, in pipeline order: classification, then the cellular cave
/// recipe in the order its steps must run, then output.
pub const NODES: &[NodeDeclaration] = &[
    definition::DECLARATION,
    noise::DECLARATION,
    blend::DECLARATION,
    cellular_seed::DECLARATION,
    cellular_smooth::DECLARATION,
    cellular_cleanup::DECLARATION,
    cellular_connect::DECLARATION,
    output::DECLARATION,
    sample::DECLARATION,
];
