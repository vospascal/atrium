//! What a material graph must contain: exactly one Output, and a Surface that reaches it.
//!
//! Domain knowledge, so it lives with the nodes it constrains rather than with the graph
//! mechanics. `voxel-rt` composes these with its own families' contracts.

use voxel_graph::{
    Cardinality, FlowConstraintStatic, GraphContractStatic, GraphKind, NodeConstraintStatic,
    OperationTag, SocketType,
};

use crate::operation::MaterialNodeOperation;

const MATERIAL_SURFACE_INTERMEDIATES: &[OperationTag] = &[
    (MaterialNodeOperation::PatternLayer).tag(),
    (MaterialNodeOperation::Displacement).tag(),
];

const MATERIAL_NODE_CONSTRAINTS: &[NodeConstraintStatic] = &[
    NodeConstraintStatic {
        operation: (MaterialNodeOperation::Output).tag(),
        cardinality: Cardinality::EXACTLY_ONE,
    },
    NodeConstraintStatic {
        operation: (MaterialNodeOperation::Surface).tag(),
        cardinality: Cardinality::EXACTLY_ONE,
    },
    NodeConstraintStatic {
        operation: (MaterialNodeOperation::PatternLayer).tag(),
        cardinality: Cardinality::up_to(voxel_material::pattern::MAX_PATTERN_LAYERS),
    },
    NodeConstraintStatic {
        operation: (MaterialNodeOperation::Displacement).tag(),
        cardinality: Cardinality::up_to(voxel_material::pattern::MAX_PATTERN_LAYERS),
    },
];

const MATERIAL_FLOWS: &[FlowConstraintStatic] = &[
    FlowConstraintStatic {
        value_type: SocketType::MaterialSurface,
        source: (MaterialNodeOperation::Surface).tag(),
        intermediates: MATERIAL_SURFACE_INTERMEDIATES,
        sink: (MaterialNodeOperation::Output).tag(),
    },
    // S3 animation nodes deliberately get NO flow constraint. An oscillator
    // that reaches nothing is already reported by the `unreached-node` warning
    // in `resolve`, which covers every node rather than three named ones. A
    // flow here would fire on the identical condition at Error severity, and
    // Error blocks material compilation — so an oscillator left unwired for a
    // moment mid-edit would stop the material building. A warning is the honest
    // severity for "this has no effect yet".
];

/// This crate's half of the catalogue's contracts.
pub static CONTRACTS: &[GraphContractStatic] = &[GraphContractStatic {
    kind: GraphKind::Material,
    nodes: MATERIAL_NODE_CONSTRAINTS,
    flows: MATERIAL_FLOWS,
}];
