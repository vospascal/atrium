//! Edits as values, so undo is a stack rather than a special case.
//!
//! Every mutation the editor can perform is a [`GraphCommand`]. Applying one returns an
//! [`EditImpact`] saying how far the consequences reach, which is what lets a consumer
//! recompile only what actually changed instead of everything after every keystroke.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::declaration::GraphKind;
use crate::document::{ConnectionError, GraphAsset, InputPin, LinkRecord, NodeRecord, OutputPin};
use crate::field::{FieldTarget, PropertyValue};
use crate::id::{LinkId, NodeId, NodeTypeId, SocketKey};
use crate::registry::NodeRegistry;

#[derive(Clone, Debug, PartialEq)]
pub enum GraphCommand {
    Transaction {
        commands: Vec<GraphCommand>,
    },
    AddNode {
        id: NodeId,
        node_type: NodeTypeId,
        position: [f32; 2],
    },
    RemoveNodes {
        nodes: Vec<NodeId>,
    },
    Connect {
        id: LinkId,
        from: OutputPin,
        to: InputPin,
    },
    Disconnect {
        id: LinkId,
    },
    SetProperty {
        node: NodeId,
        property: String,
        value: PropertyValue,
    },
    SetSocketDefault {
        node: NodeId,
        socket: SocketKey,
        value: PropertyValue,
    },
    MoveNodes {
        positions: Vec<(NodeId, [f32; 2])>,
    },
    RestoreFragment {
        nodes: BTreeMap<NodeId, NodeRecord>,
        links: BTreeMap<LinkId, LinkRecord>,
        positions: BTreeMap<NodeId, [f32; 2]>,
    },
    // Internal inverses keep public editing commands compact while making every
    // operation exactly undoable without widget-owned state.
    RemoveProperty {
        node: NodeId,
        property: String,
    },
    RemoveSocketDefault {
        node: NodeId,
        socket: SocketKey,
    },
    RestoreConnection {
        added_id: LinkId,
        replaced: Vec<(LinkId, LinkRecord)>,
    },
    RestoreGraph {
        graph: Box<GraphAsset>,
    },
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EditImpact {
    Layout,
    Parameter,
    Topology,
}

#[derive(Clone, Debug)]
pub struct AppliedCommand {
    pub inverse: GraphCommand,
    pub impact: EditImpact,
}

impl GraphCommand {
    pub fn apply(
        self,
        graph: &mut GraphAsset,
        registry: &NodeRegistry,
    ) -> Result<AppliedCommand, GraphCommandError> {
        match self {
            Self::Transaction { commands } => {
                let previous = graph.clone();
                let mut impact = EditImpact::Layout;
                for command in commands {
                    match command.apply(graph, registry) {
                        Ok(applied) => impact = impact.combine(applied.impact),
                        Err(error) => {
                            *graph = previous;
                            return Err(error);
                        }
                    }
                }
                Ok(AppliedCommand {
                    inverse: Self::RestoreGraph {
                        graph: Box::new(previous),
                    },
                    impact,
                })
            }
            Self::AddNode {
                id,
                node_type,
                position,
            } => {
                let declaration = registry
                    .find(&node_type)
                    .ok_or_else(|| GraphCommandError::UnknownNodeType(node_type.clone()))?;
                if !declaration.kinds.contains(&graph.kind) {
                    return Err(GraphCommandError::WrongGraphKind {
                        node_type,
                        graph_kind: graph.kind,
                    });
                }
                if graph.nodes.contains_key(&id) {
                    return Err(GraphCommandError::DuplicateNode(id));
                }
                if !graph.can_add_node_type(registry, &node_type) {
                    return Err(GraphCommandError::NodeCardinality(node_type));
                }
                let mut record = declaration.new_record();
                record.node_type = node_type;
                graph.nodes.insert(id.clone(), record);
                graph.layout.positions.insert(id.clone(), position);
                Ok(AppliedCommand {
                    inverse: Self::RemoveNodes { nodes: vec![id] },
                    impact: EditImpact::Topology,
                })
            }
            Self::RemoveNodes { nodes } => {
                let set: BTreeSet<_> = nodes.iter().cloned().collect();
                if set.len() != nodes.len() || set.iter().any(|id| !graph.nodes.contains_key(id)) {
                    return Err(GraphCommandError::MissingNode);
                }
                if let Some(contract) = registry.contract(graph.kind) {
                    for constraint in contract.nodes.iter().filter(|constraint| {
                        set.iter().any(|id| {
                            registry
                                .find(&graph.nodes[id].node_type)
                                .is_some_and(|node| node.operation == constraint.operation)
                        })
                    }) {
                        let remaining = graph
                            .nodes
                            .iter()
                            .filter(|(id, node)| {
                                !set.contains(*id)
                                    && registry
                                        .find(&node.node_type)
                                        .is_some_and(|node| node.operation == constraint.operation)
                            })
                            .count();
                        if remaining < constraint.cardinality.minimum {
                            let node_type = set
                                .iter()
                                .find_map(|id| {
                                    let node = &graph.nodes[id];
                                    registry.find(&node.node_type).and_then(|declaration| {
                                        (declaration.operation == constraint.operation)
                                            .then_some(node.node_type.clone())
                                    })
                                })
                                .expect("an affected constraint has a removed node type");
                            return Err(GraphCommandError::NodeCardinality(node_type));
                        }
                    }
                }
                let removed: BTreeMap<_, _> = set
                    .iter()
                    .filter_map(|id| graph.nodes.remove_entry(id))
                    .collect();
                let positions: BTreeMap<_, _> = set
                    .iter()
                    .filter_map(|id| graph.layout.positions.remove_entry(id))
                    .collect();
                let link_ids: Vec<_> = graph
                    .links
                    .iter()
                    .filter_map(|(id, link)| {
                        (set.contains(&link.from.node) || set.contains(&link.to.node))
                            .then_some(id.clone())
                    })
                    .collect();
                let links = link_ids
                    .into_iter()
                    .filter_map(|id| graph.links.remove_entry(&id))
                    .collect();
                Ok(AppliedCommand {
                    inverse: Self::RestoreFragment {
                        nodes: removed,
                        links,
                        positions,
                    },
                    impact: EditImpact::Topology,
                })
            }
            Self::Connect { id, from, to } => {
                if graph.links.contains_key(&id) {
                    return Err(GraphCommandError::DuplicateLink(id));
                }
                let plan = graph
                    .connection_plan(registry, &from, &to)
                    .map_err(GraphCommandError::InvalidConnection)?;
                let replaced = plan
                    .replaced
                    .into_iter()
                    .filter_map(|(id, _)| graph.links.remove_entry(&id))
                    .collect::<Vec<_>>();
                let link = LinkRecord { from, to, order: 0 };
                graph.links.insert(id.clone(), link);
                Ok(AppliedCommand {
                    inverse: Self::RestoreConnection {
                        added_id: id,
                        replaced,
                    },
                    impact: EditImpact::Topology,
                })
            }
            Self::Disconnect { id } => {
                let link = graph
                    .links
                    .remove(&id)
                    .ok_or_else(|| GraphCommandError::MissingLink(id.clone()))?;
                Ok(AppliedCommand {
                    inverse: Self::Connect {
                        id,
                        from: link.from,
                        to: link.to,
                    },
                    impact: EditImpact::Topology,
                })
            }
            Self::SetProperty {
                node,
                property,
                value,
            } => {
                let node_type = graph
                    .nodes
                    .get(&node)
                    .ok_or(GraphCommandError::MissingNode)?
                    .node_type
                    .clone();
                let field = registry
                    .find(&node_type)
                    .and_then(|declaration| declaration.field(FieldTarget::Property, &property))
                    .ok_or_else(|| GraphCommandError::InvalidField {
                        node_type: node_type.clone(),
                        field: property.clone(),
                    })?;
                if field.read_only || !field.accepts(&value) {
                    return Err(GraphCommandError::InvalidField {
                        node_type,
                        field: property,
                    });
                }
                let record = graph
                    .nodes
                    .get_mut(&node)
                    .ok_or(GraphCommandError::MissingNode)?;
                let previous = record.properties.insert(property.clone(), value);
                let inverse = match previous {
                    Some(value) => Self::SetProperty {
                        node,
                        property,
                        value,
                    },
                    None => Self::RemoveProperty { node, property },
                };
                Ok(AppliedCommand {
                    inverse,
                    impact: EditImpact::Parameter,
                })
            }
            Self::SetSocketDefault {
                node,
                socket,
                value,
            } => {
                let node_type = graph
                    .nodes
                    .get(&node)
                    .ok_or(GraphCommandError::MissingNode)?
                    .node_type
                    .clone();
                let field = registry
                    .find(&node_type)
                    .and_then(|declaration| declaration.field(FieldTarget::InputSocket, &socket.0))
                    .ok_or_else(|| GraphCommandError::InvalidField {
                        node_type: node_type.clone(),
                        field: socket.0.clone(),
                    })?;
                if field.read_only || !field.accepts(&value) {
                    return Err(GraphCommandError::InvalidField {
                        node_type,
                        field: socket.0,
                    });
                }
                let record = graph
                    .nodes
                    .get_mut(&node)
                    .ok_or(GraphCommandError::MissingNode)?;
                let previous = record.socket_defaults.insert(socket.clone(), value);
                let inverse = match previous {
                    Some(value) => Self::SetSocketDefault {
                        node,
                        socket,
                        value,
                    },
                    None => Self::RemoveSocketDefault { node, socket },
                };
                Ok(AppliedCommand {
                    inverse,
                    impact: EditImpact::Parameter,
                })
            }
            Self::MoveNodes { positions } => {
                if positions
                    .iter()
                    .any(|(id, _)| !graph.nodes.contains_key(id))
                {
                    return Err(GraphCommandError::MissingNode);
                }
                let mut previous = Vec::new();
                for (id, position) in positions {
                    previous.push((
                        id.clone(),
                        graph
                            .layout
                            .positions
                            .insert(id, position)
                            .unwrap_or([0.0, 0.0]),
                    ));
                }
                Ok(AppliedCommand {
                    inverse: Self::MoveNodes {
                        positions: previous,
                    },
                    impact: EditImpact::Layout,
                })
            }
            Self::RestoreFragment {
                nodes,
                links,
                positions,
            } => {
                let ids: Vec<_> = nodes.keys().cloned().collect();
                graph.nodes.extend(nodes);
                graph.links.extend(links);
                graph.layout.positions.extend(positions);
                Ok(AppliedCommand {
                    inverse: Self::RemoveNodes { nodes: ids },
                    impact: EditImpact::Topology,
                })
            }
            Self::RemoveProperty { node, property } => {
                let record = graph
                    .nodes
                    .get_mut(&node)
                    .ok_or(GraphCommandError::MissingNode)?;
                let value = record
                    .properties
                    .remove(&property)
                    .ok_or(GraphCommandError::MissingProperty)?;
                Ok(AppliedCommand {
                    inverse: Self::SetProperty {
                        node,
                        property,
                        value,
                    },
                    impact: EditImpact::Parameter,
                })
            }
            Self::RemoveSocketDefault { node, socket } => {
                let record = graph
                    .nodes
                    .get_mut(&node)
                    .ok_or(GraphCommandError::MissingNode)?;
                let value = record
                    .socket_defaults
                    .remove(&socket)
                    .ok_or(GraphCommandError::MissingProperty)?;
                Ok(AppliedCommand {
                    inverse: Self::SetSocketDefault {
                        node,
                        socket,
                        value,
                    },
                    impact: EditImpact::Parameter,
                })
            }
            Self::RestoreConnection { added_id, replaced } => {
                let added = graph
                    .links
                    .remove(&added_id)
                    .ok_or_else(|| GraphCommandError::MissingLink(added_id.clone()))?;
                for (replaced_id, replaced_link) in replaced {
                    graph.links.insert(replaced_id, replaced_link);
                }
                Ok(AppliedCommand {
                    inverse: Self::Connect {
                        id: added_id,
                        from: added.from,
                        to: added.to,
                    },
                    impact: EditImpact::Topology,
                })
            }
            Self::RestoreGraph {
                graph: mut restored,
            } => {
                std::mem::swap(graph, &mut restored);
                Ok(AppliedCommand {
                    inverse: Self::RestoreGraph { graph: restored },
                    impact: EditImpact::Topology,
                })
            }
        }
    }
}

impl EditImpact {
    fn combine(self, other: Self) -> Self {
        match (self, other) {
            (Self::Topology, _) | (_, Self::Topology) => Self::Topology,
            (Self::Parameter, _) | (_, Self::Parameter) => Self::Parameter,
            _ => Self::Layout,
        }
    }
}

#[derive(Debug)]
pub enum GraphCommandError {
    UnknownNodeType(NodeTypeId),
    WrongGraphKind {
        node_type: NodeTypeId,
        graph_kind: GraphKind,
    },
    DuplicateNode(NodeId),
    DuplicateLink(LinkId),
    NodeCardinality(NodeTypeId),
    MissingNode,
    MissingLink(LinkId),
    MissingProperty,
    InvalidField {
        node_type: NodeTypeId,
        field: String,
    },
    InvalidConnection(ConnectionError),
}

impl fmt::Display for GraphCommandError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "graph command failed: {self:?}")
    }
}

impl std::error::Error for GraphCommandError {}

#[derive(Default)]
pub struct GraphHistory {
    undo: Vec<GraphCommand>,
    redo: Vec<GraphCommand>,
}

impl GraphHistory {
    pub fn apply(
        &mut self,
        graph: &mut GraphAsset,
        registry: &NodeRegistry,
        command: GraphCommand,
    ) -> Result<EditImpact, GraphCommandError> {
        let applied = command.apply(graph, registry)?;
        self.undo.push(applied.inverse);
        self.redo.clear();
        Ok(applied.impact)
    }
    pub fn undo(
        &mut self,
        graph: &mut GraphAsset,
        registry: &NodeRegistry,
    ) -> Result<Option<EditImpact>, GraphCommandError> {
        let Some(command) = self.undo.pop() else {
            return Ok(None);
        };
        let applied = command.apply(graph, registry)?;
        self.redo.push(applied.inverse);
        Ok(Some(applied.impact))
    }
    pub fn redo(
        &mut self,
        graph: &mut GraphAsset,
        registry: &NodeRegistry,
    ) -> Result<Option<EditImpact>, GraphCommandError> {
        let Some(command) = self.redo.pop() else {
            return Ok(None);
        };
        let applied = command.apply(graph, registry)?;
        self.undo.push(applied.inverse);
        Ok(Some(applied.impact))
    }
}
