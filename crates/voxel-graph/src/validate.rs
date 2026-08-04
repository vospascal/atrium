//! Resolving a document against a catalogue, and what is wrong with it.
//!
//! Validation never mutates and never refuses — it reports. A [`Diagnostic`] is addressed to
//! the author, so its message names the node type the way the author sees it, which is why
//! [`OperationTag`](crate::declaration::OperationTag) only has to be comparable and
//! printable.

use std::collections::{BTreeMap, BTreeSet};

use serde::Serialize;

use crate::contract::GraphContractStatic;
use crate::declaration::NodeDeclaration;
use crate::document::{GraphAsset, GraphInterface};
use crate::id::{LinkId, NodeId};
use crate::registry::NodeRegistry;
use crate::socket::Cardinality;

#[derive(Clone, Debug)]
pub struct ResolvedNode {
    pub id: NodeId,
    pub declaration: Option<&'static NodeDeclaration>,
}

#[derive(Clone, Debug)]
pub struct ResolvedLink {
    pub id: LinkId,
    pub from: usize,
    pub to: usize,
}

pub(crate) fn cardinality_description(cardinality: Cardinality) -> String {
    cardinality.description()
}

/// Every node that reaches the graph's output sink through links.
///
/// The sinks are the graph's declared interface outputs together with, for a
/// kind that has a contract, every node carrying one of the contract's flow
/// sink operations. Links are then walked backwards from those, so the result
/// is exactly the set of nodes whose value can still arrive somewhere the
/// engine reads. This is the single reachability traversal in the module:
/// contract validation and the inert-node warning both read its answer rather
/// than each re-deriving one.
pub fn node_reachability(graph: &GraphAsset, registry: &NodeRegistry) -> BTreeSet<NodeId> {
    let contract = registry.contract(graph.kind);
    let mut sources_of: BTreeMap<&NodeId, Vec<&NodeId>> = BTreeMap::new();
    for link in graph.links.values() {
        if !graph.nodes.contains_key(&link.from.node) || !graph.nodes.contains_key(&link.to.node) {
            continue;
        }
        sources_of
            .entry(&link.to.node)
            .or_default()
            .push(&link.from.node);
    }
    let mut pending: Vec<&NodeId> = graph
        .interface
        .outputs
        .values()
        .map(|pin| &pin.node)
        .filter(|node| graph.nodes.contains_key(*node))
        .collect();
    for (id, record) in &graph.nodes {
        let Some(declaration) = registry.find(&record.node_type) else {
            continue;
        };
        let is_sink = contract.is_some_and(|contract| {
            contract
                .flows
                .iter()
                .any(|flow| flow.sink == declaration.operation)
        });
        if is_sink {
            pending.push(id);
        }
    }
    let mut reached = BTreeSet::new();
    while let Some(node) = pending.pop() {
        if !reached.insert(node.clone()) {
            continue;
        }
        if let Some(sources) = sources_of.get(node) {
            pending.extend(sources.iter().copied());
        }
    }
    reached
}

pub(crate) fn validate_graph_contract(
    graph: &GraphAsset,
    contract: &GraphContractStatic,
    nodes: &[ResolvedNode],
    links: &[ResolvedLink],
    reachable: &BTreeSet<NodeId>,
    diagnostics: &mut Vec<Diagnostic>,
) {
    for flow in contract.flows {
        let sources = nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                node.declaration
                    .is_some_and(|declaration| declaration.operation == flow.source)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        let sinks = nodes
            .iter()
            .enumerate()
            .filter(|(_, node)| {
                node.declaration
                    .is_some_and(|declaration| declaration.operation == flow.sink)
            })
            .map(|(index, _)| index)
            .collect::<Vec<_>>();
        if sinks.len() != 1 {
            continue;
        }
        let sink = sinks[0];

        if sources.len() != 1 {
            continue;
        }

        let source = sources[0];
        let mut current = source;
        let mut visited = BTreeSet::from([source]);
        loop {
            let outgoing = links
                .iter()
                .filter(|link| {
                    if link.from != current {
                        return false;
                    }
                    let Some(record) = graph.links.get(&link.id) else {
                        return false;
                    };
                    nodes[current]
                        .declaration
                        .and_then(|declaration| declaration.output(&record.from.socket))
                        .is_some_and(|socket| socket.value_type == flow.value_type)
                })
                .collect::<Vec<_>>();
            if outgoing.len() != 1 {
                diagnostics.push(Diagnostic::error(
                    "flow_cardinality",
                    format!(
                        "node {} has {} outgoing {:?} flow links; expected exactly one",
                        nodes[current].id,
                        outgoing.len(),
                        flow.value_type
                    ),
                ));
                break;
            }
            let next = outgoing[0].to;
            if !visited.insert(next) {
                break;
            }
            if next == sink {
                break;
            }
            let allowed = nodes[next]
                .declaration
                .is_some_and(|declaration| flow.intermediates.contains(&declaration.operation));
            if !allowed {
                diagnostics.push(Diagnostic::error(
                    "flow_node",
                    format!(
                        "node {} is not allowed in the {:?} flow",
                        nodes[next].id, flow.value_type
                    ),
                ));
                break;
            }
            current = next;
        }
        if !reachable.contains(&nodes[source].id) {
            diagnostics.push(Diagnostic::error(
                "flow_incomplete",
                format!(
                    "{:?} flow does not reach node {}",
                    flow.value_type, nodes[sink].id
                ),
            ));
        }
        for node in nodes.iter() {
            if node
                .declaration
                .is_some_and(|declaration| flow.intermediates.contains(&declaration.operation))
                && !reachable.contains(&node.id)
            {
                diagnostics.push(Diagnostic::error(
                    "flow_disconnected",
                    format!("node {} is disconnected from the canonical flow", node.id),
                ));
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct ResolvedGraph {
    pub nodes: Vec<ResolvedNode>,
    pub node_indices: BTreeMap<NodeId, usize>,
    pub links: Vec<ResolvedLink>,
    pub incoming: Vec<Vec<usize>>,
    pub outgoing: Vec<Vec<usize>>,
    pub active_nodes: BTreeSet<NodeId>,
    pub cycle_nodes: BTreeSet<NodeId>,
    pub hashes: GraphHashes,
    pub diagnostics: Vec<Diagnostic>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct GraphHashes {
    pub semantic: u64,
    pub output_topology: u64,
    pub layout: u64,
}

impl GraphHashes {
    pub(crate) fn from_graph(graph: &GraphAsset, active: &BTreeSet<NodeId>) -> Self {
        let semantic = hash_json(&(graph.kind, &graph.interface, &graph.nodes, &graph.links));
        let active_links: Vec<_> = graph
            .links
            .iter()
            .filter(|(_, link)| active.contains(&link.from.node) && active.contains(&link.to.node))
            .collect();
        let output_topology = hash_json(&(graph.kind, &graph.interface, active, active_links));
        let layout = hash_json(&graph.layout);
        Self {
            semantic,
            output_topology,
            layout,
        }
    }
}

fn hash_json(value: &impl Serialize) -> u64 {
    serde_json::to_vec(value)
        .unwrap_or_default()
        .iter()
        .fold(0xcbf29ce484222325_u64, |hash, byte| {
            (hash ^ u64::from(*byte)).wrapping_mul(0x100000001b3)
        })
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum DiagnosticSeverity {
    Error,
    Warning,
    Info,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Diagnostic {
    pub severity: DiagnosticSeverity,
    pub code: &'static str,
    pub message: String,
}

impl Diagnostic {
    pub fn error(code: &'static str, message: String) -> Self {
        Self {
            severity: DiagnosticSeverity::Error,
            code,
            message,
        }
    }

    pub fn warning(code: &'static str, message: String) -> Self {
        Self {
            severity: DiagnosticSeverity::Warning,
            code,
            message,
        }
    }

    pub fn info(code: &'static str, message: String) -> Self {
        Self {
            severity: DiagnosticSeverity::Info,
            code,
            message,
        }
    }
}

pub(crate) fn cycle_nodes(nodes: &[ResolvedNode], links: &[ResolvedLink]) -> BTreeSet<NodeId> {
    let count = nodes.len();
    let mut indegree = vec![0; count];
    let mut outgoing = vec![Vec::new(); count];
    for link in links {
        indegree[link.to] += 1;
        outgoing[link.from].push(link.to);
    }
    let mut queue: Vec<_> = indegree
        .iter()
        .enumerate()
        .filter_map(|(index, &degree)| (degree == 0).then_some(index))
        .collect();
    let mut visited = BTreeSet::new();
    while let Some(index) = queue.pop() {
        visited.insert(index);
        for &next in &outgoing[index] {
            indegree[next] -= 1;
            if indegree[next] == 0 {
                queue.push(next);
            }
        }
    }
    nodes
        .iter()
        .enumerate()
        .filter_map(|(index, node)| (!visited.contains(&index)).then_some(node.id.clone()))
        .collect()
}

pub(crate) fn active_slice(
    interface: &GraphInterface,
    indices: &BTreeMap<NodeId, usize>,
    links: &[ResolvedLink],
) -> BTreeSet<NodeId> {
    let mut reverse = vec![Vec::new(); indices.len()];
    for link in links {
        reverse[link.to].push(link.from);
    }
    let mut ids: Vec<_> = interface
        .outputs
        .values()
        .filter_map(|pin| indices.get(&pin.node).copied())
        .collect();
    let mut seen = BTreeSet::new();
    while let Some(index) = ids.pop() {
        if !seen.insert(index) {
            continue;
        }
        ids.extend(reverse[index].iter().copied());
    }
    indices
        .iter()
        .filter_map(|(id, &index)| seen.contains(&index).then_some(id.clone()))
        .collect()
}
