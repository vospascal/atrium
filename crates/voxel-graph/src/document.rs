//! The editable document: boxes, wires, and the operations that keep it consistent.
//!
//! This is the "picture" — what the node editor mutates and what gets saved as JSON. It can
//! be half-built: dangling wires and unset inputs are ordinary states, not bugs, which is
//! why validation is a separate step producing [`Diagnostic`](crate::validate::Diagnostic)s
//! rather than an invariant enforced on every edit.
//!
//! Nodes reference their type by [`NodeTypeId`] — a string — not by a typed operation. That
//! is what makes a document readable by a crate that has never heard of the node family it
//! was authored against.

use std::collections::{BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use crate::asset::{AssetId, STUDIO_ASSET_SCHEMA_VERSION};
use crate::declaration::GraphKind;
use crate::field::{FieldTarget, PropertyValue};
use crate::id::{LinkId, NodeId, NodeTypeId, SocketKey};
use crate::registry::NodeRegistry;
use crate::socket::{EvaluationRate, SocketDeclaration, SocketType};
use crate::validate::{
    active_slice, cardinality_description, cycle_nodes, node_reachability, validate_graph_contract,
    Diagnostic, GraphHashes, ResolvedGraph, ResolvedLink, ResolvedNode,
};

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct ConnectionPlan {
    pub replaced: Vec<(LinkId, LinkRecord)>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum ConnectionError {
    MissingNode(NodeId),
    UnknownOutput(OutputPin),
    UnknownInput(InputPin),
    TypeMismatch {
        from: SocketType,
        to: SocketType,
    },
    RateMismatch {
        from: EvaluationRate,
        to: EvaluationRate,
    },
    InputAtCapacity(InputPin),
    OutputAtCapacity(OutputPin),
    Cycle,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphInterface {
    #[serde(default)]
    pub inputs: BTreeMap<SocketKey, SocketDeclaration>,
    /// Named graph outputs bind the public interface to an output node pin.
    #[serde(default)]
    pub outputs: BTreeMap<SocketKey, OutputPin>,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct GraphLayout {
    #[serde(default)]
    pub positions: BTreeMap<NodeId, [f32; 2]>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct NodeRecord {
    pub node_type: NodeTypeId,
    pub node_type_version: u32,
    #[serde(default)]
    pub properties: BTreeMap<String, PropertyValue>,
    #[serde(default)]
    pub socket_defaults: BTreeMap<SocketKey, PropertyValue>,
    #[serde(default)]
    pub unknown_payload: Option<serde_json::Value>,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct OutputPin {
    pub node: NodeId,
    pub socket: SocketKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct InputPin {
    pub node: NodeId,
    pub socket: SocketKey,
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct LinkRecord {
    pub from: OutputPin,
    pub to: InputPin,
    #[serde(default)]
    pub order: u32,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct GraphAsset {
    pub schema_version: u32,
    pub id: AssetId,
    pub name: String,
    pub kind: GraphKind,
    #[serde(default)]
    pub interface: GraphInterface,
    #[serde(default)]
    pub nodes: BTreeMap<NodeId, NodeRecord>,
    #[serde(default)]
    pub links: BTreeMap<LinkId, LinkRecord>,
    #[serde(default)]
    pub layout: GraphLayout,
}

impl GraphAsset {
    pub fn new(name: impl Into<String>, kind: GraphKind) -> Self {
        Self {
            schema_version: STUDIO_ASSET_SCHEMA_VERSION,
            id: AssetId::new(),
            name: name.into(),
            kind,
            interface: GraphInterface::default(),
            nodes: BTreeMap::new(),
            links: BTreeMap::new(),
            layout: GraphLayout::default(),
        }
    }

    pub fn can_add_node_type(&self, registry: &NodeRegistry, node_type: &NodeTypeId) -> bool {
        let Some(declaration) = registry.find(node_type) else {
            return false;
        };
        if !declaration.kinds.contains(&self.kind) {
            return false;
        }
        let count = self
            .nodes
            .values()
            .filter(|node| {
                registry
                    .find(&node.node_type)
                    .is_some_and(|node| node.operation == declaration.operation)
            })
            .count();
        registry
            .node_cardinality(self.kind, declaration.operation)
            .maximum
            .is_none_or(|maximum| count < maximum)
    }

    pub fn incoming_link(&self, pin: &InputPin) -> Option<(&LinkId, &LinkRecord)> {
        self.links
            .iter()
            .find(|(_, link)| link.to.node == pin.node && link.to.socket == pin.socket)
    }

    /// Derive a complete connection decision from the node/socket schema.
    /// The editor uses this for hover affordances and commands use the same
    /// result when committing, so compatibility cannot drift between them.
    pub fn connection_plan(
        &self,
        registry: &NodeRegistry,
        from: &OutputPin,
        to: &InputPin,
    ) -> Result<ConnectionPlan, ConnectionError> {
        let from_node = self
            .nodes
            .get(&from.node)
            .ok_or_else(|| ConnectionError::MissingNode(from.node.clone()))?;
        let to_node = self
            .nodes
            .get(&to.node)
            .ok_or_else(|| ConnectionError::MissingNode(to.node.clone()))?;
        let from_socket = registry
            .find(&from_node.node_type)
            .and_then(|declaration| declaration.output(&from.socket))
            .ok_or_else(|| ConnectionError::UnknownOutput(from.clone()))?;
        let to_socket = registry
            .find(&to_node.node_type)
            .and_then(|declaration| declaration.input(&to.socket))
            .ok_or_else(|| ConnectionError::UnknownInput(to.clone()))?;
        if from_socket.value_type != to_socket.value_type {
            return Err(ConnectionError::TypeMismatch {
                from: from_socket.value_type,
                to: to_socket.value_type,
            });
        }
        if !from_socket.rate.can_feed(to_socket.rate) {
            return Err(ConnectionError::RateMismatch {
                from: from_socket.rate,
                to: to_socket.rate,
            });
        }

        let incoming = self
            .links
            .iter()
            .filter(|(_, link)| link.to.node == to.node && link.to.socket == to.socket)
            .map(|(id, link)| (id.clone(), link.clone()))
            .collect::<Vec<_>>();
        let outgoing = self
            .links
            .iter()
            .filter(|(_, link)| link.from.node == from.node && link.from.socket == from.socket)
            .map(|(id, link)| (id.clone(), link.clone()))
            .collect::<Vec<_>>();
        let mut replaced = Vec::new();
        if to_socket
            .cardinality
            .maximum
            .is_some_and(|maximum| incoming.len() >= maximum)
        {
            if to_socket.cardinality.maximum == Some(1) {
                replaced.extend(incoming);
            } else {
                return Err(ConnectionError::InputAtCapacity(to.clone()));
            }
        }
        if from_socket
            .cardinality
            .maximum
            .is_some_and(|maximum| outgoing.len() >= maximum)
        {
            if from_socket.cardinality.maximum == Some(1) {
                replaced.extend(outgoing);
            } else {
                return Err(ConnectionError::OutputAtCapacity(from.clone()));
            }
        }
        replaced.sort_by(|left, right| left.0.cmp(&right.0));
        replaced.dedup_by(|left, right| left.0 == right.0);
        let replaced_ids = replaced.iter().map(|(id, _)| id).collect::<BTreeSet<_>>();

        let mut pending = vec![to.node.clone()];
        let mut visited = BTreeSet::new();
        while let Some(node) = pending.pop() {
            if node == from.node {
                return Err(ConnectionError::Cycle);
            }
            if !visited.insert(node.clone()) {
                continue;
            }
            pending.extend(self.links.iter().filter_map(|(id, link)| {
                (!replaced_ids.contains(id) && link.from.node == node)
                    .then_some(link.to.node.clone())
            }));
        }
        Ok(ConnectionPlan { replaced })
    }

    pub fn resolve(&self, registry: &NodeRegistry) -> ResolvedGraph {
        let mut diagnostics = Vec::new();
        if self.schema_version > STUDIO_ASSET_SCHEMA_VERSION {
            diagnostics.push(Diagnostic::error(
                "unsupported_schema",
                format!(
                    "graph schema {} is newer than this Studio",
                    self.schema_version
                ),
            ));
        }
        let mut node_indices = BTreeMap::new();
        let mut nodes = Vec::new();
        for (index, (id, record)) in self.nodes.iter().enumerate() {
            let declaration = registry.find(&record.node_type);
            if let Some(declaration) = declaration {
                if !declaration.kinds.contains(&self.kind) {
                    diagnostics.push(Diagnostic::error(
                        "node_kind_mismatch",
                        format!("node {id} is not valid in this graph kind"),
                    ));
                }
                if record.node_type_version > declaration.version {
                    diagnostics.push(Diagnostic::error(
                        "unsupported_node_version",
                        format!(
                            "node {id} requires newer type version {}",
                            record.node_type_version
                        ),
                    ));
                }
                for (property, value) in &record.properties {
                    match declaration.field(FieldTarget::Property, property) {
                        Some(field) if field.accepts(value) => {}
                        Some(_) => diagnostics.push(Diagnostic::error(
                            "property_constraint",
                            format!(
                                "node {id} property `{property}` violates its declared type or constraints"
                            ),
                        )),
                        None => diagnostics.push(Diagnostic::error(
                            "unknown_property",
                            format!("node {id} has no declared property `{property}`"),
                        )),
                    }
                }
                for (socket, value) in &record.socket_defaults {
                    match declaration.input(socket) {
                        Some(input) if input.value_type == value.socket_type() => {
                            match declaration.field(FieldTarget::InputSocket, &socket.0) {
                                Some(field) if field.accepts(value) => {}
                                Some(_) => diagnostics.push(Diagnostic::error(
                                    "socket_default_constraint",
                                    format!(
                                    "node {id} default `{socket}` violates its declared constraints"
                                ),
                                )),
                                None => diagnostics.push(Diagnostic::error(
                                    "missing_socket_schema",
                                    format!(
                                    "node {id} input `{socket}` has no editable field declaration"
                                ),
                                )),
                            }
                        }
                        Some(_) => diagnostics.push(Diagnostic::error(
                            "socket_default_type",
                            format!("node {id} default `{socket}` has the wrong type"),
                        )),
                        None => diagnostics.push(Diagnostic::error(
                            "unknown_input_socket",
                            format!("node {id} has no input socket `{socket}`"),
                        )),
                    }
                }
            } else {
                diagnostics.push(Diagnostic::error(
                    "unknown_node_type",
                    format!("node {id} uses unavailable type `{}`", record.node_type),
                ));
            }
            node_indices.insert(id.clone(), index);
            nodes.push(ResolvedNode {
                id: id.clone(),
                declaration,
            });
        }

        if let Some(contract) = registry.contract(self.kind) {
            for constraint in contract.nodes {
                let count = self
                    .nodes
                    .values()
                    .filter(|node| {
                        registry
                            .find(&node.node_type)
                            .is_some_and(|node| node.operation == constraint.operation)
                    })
                    .count();
                if !constraint.cardinality.accepts(count) {
                    diagnostics.push(Diagnostic::error(
                        "node_cardinality",
                        format!(
                            "graph contains {count} {:?} node(s), expected {}",
                            constraint.operation,
                            cardinality_description(constraint.cardinality)
                        ),
                    ));
                }
            }
        }

        let mut links = Vec::new();
        let mut incoming = vec![Vec::new(); nodes.len()];
        let mut outgoing = vec![Vec::new(); nodes.len()];
        let mut input_counts = BTreeMap::new();
        let mut output_counts = BTreeMap::new();
        for (link_id, link) in &self.links {
            let Some(&from_index) = node_indices.get(&link.from.node) else {
                diagnostics.push(Diagnostic::error(
                    "missing_link_node",
                    format!("link {link_id} source node is missing"),
                ));
                continue;
            };
            let Some(&to_index) = node_indices.get(&link.to.node) else {
                diagnostics.push(Diagnostic::error(
                    "missing_link_node",
                    format!("link {link_id} destination node is missing"),
                ));
                continue;
            };
            let Some(from) = nodes[from_index]
                .declaration
                .and_then(|node| node.output(&link.from.socket))
            else {
                diagnostics.push(Diagnostic::error(
                    "unknown_output_socket",
                    format!("link {link_id} source socket is invalid"),
                ));
                continue;
            };
            let Some(to) = nodes[to_index]
                .declaration
                .and_then(|node| node.input(&link.to.socket))
            else {
                diagnostics.push(Diagnostic::error(
                    "unknown_input_socket",
                    format!("link {link_id} destination socket is invalid"),
                ));
                continue;
            };
            if from.value_type != to.value_type {
                diagnostics.push(Diagnostic::error(
                    "socket_type_mismatch",
                    format!("link {link_id} connects incompatible socket types"),
                ));
                continue;
            }
            if !from.rate.can_feed(to.rate) {
                diagnostics.push(Diagnostic::error(
                    "evaluation_rate_mismatch",
                    format!("link {link_id} feeds {:?} into {:?}", from.rate, to.rate),
                ));
                continue;
            }
            let input_key = (link.to.node.clone(), link.to.socket.clone());
            let input_count = input_counts.entry(input_key).or_insert(0);
            if to
                .cardinality
                .maximum
                .is_some_and(|maximum| *input_count >= maximum)
            {
                diagnostics.push(Diagnostic::error(
                    "input_cardinality",
                    format!("link {link_id} exceeds the destination socket cardinality"),
                ));
                continue;
            }
            let output_key = (link.from.node.clone(), link.from.socket.clone());
            let output_count = output_counts.entry(output_key).or_insert(0);
            if from
                .cardinality
                .maximum
                .is_some_and(|maximum| *output_count >= maximum)
            {
                diagnostics.push(Diagnostic::error(
                    "output_cardinality",
                    format!("link {link_id} exceeds the source socket cardinality"),
                ));
                continue;
            }
            *input_count += 1;
            *output_count += 1;
            let index = links.len();
            links.push(ResolvedLink {
                id: link_id.clone(),
                from: from_index,
                to: to_index,
            });
            outgoing[from_index].push(index);
            incoming[to_index].push(index);
        }
        for (id, record) in &self.nodes {
            let Some(declaration) = registry.find(&record.node_type) else {
                continue;
            };
            for socket in declaration.inputs {
                let count = input_counts
                    .get(&(id.clone(), SocketKey(socket.key.into())))
                    .copied()
                    .unwrap_or(0);
                if !socket.cardinality.accepts(count) {
                    diagnostics.push(Diagnostic::error(
                        "input_cardinality",
                        format!(
                            "node {id} input `{}` has {count} link(s), expected {}",
                            socket.key,
                            cardinality_description(socket.cardinality)
                        ),
                    ));
                }
            }
            for socket in declaration.outputs {
                let count = output_counts
                    .get(&(id.clone(), SocketKey(socket.key.into())))
                    .copied()
                    .unwrap_or(0);
                if !socket.cardinality.accepts(count) {
                    diagnostics.push(Diagnostic::error(
                        "output_cardinality",
                        format!(
                            "node {id} output `{}` has {count} link(s), expected {}",
                            socket.key,
                            cardinality_description(socket.cardinality)
                        ),
                    ));
                }
            }
        }
        let cycle_nodes = cycle_nodes(&nodes, &links);
        if !cycle_nodes.is_empty() {
            diagnostics.push(Diagnostic::error(
                "cycle",
                format!("graph has a cycle through {} node(s)", cycle_nodes.len()),
            ));
        }
        for (name, output) in &self.interface.outputs {
            if !node_indices.contains_key(&output.node) {
                diagnostics.push(Diagnostic::error(
                    "missing_graph_output",
                    format!("graph output `{name}` targets a missing node"),
                ));
            }
        }
        let reachable = node_reachability(self, registry);
        if let Some(contract) = registry.contract(self.kind) {
            validate_graph_contract(self, contract, &nodes, &links, &reachable, &mut diagnostics);
        }
        // An unreachable node is legal but inert, and silence is the worst way
        // for an editor to say so. Only report once the graph actually has a
        // sink to reach, otherwise every node in a sink-less draft is flagged.
        if !reachable.is_empty() {
            for id in self.nodes.keys() {
                if !reachable.contains(id) {
                    diagnostics.push(Diagnostic::warning(
                        "unreached-node",
                        format!(
                            "node {id} does not reach the graph output and has no effect on the result"
                        ),
                    ));
                }
            }
        }
        let active_nodes = active_slice(&self.interface, &node_indices, &links);
        let hashes = GraphHashes::from_graph(self, &active_nodes);
        ResolvedGraph {
            nodes,
            node_indices,
            links,
            incoming,
            outgoing,
            active_nodes,
            cycle_nodes,
            hashes,
            diagnostics,
        }
    }
}
