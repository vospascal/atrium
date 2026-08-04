//! The catalogue a graph is validated against.
//!
//! Both halves are parameters: the node declarations AND the per-kind contracts. Neither is
//! hardcoded here, which is what lets one crate serve material, texture, audio and
//! animation graphs without knowing what any of them mean.

use crate::contract::GraphContractStatic;
use crate::declaration::{GraphKind, NodeDeclaration, OperationTag};
use crate::id::NodeTypeId;
use crate::socket::Cardinality;

/// A node catalogue: the declarations, plus the per-kind contracts they must satisfy.
///
/// `Copy` and built entirely from `&'static` data, so passing one costs nothing and a
/// catalogue can be a `const`.
#[derive(Clone, Copy, Debug)]
pub struct NodeRegistry {
    declarations: &'static [NodeDeclaration],
    contracts: &'static [GraphContractStatic],
}

impl NodeRegistry {
    /// Both halves are supplied by the caller. There is deliberately no `builtin()` and no
    /// `Default`: this crate has no nodes of its own, so a default registry could only ever
    /// mean "somebody else's catalogue", which is how the contracts ended up read from a
    /// module-level `static` in the first place.
    pub const fn new(
        declarations: &'static [NodeDeclaration],
        contracts: &'static [GraphContractStatic],
    ) -> Self {
        Self {
            declarations,
            contracts,
        }
    }

    pub fn declarations(&self) -> &'static [NodeDeclaration] {
        self.declarations
    }

    pub fn find(&self, id: &NodeTypeId) -> Option<&'static NodeDeclaration> {
        self.declarations.iter().find(|node| node.id == id.0)
    }

    pub fn contracts(&self) -> &'static [GraphContractStatic] {
        self.contracts
    }

    pub fn contract(&self, kind: GraphKind) -> Option<&'static GraphContractStatic> {
        self.contracts.iter().find(|contract| contract.kind == kind)
    }

    pub fn node_cardinality(&self, kind: GraphKind, operation: OperationTag) -> Cardinality {
        self.contract(kind)
            .and_then(|contract| {
                contract
                    .nodes
                    .iter()
                    .find(|constraint| constraint.operation == operation)
            })
            .map_or(Cardinality::ANY, |constraint| constraint.cardinality)
    }
}
