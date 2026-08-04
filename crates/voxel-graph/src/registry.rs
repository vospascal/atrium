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
    families: &'static [&'static [NodeDeclaration]],
    contracts: &'static [&'static [GraphContractStatic]],
}

impl NodeRegistry {
    /// Both halves are supplied by the caller. There is deliberately no `builtin()` and no
    /// `Default`: this crate has no nodes of its own, so a default registry could only ever
    /// mean "somebody else's catalogue", which is how the contracts ended up read from a
    /// module-level `static` in the first place.
    pub const fn new(
        families: &'static [&'static [NodeDeclaration]],
        contracts: &'static [&'static [GraphContractStatic]],
    ) -> Self {
        Self {
            families,
            contracts,
        }
    }

    /// Every declaration across every family, flattened.
    ///
    /// A slice OF SLICES rather than one slice, because that is what makes a catalogue
    /// composable: `voxel-rt` names `voxel_material_graph::NODES` and its own world family
    /// side by side, and neither crate has to restate the other's nodes. Rust cannot
    /// concatenate `&'static` slices in a `const`, so the nesting is the composition.
    pub fn declarations(&self) -> impl Iterator<Item = &'static NodeDeclaration> + '_ {
        self.families.iter().copied().flatten()
    }

    pub fn find(&self, id: &NodeTypeId) -> Option<&'static NodeDeclaration> {
        self.declarations().find(|node| node.id == id.0)
    }

    /// Every contract across every family, flattened. Same reasoning as [`Self::declarations`].
    pub fn contracts(&self) -> impl Iterator<Item = &'static GraphContractStatic> + '_ {
        self.contracts.iter().copied().flatten()
    }

    pub fn contract(&self, kind: GraphKind) -> Option<&'static GraphContractStatic> {
        self.contracts().find(|contract| contract.kind == kind)
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
