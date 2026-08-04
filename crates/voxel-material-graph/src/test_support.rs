//! Graph builders shared by this crate's tests and `voxel-rt`'s.
//!
//! Not `#[cfg(test)]`, because `voxel-rt`'s shader-assembly tests need them and a `cfg(test)`
//! module is invisible across a crate boundary. The alternative was a second copy of these
//! builders in the renderer, which is the drift this whole arc is about.

use std::collections::BTreeMap;

use voxel_graph::{
    GraphAsset, GraphCommand, GraphHistory, GraphKind, InputPin, LinkId, LinkRecord, NodeId,
    NodeTypeId, OutputPin, PropertyValue, SocketKey,
};

pub fn node(value: &str) -> NodeId {
    NodeId(value.into())
}

pub fn graph_with_output() -> (GraphAsset, NodeId) {
    let mut graph = GraphAsset::new("test", GraphKind::Material);
    let output = node("output");
    let surface = node("surface");
    graph.nodes.insert(
        output.clone(),
        voxel_graph::NodeRecord {
            node_type: NodeTypeId("material.output".into()),
            node_type_version: 1,
            properties: BTreeMap::new(),
            socket_defaults: BTreeMap::new(),
            unknown_payload: None,
        },
    );
    graph.nodes.insert(
        surface.clone(),
        voxel_graph::NodeRecord {
            node_type: NodeTypeId("material.surface".into()),
            node_type_version: 1,
            properties: BTreeMap::new(),
            socket_defaults: BTreeMap::new(),
            unknown_payload: None,
        },
    );
    graph.links.insert(
        LinkId("surface-output".into()),
        LinkRecord {
            from: OutputPin {
                node: surface.clone(),
                socket: SocketKey("surface".into()),
            },
            to: InputPin {
                node: output,
                socket: SocketKey("surface".into()),
            },
            order: 0,
        },
    );
    (graph, surface)
}

/// Build a graph whose roughness is driven by `node_type`, so a single helper covers every
/// animation node's lowering path.
pub fn graph_driving_roughness(
    node_type: &str,
    socket: &str,
    properties: &[(&str, PropertyValue)],
) -> (GraphAsset, NodeId) {
    let registry = crate::CATALOGUE;
    let (mut graph, output) = graph_with_output();
    let mut history = GraphHistory::default();
    let driver = node("driver");
    history
        .apply(
            &mut graph,
            &registry,
            GraphCommand::AddNode {
                id: driver.clone(),
                node_type: NodeTypeId(node_type.into()),
                position: [0.0, 0.0],
            },
        )
        .unwrap();
    for (key, value) in properties {
        graph
            .nodes
            .get_mut(&driver)
            .unwrap()
            .properties
            .insert((*key).to_string(), value.clone());
    }
    history
        .apply(
            &mut graph,
            &registry,
            GraphCommand::Connect {
                id: voxel_graph::LinkId("drive".into()),
                from: OutputPin {
                    node: driver.clone(),
                    socket: SocketKey(socket.into()),
                },
                to: voxel_graph::InputPin {
                    node: output,
                    socket: SocketKey("roughness".into()),
                },
            },
        )
        .unwrap();
    (graph, driver)
}
