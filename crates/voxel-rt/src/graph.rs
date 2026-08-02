//! Renderer-independent, persistent node-graph foundation for Studio.
//!
//! This module owns editable graph documents and their derived validation data;
//! it deliberately does not evaluate materials, geometry, quality, or render
//! passes. Those backends consume a successfully resolved graph later.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::sync::atomic::{AtomicU64, Ordering};

use serde::{Deserialize, Serialize};

use crate::studio_assets::{AssetId, STUDIO_ASSET_SCHEMA_VERSION};

static NEXT_GRAPH_ID: AtomicU64 = AtomicU64::new(1);

macro_rules! graph_id {
    ($name:ident) => {
        #[derive(Clone, Debug, Eq, PartialEq, Ord, PartialOrd, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
        #[allow(clippy::new_without_default)]
        impl $name {
            pub fn new() -> Self {
                let sequence = NEXT_GRAPH_ID.fetch_add(1, Ordering::Relaxed);
                Self(format!("g-{sequence:016x}"))
            }
        }
        impl fmt::Display for $name {
            fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
                self.0.fmt(formatter)
            }
        }
    };
}

graph_id!(NodeId);
graph_id!(LinkId);
graph_id!(SocketKey);
graph_id!(NodeTypeId);

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphKind {
    World,
    Material,
    MaterialFunction,
    Geometry,
    Environment,
    Biome,
    SurfaceRule,
    WorldModifier,
    Feature,
    Audio,
    Animation,
    Quality,
    RenderPipeline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocketType {
    Scalar,
    Integer,
    Vector3,
    Color,
    Boolean,
    Text,
    Asset,
    MaterialSurface,
    MaterialRole,
    ScalarField,
    MaskField,
    VoxelField,
    PointField,
    SplineField,
    BiomeField,
    BiomeDefinition,
    SurfaceProfile,
    SurfaceRule,
    MaterialBinding,
    Environment,
    FeatureSet,
    AudioSignal,
    AnimationSignal,
    QualityProfile,
    RenderTarget,
}

impl SocketType {
    /// Human name for the socket's value type. Where a type has an obvious
    /// counterpart in Blender's shader editor the wording is borrowed from it,
    /// so someone arriving from that editor reads the same words here.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Scalar => "Float",
            Self::Integer => "Int",
            Self::Vector3 => "Vector",
            Self::Color => "Float Color",
            Self::Boolean => "Boolean",
            Self::Text => "String",
            Self::Asset => "Asset",
            Self::MaterialSurface => "Material Surface",
            Self::MaterialRole => "Material Role",
            Self::ScalarField => "Scalar Field",
            Self::MaskField => "Mask Field",
            Self::VoxelField => "Voxel Field",
            Self::PointField => "Point Field",
            Self::SplineField => "Spline Field",
            Self::BiomeField => "Biome Field",
            Self::BiomeDefinition => "Biome Definition",
            Self::SurfaceProfile => "Surface Profile",
            Self::SurfaceRule => "Surface Rule",
            Self::MaterialBinding => "Material Binding",
            Self::Environment => "Environment",
            Self::FeatureSet => "Feature Set",
            Self::AudioSignal => "Audio Signal",
            Self::AnimationSignal => "Animation Signal",
            Self::QualityProfile => "Quality Profile",
            Self::RenderTarget => "Render Target",
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationRate {
    Uniform,
    PerMaterial,
    PerVoxel,
    PerSample,
}

impl EvaluationRate {
    fn can_feed(self, destination: Self) -> bool {
        self <= destination
    }

    /// Human name for how often the value is recomputed.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Uniform => "Uniform",
            Self::PerMaterial => "Per Material",
            Self::PerVoxel => "Per Voxel",
            Self::PerSample => "Per Sample",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PropertyValue {
    Scalar(f32),
    Vector3([f32; 3]),
    Color([f32; 4]),
    Boolean(bool),
    Integer(i64),
    Text(String),
    Asset(AssetId),
}

impl PropertyValue {
    pub fn socket_type(&self) -> SocketType {
        match self {
            Self::Scalar(_) => SocketType::Scalar,
            Self::Vector3(_) => SocketType::Vector3,
            Self::Color(_) => SocketType::Color,
            Self::Boolean(_) => SocketType::Boolean,
            Self::Integer(_) => SocketType::Integer,
            Self::Text(_) => SocketType::Text,
            Self::Asset(_) => SocketType::Asset,
        }
    }
}

/// Where an editable field is stored in a node record. Both properties and
/// unconnected input defaults use the same schema and therefore the same UI,
/// validation, and persistence rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldTarget {
    Property,
    InputSocket,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldDefault {
    Scalar(f32),
    Integer(i64),
    Vector3([f32; 3]),
    Color([f32; 4]),
    Boolean(bool),
    Text(&'static str),
}

impl FieldDefault {
    pub fn value(self) -> PropertyValue {
        match self {
            Self::Scalar(value) => PropertyValue::Scalar(value),
            Self::Integer(value) => PropertyValue::Integer(value),
            Self::Vector3(value) => PropertyValue::Vector3(value),
            Self::Color(value) => PropertyValue::Color(value),
            Self::Boolean(value) => PropertyValue::Boolean(value),
            Self::Text(value) => PropertyValue::Text(value.to_string()),
        }
    }

    fn socket_type(self) -> SocketType {
        self.value().socket_type()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumericRange {
    pub min: f32,
    pub max: f32,
}

impl NumericRange {
    pub const fn new(min: f32, max: f32) -> Self {
        Self { min, max }
    }

    pub fn contains(self, value: f32) -> bool {
        value.is_finite() && value >= self.min && value <= self.max
    }
}

/// One selectable option of a text-valued field. `value` is the string that is
/// persisted and that compilers dispatch on; `label` and `description` exist
/// only so the editor can explain the option instead of showing a bare id.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChoiceDeclaration {
    pub value: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

const fn choice(
    value: &'static str,
    label: &'static str,
    description: &'static str,
) -> ChoiceDeclaration {
    ChoiceDeclaration {
        value,
        label,
        description,
    }
}

/// Canonical editable-field definition. `hard_range` is enforced by graph
/// validation and compilers; `soft_range` controls the ordinary UI widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldDeclarationStatic {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub target: FieldTarget,
    pub default: FieldDefault,
    pub hard_range: Option<NumericRange>,
    pub soft_range: Option<NumericRange>,
    pub step: Option<f32>,
    pub choices: &'static [ChoiceDeclaration],
    pub read_only: bool,
}

impl FieldDeclarationStatic {
    pub fn accepts(self, value: &PropertyValue) -> bool {
        if self.default.socket_type() != value.socket_type() {
            return false;
        }
        match (self.hard_range, value) {
            (Some(range), PropertyValue::Scalar(value)) => range.contains(*value),
            (Some(range), PropertyValue::Integer(value)) => range.contains(*value as f32),
            (_, PropertyValue::Vector3(value)) => value.iter().all(|value| value.is_finite()),
            (_, PropertyValue::Color(value)) => value.iter().all(|value| value.is_finite()),
            (_, PropertyValue::Text(value)) if !self.choices.is_empty() => self
                .choices
                .iter()
                .any(|choice| choice.value == value.as_str()),
            _ => true,
        }
    }

    /// The declared option carrying this persisted value, if the field offers
    /// choices at all.
    pub fn choice(&self, value: &str) -> Option<&'static ChoiceDeclaration> {
        let choices: &'static [ChoiceDeclaration] = self.choices;
        choices.iter().find(|choice| choice.value == value)
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeCategory {
    MaterialOutput,
    Inputs,
    Layers,
    Procedural,
    Coordinates,
    Utilities,
    Conditions,
    Environment,
    Biomes,
    Surface,
    Features,
    Audio,
    Animation,
    Quality,
    Render,
}

impl NodeCategory {
    pub const ALL: &'static [Self] = &[
        Self::MaterialOutput,
        Self::Inputs,
        Self::Layers,
        Self::Procedural,
        Self::Coordinates,
        Self::Utilities,
        Self::Conditions,
        Self::Environment,
        Self::Biomes,
        Self::Surface,
        Self::Features,
        Self::Audio,
        Self::Animation,
        Self::Quality,
        Self::Render,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::MaterialOutput => "Material Output",
            Self::Inputs => "Inputs",
            Self::Layers => "Layers",
            Self::Procedural => "Procedural",
            Self::Coordinates => "Coordinates & Vectors",
            Self::Utilities => "Utilities",
            Self::Conditions => "Conditions",
            Self::Environment => "Environment",
            Self::Biomes => "Biomes",
            Self::Surface => "Surface Composition",
            Self::Features => "Features",
            Self::Audio => "Audio",
            Self::Animation => "Animation",
            Self::Quality => "Quality",
            Self::Render => "Render",
        }
    }

    pub const fn color(self) -> [u8; 3] {
        match self {
            Self::MaterialOutput => [137, 57, 68],
            Self::Inputs => [125, 61, 76],
            Self::Layers => [84, 118, 144],
            Self::Procedural => [161, 91, 47],
            Self::Coordinates => [55, 119, 149],
            Self::Utilities => [161, 127, 47],
            Self::Conditions => [126, 93, 51],
            Self::Environment => [55, 128, 121],
            Self::Biomes => [69, 132, 78],
            Self::Surface => [104, 126, 50],
            Self::Features => [109, 105, 58],
            Self::Audio => [116, 72, 139],
            Self::Animation => [139, 72, 111],
            Self::Quality => [72, 101, 157],
            Self::Render => [77, 84, 101],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodePreview {
    None,
    Value,
    ColorWheel,
    MaterialSphere,
    Noise,
    ColorRamp,
}

/// Typed backend operation associated with a node declaration. Persistence
/// keeps the stable textual node ID, while compilers dispatch through this enum
/// so behavior cannot drift from the schema through a second string table.
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FlowConstraintStatic {
    pub value_type: SocketType,
    pub source: NodeOperation,
    /// The node operations allowed to sit between source and sink. This is a
    /// canonical chain: the route is walked link by link and anything else on
    /// it is an error, so only declare a flow for a route that is genuinely
    /// prescribed. "This node must reach the output somehow" is not a flow —
    /// that is the `unreached-node` warning, which already covers every node.
    pub intermediates: &'static [NodeOperation],
    pub sink: NodeOperation,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeConstraintStatic {
    pub operation: NodeOperation,
    pub cardinality: Cardinality,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct GraphContractStatic {
    pub kind: GraphKind,
    pub nodes: &'static [NodeConstraintStatic],
    pub flows: &'static [FlowConstraintStatic],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialNodeOperation {
    Output,
    Surface,
    PatternLayer,
    PatternFlat,
    PatternNoise,
    PatternSpeckle,
    ConstantScalar,
    ConstantColor,
    AddScalar,
    MixColor,
    ClampScalar,
    Position,
    Normal,
    EmissionStrength,
    FaceColor,
    FaceRoughness,
    RemapScalar,
    Noise,
    Fbm,
    ColorRamp,
    VectorAdd,
    VectorScale,
    NormalizeVector,
    DotVector,
    PositionComponent,
    NormalComponent,
    PassthroughScalar,
    /// S3 — monotone seconds since start.
    Time,
    /// S3 — a periodic wave with an authored sync/de-sync source.
    Oscillator,
    /// S3 — "did something happen within X metres of me, and how long ago?"
    EventSensor,
    MultiplyScalar,
    /// S3 — speed + angles to a velocity vector.
    Direction,
    RerouteScalar,
    RerouteColor,
    RerouteVector,
}

const MATERIAL_SURFACE_INTERMEDIATES: &[NodeOperation] =
    &[NodeOperation::Material(MaterialNodeOperation::PatternLayer)];
const MATERIAL_NODE_CONSTRAINTS: &[NodeConstraintStatic] = &[
    NodeConstraintStatic {
        operation: NodeOperation::Material(MaterialNodeOperation::Output),
        cardinality: Cardinality::EXACTLY_ONE,
    },
    NodeConstraintStatic {
        operation: NodeOperation::Material(MaterialNodeOperation::Surface),
        cardinality: Cardinality::EXACTLY_ONE,
    },
    NodeConstraintStatic {
        operation: NodeOperation::Material(MaterialNodeOperation::PatternLayer),
        cardinality: Cardinality::up_to(crate::pattern::MAX_PATTERN_LAYERS),
    },
];
const MATERIAL_FLOWS: &[FlowConstraintStatic] = &[
    FlowConstraintStatic {
        value_type: SocketType::MaterialSurface,
        source: NodeOperation::Material(MaterialNodeOperation::Surface),
        intermediates: MATERIAL_SURFACE_INTERMEDIATES,
        sink: NodeOperation::Material(MaterialNodeOperation::Output),
    },
    // S3 animation nodes deliberately get NO flow constraint. An oscillator
    // that reaches nothing is already reported by the `unreached-node` warning
    // in `resolve`, which covers every node rather than three named ones. A
    // flow here would fire on the identical condition at Error severity, and
    // Error blocks material compilation — so an oscillator left unwired for a
    // moment mid-edit would stop the material building. A warning is the honest
    // severity for "this has no effect yet".
];
const WORLD_NODE_CONSTRAINTS: &[NodeConstraintStatic] = &[NodeConstraintStatic {
    operation: NodeOperation::World(WorldNodeOperation::Output),
    cardinality: Cardinality::EXACTLY_ONE,
}];
const WORLD_FLOWS: &[FlowConstraintStatic] = &[FlowConstraintStatic {
    value_type: SocketType::VoxelField,
    source: NodeOperation::World(WorldNodeOperation::GeneratedTerrain),
    intermediates: &[NodeOperation::World(WorldNodeOperation::Compose)],
    sink: NodeOperation::World(WorldNodeOperation::Output),
}];
pub static GRAPH_CONTRACTS: &[GraphContractStatic] = &[
    GraphContractStatic {
        kind: GraphKind::Material,
        nodes: MATERIAL_NODE_CONSTRAINTS,
        flows: MATERIAL_FLOWS,
    },
    GraphContractStatic {
        kind: GraphKind::World,
        nodes: WORLD_NODE_CONSTRAINTS,
        flows: WORLD_FLOWS,
    },
];

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

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SocketDeclaration {
    pub key: SocketKey,
    pub label: String,
    pub description: String,
    pub value_type: SocketType,
    pub rate: EvaluationRate,
    pub cardinality: Cardinality,
}

/// Inclusive connection/instance bounds used by sockets and graph contracts.
/// `maximum: None` means unbounded. Keeping the same vocabulary at both
/// levels lets validation and UI affordances derive from one model.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Cardinality {
    pub minimum: usize,
    pub maximum: Option<usize>,
}

impl Cardinality {
    pub const ANY: Self = Self::new(0, None);
    pub const OPTIONAL_SINGLE: Self = Self::new(0, Some(1));
    pub const REQUIRED_SINGLE: Self = Self::new(1, Some(1));
    pub const EXACTLY_ONE: Self = Self::REQUIRED_SINGLE;

    pub const fn new(minimum: usize, maximum: Option<usize>) -> Self {
        Self { minimum, maximum }
    }

    pub const fn up_to(maximum: usize) -> Self {
        Self::new(0, Some(maximum))
    }

    pub fn accepts(self, count: usize) -> bool {
        count >= self.minimum && self.maximum.is_none_or(|maximum| count <= maximum)
    }

    /// Whether the current occupancy leaves room for one more link/instance.
    /// A saturated single-link socket is still connectable: the connection
    /// planner replaces its existing link instead of exceeding this bound.
    pub const fn accepts_additional(self, count: usize) -> bool {
        match self.maximum {
            Some(maximum) => count < maximum,
            None => true,
        }
    }

    pub const fn allows_many(self) -> bool {
        !matches!(self.maximum, Some(0 | 1))
    }

    pub fn description(self) -> String {
        match (self.minimum, self.maximum) {
            (0, None) => "any number".to_string(),
            (minimum, None) => format!("at least {minimum}"),
            (minimum, Some(maximum)) if minimum == maximum => format!("exactly {minimum}"),
            (minimum, Some(maximum)) => format!("between {minimum} and {maximum}"),
        }
    }
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

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeDeclaration {
    pub id: &'static str,
    pub version: u32,
    pub title: &'static str,
    pub description: &'static str,
    pub category: NodeCategory,
    pub preview: NodePreview,
    pub operation: NodeOperation,
    pub kinds: &'static [GraphKind],
    pub inputs: &'static [SocketDeclarationStatic],
    pub outputs: &'static [SocketDeclarationStatic],
    pub fields: &'static [FieldDeclarationStatic],
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SocketDeclarationStatic {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub value_type: SocketType,
    pub rate: EvaluationRate,
    pub cardinality: Cardinality,
}

impl SocketDeclarationStatic {
    pub fn can_feed(self, destination: Self) -> bool {
        self.value_type == destination.value_type && self.rate.can_feed(destination.rate)
    }
}

impl NodeDeclaration {
    pub fn input(&self, key: &SocketKey) -> Option<SocketDeclarationStatic> {
        self.inputs
            .iter()
            .copied()
            .find(|socket| socket.key == key.0)
    }
    pub fn output(&self, key: &SocketKey) -> Option<SocketDeclarationStatic> {
        self.outputs
            .iter()
            .copied()
            .find(|socket| socket.key == key.0)
    }

    pub fn field(&self, target: FieldTarget, key: &str) -> Option<FieldDeclarationStatic> {
        self.fields
            .iter()
            .copied()
            .find(|field| field.target == target && field.key == key)
    }

    pub fn new_record(&self) -> NodeRecord {
        let mut properties = BTreeMap::new();
        let mut socket_defaults = BTreeMap::new();
        for field in self.fields {
            match field.target {
                FieldTarget::Property => {
                    properties.insert(field.key.to_string(), field.default.value());
                }
                FieldTarget::InputSocket => {
                    socket_defaults.insert(SocketKey(field.key.to_string()), field.default.value());
                }
            }
        }
        NodeRecord {
            node_type: NodeTypeId(self.id.to_string()),
            node_type_version: self.version,
            properties,
            socket_defaults,
            unknown_payload: None,
        }
    }
}

#[derive(Clone, Copy, Debug)]
pub struct NodeRegistry {
    declarations: &'static [NodeDeclaration],
}

impl NodeRegistry {
    pub const fn new(declarations: &'static [NodeDeclaration]) -> Self {
        Self { declarations }
    }

    pub const fn builtin() -> Self {
        Self::new(BUILTIN_NODES)
    }

    pub fn declarations(&self) -> &'static [NodeDeclaration] {
        self.declarations
    }

    pub fn find(&self, id: &NodeTypeId) -> Option<&'static NodeDeclaration> {
        self.declarations.iter().find(|node| node.id == id.0)
    }

    pub fn contract(&self, kind: GraphKind) -> Option<&'static GraphContractStatic> {
        GRAPH_CONTRACTS
            .iter()
            .find(|contract| contract.kind == kind)
    }

    pub fn node_cardinality(&self, kind: GraphKind, operation: NodeOperation) -> Cardinality {
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

impl Default for NodeRegistry {
    fn default() -> Self {
        Self::builtin()
    }
}

/// Built-in registry value retained as the ordinary application registry.
/// Tests and domain modules may construct a registry over another static set.
#[allow(non_upper_case_globals)]
pub const NodeRegistry: NodeRegistry = NodeRegistry::builtin();

const MATERIAL: &[GraphKind] = &[GraphKind::Material];
const WORLD: &[GraphKind] = &[GraphKind::World];

macro_rules! socket {
    ($key:literal, $label:literal, $description:literal, $value_type:expr, $rate:expr, $cardinality:expr) => {
        SocketDeclarationStatic {
            key: $key,
            label: $label,
            description: $description,
            value_type: $value_type,
            rate: $rate,
            cardinality: $cardinality,
        }
    };
}

macro_rules! node {
    ($id:literal, $operation:expr, $title:literal, $description:literal, $category:expr, $preview:expr,
     $kinds:expr, $inputs:expr, $outputs:expr, $fields:expr) => {
        NodeDeclaration {
            id: $id,
            version: 1,
            title: $title,
            description: $description,
            category: $category,
            preview: $preview,
            operation: $operation,
            kinds: $kinds,
            inputs: $inputs,
            outputs: $outputs,
            fields: $fields,
        }
    };
}

// Socket schemas are declared once per node rather than shared between nodes
// that merely happen to agree on type and rate: the prose is the point, and
// "the a input" helps nobody. Two nodes share a constant only when the socket
// genuinely means the same thing in both.

// --- Inputs ------------------------------------------------------------------
const CONSTANT_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The authored constant, unchanged.",
    SocketType::Scalar,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];
const CONSTANT_COLOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "The authored constant, as linear RGBA.",
    SocketType::Color,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];
const BASE_COLOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "The intrinsic base color, ready for the surface's Base Color input.",
    SocketType::Color,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];
const ROUGHNESS_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Roughness",
    "The intrinsic microsurface roughness, 0 mirror-smooth to 1 fully diffuse.",
    SocketType::Scalar,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];
const EMISSION_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "The emitted color in linear RGBA, before any strength scaling.",
    SocketType::Color,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];

// --- Material output and surface ---------------------------------------------
const MATERIAL_OUTPUT_IN: &[SocketDeclarationStatic] = &[socket!(
    "surface",
    "Surface",
    "The finished surface the renderer shades with; every material graph must \
     terminate here.",
    SocketType::MaterialSurface,
    EvaluationRate::PerMaterial,
    Cardinality::REQUIRED_SINGLE
)];
const MATERIAL_SURFACE_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "base_color",
        "Base Color",
        "Diffuse albedo of the material in linear RGBA, before pattern layers.",
        SocketType::Color,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "roughness",
        "Roughness",
        "Microsurface roughness before pattern layers, 0 mirror-smooth to 1 fully \
         diffuse.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "emission",
        "Emission",
        "Light the surface gives off, in linear RGBA already scaled by its strength.",
        SocketType::Color,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const MATERIAL_SURFACE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "surface",
    "Surface",
    "The intrinsic surface, ready for pattern layers or straight into the \
     Material Output.",
    SocketType::MaterialSurface,
    EvaluationRate::PerMaterial,
    Cardinality::REQUIRED_SINGLE
)];

// --- Pattern layers -----------------------------------------------------------
const PATTERN_LAYER_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "surface",
        "Surface",
        "The surface this layer modifies; chain layers by feeding one into the next.",
        SocketType::MaterialSurface,
        EvaluationRate::PerMaterial,
        Cardinality::REQUIRED_SINGLE
    ),
    socket!(
        "pattern",
        "Pattern",
        "The mask deciding where this layer applies, 0 untouched to 1 full effect.",
        SocketType::MaskField,
        EvaluationRate::PerSample,
        Cardinality::REQUIRED_SINGLE
    ),
    // S3 — animation. Optional, and identity when unconnected, so every graph
    // authored before S3 keeps its exact behaviour.
    socket!(
        "animation_gain",
        "Animation Gain",
        "Multiplies this layer's Amount, 0 off to 1 as authored; unconnected it is \
         the identity.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "drift_velocity",
        "Drift",
        "How fast the pattern travels through world space, in metres per second; \
         the shader applies the clock itself.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const PATTERN_LAYER_OUT: &[SocketDeclarationStatic] = &[socket!(
    "surface",
    "Surface",
    "The incoming surface with this layer applied; feed the next layer or the \
     Material Output.",
    SocketType::MaterialSurface,
    EvaluationRate::PerMaterial,
    Cardinality::REQUIRED_SINGLE
)];
const PATTERN_FLAT_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "One stable value per sampling cell, 0..1.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const PATTERN_NOISE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Fractal value noise across the sampling cells, 0..1.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const PATTERN_SPECKLE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "1 inside a speck and 0 everywhere else.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

// --- Scalar and color utilities -----------------------------------------------
const ADD_SCALAR_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "a",
        "A",
        "First term of the sum.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "b",
        "B",
        "Second term of the sum.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const ADD_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "A plus B.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const MULTIPLY_SCALAR_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "a",
        "A",
        "First factor — usually the thing being gated.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "b",
        "B",
        "Second factor — wire a sensor signal here to gate A, since 0 mutes it and \
         1 passes it through.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const MULTIPLY_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "A times B.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const SCALAR_CLAMP_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "value",
        "Value",
        "The scalar to hold inside the bounds.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "minimum",
        "Minimum",
        "Lowest value the result may take.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "maximum",
        "Maximum",
        "Highest value the result may take.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const CLAMP_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "Value pulled back inside Minimum..Maximum.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const COLOR_MIX_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "a",
        "A",
        "Color returned when Factor is 0.",
        SocketType::Color,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "b",
        "B",
        "Color returned when Factor is 1.",
        SocketType::Color,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "factor",
        "Factor",
        "Blend position between the two colors, 0..1.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const MIX_COLOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "A and B blended at Factor.",
    SocketType::Color,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];
const REMAP_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "value",
        "Value",
        "The scalar to rescale, read against the From interval.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "from_min",
        "From Min",
        "Input value that maps onto To Min.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "from_max",
        "From Max",
        "Input value that maps onto To Max.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "to_min",
        "To Min",
        "Result produced at From Min.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "to_max",
        "To Max",
        "Result produced at From Max.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const REMAP_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "Value rescaled into the To Min..To Max interval.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const PASSTHROUGH_SCALAR_IN: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The scalar to pass through untouched.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::OPTIONAL_SINGLE
)];
const PASSTHROUGH_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The incoming scalar, unchanged.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

// --- Reroutes ------------------------------------------------------------------
const REROUTE_SCALAR_IN: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The scalar whose wire is being redirected.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::OPTIONAL_SINGLE
)];
const REROUTE_SCALAR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The same scalar; only the wire's path through the editor differs.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const REROUTE_COLOR_IN: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Color",
    "The color whose wire is being redirected.",
    SocketType::Color,
    EvaluationRate::PerSample,
    Cardinality::OPTIONAL_SINGLE
)];
const REROUTE_COLOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "The same color; only the wire's path through the editor differs.",
    SocketType::Color,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const REROUTE_VECTOR_IN: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "The vector whose wire is being redirected.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::OPTIONAL_SINGLE
)];
const REROUTE_VECTOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "The same vector; only the wire's path through the editor differs.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

// --- Emission and per-face selection -------------------------------------------
const COLOR_STRENGTH_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "color",
        "Color",
        "The emitted color to scale, in linear RGBA.",
        SocketType::Color,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    // PER SAMPLE, not uniform. It was declared uniform when nothing in the
    // catalog could vary within a material — but an oscillator or an event
    // sensor is exactly a per-sample scalar, and "pulse this emitter" is the
    // first thing anyone reaches for. A rate declaration only constrains what
    // may FEED a socket, so widening it rejects nothing that used to be legal.
    socket!(
        "strength",
        "Strength",
        "Multiplier on the emitted color; 1 leaves it as authored and 0 turns the \
         emitter off.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const EMISSION_STRENGTH_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "Color times Strength, ready for a surface's Emission input.",
    SocketType::Color,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const FACE_COLOR_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "base",
        "Base",
        "Color used by any face whose own input is left at its default.",
        SocketType::Color,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "top",
        "Top",
        "Color for up-facing voxel faces — the grass cap of a turf block.",
        SocketType::Color,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "side",
        "Side",
        "Color for the four vertical voxel faces.",
        SocketType::Color,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "bottom",
        "Bottom",
        "Color for down-facing voxel faces.",
        SocketType::Color,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const FACE_COLOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "The color belonging to the face currently being shaded.",
    SocketType::Color,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const FACE_SCALAR_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "base",
        "Base",
        "Roughness used by any face whose own input is left at its default.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "top",
        "Top",
        "Roughness for up-facing voxel faces, 0..1.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "side",
        "Side",
        "Roughness for the four vertical voxel faces, 0..1.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "bottom",
        "Bottom",
        "Roughness for down-facing voxel faces, 0..1.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const FACE_ROUGHNESS_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Roughness",
    "The roughness belonging to the face currently being shaded, 0..1.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

// --- Procedural noise ----------------------------------------------------------
const PROCEDURAL_SCALAR_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "position",
        "Position",
        "World-space point to sample the noise at, in metres.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "scale",
        "Scale",
        "Noise frequency: larger values pack more features into the same metre.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "detail",
        "Detail",
        "How many fractal octaves are summed; higher adds finer grain.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "roughness",
        "Roughness",
        "How much amplitude each octave keeps, 0..1; higher makes the noise grittier.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const NOISE_OUT: &[SocketDeclarationStatic] = &[
    socket!(
        "factor",
        "Factor",
        "Noise amplitude at Position, 0..1.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::ANY
    ),
    socket!(
        "color",
        "Color",
        "Three decorrelated noise channels packed as an RGB color, each 0..1.",
        SocketType::Color,
        EvaluationRate::PerSample,
        Cardinality::ANY
    ),
];
const FBM_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "position",
        "Position",
        "World-space point to sample the noise at, in metres.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "scale",
        "Scale",
        "Frequency of the first octave: larger values pack more features into the \
         same metre.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "octaves",
        "Octaves",
        "How many doublings of frequency are summed.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "roughness",
        "Roughness",
        "How much amplitude each octave keeps, 0..1; higher makes the noise grittier.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const FBM_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The summed fractal noise at Position, 0..1.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const COLOR_RAMP_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "factor",
        "Factor",
        "Where to read the ramp; values at or below Position A give Color A and at \
         or above Position B give Color B.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "color_a",
        "Color A",
        "Color at the first stop.",
        SocketType::Color,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "color_b",
        "Color B",
        "Color at the second stop.",
        SocketType::Color,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "position_a",
        "Position A",
        "Where the first stop sits along the ramp, 0..1.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "position_b",
        "Position B",
        "Where the second stop sits along the ramp, 0..1.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const COLOR_RAMP_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "The ramp sampled at Factor.",
    SocketType::Color,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

// --- Coordinates and vectors ----------------------------------------------------
const POSITION_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "World-space position of the point being shaded, in metres.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const NORMAL_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "Unit outward normal of the voxel face being shaded, in world space.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const POSITION_COMPONENT_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The chosen axis of the world-space sample position, in metres.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const NORMAL_COMPONENT_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The chosen axis of the world-space surface normal, -1..1.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const VECTOR_ADD_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "a",
        "A",
        "First vector of the sum.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "b",
        "B",
        "Second vector of the sum, added component by component.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const VECTOR_ADD_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "A plus B, component by component.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const VECTOR_DOT_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "a",
        "A",
        "First vector of the product.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "b",
        "B",
        "Second vector of the product; wire the surface normal here to test which \
         way a face points.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const VECTOR_DOT_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The dot product; for unit-length inputs this is the cosine of the angle \
     between them, -1..1.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const VECTOR_SCALE_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "vector",
        "Vector",
        "The vector to lengthen or shorten.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "scale",
        "Scale",
        "Multiplier applied to every component; a negative value reverses the \
         direction.",
        SocketType::Scalar,
        EvaluationRate::Uniform,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const VECTOR_SCALE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "Vector times Scale.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const NORMALIZE_VECTOR_IN: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "The vector to rescale to unit length; its direction is kept.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::OPTIONAL_SINGLE
)];
const NORMALIZE_VECTOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "The input pointing the same way but exactly one unit long; a zero-length \
     input stays zero.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

// --- S3 animation ----------------------------------------------------------------
/// Speed and angles, all connectable so a flow can itself be animated.
const DIRECTION_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "speed",
        "Speed",
        "Length of the resulting vector; for a pattern drift this is metres per \
         second.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "azimuth_degrees",
        "Azimuth",
        "Heading around the vertical axis in degrees, 0 along +X and 90 along +Z; \
         it has no effect at an elevation of -90 or +90.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "elevation_degrees",
        "Elevation",
        "Angle above horizontal in degrees, -90 straight down to +90 straight up.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const DIRECTION_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "A velocity of length Speed pointing along Azimuth and Elevation, in metres \
     per second.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const TIME_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "Seconds since the session started, counting up and never stepping backwards.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
/// The oscillator's numeric controls, all connectable.
const OSCILLATOR_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "rate_hz",
        "Rate",
        "Oscillation rate in hertz — cycles per second.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "phase",
        "Phase",
        "Where in the cycle the wave starts, in turns, 0..1; added before the sync \
         offset.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "duty",
        "Duty",
        "Pulse only: the fraction of each cycle spent high, 0..1.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "low",
        "Low",
        "Value produced at the bottom of the wave.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "high",
        "High",
        "Value produced at the top of the wave.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const OSCILLATOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The wave's current value, travelling between Low and High.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
/// The event sensor's three outputs. All read from ONE winning event, so the
/// three are mutually consistent — see the lowering for why an independent
/// per-output maximum would report a combination that never existed.
const EVENT_SENSOR_OUT: &[SocketDeclarationStatic] = &[
    socket!(
        "signal",
        "Signal",
        "Falloff times envelope times the event's strength, 0..1 — the output most \
         graphs want.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::ANY
    ),
    socket!(
        "nearness",
        "Nearness",
        "How close the event is, 1 at the sample and 0 at Radius, shaped by Falloff.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::ANY
    ),
    socket!(
        "envelope",
        "Envelope",
        "The attack/hold/release curve on its own, 0..1, driven by the event's \
         timestamp rather than by distance.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::ANY
    ),
];

// --- World ------------------------------------------------------------------------
const GENERATED_TERRAIN_OUT: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The deterministic base terrain, before any environment, biome or surface \
     program runs.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];
const WORLD_COMPOSE_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "world",
        "World",
        "The base terrain the registered programs are applied to.",
        SocketType::VoxelField,
        EvaluationRate::PerVoxel,
        Cardinality::REQUIRED_SINGLE
    ),
    socket!(
        "environment",
        "Environment",
        "Climate and lighting context the surface rules read; omitted, the profile's \
         own defaults apply.",
        SocketType::Environment,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "biomes",
        "Biomes",
        "Per-sample biome weights selecting which surface profile wins where.",
        SocketType::BiomeField,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];
const WORLD_COMPOSE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The terrain with environment, biome and surface programs applied.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];
const STUDIO_PREVIEW_OUT: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The isolated preview plate and subject used to look at a single material.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::ANY
)];
const VOXEL_FIELD_IN: &[SocketDeclarationStatic] = &[socket!(
    "world",
    "World",
    "The finished voxel world the engine streams and renders; every world graph \
     must terminate here.",
    SocketType::VoxelField,
    EvaluationRate::PerVoxel,
    Cardinality::REQUIRED_SINGLE
)];
const NONE: Option<NumericRange> = None;
const UNIT: Option<NumericRange> = Some(NumericRange::new(0.0, 1.0));
const SIGNED: Option<NumericRange> = Some(NumericRange::new(-1.0, 1.0));
const WIDE: Option<NumericRange> = Some(NumericRange::new(-1_000_000.0, 1_000_000.0));
const POSITIVE: Option<NumericRange> = Some(NumericRange::new(0.0, 1_000_000.0));
const EMPTY_CHOICES: &[ChoiceDeclaration] = &[];

#[allow(clippy::too_many_arguments)]
const fn field(
    key: &'static str,
    label: &'static str,
    description: &'static str,
    target: FieldTarget,
    default: FieldDefault,
    hard_range: Option<NumericRange>,
    soft_range: Option<NumericRange>,
    step: Option<f32>,
    choices: &'static [ChoiceDeclaration],
    read_only: bool,
) -> FieldDeclarationStatic {
    FieldDeclarationStatic {
        key,
        label,
        description,
        target,
        default,
        hard_range,
        soft_range,
        step,
        choices,
        read_only,
    }
}

const CONSTANT_SCALAR_FIELDS: &[FieldDeclarationStatic] = &[field(
    "value",
    "Value",
    "Constant scalar value.",
    FieldTarget::Property,
    FieldDefault::Scalar(0.5),
    WIDE,
    SIGNED,
    Some(0.01),
    EMPTY_CHOICES,
    false,
)];
const CONSTANT_COLOR_FIELDS: &[FieldDeclarationStatic] = &[field(
    "value",
    "Color",
    "Constant linear RGBA color.",
    FieldTarget::Property,
    FieldDefault::Color([0.8, 0.8, 0.8, 1.0]),
    NONE,
    NONE,
    None,
    EMPTY_CHOICES,
    false,
)];
const BASE_COLOR_FIELDS: &[FieldDeclarationStatic] = &[field(
    "value",
    "Color",
    "Intrinsic base color of the material.",
    FieldTarget::Property,
    FieldDefault::Color([0.4, 0.7, 0.25, 1.0]),
    NONE,
    NONE,
    None,
    EMPTY_CHOICES,
    false,
)];
const ROUGHNESS_FIELDS: &[FieldDeclarationStatic] = &[field(
    "value",
    "Roughness",
    "Microsurface roughness.",
    FieldTarget::Property,
    FieldDefault::Scalar(0.6),
    UNIT,
    UNIT,
    Some(0.01),
    EMPTY_CHOICES,
    false,
)];
const EMISSION_FIELDS: &[FieldDeclarationStatic] = &[field(
    "value",
    "Emission",
    "Linear emitted color before intensity scaling.",
    FieldTarget::Property,
    FieldDefault::Color([0.0, 0.0, 0.0, 1.0]),
    NONE,
    NONE,
    None,
    EMPTY_CHOICES,
    false,
)];
const MATERIAL_OUTPUT_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "base_color",
        "Base Color",
        "Surface base color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.8, 0.8, 0.8, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "roughness",
        "Roughness",
        "Surface roughness.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.6),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "emission",
        "Emission",
        "Surface emitted color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.0, 0.0, 0.0, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
];
const ADD_SCALAR_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "a",
        "A",
        "First operand.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "b",
        "B",
        "Second operand.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];
const MULTIPLY_SCALAR_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "a",
        "A",
        "First operand.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "b",
        "B",
        "Second operand.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];
/// The oscillator's shape. Every numeric control is an input socket, so a
/// sensor can drive the rate or the range and "trigger a pulse" composes out of
/// nodes rather than needing a mode on this one.
/// Azimuth is measured around the vertical axis with 0 degrees along +X and 90
/// along +Z; elevation is the angle above horizontal. That is the same meaning
/// `SunSettings` already gives those words (`lighting.rs`) — reused so the
/// codebase has ONE definition of an angle pair, not because a flow has
/// anything to do with the sun.
const DIRECTION_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "speed",
        "Speed",
        "Length of the resulting vector. For a pattern drift this is metres per \
         second; a texel is 1 m / texels-per-voxel, so 0.25 m/s at 8 texels is \
         two rows a second.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.25),
        WIDE,
        Some(NumericRange::new(0.0, 4.0)),
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "azimuth_degrees",
        "Azimuth",
        "Heading around the vertical axis: 0 points along +X, 90 along +Z. \
         \n\nAT AN ELEVATION OF -90 OR +90 THIS DOES NOTHING: straight down has \
         no horizontal part to steer, so the slider will appear dead. For a \
         diagonal, back the elevation off the pole first — -45 splits the \
         motion evenly between downward and sideways, and the azimuth then \
         chooses which way sideways.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        Some(NumericRange::new(-360.0, 360.0)),
        Some(NumericRange::new(0.0, 360.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "elevation_degrees",
        "Elevation",
        "Angle above horizontal. -90 is straight down a wall, 0 is level across \
         a floor or a lake, and anything between is a diagonal. Note that -90 \
         and +90 are poles where the azimuth stops having any effect.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        Some(NumericRange::new(-90.0, 90.0)),
        Some(NumericRange::new(-90.0, 90.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
];
const OSCILLATOR_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "enabled",
        "Enabled",
        "Turn the node off. A disabled oscillator is not merely held still — it \
         is removed from the graph, so whatever it feeds falls back to that \
         socket's own default, exactly as if the link were not there. That is \
         why there is no 'value while disabled' setting: the neutral value \
         belongs to the consumer, and a layer's gain, an emission strength and \
         a mix factor do not share one.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "wave",
        "Wave",
        "Waveform. `pulse` is the interval/blink shape (see Duty); `flicker` is \
         sample-and-hold — it SNAPS to a new random level each step, which is what \
         reads as a failing lamp rather than a wobbly sine. There is no `square`: \
         that is Pulse at duty 0.5.",
        FieldTarget::Property,
        FieldDefault::Text("sine"),
        NONE,
        NONE,
        None,
        &[
            choice(
                "sine",
                "Sine",
                "Ease smoothly between Low and High and back, with no corners — a \
                 breathing glow.",
            ),
            choice(
                "triangle",
                "Triangle",
                "Travel between Low and High at a constant speed, turning sharply at \
                 each end.",
            ),
            choice(
                "saw",
                "Saw",
                "Ramp from Low up to High, then snap back to Low — a one-way sweep \
                 that repeats.",
            ),
            choice(
                "pulse",
                "Pulse",
                "Sit at High for Duty of the cycle and at Low for the rest; Duty 0.5 \
                 is a square wave.",
            ),
            choice(
                "flicker",
                "Flicker",
                "Snap to a new random level once per cycle and hold it — a failing \
                 lamp rather than a wobble.",
            ),
        ],
        false,
    ),
    field(
        "rate_hz",
        "Rate (Hz)",
        "Cycles per second.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        Some(NumericRange::new(0.01, 20.0)),
        Some(NumericRange::new(0.05, 4.0)),
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "phase",
        "Phase",
        "Offset in turns, before the sync offset is added.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "duty",
        "Duty",
        "Pulse only: the fraction of each cycle spent high. A low duty gives long \
         dark and a short flash — the fade-in-intervals control.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.5),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "low",
        "Low",
        "Output at the bottom of the wave. Lands directly on an amount or an \
         emission strength without a remap.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "high",
        "High",
        "Output at the top of the wave.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "sync",
        "Sync",
        "Whether blocks of this material beat together. `global` is one heartbeat \
         across the whole material; `per_voxel` offsets each authored one-metre \
         block; `per_face` offsets each face of each block; `per_material` uses \
         Seed alone, so two materials can be deliberately out of step.",
        FieldTarget::Property,
        FieldDefault::Text("global"),
        NONE,
        NONE,
        None,
        &[
            choice(
                "global",
                "Global",
                "Give every block of this material one shared heartbeat, all in step.",
            ),
            choice(
                "per_voxel",
                "Per Voxel",
                "Offset each authored one-metre block so a wall of them shimmers \
                 instead of blinking as one.",
            ),
            choice(
                "per_face",
                "Per Face",
                "Offset each face of each block, for the finest-grained scatter.",
            ),
            choice(
                "per_material",
                "Per Material",
                "Offset by Seed alone, so this material stays internally in step but \
                 deliberately out of step with another.",
            ),
        ],
        false,
    ),
    field(
        "seed",
        "Seed",
        "The per-material offset, and the flicker sequence.",
        FieldTarget::Property,
        FieldDefault::Integer(0),
        Some(NumericRange::new(0.0, 65535.0)),
        Some(NumericRange::new(0.0, 64.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
];
/// The event sensor's configuration.
///
/// Every field is a PROPERTY rather than an input socket, and deliberately: the
/// hold + release budget is validated at compile time against
/// `MAX_EVENT_LIFETIME_SECONDS`, and a socket-driven value could not be checked
/// there. Authoring catches an over-long envelope; the runtime never has to.
const EVENT_SENSOR_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "enabled",
        "Enabled",
        "Turn the node off. A disabled sensor is removed from the compiled graph, so
         anything it feeds falls back to that input's own default just as if the
         link were absent.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "channel",
        "Channel",
        "Which kind of event to listen for. 0 is presence — an entity simply being \
         somewhere. The player is one entity; a mob is another, and this node \
         cannot tell them apart.",
        FieldTarget::Property,
        FieldDefault::Integer(0),
        Some(NumericRange::new(0.0, 255.0)),
        Some(NumericRange::new(0.0, 8.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "radius_meters",
        "Radius (m)",
        "Detection radius, intersected with each event's own reach — so a large \
         creature is felt further away without re-authoring the material.",
        FieldTarget::Property,
        FieldDefault::Scalar(6.0),
        Some(NumericRange::new(0.0, 256.0)),
        Some(NumericRange::new(0.5, 32.0)),
        Some(0.1),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "falloff",
        "Falloff",
        "How nearness falls off across the radius.",
        FieldTarget::Property,
        FieldDefault::Text("smoothstep"),
        NONE,
        NONE,
        None,
        &[
            choice(
                "smoothstep",
                "Smoothstep",
                "Ease in and out across the radius, so the edge of the sensed area \
                 has no visible seam.",
            ),
            choice(
                "linear",
                "Linear",
                "Fall off evenly with distance, reaching zero exactly at Radius.",
            ),
            choice(
                "inverse_square",
                "Inverse Square",
                "Drop off the way real light does: very strong up close, nearly \
                 nothing at arm's length.",
            ),
            choice(
                "step",
                "Step",
                "Give full strength anywhere inside Radius and nothing outside it — \
                 a hard trigger zone.",
            ),
        ],
        false,
    ),
    field(
        "attack_seconds",
        "Attack (s)",
        "Ramp up after the event starts. This is the part a distance-only sensor \
         cannot do: it runs off the event's timestamp, not off how far away the \
         entity is, so standing still holds the value steady instead of freezing \
         it mid-ramp.",
        FieldTarget::Property,
        FieldDefault::Scalar(0.25),
        Some(NumericRange::new(0.0, 8.0)),
        Some(NumericRange::new(0.0, 2.0)),
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "hold_seconds",
        "Hold (s)",
        "Stay at full for this long AFTER the event closes, before releasing. \
         Hold + Release must not exceed the 8 s event lifetime, or the event is \
         reclaimed while the sensor is still fading — the graph reports that.",
        FieldTarget::Property,
        FieldDefault::Scalar(0.0),
        Some(NumericRange::new(0.0, 8.0)),
        Some(NumericRange::new(0.0, 4.0)),
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "release_seconds",
        "Release (s)",
        "Ramp down once the hold expires. Capped with Hold at the event lifetime.",
        FieldTarget::Property,
        FieldDefault::Scalar(1.0),
        Some(NumericRange::new(0.0, 8.0)),
        Some(NumericRange::new(0.0, 4.0)),
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "invert",
        "Invert",
        "Fire when NOTHING is near. Affects Signal only — Nearness and Envelope \
         keep their literal meanings so they stay usable as diagnostics.",
        FieldTarget::Property,
        FieldDefault::Boolean(false),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
];
const MIX_COLOR_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "a",
        "A",
        "First color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.0, 0.0, 0.0, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "b",
        "B",
        "Second color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([1.0, 1.0, 1.0, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "factor",
        "Factor",
        "Blend weight.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.5),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];
const CLAMP_SCALAR_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "value",
        "Value",
        "Value to clamp.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "minimum",
        "Minimum",
        "Lower bound.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "maximum",
        "Maximum",
        "Upper bound.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];
const COLOR_STRENGTH_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "color",
        "Color",
        "Emission color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.0, 0.0, 0.0, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "strength",
        "Strength",
        "Emission intensity.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        POSITIVE,
        Some(NumericRange::new(0.0, 16.0)),
        Some(0.05),
        EMPTY_CHOICES,
        false,
    ),
];
const FACE_COLOR_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "base",
        "Base",
        "Fallback face color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.8, 0.8, 0.8, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "top",
        "Top",
        "Up-facing color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.8, 0.8, 0.8, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "side",
        "Side",
        "Side-facing color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.8, 0.8, 0.8, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "bottom",
        "Bottom",
        "Down-facing color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.8, 0.8, 0.8, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
];
const FACE_ROUGHNESS_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "base",
        "Base",
        "Fallback roughness.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.6),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "top",
        "Top",
        "Up-facing roughness.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.6),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "side",
        "Side",
        "Side-facing roughness.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.6),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "bottom",
        "Bottom",
        "Down-facing roughness.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.6),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];
const REMAP_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "value",
        "Value",
        "Value to remap.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "from_min",
        "From Min",
        "Input lower bound.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "from_max",
        "From Max",
        "Input upper bound.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "to_min",
        "To Min",
        "Output lower bound.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "to_max",
        "To Max",
        "Output upper bound.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "clamp",
        "Clamp",
        "Clamp the normalized factor.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
];
const NOISE_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "position",
        "Position",
        "Sampling position.",
        FieldTarget::InputSocket,
        FieldDefault::Vector3([0.0; 3]),
        NONE,
        NONE,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "scale",
        "Scale",
        "Noise frequency.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        POSITIVE,
        Some(NumericRange::new(0.01, 32.0)),
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "detail",
        "Detail",
        "Fractal octave detail.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(3.0),
        Some(NumericRange::new(1.0, 16.0)),
        Some(NumericRange::new(1.0, 8.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "roughness",
        "Roughness",
        "Fractal amplitude decay.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.5),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];
const FBM_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "position",
        "Position",
        "Sampling position.",
        FieldTarget::InputSocket,
        FieldDefault::Vector3([0.0; 3]),
        NONE,
        NONE,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "scale",
        "Scale",
        "Noise frequency.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        POSITIVE,
        Some(NumericRange::new(0.01, 32.0)),
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "octaves",
        "Octaves",
        "Fractal octave count.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(5.0),
        Some(NumericRange::new(1.0, 16.0)),
        Some(NumericRange::new(1.0, 8.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "roughness",
        "Roughness",
        "Fractal amplitude decay.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.5),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];
const COLOR_RAMP_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "factor",
        "Factor",
        "Ramp coordinate.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.5),
        WIDE,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "color_a",
        "Color A",
        "First stop color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.08, 0.2, 0.03, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "color_b",
        "Color B",
        "Second stop color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.55, 0.8, 0.12, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "position_a",
        "Position A",
        "First stop position.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.25),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "position_b",
        "Position B",
        "Second stop position.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.75),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];
const VECTOR_BINARY_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "a",
        "A",
        "First vector.",
        FieldTarget::InputSocket,
        FieldDefault::Vector3([0.0; 3]),
        NONE,
        NONE,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "b",
        "B",
        "Second vector.",
        FieldTarget::InputSocket,
        FieldDefault::Vector3([0.0; 3]),
        NONE,
        NONE,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];
const VECTOR_SCALE_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "vector",
        "Vector",
        "Vector to scale.",
        FieldTarget::InputSocket,
        FieldDefault::Vector3([0.0; 3]),
        NONE,
        NONE,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "scale",
        "Scale",
        "Scalar multiplier.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        WIDE,
        SIGNED,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
];
const VECTOR_INPUT_FIELDS: &[FieldDeclarationStatic] = &[field(
    "vector",
    "Vector",
    "Input vector.",
    FieldTarget::InputSocket,
    FieldDefault::Vector3([0.0, 1.0, 0.0]),
    NONE,
    NONE,
    Some(0.01),
    EMPTY_CHOICES,
    false,
)];
const COMPONENT_FIELDS: &[FieldDeclarationStatic] = &[field(
    "axis",
    "Axis",
    "Component axis: 0 = X, 1 = Y, 2 = Z.",
    FieldTarget::Property,
    FieldDefault::Integer(1),
    Some(NumericRange::new(0.0, 2.0)),
    Some(NumericRange::new(0.0, 2.0)),
    Some(1.0),
    EMPTY_CHOICES,
    false,
)];
const SCALAR_INPUT_FIELDS: &[FieldDeclarationStatic] = &[field(
    "value",
    "Value",
    "Input scalar.",
    FieldTarget::InputSocket,
    FieldDefault::Scalar(0.0),
    WIDE,
    SIGNED,
    Some(0.01),
    EMPTY_CHOICES,
    false,
)];
const COLOR_INPUT_FIELDS: &[FieldDeclarationStatic] = &[field(
    "value",
    "Value",
    "Input color.",
    FieldTarget::InputSocket,
    FieldDefault::Color([0.0; 4]),
    NONE,
    NONE,
    None,
    EMPTY_CHOICES,
    false,
)];
const VECTOR_REROUTE_FIELDS: &[FieldDeclarationStatic] = &[field(
    "vector",
    "Vector",
    "Input vector.",
    FieldTarget::InputSocket,
    FieldDefault::Vector3([0.0; 3]),
    NONE,
    NONE,
    Some(0.01),
    EMPTY_CHOICES,
    false,
)];
const PATTERN_FRAME_FIELD: FieldDeclarationStatic = field(
    "frame",
    "Frame",
    "Coordinate frame.",
    FieldTarget::Property,
    FieldDefault::Text("world"),
    NONE,
    NONE,
    None,
    &[
        choice(
            "world",
            "World",
            "Anchor the pattern to world space, so it stays put while blocks are \
             placed and removed around it.",
        ),
        choice(
            "voxel",
            "Voxel",
            "Anchor the pattern to each one-metre block, so every block carries an \
             identical copy.",
        ),
        choice(
            "face",
            "Face",
            "Anchor the pattern to each face's own 2D surface, so it reads flat \
             rather than sliced out of a volume.",
        ),
    ],
    false,
);
const PATTERN_PERIOD_FIELD: FieldDeclarationStatic = field(
    "period_meters",
    "Period",
    "Pattern period in meters.",
    FieldTarget::Property,
    FieldDefault::Scalar(0.02),
    Some(NumericRange::new(0.005, 4.0)),
    Some(NumericRange::new(0.005, 4.0)),
    Some(0.005),
    EMPTY_CHOICES,
    false,
);
const PATTERN_TEXELS_FIELD: FieldDeclarationStatic = field(
    "texels_per_voxel",
    "Texels Per Voxel",
    "Quantization resolution.",
    FieldTarget::Property,
    FieldDefault::Integer(8),
    Some(NumericRange::new(0.0, 32.0)),
    Some(NumericRange::new(0.0, 32.0)),
    Some(1.0),
    EMPTY_CHOICES,
    false,
);
const PATTERN_VARIATION_FIELD: FieldDeclarationStatic = field(
    "vary_per_face",
    "Vary Per Face",
    "Use a stable face-specific variation.",
    FieldTarget::Property,
    FieldDefault::Boolean(true),
    NONE,
    NONE,
    None,
    EMPTY_CHOICES,
    false,
);
const PATTERN_OCTAVES_FIELD: FieldDeclarationStatic = field(
    "octaves",
    "Octaves",
    "Noise octave count. The renderer evaluates at most MAX_NOISE_OCTAVES of \
     them, so this range is bounded by that rather than by taste.",
    FieldTarget::Property,
    FieldDefault::Integer(3),
    Some(NumericRange::new(
        1.0,
        crate::pattern::MAX_NOISE_OCTAVES as f32,
    )),
    Some(NumericRange::new(
        1.0,
        crate::pattern::MAX_NOISE_OCTAVES as f32,
    )),
    Some(1.0),
    EMPTY_CHOICES,
    false,
);
const PATTERN_DENSITY_FIELD: FieldDeclarationStatic = field(
    "density",
    "Density",
    "Fraction of cells containing a speck.",
    FieldTarget::Property,
    FieldDefault::Scalar(0.25),
    UNIT,
    UNIT,
    Some(0.01),
    EMPTY_CHOICES,
    false,
);
const PATTERN_FLAT_FIELDS: &[FieldDeclarationStatic] = &[
    PATTERN_FRAME_FIELD,
    PATTERN_PERIOD_FIELD,
    PATTERN_TEXELS_FIELD,
    PATTERN_VARIATION_FIELD,
];
const PATTERN_NOISE_FIELDS: &[FieldDeclarationStatic] = &[
    PATTERN_FRAME_FIELD,
    PATTERN_PERIOD_FIELD,
    PATTERN_TEXELS_FIELD,
    PATTERN_VARIATION_FIELD,
    PATTERN_OCTAVES_FIELD,
];
const PATTERN_SPECKLE_FIELDS: &[FieldDeclarationStatic] = &[
    PATTERN_FRAME_FIELD,
    PATTERN_PERIOD_FIELD,
    PATTERN_TEXELS_FIELD,
    PATTERN_VARIATION_FIELD,
    PATTERN_DENSITY_FIELD,
];
const PATTERN_LAYER_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "animation_gain",
        "Animation Gain",
        "Multiplies this layer's Amount. Wire an oscillator here to blink one \
         noise layer on its own without touching the base surface. It is a \
         SEPARATE value from Amount rather than a second way to set it, so \
         leaving it unconnected is plainly the identity.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        Some(NumericRange::new(0.0, 16.0)),
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "drift_velocity",
        "Drift (m/s)",
        "How fast this layer's pattern travels, in metres per second, world \
         space. A VELOCITY, not an offset: the shader applies the clock, so a \
         constant vector wired straight in makes the pattern flow. This is what \
         makes lava creep. For a flow that itself varies, scale a vector by an \
         oscillator.",
        FieldTarget::InputSocket,
        FieldDefault::Vector3([0.0; 3]),
        NONE,
        NONE,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "enabled",
        "Enabled",
        "Include this layer in the material stack.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "target",
        "Target",
        "Material channel to modify.",
        FieldTarget::Property,
        FieldDefault::Text("albedo"),
        NONE,
        NONE,
        None,
        &[
            choice("albedo", "Albedo", "Modify the surface's base color."),
            choice(
                "roughness",
                "Roughness",
                "Modify how rough the surface is, turning patches glossy or matte.",
            ),
            choice(
                "emission",
                "Emission",
                "Modify the light the surface gives off, for glowing veins or embers.",
            ),
        ],
        false,
    ),
    field(
        "blend",
        "Blend",
        "Layer blending operation.",
        FieldTarget::Property,
        FieldDefault::Text("multiply"),
        NONE,
        NONE,
        None,
        &[
            choice(
                "multiply",
                "Multiply",
                "Scale the target channel by the pattern, so the mask can only \
                 darken or weaken it.",
            ),
            choice(
                "mix_to_color",
                "Mix To Color",
                "Blend the target channel towards Target Color wherever the pattern \
                 is high.",
            ),
            choice(
                "add",
                "Add",
                "Add the pattern into the target channel, so the mask can only \
                 brighten or strengthen it.",
            ),
        ],
        false,
    ),
    field(
        "amount",
        "Amount",
        "Layer strength.",
        FieldTarget::Property,
        FieldDefault::Scalar(1.0),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "target_color",
        "Target Color",
        "Color used by color operations.",
        FieldTarget::Property,
        FieldDefault::Color([1.0; 4]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "faces_top",
        "Top Faces",
        "Affect top faces.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "faces_side",
        "Side Faces",
        "Affect side faces.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "faces_bottom",
        "Bottom Faces",
        "Affect bottom faces.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "emission_intensity",
        "Emission Intensity",
        "Emission multiplier.",
        FieldTarget::Property,
        FieldDefault::Scalar(1.0),
        Some(NumericRange::new(0.0, 16.0)),
        Some(NumericRange::new(0.0, 16.0)),
        Some(0.05),
        EMPTY_CHOICES,
        false,
    ),
];

/// Canonical node schemas. Backends register execution independently, but node
/// construction, validation, persistence, catalog presentation, and every
/// editable widget derive from this table.
pub static BUILTIN_NODES: &[NodeDeclaration] = &[
    node!(
        "material.constant_scalar",
        NodeOperation::Material(MaterialNodeOperation::ConstantScalar),
        "Scalar",
        "A constant scalar value.",
        NodeCategory::Inputs,
        NodePreview::Value,
        MATERIAL,
        &[],
        CONSTANT_SCALAR_OUT,
        CONSTANT_SCALAR_FIELDS
    ),
    node!(
        "material.output",
        NodeOperation::Material(MaterialNodeOperation::Output),
        "Material Output",
        "Final material surface consumed by the renderer.",
        NodeCategory::MaterialOutput,
        NodePreview::MaterialSphere,
        MATERIAL,
        MATERIAL_OUTPUT_IN,
        &[],
        &[]
    ),
    node!(
        "material.surface",
        NodeOperation::Material(MaterialNodeOperation::Surface),
        "Material Surface",
        "Intrinsic surface values before ordered pattern layers.",
        NodeCategory::MaterialOutput,
        NodePreview::MaterialSphere,
        MATERIAL,
        MATERIAL_SURFACE_IN,
        MATERIAL_SURFACE_OUT,
        MATERIAL_OUTPUT_FIELDS
    ),
    node!(
        "material.constant_color",
        NodeOperation::Material(MaterialNodeOperation::ConstantColor),
        "Color",
        "A constant linear color.",
        NodeCategory::Inputs,
        NodePreview::ColorWheel,
        MATERIAL,
        &[],
        CONSTANT_COLOR_OUT,
        CONSTANT_COLOR_FIELDS
    ),
    node!(
        "material.add_scalar",
        NodeOperation::Material(MaterialNodeOperation::AddScalar),
        "Add",
        "Adds two scalar values.",
        NodeCategory::Utilities,
        NodePreview::Value,
        MATERIAL,
        ADD_SCALAR_IN,
        ADD_SCALAR_OUT,
        ADD_SCALAR_FIELDS
    ),
    node!(
        "material.mix_color",
        NodeOperation::Material(MaterialNodeOperation::MixColor),
        "Mix Color",
        "Blends two colors.",
        NodeCategory::Utilities,
        NodePreview::Value,
        MATERIAL,
        COLOR_MIX_IN,
        MIX_COLOR_OUT,
        MIX_COLOR_FIELDS
    ),
    node!(
        "material.clamp_scalar",
        NodeOperation::Material(MaterialNodeOperation::ClampScalar),
        "Clamp",
        "Clamps a scalar between two bounds.",
        NodeCategory::Utilities,
        NodePreview::Value,
        MATERIAL,
        SCALAR_CLAMP_IN,
        CLAMP_SCALAR_OUT,
        CLAMP_SCALAR_FIELDS
    ),
    node!(
        "material.position",
        NodeOperation::Material(MaterialNodeOperation::Position),
        "Position",
        "World-space sample position.",
        NodeCategory::Coordinates,
        NodePreview::Value,
        MATERIAL,
        &[],
        POSITION_OUT,
        &[]
    ),
    node!(
        "material.normal",
        NodeOperation::Material(MaterialNodeOperation::Normal),
        "Normal",
        "World-space surface normal.",
        NodeCategory::Coordinates,
        NodePreview::Value,
        MATERIAL,
        &[],
        NORMAL_OUT,
        &[]
    ),
    node!(
        "material.base_color",
        NodeOperation::Material(MaterialNodeOperation::ConstantColor),
        "Base Color",
        "Intrinsic base color input.",
        NodeCategory::MaterialOutput,
        NodePreview::ColorWheel,
        MATERIAL,
        &[],
        BASE_COLOR_OUT,
        BASE_COLOR_FIELDS
    ),
    node!(
        "material.roughness",
        NodeOperation::Material(MaterialNodeOperation::ConstantScalar),
        "Roughness",
        "Intrinsic microsurface roughness.",
        NodeCategory::MaterialOutput,
        NodePreview::Value,
        MATERIAL,
        &[],
        ROUGHNESS_OUT,
        ROUGHNESS_FIELDS
    ),
    node!(
        "material.emission",
        NodeOperation::Material(MaterialNodeOperation::ConstantColor),
        "Emission",
        "Intrinsic emitted color.",
        NodeCategory::MaterialOutput,
        NodePreview::ColorWheel,
        MATERIAL,
        &[],
        EMISSION_OUT,
        EMISSION_FIELDS
    ),
    node!(
        "material.emission_strength",
        NodeOperation::Material(MaterialNodeOperation::EmissionStrength),
        "Emission Strength",
        "Scales emitted color intensity.",
        NodeCategory::MaterialOutput,
        NodePreview::Value,
        MATERIAL,
        COLOR_STRENGTH_IN,
        EMISSION_STRENGTH_OUT,
        COLOR_STRENGTH_FIELDS
    ),
    node!(
        "material.face_color",
        NodeOperation::Material(MaterialNodeOperation::FaceColor),
        "Face Color",
        "Selects color by voxel face orientation.",
        NodeCategory::MaterialOutput,
        NodePreview::Value,
        MATERIAL,
        FACE_COLOR_IN,
        FACE_COLOR_OUT,
        FACE_COLOR_FIELDS
    ),
    node!(
        "material.face_roughness",
        NodeOperation::Material(MaterialNodeOperation::FaceRoughness),
        "Face Roughness",
        "Selects roughness by voxel face orientation.",
        NodeCategory::MaterialOutput,
        NodePreview::Value,
        MATERIAL,
        FACE_SCALAR_IN,
        FACE_ROUGHNESS_OUT,
        FACE_ROUGHNESS_FIELDS
    ),
    node!(
        "material.pattern_flat",
        NodeOperation::Material(MaterialNodeOperation::PatternFlat),
        "Flat Pattern",
        "One stable value per sampling cell.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        &[],
        PATTERN_FLAT_OUT,
        PATTERN_FLAT_FIELDS
    ),
    node!(
        "material.pattern_noise",
        NodeOperation::Material(MaterialNodeOperation::PatternNoise),
        "Noise Pattern",
        "Fractal value-noise pattern.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        &[],
        PATTERN_NOISE_OUT,
        PATTERN_NOISE_FIELDS
    ),
    node!(
        "material.pattern_speckle",
        NodeOperation::Material(MaterialNodeOperation::PatternSpeckle),
        "Speckle Pattern",
        "Scattered specks controlled by cell density.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        &[],
        PATTERN_SPECKLE_OUT,
        PATTERN_SPECKLE_FIELDS
    ),
    node!(
        "material.pattern_layer",
        NodeOperation::Material(MaterialNodeOperation::PatternLayer),
        "Pattern Layer",
        "An ordered procedural modification of the incoming surface.",
        NodeCategory::Layers,
        NodePreview::Noise,
        MATERIAL,
        PATTERN_LAYER_IN,
        PATTERN_LAYER_OUT,
        PATTERN_LAYER_FIELDS
    ),
    node!(
        "material.multiply_scalar",
        NodeOperation::Material(MaterialNodeOperation::MultiplyScalar),
        "Multiply",
        "Multiplies two scalar values. This is how a trigger GATES something: \
         an event sensor's signal times an oscillator is a pulse that only runs \
         while the sensor is firing.",
        NodeCategory::Utilities,
        NodePreview::Value,
        MATERIAL,
        MULTIPLY_SCALAR_IN,
        MULTIPLY_SCALAR_OUT,
        MULTIPLY_SCALAR_FIELDS
    ),
    node!(
        "material.direction",
        NodeOperation::Material(MaterialNodeOperation::Direction),
        "Direction",
        "Speed and two angles to a velocity vector — the authoring form for a \
         pattern drift, where dialling an angle beats editing three components. \
         Every input is connectable, so an oscillator on the azimuth swirls the \
         flow and one on the speed makes it surge.",
        NodeCategory::Coordinates,
        NodePreview::Value,
        MATERIAL,
        DIRECTION_IN,
        DIRECTION_OUT,
        DIRECTION_FIELDS
    ),
    node!(
        "material.time",
        NodeOperation::Material(MaterialNodeOperation::Time),
        "Time",
        "Monotone seconds since the session started. Never steps backwards. \
         Pattern drift does not need this — a layer's drift socket is a VELOCITY \
         and applies the clock itself.",
        NodeCategory::Animation,
        NodePreview::Value,
        MATERIAL,
        &[],
        TIME_OUT,
        &[]
    ),
    node!(
        "material.oscillator",
        NodeOperation::Material(MaterialNodeOperation::Oscillator),
        "Oscillator",
        "A periodic wave between Low and High. Drive an emission strength for a \
         pulsing block, a mix factor to travel between two colours, or a pattern \
         layer's gain to blink one noise layer on its own.",
        NodeCategory::Animation,
        NodePreview::Value,
        MATERIAL,
        OSCILLATOR_IN,
        OSCILLATOR_OUT,
        OSCILLATOR_FIELDS
    ),
    node!(
        "material.event_sensor",
        NodeOperation::Material(MaterialNodeOperation::EventSensor),
        "Event Sensor",
        "Did something happen within Radius of me, and how long ago? Signal is \
         falloff x envelope x strength and is what most graphs use; Nearness and \
         Envelope expose the two halves separately.",
        NodeCategory::Animation,
        NodePreview::Value,
        MATERIAL,
        &[],
        EVENT_SENSOR_OUT,
        EVENT_SENSOR_FIELDS
    ),
    node!(
        "material.remap_scalar",
        NodeOperation::Material(MaterialNodeOperation::RemapScalar),
        "Remap",
        "Remaps one scalar interval to another.",
        NodeCategory::Procedural,
        NodePreview::Value,
        MATERIAL,
        REMAP_IN,
        REMAP_SCALAR_OUT,
        REMAP_FIELDS
    ),
    node!(
        "material.noise",
        NodeOperation::Material(MaterialNodeOperation::Noise),
        "Noise",
        "Procedural multi-octave noise.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        PROCEDURAL_SCALAR_IN,
        NOISE_OUT,
        NOISE_FIELDS
    ),
    node!(
        "material.fbm",
        NodeOperation::Material(MaterialNodeOperation::Fbm),
        "Fractal Noise",
        "Fractal Brownian motion scalar noise.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        FBM_IN,
        FBM_OUT,
        FBM_FIELDS
    ),
    node!(
        "material.color_ramp",
        NodeOperation::Material(MaterialNodeOperation::ColorRamp),
        "Color Ramp",
        "Maps a scalar through two color stops.",
        NodeCategory::Procedural,
        NodePreview::ColorRamp,
        MATERIAL,
        COLOR_RAMP_IN,
        COLOR_RAMP_OUT,
        COLOR_RAMP_FIELDS
    ),
    node!(
        "material.vector_add",
        NodeOperation::Material(MaterialNodeOperation::VectorAdd),
        "Vector Add",
        "Adds two vectors.",
        NodeCategory::Coordinates,
        NodePreview::Value,
        MATERIAL,
        VECTOR_ADD_IN,
        VECTOR_ADD_OUT,
        VECTOR_BINARY_FIELDS
    ),
    node!(
        "material.vector_scale",
        NodeOperation::Material(MaterialNodeOperation::VectorScale),
        "Vector Scale",
        "Scales a vector.",
        NodeCategory::Coordinates,
        NodePreview::Value,
        MATERIAL,
        VECTOR_SCALE_IN,
        VECTOR_SCALE_OUT,
        VECTOR_SCALE_FIELDS
    ),
    node!(
        "material.normalize_vector",
        NodeOperation::Material(MaterialNodeOperation::NormalizeVector),
        "Normalize",
        "Normalizes a vector.",
        NodeCategory::Coordinates,
        NodePreview::Value,
        MATERIAL,
        NORMALIZE_VECTOR_IN,
        NORMALIZE_VECTOR_OUT,
        VECTOR_INPUT_FIELDS
    ),
    node!(
        "material.dot_vector",
        NodeOperation::Material(MaterialNodeOperation::DotVector),
        "Dot Product",
        "Computes a vector dot product.",
        NodeCategory::Coordinates,
        NodePreview::Value,
        MATERIAL,
        VECTOR_DOT_IN,
        VECTOR_DOT_OUT,
        VECTOR_BINARY_FIELDS
    ),
    node!(
        "material.position_component",
        NodeOperation::Material(MaterialNodeOperation::PositionComponent),
        "Position Component",
        "Selects one position axis.",
        NodeCategory::Coordinates,
        NodePreview::Value,
        MATERIAL,
        &[],
        POSITION_COMPONENT_OUT,
        COMPONENT_FIELDS
    ),
    node!(
        "material.normal_component",
        NodeOperation::Material(MaterialNodeOperation::NormalComponent),
        "Normal Component",
        "Selects one normal axis.",
        NodeCategory::Coordinates,
        NodePreview::Value,
        MATERIAL,
        &[],
        NORMAL_COMPONENT_OUT,
        COMPONENT_FIELDS
    ),
    node!(
        "material.passthrough_scalar",
        NodeOperation::Material(MaterialNodeOperation::PassthroughScalar),
        "Scalar Passthrough",
        "Passes a scalar unchanged.",
        NodeCategory::Utilities,
        NodePreview::Value,
        MATERIAL,
        PASSTHROUGH_SCALAR_IN,
        PASSTHROUGH_SCALAR_OUT,
        SCALAR_INPUT_FIELDS
    ),
    node!(
        "material.reroute_scalar",
        NodeOperation::Material(MaterialNodeOperation::RerouteScalar),
        "Scalar Reroute",
        "Reroutes a scalar connection.",
        NodeCategory::Utilities,
        NodePreview::None,
        MATERIAL,
        REROUTE_SCALAR_IN,
        REROUTE_SCALAR_OUT,
        SCALAR_INPUT_FIELDS
    ),
    node!(
        "material.reroute_color",
        NodeOperation::Material(MaterialNodeOperation::RerouteColor),
        "Color Reroute",
        "Reroutes a color connection.",
        NodeCategory::Utilities,
        NodePreview::None,
        MATERIAL,
        REROUTE_COLOR_IN,
        REROUTE_COLOR_OUT,
        COLOR_INPUT_FIELDS
    ),
    node!(
        "material.reroute_vector",
        NodeOperation::Material(MaterialNodeOperation::RerouteVector),
        "Vector Reroute",
        "Reroutes a vector connection.",
        NodeCategory::Utilities,
        NodePreview::None,
        MATERIAL,
        REROUTE_VECTOR_IN,
        REROUTE_VECTOR_OUT,
        VECTOR_REROUTE_FIELDS
    ),
    node!(
        "world.generated_terrain",
        NodeOperation::World(WorldNodeOperation::GeneratedTerrain),
        "Generated Terrain",
        "Creates the deterministic base voxel terrain.",
        NodeCategory::Environment,
        NodePreview::None,
        WORLD,
        &[],
        GENERATED_TERRAIN_OUT,
        &[]
    ),
    node!(
        "world.compose",
        NodeOperation::World(WorldNodeOperation::Compose),
        "Surface Composer",
        "Applies registered environment, biome, and surface programs to terrain.",
        NodeCategory::Surface,
        NodePreview::None,
        WORLD,
        WORLD_COMPOSE_IN,
        WORLD_COMPOSE_OUT,
        &[]
    ),
    node!(
        "world.output",
        NodeOperation::World(WorldNodeOperation::Output),
        "World Output",
        "Final voxel world consumed by the engine.",
        NodeCategory::Render,
        NodePreview::None,
        WORLD,
        VOXEL_FIELD_IN,
        &[],
        &[]
    ),
    node!(
        "world.studio_preview",
        NodeOperation::World(WorldNodeOperation::StudioPreview),
        "Studio Preview",
        "Builds the isolated material preview plate and subject.",
        NodeCategory::Inputs,
        NodePreview::None,
        WORLD,
        &[],
        STUDIO_PREVIEW_OUT,
        &[]
    ),
];

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

fn cardinality_description(cardinality: Cardinality) -> String {
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

fn validate_graph_contract(
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
    fn from_graph(graph: &GraphAsset, active: &BTreeSet<NodeId>) -> Self {
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

fn cycle_nodes(nodes: &[ResolvedNode], links: &[ResolvedLink]) -> BTreeSet<NodeId> {
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

fn active_slice(
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

#[cfg(test)]
mod tests {
    use super::*;
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
        for declaration in BUILTIN_NODES {
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
        for contract in GRAPH_CONTRACTS {
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
        let declaration = NodeRegistry
            .find(&NodeTypeId("material.pattern_noise".into()))
            .expect("the noise generator is registered");
        let octaves = declaration
            .field(FieldTarget::Property, "octaves")
            .expect("the generator declares an octave count");
        let range = octaves.hard_range.expect("octaves are bounded");
        assert_eq!(
            range.max,
            crate::pattern::MAX_NOISE_OCTAVES as f32,
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
        let registry = NodeRegistry;
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
        let registry = NodeRegistry::builtin();
        let mut graph = crate::material_graph::new_material_graph("test");
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
        let registry = NodeRegistry::builtin();
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
        let registry = NodeRegistry::builtin();
        let mut graph = crate::material_graph::new_material_graph("test");
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
        let registry = NodeRegistry::builtin();
        let mut graph = crate::material_graph::new_material_graph("test");
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
        let registry = NodeRegistry;
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
        for declaration in BUILTIN_NODES {
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
        let registry = NodeRegistry::builtin();
        let mut graph = crate::material_graph::new_material_graph("test");
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
        let registry = NodeRegistry;
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
        let registry = NodeRegistry;
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
        let registry = NodeRegistry::builtin();
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
        let registry = NodeRegistry;
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
}
