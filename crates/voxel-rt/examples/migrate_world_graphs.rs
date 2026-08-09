//! Rebuild the checked-in world and biome graphs for the decomposed node set.
//!
//! Written as an example rather than done by hand in the JSON because a node record
//! carries more than its properties — a type version, socket defaults — so a
//! hand-authored node produces a file that will not deserialize. Building each one
//! from its declaration is the only way to get a graph that matches the catalogue
//! that will read it.
//!
//! Run once after the node split:
//! `cargo run -p voxel-rt --example migrate_world_graphs`

use std::path::PathBuf;

use voxel_graph::{
    GraphAsset, GraphKind, InputPin, LinkId, LinkRecord, NodeId, NodeTypeId, OutputPin,
    PropertyValue, SocketKey,
};

/// `(node id, node type, layout position)`.
type NodeSpec = (&'static str, &'static str, [f32; 2]);
/// `(link id, from node, from socket, to node, to socket)`.
type LinkSpec = (
    &'static str,
    &'static str,
    &'static str,
    &'static str,
    &'static str,
);

const WORLD_NODES: &[NodeSpec] = &[
    ("terrain", "world.generated_terrain", [0.0, 0.0]),
    ("caves", "world.cave_carver", [260.0, 0.0]),
    ("chambers", "world.cave_chambers", [520.0, 0.0]),
    ("detail", "world.cave_detail", [780.0, 0.0]),
    ("fluids", "world.cave_fluids", [1040.0, 0.0]),
    ("scatter", "world.scatter", [1300.0, 0.0]),
    ("compose", "world.compose", [1560.0, 0.0]),
    ("output", "world.output", [1820.0, 0.0]),
    // The decoration rules, one node per class, feeding the scatter pass.
    ("grass", "world.scatter_rule", [1300.0, 200.0]),
    ("flowers", "world.scatter_rule", [1300.0, 300.0]),
    ("moss", "world.scatter_rule", [1300.0, 400.0]),
    ("mushrooms", "world.scatter_rule", [1300.0, 500.0]),
    ("vines", "world.scatter_rule", [1300.0, 600.0]),
    ("crystals", "world.scatter_rule", [1300.0, 700.0]),
];

const WORLD_LINKS: &[LinkSpec] = &[
    ("terrain-caves", "terrain", "world", "caves", "world"),
    ("caves-chambers", "caves", "world", "chambers", "world"),
    ("chambers-detail", "chambers", "world", "detail", "world"),
    ("detail-fluids", "detail", "world", "fluids", "world"),
    ("fluids-scatter", "fluids", "world", "scatter", "world"),
    ("scatter-compose", "scatter", "world", "compose", "world"),
    ("compose-output", "compose", "world", "output", "world"),
    ("grass-scatter", "grass", "rules", "scatter", "rules"),
    ("flowers-scatter", "flowers", "rules", "scatter", "rules"),
    ("moss-scatter", "moss", "rules", "scatter", "rules"),
    (
        "mushrooms-scatter",
        "mushrooms",
        "rules",
        "scatter",
        "rules",
    ),
    ("vines-scatter", "vines", "rules", "scatter", "rules"),
    ("crystals-scatter", "crystals", "rules", "scatter", "rules"),
];

const BIOME_NODES: &[NodeSpec] = &[
    ("definition", "biome.definition", [0.0, 0.0]),
    ("noise", "biome.noise", [0.0, 200.0]),
    ("sample", "biome.sample", [260.0, 200.0]),
    ("blend", "biome.blend", [520.0, 200.0]),
    ("seed", "biome.cellular_seed", [780.0, 200.0]),
    ("smooth", "biome.cellular_smooth", [1040.0, 200.0]),
    ("cleanup", "biome.cellular_cleanup", [1300.0, 200.0]),
    ("connect", "biome.cellular_connect", [1560.0, 200.0]),
    ("output", "biome.output", [1820.0, 80.0]),
];

const BIOME_LINKS: &[LinkSpec] = &[
    (
        "definition-output",
        "definition",
        "biome",
        "output",
        "biome",
    ),
    ("noise-sample", "noise", "factor", "sample", "noise"),
    ("sample-blend", "sample", "biomes", "blend", "biomes"),
    ("blend-seed", "blend", "biomes", "seed", "biomes"),
    ("seed-smooth", "seed", "biomes", "smooth", "biomes"),
    ("smooth-cleanup", "smooth", "biomes", "cleanup", "biomes"),
    ("cleanup-connect", "cleanup", "biomes", "connect", "biomes"),
    ("connect-output", "connect", "biomes", "output", "biomes"),
];

/// The authored values, so the migrated graphs keep the world that was tuned in-app
/// rather than silently resetting to declaration defaults.
const WORLD_PROPERTIES: &[(&str, &str, PropertyValueSpec)] = &[
    (
        "terrain",
        "surface_headroom",
        PropertyValueSpec::Integer(10),
    ),
    ("terrain", "sea_level", PropertyValueSpec::Integer(32)),
    ("terrain", "base_height", PropertyValueSpec::Integer(34)),
    ("terrain", "relief", PropertyValueSpec::Integer(20)),
    (
        "caves",
        "shape",
        PropertyValueSpec::Text("cellular_automata"),
    ),
    ("caves", "min_y", PropertyValueSpec::Integer(4)),
    ("caves", "max_y", PropertyValueSpec::Integer(44)),
    (
        "chambers",
        "cellular_worley_scale",
        PropertyValueSpec::Scalar(0.05),
    ),
    (
        "chambers",
        "cellular_openness",
        PropertyValueSpec::Scalar(0.72),
    ),
    (
        "detail",
        "cellular_billow_scale",
        PropertyValueSpec::Scalar(0.08),
    ),
    (
        "detail",
        "cellular_billow_strength",
        PropertyValueSpec::Scalar(0.40),
    ),
    (
        "detail",
        "cellular_billow_octaves",
        PropertyValueSpec::Integer(3),
    ),
    ("fluids", "fluids_enabled", PropertyValueSpec::Boolean(true)),
    ("fluids", "water_level", PropertyValueSpec::Integer(24)),
    ("fluids", "lava_level", PropertyValueSpec::Integer(14)),
    ("fluids", "fluid_min_y", PropertyValueSpec::Integer(2)),
    ("grass", "class", PropertyValueSpec::Text("tall_grass")),
    ("grass", "rate", PropertyValueSpec::Scalar(0.28)),
    ("flowers", "class", PropertyValueSpec::Text("flowers")),
    ("flowers", "rate", PropertyValueSpec::Scalar(0.05)),
    ("moss", "class", PropertyValueSpec::Text("cave_moss")),
    ("moss", "rate", PropertyValueSpec::Scalar(0.22)),
    (
        "mushrooms",
        "class",
        PropertyValueSpec::Text("cave_mushroom"),
    ),
    ("mushrooms", "rate", PropertyValueSpec::Scalar(0.08)),
    ("vines", "class", PropertyValueSpec::Text("cave_vine")),
    ("vines", "rate", PropertyValueSpec::Scalar(0.14)),
    ("crystals", "class", PropertyValueSpec::Text("cave_crystal")),
    ("crystals", "rate", PropertyValueSpec::Scalar(0.035)),
];

const BIOME_PROPERTIES: &[(&str, &str, PropertyValueSpec)] = &[
    ("seed", "cellular_fill", PropertyValueSpec::Scalar(0.40)),
    ("seed", "cellular_cell_size", PropertyValueSpec::Integer(2)),
    ("smooth", "iterations", PropertyValueSpec::Integer(8)),
    ("smooth", "r1_cutoff", PropertyValueSpec::Integer(5)),
    ("smooth", "r2_cutoff", PropertyValueSpec::Integer(2)),
    (
        "cleanup",
        "cleanup_iterations",
        PropertyValueSpec::Integer(2),
    ),
    ("connect", "min_room_cells", PropertyValueSpec::Integer(12)),
    ("connect", "corridor_radius", PropertyValueSpec::Integer(1)),
];

enum PropertyValueSpec {
    Integer(i64),
    Scalar(f32),
    Boolean(bool),
    Text(&'static str),
}

impl PropertyValueSpec {
    fn value(&self) -> PropertyValue {
        match self {
            Self::Integer(value) => PropertyValue::Integer(*value),
            Self::Scalar(value) => PropertyValue::Scalar(*value),
            Self::Boolean(value) => PropertyValue::Boolean(*value),
            Self::Text(value) => PropertyValue::Text((*value).to_string()),
        }
    }
}

fn build(
    name: &str,
    id: &str,
    kind: GraphKind,
    nodes: &[NodeSpec],
    links: &[LinkSpec],
    properties: &[(&str, &str, PropertyValueSpec)],
) -> GraphAsset {
    let mut graph = GraphAsset::new(name, kind);
    graph.id = voxel_graph::AssetId(id.to_string());
    for (node_id, node_type, position) in nodes {
        let declaration = voxel_rt::graph::CATALOGUE
            .find(&NodeTypeId((*node_type).to_string()))
            .unwrap_or_else(|| panic!("{node_type} is declared"));
        let node = NodeId((*node_id).to_string());
        graph.nodes.insert(node.clone(), declaration.new_record());
        graph.layout.positions.insert(node, *position);
    }
    for (node_id, key, value) in properties {
        graph
            .nodes
            .get_mut(&NodeId((*node_id).to_string()))
            .unwrap_or_else(|| panic!("{node_id} is in the node list"))
            .properties
            .insert((*key).to_string(), value.value());
    }
    for (link_id, from_node, from_socket, to_node, to_socket) in links {
        graph.links.insert(
            LinkId((*link_id).to_string()),
            LinkRecord {
                from: OutputPin {
                    node: NodeId((*from_node).to_string()),
                    socket: SocketKey((*from_socket).to_string()),
                },
                to: InputPin {
                    node: NodeId((*to_node).to_string()),
                    socket: SocketKey((*to_socket).to_string()),
                },
                order: 0,
            },
        );
    }
    graph
}

fn write(path: &PathBuf, graph: &GraphAsset) {
    let resolved = graph.resolve(&voxel_rt::graph::CATALOGUE);
    for diagnostic in &resolved.diagnostics {
        println!("  {:?}: {}", diagnostic.severity, diagnostic.message);
    }
    let serialized = serde_json::to_string_pretty(graph).expect("serializable graph");
    std::fs::write(path, format!("{serialized}\n")).expect("writing the graph");
    println!("  wrote {}", path.display());
}

fn main() {
    let root = PathBuf::from("studio-project/graphs");
    println!("world graph:");
    let world = build(
        "World",
        "world-main-v3",
        GraphKind::World,
        WORLD_NODES,
        WORLD_LINKS,
        WORLD_PROPERTIES,
    );
    write(&root.join("world.vgraph.json"), &world);

    println!("biome graph:");
    let biome = build(
        "Biome Graph",
        "biome-main-v1",
        GraphKind::Biome,
        BIOME_NODES,
        BIOME_LINKS,
        BIOME_PROPERTIES,
    );
    write(&root.join("biomes.vgraph.json"), &biome);

    let mut program = voxel_rt::engine_runtime::CompiledWorldProgram::compile(&world)
        .expect("the migrated world graph compiles");
    program
        .apply_biome_graph(&biome)
        .expect("the migrated biome graph lowers");
    let lowered = program.generation_program().expect("a generated world");
    println!("lowered: {:#?}", lowered.terrain);
    println!(
        "caves {} band {}..{} openness {:.2} fill {:.2} iterations {}",
        lowered.caves_enabled,
        lowered.cave_program.min_y,
        lowered.cave_program.max_y,
        lowered.cave_program.cellular_openness,
        lowered.cave_program.cellular_fill,
        lowered.cave_program.cellular_iterations
    );
    println!("fluids {:?}", lowered.cave_fluids);
    println!("scatter rules {}", lowered.scatter.rules.len());
}
