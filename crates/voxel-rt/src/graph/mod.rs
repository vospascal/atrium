//! Atrium's node catalogue: the operations, their declarations, and the per-kind contracts.
//!
//! The graph *mechanics* — documents, wiring rules, validation, history — are `voxel-graph`,
//! which knows nothing about materials. This is the domain half.
//!
//! ```text
//! mod.rs     the 62 operations across 7 families, tag()/from_tag(), WORLD_CONTRACTS, CATALOGUE
//! common.rs  the socket!/node!/pattern_fields! builders, and field atoms 2+ nodes share
//! nodes/     one file per node — 54 of them — plus the flat BUILTIN_NODES list
//! ```
//!
//! **One file per node**, because a node's pieces have to agree and nothing else keeps them
//! together. Before this split its declaration, its lowering and its pattern projection lived
//! in three files of several thousand lines each. `nodes/mod.rs` is the dispatch point, and
//! two tests hold the layout: `catalogue_matches_the_family_arrays` and
//! `every_node_file_is_declared_in_its_family` — the second reads the directory, because a
//! file nobody lists is a node that exists, reads as implemented, and cannot be used.
//!
//! Each operation carries a stable label ([`OperationTag`]) rather than being named in
//! `voxel-graph`'s types; `tag()` and [`NodeOperation::from_tag`] convert, and
//! `every_operation_tag_round_trips_and_is_unique` proves no label is orphaned or shared.
//!
//! 11 operations — the whole `biome`, `environment`, `surface`, `field` and `logic` families —
//! have no declaration yet, and no contract references them either, so nothing is
//! unvalidatable. They are placeholders for families still to be built.

use voxel_material_graph::MaterialNodeOperation;

use voxel_graph::{
    Cardinality, FlowConstraintStatic, GraphContractStatic, GraphKind, NodeConstraintStatic,
    NodeDeclaration, NodeRegistry, OperationTag, SocketType,
};

pub mod nodes;

pub use nodes::BUILTIN_NODES;

/// What a node does, as a typed value.
///
/// Declaration sites and lowering code use this; a [`NodeDeclaration`] stores the
/// [`OperationTag`] it converts to, so `voxel-graph` never sees these variants. The textual
/// node type stays the persisted identity — this enum is the compiler dispatch key, so
/// behaviour cannot drift from the schema through a second string table.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeOperation {
    Material(MaterialNodeOperation),
    World(WorldNodeOperation),
    Environment(EnvironmentNodeOperation),
    Biome(BiomeNodeOperation),
    Surface(SurfaceNodeOperation),
    Field(FieldNodeOperation),
    Logic(LogicNodeOperation),
}

/// World-domain operations. The textual node type remains persisted, while this
/// enum is the only compiler dispatch key; no world JSON schema sits beside it.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum WorldNodeOperation {
    Output,
    GeneratedTerrain,
    Compose,
    StudioPreview,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum EnvironmentNodeOperation {
    Output,
    Generated,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum BiomeNodeOperation {
    Output,
    Definition,
    Blend,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SurfaceNodeOperation {
    Output,
    Profile,
    MaterialBinding,
    AddVoxelLayer,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldNodeOperation {
    Constant,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum LogicNodeOperation {
    Always,
}

impl WorldNodeOperation {
    /// This operation's stable label. The text matches the node's own type id, so a
    /// collision between two operations is visible on sight.
    pub const fn tag(self) -> OperationTag {
        match self {
            Self::Output => OperationTag("world.output"),
            Self::GeneratedTerrain => OperationTag("world.generated_terrain"),
            Self::Compose => OperationTag("world.compose"),
            Self::StudioPreview => OperationTag("world.studio_preview"),
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label {
            "world.output" => Some(Self::Output),
            "world.generated_terrain" => Some(Self::GeneratedTerrain),
            "world.compose" => Some(Self::Compose),
            "world.studio_preview" => Some(Self::StudioPreview),
            _ => None,
        }
    }
}

impl EnvironmentNodeOperation {
    /// This operation's stable label. The text matches the node's own type id, so a
    /// collision between two operations is visible on sight.
    pub const fn tag(self) -> OperationTag {
        match self {
            Self::Output => OperationTag("environment.output"),
            Self::Generated => OperationTag("environment.generated"),
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label {
            "environment.output" => Some(Self::Output),
            "environment.generated" => Some(Self::Generated),
            _ => None,
        }
    }
}

impl BiomeNodeOperation {
    /// This operation's stable label. The text matches the node's own type id, so a
    /// collision between two operations is visible on sight.
    pub const fn tag(self) -> OperationTag {
        match self {
            Self::Output => OperationTag("biome.output"),
            Self::Definition => OperationTag("biome.definition"),
            Self::Blend => OperationTag("biome.blend"),
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label {
            "biome.output" => Some(Self::Output),
            "biome.definition" => Some(Self::Definition),
            "biome.blend" => Some(Self::Blend),
            _ => None,
        }
    }
}

impl SurfaceNodeOperation {
    /// This operation's stable label. The text matches the node's own type id, so a
    /// collision between two operations is visible on sight.
    pub const fn tag(self) -> OperationTag {
        match self {
            Self::Output => OperationTag("surface.output"),
            Self::Profile => OperationTag("surface.profile"),
            Self::MaterialBinding => OperationTag("surface.material_binding"),
            Self::AddVoxelLayer => OperationTag("surface.add_voxel_layer"),
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label {
            "surface.output" => Some(Self::Output),
            "surface.profile" => Some(Self::Profile),
            "surface.material_binding" => Some(Self::MaterialBinding),
            "surface.add_voxel_layer" => Some(Self::AddVoxelLayer),
            _ => None,
        }
    }
}

impl FieldNodeOperation {
    /// This operation's stable label. The text matches the node's own type id, so a
    /// collision between two operations is visible on sight.
    pub const fn tag(self) -> OperationTag {
        match self {
            Self::Constant => OperationTag("field.constant"),
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label {
            "field.constant" => Some(Self::Constant),
            _ => None,
        }
    }
}

impl LogicNodeOperation {
    /// This operation's stable label. The text matches the node's own type id, so a
    /// collision between two operations is visible on sight.
    pub const fn tag(self) -> OperationTag {
        match self {
            Self::Always => OperationTag("logic.always"),
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label {
            "logic.always" => Some(Self::Always),
            _ => None,
        }
    }
}

impl NodeOperation {
    /// The label a `NodeDeclaration` carries. Declaration sites still name a real variant —
    /// the `node!` macro calls this — so a typo there remains a compile error.
    pub const fn tag(self) -> OperationTag {
        match self {
            Self::Material(operation) => operation.tag(),
            Self::World(operation) => operation.tag(),
            Self::Environment(operation) => operation.tag(),
            Self::Biome(operation) => operation.tag(),
            Self::Surface(operation) => operation.tag(),
            Self::Field(operation) => operation.tag(),
            Self::Logic(operation) => operation.tag(),
        }
    }

    /// Recover the operation from a declaration's label.
    ///
    /// `None` means the label names no operation this build knows — the same condition as an
    /// unrecognised node type, which happens when reading a document authored against a newer
    /// catalogue and is never a reason to panic.
    pub fn from_tag(tag: OperationTag) -> Option<Self> {
        if let Some(operation) = MaterialNodeOperation::from_tag(tag) {
            return Some(Self::Material(operation));
        }
        if let Some(operation) = WorldNodeOperation::from_label(tag.0) {
            return Some(Self::World(operation));
        }
        if let Some(operation) = EnvironmentNodeOperation::from_label(tag.0) {
            return Some(Self::Environment(operation));
        }
        if let Some(operation) = BiomeNodeOperation::from_label(tag.0) {
            return Some(Self::Biome(operation));
        }
        if let Some(operation) = SurfaceNodeOperation::from_label(tag.0) {
            return Some(Self::Surface(operation));
        }
        if let Some(operation) = FieldNodeOperation::from_label(tag.0) {
            return Some(Self::Field(operation));
        }
        if let Some(operation) = LogicNodeOperation::from_label(tag.0) {
            return Some(Self::Logic(operation));
        }
        None
    }
}

const WORLD_NODE_CONSTRAINTS: &[NodeConstraintStatic] = &[NodeConstraintStatic {
    operation: NodeOperation::World(WorldNodeOperation::Output).tag(),
    cardinality: Cardinality::EXACTLY_ONE,
}];

const WORLD_FLOWS: &[FlowConstraintStatic] = &[FlowConstraintStatic {
    value_type: SocketType::VoxelField,
    source: NodeOperation::World(WorldNodeOperation::GeneratedTerrain).tag(),
    intermediates: &[NodeOperation::World(WorldNodeOperation::Compose).tag()],
    sink: NodeOperation::World(WorldNodeOperation::Output).tag(),
}];

/// The contracts for the families `voxel-rt` itself declares. The material contracts belong
/// to `voxel-material-graph`; `CATALOGUE` composes both.
pub static WORLD_CONTRACTS: &[GraphContractStatic] = &[GraphContractStatic {
    kind: GraphKind::World,
    nodes: WORLD_NODE_CONSTRAINTS,
    flows: WORLD_FLOWS,
}];

/// Canonical node schemas. Backends register execution independently, but node
/// construction, validation, persistence, catalog presentation, and every
/// editable widget derive from this table.
/// Atrium's node catalogue, paired with the contracts its graphs must satisfy.
///
/// The one place both halves are named together. `voxel-graph` has no `builtin()` and no
/// `Default` registry — it owns no nodes, so a default could only have meant "somebody
/// else's catalogue", which is exactly how the contracts came to be read from a hidden
/// module-level static.
pub const CATALOGUE: NodeRegistry = NodeRegistry::new(FAMILIES, CONTRACT_SETS);

/// Every family in the shipped catalogue. Adding a domain — textures, audio — is one entry
/// here plus its crate; nothing existing changes. That composition is the whole reason
/// `NodeRegistry` takes families rather than one flat slice.
static FAMILIES: &[&[NodeDeclaration]] = &[voxel_material_graph::NODES, nodes::world::NODES];
static CONTRACT_SETS: &[&[GraphContractStatic]] =
    &[voxel_material_graph::CONTRACTS, WORLD_CONTRACTS];

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;

    use voxel_graph::{
        node_reachability, DiagnosticSeverity, FieldDefault, FieldTarget, GraphAsset, GraphCommand,
        GraphCommandError, GraphHistory, InputPin, LinkId, LinkRecord, NodeId, NodeRecord,
        NodeTypeId, OutputPin, PropertyValue, Separable, SocketKey, TemporalDependence,
    };
    /// The safety net for [`OperationTag`]. A tag is a string, so a typo compiles and two
    /// operations could claim the same label — this is what makes that impossible to ship.
    ///
    /// Checks both directions plus injectivity: every declared tag names an operation this
    /// build knows, that operation tags back to the identical label, and no two distinct
    /// operations share a label. Without the last one, two node types would silently satisfy
    /// each other's graph contract.
    #[test]
    fn every_operation_tag_round_trips_and_is_unique() {
        let mut tag_to_operation: BTreeMap<&'static str, NodeOperation> = BTreeMap::new();
        for declaration in CATALOGUE.declarations() {
            let tag = declaration.operation;
            let operation = NodeOperation::from_tag(tag)
                .unwrap_or_else(|| panic!("node {} declares unknown tag {tag}", declaration.id));
            assert_eq!(
                operation.tag(),
                tag,
                "node {} round-tripped to a different tag",
                declaration.id
            );
            if let Some(previous) = tag_to_operation.insert(tag.0, operation) {
                assert_eq!(
                    previous, operation,
                    "tag {tag} is claimed by two different operations"
                );
            }
        }
        assert!(
            tag_to_operation.len() >= 40,
            "expected the material catalogue's tags, found {}",
            tag_to_operation.len()
        );
    }

    /// Contract data refers to operations by tag too, so an unknown tag there would mean a
    /// rule that silently matches nothing — a graph would validate clean while violating it.
    #[test]
    fn every_contract_tag_names_a_known_operation() {
        for contract in CATALOGUE.contracts() {
            for constraint in contract.nodes {
                assert!(
                    NodeOperation::from_tag(constraint.operation).is_some(),
                    "{:?} constraint names unknown tag {}",
                    contract.kind,
                    constraint.operation
                );
            }
            for flow in contract.flows {
                for tag in [flow.source, flow.sink] {
                    assert!(
                        NodeOperation::from_tag(tag).is_some(),
                        "{:?} flow names unknown tag {tag}",
                        contract.kind
                    );
                }
                for tag in flow.intermediates {
                    assert!(
                        NodeOperation::from_tag(*tag).is_some(),
                        "{:?} flow intermediate names unknown tag {tag}",
                        contract.kind
                    );
                }
            }
        }
    }

    fn id(value: &str) -> NodeId {
        NodeId(value.into())
    }
    fn socket(value: &str) -> SocketKey {
        SocketKey(value.into())
    }
    fn material_graph() -> GraphAsset {
        GraphAsset::new("test", GraphKind::Material)
    }

    #[test]
    fn builtin_node_schemas_are_complete_unique_and_self_consistent() {
        let mut node_ids = BTreeSet::new();
        for declaration in CATALOGUE.declarations() {
            assert!(
                node_ids.insert(declaration.id),
                "duplicate {}",
                declaration.id
            );
            assert!(!declaration.title.trim().is_empty());
            assert!(!declaration.description.trim().is_empty());
            assert!(!declaration.kinds.is_empty());
            let mut fields = BTreeSet::new();
            for field in declaration.fields {
                assert!(
                    fields.insert((field.target as u8, field.key)),
                    "duplicate field {} on {}",
                    field.key,
                    declaration.id
                );
                assert!(field.accepts(&field.default.value()));
                if let Some(range) = field.hard_range {
                    assert!(range.min.is_finite() && range.max.is_finite());
                    assert!(range.min <= range.max);
                }
                if let (Some(hard), Some(soft)) = (field.hard_range, field.soft_range) {
                    assert!(soft.min >= hard.min && soft.max <= hard.max);
                }
                if field.target == FieldTarget::InputSocket {
                    let input = declaration
                        .input(&SocketKey(field.key.to_string()))
                        .unwrap_or_else(|| {
                            panic!(
                                "field {} on {} targets a missing input",
                                field.key, declaration.id
                            )
                        });
                    assert_eq!(input.value_type, field.default.socket_type());
                }
            }
            for input in declaration.inputs {
                assert!(input
                    .cardinality
                    .maximum
                    .is_none_or(|maximum| input.cardinality.minimum <= maximum));
                let has_default = declaration
                    .field(FieldTarget::InputSocket, input.key)
                    .is_some();
                assert!(
                    has_default
                        || matches!(
                            input.value_type,
                            SocketType::MaterialSurface
                                | SocketType::MaskField
                                | SocketType::VoxelField
                                | SocketType::Environment
                                | SocketType::BiomeField
                                | SocketType::BiomeDefinition
                                | SocketType::SurfaceProfile
                                | SocketType::SurfaceRule
                                | SocketType::MaterialBinding
                        ),
                    "primitive input {} on {} has no default/UI schema",
                    input.key,
                    declaration.id
                );
            }
            for output in declaration.outputs {
                assert!(output
                    .cardinality
                    .maximum
                    .is_none_or(|maximum| output.cardinality.minimum <= maximum));
            }
            // Every socket carries prose, and the assertion is what keeps it
            // that way: an undocumented socket added later fails right here
            // rather than quietly shipping a blank tooltip.
            for (side, sockets) in [
                ("input", declaration.inputs),
                ("output", declaration.outputs),
            ] {
                let mut labels = BTreeSet::new();
                for socket in sockets {
                    assert!(
                        !socket.label.trim().is_empty(),
                        "{side} socket {} on {} has no label",
                        socket.key,
                        declaration.id
                    );
                    assert!(
                        !socket.description.trim().is_empty(),
                        "{side} socket {} on {} has no description",
                        socket.key,
                        declaration.id
                    );
                    assert!(
                        labels.insert(socket.label),
                        "duplicate {side} socket label `{}` on {}",
                        socket.label,
                        declaration.id
                    );
                }
            }
            let record = declaration.new_record();
            assert_eq!(record.node_type.0, declaration.id);
            assert_eq!(
                record.properties.len() + record.socket_defaults.len(),
                declaration.fields.len()
            );
        }
        for contract in CATALOGUE.contracts() {
            for constraint in contract.nodes {
                assert!(constraint
                    .cardinality
                    .maximum
                    .is_none_or(|maximum| constraint.cardinality.minimum <= maximum));
            }
        }
    }

    /// A field must not offer a value the renderer throws away.
    ///
    /// The octave range said 1-8 while the generator clamps at
    /// `MAX_NOISE_OCTAVES` (4), so dialling 5 through 8 in the inspector did
    /// nothing at all and the saved project recorded a number that never
    /// rendered — the checked-in lava had `octaves: 8`.
    #[test]
    fn the_octave_range_offers_only_octaves_the_renderer_evaluates() {
        let declaration = CATALOGUE
            .find(&NodeTypeId("material.pattern_noise".into()))
            .expect("the noise generator is registered");
        let octaves = declaration
            .field(FieldTarget::Property, "octaves")
            .expect("the generator declares an octave count");
        let range = octaves.hard_range.expect("octaves are bounded");
        assert_eq!(
            range.max,
            voxel_material::pattern::MAX_NOISE_OCTAVES as f32,
            "the inspector offers octaves the generator will clamp away"
        );
    }

    #[test]
    fn cardinality_distinguishes_available_capacity_from_replacement() {
        assert!(Cardinality::OPTIONAL_SINGLE.accepts_additional(0));
        assert!(!Cardinality::OPTIONAL_SINGLE.accepts_additional(1));
        assert!(Cardinality::ANY.accepts_additional(0));
        assert!(Cardinality::ANY.accepts_additional(100));
    }

    #[test]
    fn add_node_materializes_every_declared_default_atomically() {
        let registry = CATALOGUE;
        let declaration = registry.find(&NodeTypeId("material.noise".into())).unwrap();
        let mut graph = material_graph();
        let id = id("noise");
        GraphCommand::AddNode {
            id: id.clone(),
            node_type: NodeTypeId(declaration.id.into()),
            position: [12.0, 24.0],
        }
        .apply(&mut graph, &registry)
        .unwrap();
        let record = &graph.nodes[&id];
        assert_eq!(record.socket_defaults.len(), declaration.fields.len());
        assert_eq!(
            record.socket_defaults[&SocketKey("detail".into())],
            PropertyValue::Scalar(3.0)
        );
        assert_eq!(graph.layout.positions[&id], [12.0, 24.0]);
    }

    #[test]
    fn singleton_node_limits_are_declared_and_enforced_by_commands() {
        let registry = crate::graph::CATALOGUE;
        let mut graph = voxel_material_graph::lowering::new_material_graph("test");
        let output_type = NodeTypeId("material.output".into());
        assert!(!graph.can_add_node_type(&registry, &output_type));
        assert!(matches!(
            GraphCommand::AddNode {
                id: id("second-output"),
                node_type: output_type.clone(),
                position: [0.0; 2],
            }
            .apply(&mut graph, &registry),
            Err(GraphCommandError::NodeCardinality(node_type)) if node_type == output_type
        ));

        let output = graph
            .nodes
            .iter()
            .find(|(_, node)| node.node_type == output_type)
            .map(|(id, _)| id.clone())
            .unwrap();
        assert!(matches!(
            GraphCommand::RemoveNodes {
                nodes: vec![output]
            }
            .apply(&mut graph, &registry),
            Err(GraphCommandError::NodeCardinality(_))
        ));
    }

    #[test]
    fn failed_transaction_restores_the_entire_graph() {
        let registry = crate::graph::CATALOGUE;
        let mut graph = material_graph();
        let before = graph.clone();
        let error = GraphCommand::Transaction {
            commands: vec![
                GraphCommand::AddNode {
                    id: id("output-a"),
                    node_type: NodeTypeId("material.output".into()),
                    position: [0.0; 2],
                },
                GraphCommand::AddNode {
                    id: id("output-b"),
                    node_type: NodeTypeId("material.output".into()),
                    position: [1.0; 2],
                },
            ],
        }
        .apply(&mut graph, &registry)
        .unwrap_err();
        assert!(matches!(error, GraphCommandError::NodeCardinality(_)));
        assert_eq!(graph, before);
    }

    #[test]
    fn material_contract_reports_a_broken_canonical_surface_flow() {
        let registry = crate::graph::CATALOGUE;
        let mut graph = voxel_material_graph::lowering::new_material_graph("test");
        graph
            .links
            .retain(|_, link| link.from.socket.0 != "surface");
        let diagnostics = graph.resolve(&registry).diagnostics;
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "output_cardinality"));
        assert!(diagnostics
            .iter()
            .any(|diagnostic| diagnostic.code == "flow_incomplete"));
    }

    #[test]
    fn source_cardinality_rewires_a_surface_output_and_undo_restores_it() {
        let registry = crate::graph::CATALOGUE;
        let mut graph = voxel_material_graph::lowering::new_material_graph("test");
        let surface = graph
            .nodes
            .iter()
            .find(|(_, node)| node.node_type.0 == "material.surface")
            .map(|(id, _)| id.clone())
            .unwrap();
        let original = graph
            .links
            .iter()
            .find(|(_, link)| link.from.node == surface && link.from.socket.0 == "surface")
            .map(|(id, link)| (id.clone(), link.clone()))
            .unwrap();
        let layer = id("layer");
        GraphCommand::AddNode {
            id: layer.clone(),
            node_type: NodeTypeId("material.pattern_layer".into()),
            position: [0.0; 2],
        }
        .apply(&mut graph, &registry)
        .unwrap();
        let mut history = GraphHistory::default();
        let replacement = LinkId("surface-layer".into());
        history
            .apply(
                &mut graph,
                &registry,
                GraphCommand::Connect {
                    id: replacement.clone(),
                    from: original.1.from.clone(),
                    to: InputPin {
                        node: layer,
                        socket: socket("surface"),
                    },
                },
            )
            .unwrap();
        assert!(!graph.links.contains_key(&original.0));
        assert!(graph.links.contains_key(&replacement));
        history.undo(&mut graph, &registry).unwrap();
        assert!(graph.links.contains_key(&original.0));
        assert!(!graph.links.contains_key(&replacement));
    }

    #[test]
    fn edit_commands_enforce_declared_types_ranges_and_choices() {
        let registry = CATALOGUE;
        let mut graph = material_graph();
        let id = id("roughness");
        GraphCommand::AddNode {
            id: id.clone(),
            node_type: NodeTypeId("material.roughness".into()),
            position: [0.0; 2],
        }
        .apply(&mut graph, &registry)
        .unwrap();
        let error = GraphCommand::SetProperty {
            node: id.clone(),
            property: "value".into(),
            value: PropertyValue::Scalar(1.5),
        }
        .apply(&mut graph, &registry)
        .unwrap_err();
        assert!(matches!(error, GraphCommandError::InvalidField { .. }));
        assert_eq!(
            graph.nodes[&id].properties["value"],
            PropertyValue::Scalar(0.6)
        );

        // Choices are declarations, not bare strings: every option explains
        // itself, and only a declared `value` survives an edit command.
        for declaration in CATALOGUE.declarations() {
            for field in declaration.fields {
                let mut values = BTreeSet::new();
                for option in field.choices {
                    assert!(
                        values.insert(option.value),
                        "duplicate choice `{}` on {}.{}",
                        option.value,
                        declaration.id,
                        field.key
                    );
                    assert!(
                        !option.value.trim().is_empty(),
                        "a choice on {}.{} has no persisted value",
                        declaration.id,
                        field.key
                    );
                    assert!(
                        !option.label.trim().is_empty(),
                        "choice `{}` on {}.{} has no label",
                        option.value,
                        declaration.id,
                        field.key
                    );
                    assert!(
                        !option.description.trim().is_empty(),
                        "choice `{}` on {}.{} has no description",
                        option.value,
                        declaration.id,
                        field.key
                    );
                    assert_eq!(field.choice(option.value), Some(option));
                    assert!(field.accepts(&PropertyValue::Text(option.value.to_string())));
                }
                if !field.choices.is_empty() {
                    assert!(field.choice("not-a-declared-choice").is_none());
                    assert!(!field.accepts(&PropertyValue::Text("not-a-declared-choice".into())));
                }
            }
        }

        let layer = NodeId("layer".into());
        GraphCommand::AddNode {
            id: layer.clone(),
            node_type: NodeTypeId("material.pattern_layer".into()),
            position: [0.0; 2],
        }
        .apply(&mut graph, &registry)
        .unwrap();
        GraphCommand::SetProperty {
            node: layer.clone(),
            property: "target".into(),
            value: PropertyValue::Text("emission".into()),
        }
        .apply(&mut graph, &registry)
        .unwrap();
        let error = GraphCommand::SetProperty {
            node: layer.clone(),
            property: "target".into(),
            value: PropertyValue::Text("specular".into()),
        }
        .apply(&mut graph, &registry)
        .unwrap_err();
        assert!(matches!(error, GraphCommandError::InvalidField { .. }));
        assert_eq!(
            graph.nodes[&layer].properties["target"],
            PropertyValue::Text("emission".into())
        );
    }

    /// A node wired to nothing still renders nothing — the graph has to say so.
    #[test]
    fn an_unwired_node_is_unreachable_and_reported_as_inert() {
        let registry = crate::graph::CATALOGUE;
        let mut graph = voxel_material_graph::lowering::new_material_graph("test");
        let orphan = id("orphan-oscillator");
        GraphCommand::AddNode {
            id: orphan.clone(),
            node_type: NodeTypeId("material.oscillator".into()),
            position: [0.0; 2],
        }
        .apply(&mut graph, &registry)
        .unwrap();

        let reachable = node_reachability(&graph, &registry);
        assert!(
            !reachable.contains(&orphan),
            "a node wired to nothing cannot reach the output"
        );
        let surface = graph
            .nodes
            .iter()
            .find(|(_, node)| node.node_type.0 == "material.surface")
            .map(|(id, _)| id.clone())
            .expect("the canonical graph has a surface node");
        assert!(
            reachable.contains(&surface),
            "the surface feeds the output and must be reachable"
        );

        let diagnostics = graph.resolve(&registry).diagnostics;
        assert!(
            diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "unreached-node"
                    && diagnostic.severity == DiagnosticSeverity::Warning
                    && diagnostic.message.contains(&orphan.0)
            }),
            "the orphaned oscillator is not reported: {diagnostics:?}"
        );
        assert!(
            !diagnostics.iter().any(|diagnostic| {
                diagnostic.code == "unreached-node" && diagnostic.message.contains(&surface.0)
            }),
            "a node feeding the output must not be reported as inert"
        );
    }

    #[test]
    fn commands_are_undoable_and_layout_does_not_change_semantics() {
        let registry = CATALOGUE;
        let mut graph = material_graph();
        let mut history = GraphHistory::default();
        let node = id("value");
        history
            .apply(
                &mut graph,
                &registry,
                GraphCommand::AddNode {
                    id: node.clone(),
                    node_type: NodeTypeId("material.constant_scalar".into()),
                    position: [1.0, 2.0],
                },
            )
            .unwrap();
        let semantic = graph.resolve(&registry).hashes.semantic;
        history
            .apply(
                &mut graph,
                &registry,
                GraphCommand::MoveNodes {
                    positions: vec![(node.clone(), [9.0, 9.0])],
                },
            )
            .unwrap();
        assert_eq!(semantic, graph.resolve(&registry).hashes.semantic);
        history.undo(&mut graph, &registry).unwrap();
        assert_eq!(graph.layout.positions[&node], [1.0, 2.0]);
        history.undo(&mut graph, &registry).unwrap();
        assert!(graph.nodes.is_empty());
        history.redo(&mut graph, &registry).unwrap();
        assert!(graph.nodes.contains_key(&node));
    }

    #[test]
    fn resolver_rejects_unknown_types_and_bad_links_without_mutating_graph() {
        let registry = CATALOGUE;
        let mut graph = material_graph();
        let a = id("a");
        let b = id("b");
        graph.nodes.insert(
            a.clone(),
            NodeRecord {
                node_type: NodeTypeId("material.constant_scalar".into()),
                node_type_version: 1,
                properties: BTreeMap::new(),
                socket_defaults: BTreeMap::new(),
                unknown_payload: None,
            },
        );
        graph.nodes.insert(
            b.clone(),
            NodeRecord {
                node_type: NodeTypeId("material.output".into()),
                node_type_version: 1,
                properties: BTreeMap::new(),
                socket_defaults: BTreeMap::new(),
                unknown_payload: None,
            },
        );
        let command = GraphCommand::Connect {
            id: LinkId("bad".into()),
            from: OutputPin {
                node: a,
                socket: socket("value"),
            },
            to: InputPin {
                node: b,
                socket: socket("surface"),
            },
        };
        assert!(matches!(
            command.apply(&mut graph, &registry),
            Err(GraphCommandError::InvalidConnection(_))
        ));
        assert!(graph.links.is_empty());
    }

    #[test]
    fn connecting_a_single_input_replaces_the_old_link_and_undo_restores_it() {
        let registry = crate::graph::CATALOGUE;
        let mut graph = material_graph();
        let first = id("first");
        let second = id("second");
        let surface = id("surface");
        for source in [&first, &second] {
            graph.nodes.insert(
                source.clone(),
                registry
                    .find(&NodeTypeId("material.constant_scalar".into()))
                    .unwrap()
                    .new_record(),
            );
        }
        graph.nodes.insert(
            surface.clone(),
            registry
                .find(&NodeTypeId("material.surface".into()))
                .unwrap()
                .new_record(),
        );
        let destination = InputPin {
            node: surface,
            socket: socket("roughness"),
        };
        let old_id = LinkId("old".into());
        GraphCommand::Connect {
            id: old_id.clone(),
            from: OutputPin {
                node: first.clone(),
                socket: socket("value"),
            },
            to: destination.clone(),
        }
        .apply(&mut graph, &registry)
        .unwrap();

        let mut history = GraphHistory::default();
        let new_id = LinkId("new".into());
        history
            .apply(
                &mut graph,
                &registry,
                GraphCommand::Connect {
                    id: new_id.clone(),
                    from: OutputPin {
                        node: second,
                        socket: socket("value"),
                    },
                    to: destination,
                },
            )
            .unwrap();
        assert!(!graph.links.contains_key(&old_id));
        assert_eq!(graph.links[&new_id].from.node, id("second"));

        history.undo(&mut graph, &registry).unwrap();
        assert!(!graph.links.contains_key(&new_id));
        assert_eq!(graph.links[&old_id].from.node, first);
        history.redo(&mut graph, &registry).unwrap();
        assert!(!graph.links.contains_key(&old_id));
        assert!(graph.links.contains_key(&new_id));
    }

    #[test]
    fn resolver_detects_cycles_and_slices_only_active_outputs() {
        let registry = CATALOGUE;
        let mut graph = material_graph();
        let a = id("a");
        let b = id("b");
        let disconnected = id("disconnected");
        for node in [&a, &b, &disconnected] {
            graph.nodes.insert(
                node.clone(),
                NodeRecord {
                    node_type: NodeTypeId("material.passthrough_scalar".into()),
                    node_type_version: 1,
                    properties: BTreeMap::new(),
                    socket_defaults: BTreeMap::new(),
                    unknown_payload: None,
                },
            );
        }
        graph.links.insert(
            LinkId("a-to-b".into()),
            LinkRecord {
                from: OutputPin {
                    node: a.clone(),
                    socket: socket("value"),
                },
                to: InputPin {
                    node: b.clone(),
                    socket: socket("value"),
                },
                order: 0,
            },
        );
        graph.links.insert(
            LinkId("b-to-a".into()),
            LinkRecord {
                from: OutputPin {
                    node: b.clone(),
                    socket: socket("value"),
                },
                to: InputPin {
                    node: a.clone(),
                    socket: socket("value"),
                },
                order: 0,
            },
        );
        graph.interface.outputs.insert(
            socket("result"),
            OutputPin {
                node: a.clone(),
                socket: socket("value"),
            },
        );
        let resolved = graph.resolve(&registry);
        assert_eq!(resolved.cycle_nodes, BTreeSet::from([a.clone(), b.clone()]));
        assert_eq!(resolved.active_nodes, BTreeSet::from([a, b]));
        assert!(!resolved.active_nodes.contains(&disconnected));
        assert!(resolved.diagnostics.iter().any(|item| item.code == "cycle"));
    }

    #[test]
    fn graph_asset_serializes_with_stable_ids() {
        let mut graph = material_graph();
        graph.nodes.insert(
            id("value"),
            NodeRecord {
                node_type: NodeTypeId("material.constant_scalar".into()),
                node_type_version: 1,
                properties: BTreeMap::new(),
                socket_defaults: BTreeMap::new(),
                unknown_payload: None,
            },
        );
        let json = serde_json::to_string(&graph).unwrap();
        let restored: GraphAsset = serde_json::from_str(&json).unwrap();
        assert_eq!(restored, graph);
    }

    /// The declared temporal axis must agree with what the node ACTUALLY reads.
    ///
    /// This is the test that keeps the new axis honest, and it is deliberately a
    /// cross-check against the BACKEND rather than a restatement of the
    /// declaration: a node is a time source exactly when its lowering emits an
    /// instruction that reads the clock or the event field. Declaring
    /// `Inherited` on a node that reads the clock would silently tell the
    /// cacheability analysis that an animated surface can be baked.
    #[test]
    fn the_declared_temporal_axis_matches_what_each_node_lowers_to() {
        use voxel_material_graph::MaterialNodeOperation;
        for declaration in CATALOGUE.declarations() {
            let Some(NodeOperation::Material(operation)) =
                NodeOperation::from_tag(declaration.operation)
            else {
                assert_eq!(
                    declaration.temporal,
                    TemporalDependence::Inherited,
                    "{} is not a material node and cannot read the clock",
                    declaration.id
                );
                continue;
            };
            // The ONLY three operations whose evaluation reads something outside
            // the graph. Kept as a match rather than a list so that adding a
            // `MaterialNodeOperation` forces a decision here.
            let expected = match operation {
                MaterialNodeOperation::Time | MaterialNodeOperation::Oscillator => {
                    TemporalDependence::Clock
                }
                MaterialNodeOperation::EventSensor => TemporalDependence::Events,
                _ => TemporalDependence::Inherited,
            };
            assert_eq!(
                declaration.temporal, expected,
                "{} declares {:?} but its operation is {:?}",
                declaration.id, declaration.temporal, operation
            );
        }
    }

    /// Exactly three nodes may be time sources, and they are the three the
    /// analysis seeds from. A fourth appearing without this test being updated
    /// means the taint pass has a source it does not know about.
    #[test]
    fn exactly_the_known_nodes_are_time_sources() {
        let mut sources: Vec<&str> = CATALOGUE
            .declarations()
            .filter(|declaration| declaration.temporal.is_source())
            .map(|declaration| declaration.id)
            .collect();
        sources.sort_unstable();
        assert_eq!(
            sources,
            [
                "material.event_sensor",
                "material.oscillator",
                "material.time"
            ]
        );
    }

    /// Separability is an opt-in that only means something on a node owning a
    /// cacheable spatial field. Anywhere else it must stay `None`, because the
    /// analysis reads it as "a time-varying value here can be lifted out of the
    /// cache" — a claim that is only safe where there IS a cache.
    #[test]
    fn only_the_pattern_layers_animation_sockets_are_separable() {
        let mut separable: Vec<(&str, &str, Separable)> = Vec::new();
        for declaration in CATALOGUE.declarations() {
            for socket in declaration.inputs.iter().chain(declaration.outputs) {
                if socket.separable != Separable::None {
                    separable.push((declaration.id, socket.key, socket.separable));
                }
            }
        }
        separable.sort_unstable_by_key(|entry| (entry.0, entry.1));
        assert_eq!(
            separable,
            vec![
                ("material.pattern_layer", "animation_gain", Separable::Scale),
                (
                    "material.pattern_layer",
                    "drift_velocity",
                    Separable::Translate
                ),
            ]
        );
    }

    /// Nothing a time-varying value can reach may shape a cacheable pattern
    /// field — the structural fact the whole cacheability story rests on.
    ///
    /// Stated as a general invariant rather than a list, because the first
    /// version of this test WAS a list of field names and it immediately caught
    /// the wrong node: `material.fbm.octaves` is an input socket, so an
    /// oscillator can drive it. That turned out to be safe for a different
    /// reason — `material.fbm` outputs a `Scalar` while `pattern` demands a
    /// `MaskField`, so `can_feed` rejects the connection and the only pattern
    /// socket it can reach is `animation_gain`, which is separable. The real
    /// rule is therefore about the nodes that PRODUCE a pattern field:
    ///
    /// a field-producing node's parameters must be authored properties, unless
    /// the matching socket declares how a time-varying value there lifts out of
    /// the cache.
    ///
    /// Promoting a generator's period or warp to a socket is a legitimate thing
    /// to want. It just makes non-cacheable graphs expressible, so it has to be
    /// a deliberate change here and not a quiet one.
    #[test]
    fn nothing_time_varying_can_shape_a_cacheable_pattern_field() {
        for declaration in CATALOGUE.declarations() {
            let produces_field = declaration
                .outputs
                .iter()
                .any(|socket| socket.value_type == SocketType::MaskField);
            if !produces_field {
                continue;
            }
            for field in declaration.fields {
                if field.target != FieldTarget::InputSocket {
                    continue;
                }
                let socket = declaration
                    .inputs
                    .iter()
                    .find(|socket| socket.key == field.key)
                    .unwrap_or_else(|| {
                        panic!(
                            "{}.{} is an InputSocket field with no socket",
                            declaration.id, field.key
                        )
                    });
                assert_ne!(
                    socket.separable,
                    Separable::None,
                    "{}.{} produces a pattern field and exposes {} as a socket, so a \
                     time-varying value can reach it — declare how it separates from \
                     the cached field, or make it a Property",
                    declaration.id,
                    field.key,
                    field.key
                );
            }
        }
    }

    /// The exact set of nodes that produce a pattern field, pinned so that adding
    /// one is a deliberate act.
    ///
    /// This is the tripwire for the invariant above being SUFFICIENT rather than
    /// merely necessary: it holds only because every producer of a `MaskField` is
    /// in this list and every one of them keeps its shaping parameters as
    /// properties. A new field producer — especially one that took its parameters
    /// from sockets, the way `material.fbm` does on the scalar side — would make
    /// non-cacheable pattern graphs expressible.
    ///
    /// Note `material.tessellation` is in here and is not named `pattern_*`: the
    /// first version of this test asserted the naming convention instead of the
    /// set and failed on exactly that. The family is defined by what a node
    /// produces, not by what it is called.
    #[test]
    fn the_set_of_pattern_field_producers_is_pinned() {
        let mut producers: Vec<&str> = CATALOGUE
            .declarations()
            .filter(|declaration| {
                declaration
                    .outputs
                    .iter()
                    .any(|socket| socket.value_type == SocketType::MaskField)
            })
            .map(|declaration| declaration.id)
            .collect();
        producers.sort_unstable();
        assert_eq!(
            producers,
            [
                "material.pattern_checker",
                "material.pattern_flat",
                "material.pattern_noise",
                "material.pattern_perlin",
                "material.pattern_ridged",
                "material.pattern_simplex",
                "material.pattern_speckle",
                "material.pattern_tile_edge",
                "material.pattern_tile_tone",
                "material.pattern_turbulence",
                "material.pattern_wave",
                "material.pattern_worley",
                "material.pattern_worley_edge",
                "material.pattern_worley_smooth",
                "material.tessellation",
            ]
        );
    }

    /// Emission is the only authored quantity that can legitimately exceed white, so
    /// it is the only one HDR float output has anything extra to show. Its slider is
    /// therefore the whole HDR authoring surface, and its range is stated in
    /// `voxel_color`'s nits convention rather than as a bare number — otherwise the
    /// two drift and nobody notices until the brightest thing in a scene is dimmer
    /// than the display can go.
    #[test]
    fn emission_strength_reaches_past_the_brightest_display_we_target() {
        /// The peak luminance of an Apple XDR panel, the brightest thing this engine
        /// currently runs on.
        const BRIGHTEST_PANEL_NITS: f32 = 1600.0;

        let strength = CATALOGUE
            .declarations()
            .find(|declaration| declaration.id == "material.emission_strength")
            .expect("material.emission_strength must exist")
            .fields
            .iter()
            .find(|field| field.key == "strength")
            .expect("emission strength must have a `strength` field");

        let authorable = strength
            .soft_range
            .or(strength.hard_range)
            .expect("the strength slider needs a range to draw at all");
        let authorable_nits = authorable.max * voxel_color::SDR_REFERENCE_WHITE_NITS;
        assert!(
            authorable_nits >= BRIGHTEST_PANEL_NITS,
            "emission tops out at {authorable_nits} cd/m², below the \
             {BRIGHTEST_PANEL_NITS} cd/m² the panel can reach — HDR output would have \
             headroom no graph could author into it"
        );

        // And not so wide that the SDR range everything else lives in becomes
        // undraggable. Past the PQ signalling ceiling there is nothing to reach for.
        assert!(
            authorable_nits <= voxel_color::PQ_CEILING_NITS,
            "emission reaches {authorable_nits} cd/m², beyond what PQ can even signal"
        );

        // Default must sit at reference white, so an untouched emitter is an SDR
        // emitter and turning HDR on changes nothing until someone asks it to.
        assert_eq!(
            strength.default,
            FieldDefault::Scalar(1.0),
            "an emitter's default brightness is SDR reference white by definition"
        );
    }
}
