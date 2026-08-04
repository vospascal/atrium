//! Per-graph-kind rules: which node types a graph must contain, and which routes are prescribed.
//!
//! The rules are DATA, supplied by the catalogue that owns the node family — this crate
//! only checks them. `voxel-material-graph` declares, for instance, that a material graph
//! holds exactly one Output node and that a Surface must reach it.

use crate::declaration::{GraphKind, OperationTag};
use crate::socket::{Cardinality, SocketType};

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlowConstraintStatic {
    pub value_type: SocketType,
    pub source: OperationTag,
    /// The node operations allowed to sit between source and sink. This is a
    /// canonical chain: the route is walked link by link and anything else on
    /// it is an error, so only declare a flow for a route that is genuinely
    /// prescribed. "This node must reach the output somehow" is not a flow —
    /// that is the `unreached-node` warning, which already covers every node.
    pub intermediates: &'static [OperationTag],
    pub sink: OperationTag,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeConstraintStatic {
    pub operation: OperationTag,
    pub cardinality: Cardinality,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphContractStatic {
    pub kind: GraphKind,
    pub nodes: &'static [NodeConstraintStatic],
    pub flows: &'static [FlowConstraintStatic],
}
