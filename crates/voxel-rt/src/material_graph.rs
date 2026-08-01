//! Material-graph lowering shared by the CPU preview and generated WGSL.
//!
//! The editable [`GraphAsset`](crate::graph::GraphAsset) is never evaluated
//! directly. It first becomes this small typed IR, which gives CPU preview and
//! GPU code generation the same node semantics.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::graph::{
    Diagnostic, DiagnosticSeverity, FieldTarget, GraphAsset, GraphCommand, GraphHistory, GraphKind,
    InputPin, LinkId, LinkRecord, MaterialNodeOperation, NodeId, NodeOperation, NodeRecord,
    NodeRegistry, NodeTypeId, OutputPin, PropertyValue, SocketKey,
};
use crate::material::Material;
use crate::material_graph_layers::{
    project_pattern_stack, resolve_material_surface_chain, LayerGraphError, PATTERN_LAYER_NODE,
    PATTERN_NOISE_NODE,
};
use crate::pattern::MAX_PATTERN_LAYERS;
use crate::studio_assets::AssetId;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueType {
    Scalar,
    Color,
    Vector3,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ValueId(pub usize);

#[derive(Clone, Debug)]
pub enum MaterialInstruction {
    Scalar(f32),
    Color([f32; 4]),
    Vector3([f32; 3]),
    AddScalar(ValueId, ValueId),
    RemapScalar {
        value: ValueId,
        from_min: ValueId,
        from_max: ValueId,
        to_min: ValueId,
        to_max: ValueId,
        clamp: bool,
    },
    MixColor {
        a: ValueId,
        b: ValueId,
        factor: ValueId,
    },
    ColorScale {
        color: ValueId,
        strength: ValueId,
    },
    FaceColor {
        base: ValueId,
        top: ValueId,
        side: ValueId,
        bottom: ValueId,
    },
    FaceScalar {
        base: ValueId,
        top: ValueId,
        side: ValueId,
        bottom: ValueId,
    },
    ColorRamp {
        factor: ValueId,
        color_a: ValueId,
        color_b: ValueId,
        position_a: ValueId,
        position_b: ValueId,
    },
    ClampScalar {
        value: ValueId,
        minimum: ValueId,
        maximum: ValueId,
    },
    Noise {
        position: ValueId,
        scale: ValueId,
        detail: ValueId,
        roughness: ValueId,
    },
    NoiseColor {
        position: ValueId,
        scale: ValueId,
        detail: ValueId,
        roughness: ValueId,
    },
    Fbm {
        position: ValueId,
        scale: ValueId,
        octaves: ValueId,
        roughness: ValueId,
    },
    PassthroughScalar(ValueId),
    PassthroughColor(ValueId),
    PassthroughVector(ValueId),
    Position,
    Normal,
    VectorAdd(ValueId, ValueId),
    VectorScale {
        vector: ValueId,
        scale: ValueId,
    },
    NormalizeVector(ValueId),
    DotVector(ValueId, ValueId),
    Component {
        vector: ValueId,
        axis: u8,
    },
}

impl MaterialInstruction {
    fn value_type(&self) -> ValueType {
        match self {
            Self::Scalar(_)
            | Self::AddScalar(..)
            | Self::RemapScalar { .. }
            | Self::ClampScalar { .. }
            | Self::Noise { .. }
            | Self::Fbm { .. }
            | Self::DotVector(..)
            | Self::Component { .. }
            | Self::FaceScalar { .. }
            | Self::PassthroughScalar(_) => ValueType::Scalar,
            Self::Color(_)
            | Self::MixColor { .. }
            | Self::ColorScale { .. }
            | Self::ColorRamp { .. }
            | Self::FaceColor { .. }
            | Self::NoiseColor { .. }
            | Self::PassthroughColor(_) => ValueType::Color,
            Self::Position
            | Self::Normal
            | Self::Vector3(_)
            | Self::VectorAdd(..)
            | Self::VectorScale { .. }
            | Self::NormalizeVector(_)
            | Self::PassthroughVector(_) => ValueType::Vector3,
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct MaterialOutput {
    pub base_color: ValueId,
    pub roughness: ValueId,
    pub emission: ValueId,
}

#[derive(Clone, Debug)]
pub struct MaterialGraphProgram {
    pub graph_id: AssetId,
    pub semantic_hash: u64,
    pub instructions: Vec<MaterialInstruction>,
    pub output: MaterialOutput,
    pub wgsl: String,
}

const GRAPH_MATERIAL_STRUCT: &str = concat!(
    "struct GraphMaterial { base_color: vec4<f32>, roughness: f32, emission: vec4<f32>, graph_active: bool, face_color_active: bool, face_roughness_active: bool, };\n",
    "\nfn graph_hash3(point: vec3<f32>) -> f32 {\n",
    "    return fract(sin(dot(point, vec3<f32>(127.1, 311.7, 74.7))) * 43758.5453);\n",
    "}\n\n",
    "fn graph_value_noise(point: vec3<f32>) -> f32 {\n",
    "    let cell = floor(point);\n",
    "    let local = fract(point);\n",
    "    let blend = local * local * (vec3<f32>(3.0, 3.0, 3.0) - 2.0 * local);\n",
    "    let n000 = graph_hash3(cell + vec3<f32>(0.0, 0.0, 0.0));\n",
    "    let n100 = graph_hash3(cell + vec3<f32>(1.0, 0.0, 0.0));\n",
    "    let n010 = graph_hash3(cell + vec3<f32>(0.0, 1.0, 0.0));\n",
    "    let n110 = graph_hash3(cell + vec3<f32>(1.0, 1.0, 0.0));\n",
    "    let n001 = graph_hash3(cell + vec3<f32>(0.0, 0.0, 1.0));\n",
    "    let n101 = graph_hash3(cell + vec3<f32>(1.0, 0.0, 1.0));\n",
    "    let n011 = graph_hash3(cell + vec3<f32>(0.0, 1.0, 1.0));\n",
    "    let n111 = graph_hash3(cell + vec3<f32>(1.0, 1.0, 1.0));\n",
    "    let x00 = mix(n000, n100, blend.x);\n",
    "    let x10 = mix(n010, n110, blend.x);\n",
    "    let x01 = mix(n001, n101, blend.x);\n",
    "    let x11 = mix(n011, n111, blend.x);\n",
    "    return mix(mix(x00, x10, blend.y), mix(x01, x11, blend.y), blend.z);\n",
    "}\n\n",
    "fn graph_fbm(point: vec3<f32>, octaves: f32, roughness: f32) -> f32 {\n",
    "    var total = 0.0;\n",
    "    var amplitude = 1.0;\n",
    "    var frequency = 1.0;\n",
    "    var normalisation = 0.0;\n",
    "    for (var octave = 0u; octave < 8u; octave = octave + 1u) {\n",
    "        let enabled = select(0.0, 1.0, f32(octave) < max(octaves, 1.0));\n",
    "        total = total + graph_value_noise(point * frequency) * amplitude * enabled;\n",
    "        normalisation = normalisation + amplitude * enabled;\n",
    "        frequency = frequency * 2.0;\n",
    "        amplitude = amplitude * clamp(roughness, 0.0, 1.0);\n",
    "    }\n",
    "    return select(0.0, total / normalisation, normalisation > 0.0);\n",
    "}\n\n",
    "fn graph_safe_normalize(vector: vec3<f32>) -> vec3<f32> {\n",
    "    let magnitude = length(vector);\n",
    "    return select(vec3<f32>(0.0, 1.0, 0.0), vector / magnitude, magnitude > 0.000001);\n",
    "}\n\n",
    "fn graph_face_color(normal: vec3<f32>, base: vec4<f32>, top: vec4<f32>, side: vec4<f32>, bottom: vec4<f32>) -> vec4<f32> {\n",
    "    if (normal.y > 0.5) { return top; }\n",
    "    if (normal.y < -0.5) { return bottom; }\n",
    "    return side;\n",
    "}\n\n",
    "fn graph_face_scalar(normal: vec3<f32>, base: f32, top: f32, side: f32, bottom: f32) -> f32 {\n",
    "    if (normal.y > 0.5) { return top; }\n",
    "    if (normal.y < -0.5) { return bottom; }\n",
    "    return side;\n",
    "}\n"
);

/// Runtime activation is deliberately separate from editing and compilation.
/// A failed candidate leaves the last working program installed.
#[derive(Default)]
pub struct MaterialGraphLibrary {
    active: BTreeMap<AssetId, u64>,
    compiled: BTreeMap<u64, MaterialGraphProgram>,
}

/// Compiled graph programs selected for concrete voxel-material slots. The
/// dispatch is generated into the DDA source; slots absent from this map use
/// the existing material-table/pattern path unchanged.
#[derive(Default)]
pub struct MaterialGraphShaderSet {
    programs: BTreeMap<u8, MaterialGraphProgram>,
}

impl MaterialGraphShaderSet {
    pub fn insert(&mut self, material_slot: u8, program: MaterialGraphProgram) {
        self.programs.insert(material_slot, program);
    }

    pub fn len(&self) -> usize {
        self.programs.len()
    }

    pub fn is_empty(&self) -> bool {
        self.programs.is_empty()
    }

    /// Patch the marker in the shared fallback function and append the generated
    /// graph functions. No graph means byte-for-byte unchanged source.
    pub fn inject_into_dda(&self, source: &str) -> String {
        if self.programs.is_empty() {
            return source.to_string();
        }
        let mut branches = String::new();
        let mut functions = String::new();
        for (slot, program) in &self.programs {
            let function_name = format!("graph_material_{slot}");
            branches.push_str(&format!(
                "    if (material == {slot}u) {{ return {function_name}(position, normal); }}\n"
            ));
            functions.push_str(&program.wgsl_function(&function_name));
            functions.push('\n');
        }
        let patched = source.replace("    // GRAPH_DISPATCH_POINT\n", &branches);
        format!("{patched}\n{functions}")
    }
}

impl MaterialGraphLibrary {
    pub fn try_activate(
        &mut self,
        graph: &GraphAsset,
        registry: &NodeRegistry,
    ) -> Result<(), MaterialGraphError> {
        let program = compile(graph, registry)?;
        let hash = program.semantic_hash;
        self.compiled.entry(hash).or_insert(program);
        self.active.insert(graph.id.clone(), hash);
        Ok(())
    }

    pub fn active(&self, graph: &AssetId) -> Option<&MaterialGraphProgram> {
        self.active
            .get(graph)
            .and_then(|hash| self.compiled.get(hash))
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialSampleContext {
    pub position: [f32; 3],
    pub normal: [f32; 3],
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialSample {
    pub base_color: [f32; 4],
    pub roughness: f32,
    pub emission: [f32; 4],
}

/// Create the smallest valid material graph entirely from the registered node
/// declarations. New/reset documents therefore obey the same schema as loaded
/// documents and can accept a layer immediately.
pub fn new_material_graph(name: impl Into<String>) -> GraphAsset {
    let registry = NodeRegistry::builtin();
    let mut graph = GraphAsset::new(name, GraphKind::Material);
    let surface = NodeId::new();
    let output = NodeId::new();
    let surface_type = NodeTypeId("material.surface".into());
    let output_type = NodeTypeId("material.output".into());
    graph.nodes.insert(
        surface.clone(),
        registry
            .find(&surface_type)
            .expect("builtin material surface declaration")
            .new_record(),
    );
    graph.nodes.insert(
        output.clone(),
        registry
            .find(&output_type)
            .expect("builtin material output declaration")
            .new_record(),
    );
    graph.links.insert(
        LinkId::new(),
        LinkRecord {
            from: OutputPin {
                node: surface.clone(),
                socket: SocketKey("surface".into()),
            },
            to: InputPin {
                node: output.clone(),
                socket: SocketKey("surface".into()),
            },
            order: 0,
        },
    );
    graph.layout.positions.insert(surface, [160.0, 120.0]);
    graph.layout.positions.insert(output, [440.0, 160.0]);
    graph
}

/// Build the initial canonical surface graph for a material definition. This
/// keeps project bootstrapping lossless for the properties the graph ABI exposes:
/// albedo/face roles become Base Color, roughness/face roughness become
/// Roughness, and emission becomes Emission. The graph is intentionally unsaved
/// until the author presses Save, so opening an old material never mutates the
/// project by itself. Pattern nodes compile into the renderer projection after
/// the intrinsic graph surface.
pub fn graph_from_material(material: &Material) -> GraphAsset {
    let mut graph = GraphAsset::new(
        format!("{} Material Graph", material.name),
        GraphKind::Material,
    );
    let output = NodeId::new();
    let surface = NodeId::new();
    let base_color = NodeId::new();
    let roughness = NodeId::new();
    let emission = NodeId::new();
    graph.nodes.insert(
        output.clone(),
        NodeRecord {
            node_type: NodeTypeId("material.output".into()),
            node_type_version: 1,
            properties: BTreeMap::new(),
            socket_defaults: BTreeMap::new(),
            unknown_payload: None,
        },
    );
    graph.nodes.insert(
        surface.clone(),
        NodeRecord {
            node_type: NodeTypeId("material.surface".into()),
            node_type_version: 1,
            properties: BTreeMap::new(),
            socket_defaults: BTreeMap::new(),
            unknown_payload: None,
        },
    );
    graph.nodes.insert(
        base_color.clone(),
        NodeRecord {
            node_type: NodeTypeId("material.base_color".into()),
            node_type_version: 1,
            properties: BTreeMap::from([(
                "value".to_string(),
                PropertyValue::Color([
                    material.albedo[0],
                    material.albedo[1],
                    material.albedo[2],
                    1.0,
                ]),
            )]),
            socket_defaults: BTreeMap::new(),
            unknown_payload: None,
        },
    );
    graph.nodes.insert(
        roughness.clone(),
        NodeRecord {
            node_type: NodeTypeId("material.roughness".into()),
            node_type_version: 1,
            properties: BTreeMap::from([(
                "value".to_string(),
                PropertyValue::Scalar(material.roughness),
            )]),
            socket_defaults: BTreeMap::new(),
            unknown_payload: None,
        },
    );
    if let Some(roles) = material.face_roles {
        let color = |value: [f32; 3]| PropertyValue::Color([value[0], value[1], value[2], 1.0]);
        if let Some(node) = graph.nodes.get_mut(&base_color) {
            node.node_type = NodeTypeId("material.face_color".into());
            node.properties.clear();
            node.socket_defaults = BTreeMap::from([
                (
                    SocketKey("base".into()),
                    PropertyValue::Color([
                        material.albedo[0],
                        material.albedo[1],
                        material.albedo[2],
                        1.0,
                    ]),
                ),
                (SocketKey("top".into()), color(roles.top.albedo)),
                (SocketKey("side".into()), color(roles.side.albedo)),
                (SocketKey("bottom".into()), color(roles.bottom.albedo)),
            ]);
        }
        if let Some(node) = graph.nodes.get_mut(&roughness) {
            node.node_type = NodeTypeId("material.face_roughness".into());
            node.properties.clear();
            node.socket_defaults = BTreeMap::from([
                (
                    SocketKey("base".into()),
                    PropertyValue::Scalar(material.roughness),
                ),
                (
                    SocketKey("top".into()),
                    PropertyValue::Scalar(roles.top.roughness),
                ),
                (
                    SocketKey("side".into()),
                    PropertyValue::Scalar(roles.side.roughness),
                ),
                (
                    SocketKey("bottom".into()),
                    PropertyValue::Scalar(roles.bottom.roughness),
                ),
            ]);
        }
    }
    let emission_color = material.emission.unwrap_or([0.0; 3]);
    graph.nodes.insert(
        emission.clone(),
        NodeRecord {
            node_type: NodeTypeId("material.emission".into()),
            node_type_version: 1,
            properties: BTreeMap::from([(
                "value".to_string(),
                PropertyValue::Color([
                    emission_color[0],
                    emission_color[1],
                    emission_color[2],
                    1.0,
                ]),
            )]),
            socket_defaults: BTreeMap::new(),
            unknown_payload: None,
        },
    );
    graph.links.insert(
        LinkId::new(),
        LinkRecord {
            from: OutputPin {
                node: base_color.clone(),
                socket: SocketKey("color".into()),
            },
            to: InputPin {
                node: surface.clone(),
                socket: SocketKey("base_color".into()),
            },
            order: 0,
        },
    );
    graph.links.insert(
        LinkId::new(),
        LinkRecord {
            from: OutputPin {
                node: roughness.clone(),
                socket: SocketKey("value".into()),
            },
            to: InputPin {
                node: surface.clone(),
                socket: SocketKey("roughness".into()),
            },
            order: 0,
        },
    );
    graph.links.insert(
        LinkId::new(),
        LinkRecord {
            from: OutputPin {
                node: emission.clone(),
                socket: SocketKey("color".into()),
            },
            to: InputPin {
                node: surface.clone(),
                socket: SocketKey("emission".into()),
            },
            order: 0,
        },
    );
    graph
        .layout
        .positions
        .insert(surface.clone(), [440.0, 160.0]);
    graph.layout.positions.insert(base_color, [80.0, 20.0]);
    graph.layout.positions.insert(roughness, [80.0, 220.0]);
    graph.layout.positions.insert(emission, [80.0, 420.0]);
    crate::material_graph_layers::append_pattern_layer_nodes(
        &mut graph,
        &surface,
        &output,
        &material.patterns,
    );
    graph
}

#[derive(Clone, Copy, Debug, PartialEq)]
enum RuntimeValue {
    Scalar(f32),
    Color([f32; 4]),
    Vector3([f32; 3]),
}

impl MaterialGraphProgram {
    /// Return only the generated function, renamed for a material-slot
    /// dispatch. The shared `GraphMaterial` struct is emitted once by the DDA
    /// adapter, avoiding duplicate WGSL declarations when several slots use
    /// graphs.
    pub fn wgsl_function(&self, name: &str) -> String {
        self.wgsl
            .strip_prefix(GRAPH_MATERIAL_STRUCT)
            .unwrap_or(&self.wgsl)
            .replacen("fn graph_material(", &format!("fn {name}("), 1)
    }

    pub fn evaluate(&self, context: MaterialSampleContext) -> MaterialSample {
        let mut values: Vec<RuntimeValue> = Vec::with_capacity(self.instructions.len());
        for instruction in &self.instructions {
            let value = match *instruction {
                MaterialInstruction::Scalar(value) => RuntimeValue::Scalar(value),
                MaterialInstruction::Color(value) => RuntimeValue::Color(value),
                MaterialInstruction::Vector3(value) => RuntimeValue::Vector3(value),
                MaterialInstruction::AddScalar(a, b) => {
                    RuntimeValue::Scalar(values[a.0].scalar() + values[b.0].scalar())
                }
                MaterialInstruction::RemapScalar {
                    value,
                    from_min,
                    from_max,
                    to_min,
                    to_max,
                    clamp,
                } => {
                    let input = values[value.0].scalar();
                    let source_span = values[from_max.0].scalar() - values[from_min.0].scalar();
                    let factor = if source_span.abs() < f32::EPSILON {
                        0.0
                    } else {
                        (input - values[from_min.0].scalar()) / source_span
                    };
                    let factor = if clamp {
                        factor.clamp(0.0, 1.0)
                    } else {
                        factor
                    };
                    RuntimeValue::Scalar(
                        values[to_min.0].scalar()
                            + factor * (values[to_max.0].scalar() - values[to_min.0].scalar()),
                    )
                }
                MaterialInstruction::MixColor { a, b, factor } => {
                    let factor = values[factor.0].scalar();
                    let a = values[a.0].color();
                    let b = values[b.0].color();
                    RuntimeValue::Color(std::array::from_fn(|index| {
                        a[index] * (1.0 - factor) + b[index] * factor
                    }))
                }
                MaterialInstruction::ColorScale { color, strength } => {
                    let color = values[color.0].color();
                    let strength = values[strength.0].scalar();
                    RuntimeValue::Color(color.map(|component| component * strength))
                }
                MaterialInstruction::FaceColor {
                    base,
                    top,
                    side,
                    bottom,
                } => RuntimeValue::Color(face_color(
                    context.normal,
                    values[base.0].color(),
                    values[top.0].color(),
                    values[side.0].color(),
                    values[bottom.0].color(),
                )),
                MaterialInstruction::FaceScalar {
                    base,
                    top,
                    side,
                    bottom,
                } => RuntimeValue::Scalar(face_scalar(
                    context.normal,
                    values[base.0].scalar(),
                    values[top.0].scalar(),
                    values[side.0].scalar(),
                    values[bottom.0].scalar(),
                )),
                MaterialInstruction::ColorRamp {
                    factor,
                    color_a,
                    color_b,
                    position_a,
                    position_b,
                } => {
                    let span = values[position_b.0].scalar() - values[position_a.0].scalar();
                    let t = if span.abs() < f32::EPSILON {
                        0.0
                    } else {
                        ((values[factor.0].scalar() - values[position_a.0].scalar()) / span)
                            .clamp(0.0, 1.0)
                    };
                    let a = values[color_a.0].color();
                    let b = values[color_b.0].color();
                    RuntimeValue::Color(std::array::from_fn(|index| {
                        a[index] * (1.0 - t) + b[index] * t
                    }))
                }
                MaterialInstruction::ClampScalar {
                    value,
                    minimum,
                    maximum,
                } => RuntimeValue::Scalar(
                    values[value.0]
                        .scalar()
                        .clamp(values[minimum.0].scalar(), values[maximum.0].scalar()),
                ),
                MaterialInstruction::Noise {
                    position,
                    scale,
                    detail,
                    roughness,
                } => RuntimeValue::Scalar(graph_fbm(
                    values[position.0].vector(),
                    values[detail.0].scalar(),
                    values[roughness.0].scalar(),
                    values[scale.0].scalar(),
                )),
                MaterialInstruction::NoiseColor {
                    position,
                    scale,
                    detail,
                    roughness,
                } => {
                    let value = graph_fbm(
                        values[position.0].vector(),
                        values[detail.0].scalar(),
                        values[roughness.0].scalar(),
                        values[scale.0].scalar(),
                    );
                    RuntimeValue::Color([value, value, value, 1.0])
                }
                MaterialInstruction::Fbm {
                    position,
                    scale,
                    octaves,
                    roughness,
                } => RuntimeValue::Scalar(graph_fbm(
                    values[position.0].vector(),
                    values[octaves.0].scalar(),
                    values[roughness.0].scalar(),
                    values[scale.0].scalar(),
                )),
                MaterialInstruction::PassthroughScalar(value) => values[value.0],
                MaterialInstruction::PassthroughColor(value) => values[value.0],
                MaterialInstruction::PassthroughVector(value) => values[value.0],
                MaterialInstruction::Position => RuntimeValue::Vector3(context.position),
                MaterialInstruction::Normal => RuntimeValue::Vector3(context.normal),
                MaterialInstruction::VectorAdd(a, b) => {
                    RuntimeValue::Vector3(std::array::from_fn(|index| {
                        values[a.0].vector()[index] + values[b.0].vector()[index]
                    }))
                }
                MaterialInstruction::VectorScale { vector, scale } => RuntimeValue::Vector3(
                    values[vector.0]
                        .vector()
                        .map(|component| component * values[scale.0].scalar()),
                ),
                MaterialInstruction::NormalizeVector(vector) => {
                    RuntimeValue::Vector3(graph_safe_normalize(values[vector.0].vector()))
                }
                MaterialInstruction::DotVector(a, b) => RuntimeValue::Scalar(
                    values[a.0]
                        .vector()
                        .iter()
                        .zip(values[b.0].vector())
                        .map(|(a, b)| a * b)
                        .sum(),
                ),
                MaterialInstruction::Component { vector, axis } => {
                    RuntimeValue::Scalar(values[vector.0].vector()[axis as usize])
                }
            };
            values.push(value);
        }
        MaterialSample {
            base_color: values[self.output.base_color.0].color(),
            roughness: values[self.output.roughness.0].scalar(),
            emission: values[self.output.emission.0].color(),
        }
    }
}

impl RuntimeValue {
    fn scalar(self) -> f32 {
        match self {
            Self::Scalar(value) => value,
            _ => panic!("typed material IR requested a scalar from another value"),
        }
    }
    fn color(self) -> [f32; 4] {
        match self {
            Self::Color(value) => value,
            _ => panic!("typed material IR requested a color from another value"),
        }
    }
    fn vector(self) -> [f32; 3] {
        match self {
            Self::Vector3(value) => value,
            _ => panic!("typed material IR requested a vector from another value"),
        }
    }
}

fn graph_hash3(point: [f32; 3]) -> f32 {
    let dot = point[0] * 127.1 + point[1] * 311.7 + point[2] * 74.7;
    (dot.sin() * 43_758.547).fract().rem_euclid(1.0)
}

fn graph_value_noise(point: [f32; 3]) -> f32 {
    let cell = point.map(f32::floor);
    let local = point.map(f32::fract);
    let smooth = local.map(|value| value * value * (3.0 - 2.0 * value));
    let sample = |x: f32, y: f32, z: f32| graph_hash3([cell[0] + x, cell[1] + y, cell[2] + z]);
    let x00 = sample(0.0, 0.0, 0.0) * (1.0 - smooth[0]) + sample(1.0, 0.0, 0.0) * smooth[0];
    let x10 = sample(0.0, 1.0, 0.0) * (1.0 - smooth[0]) + sample(1.0, 1.0, 0.0) * smooth[0];
    let x01 = sample(0.0, 0.0, 1.0) * (1.0 - smooth[0]) + sample(1.0, 0.0, 1.0) * smooth[0];
    let x11 = sample(0.0, 1.0, 1.0) * (1.0 - smooth[0]) + sample(1.0, 1.0, 1.0) * smooth[0];
    let y0 = x00 * (1.0 - smooth[1]) + x10 * smooth[1];
    let y1 = x01 * (1.0 - smooth[1]) + x11 * smooth[1];
    y0 * (1.0 - smooth[2]) + y1 * smooth[2]
}

fn graph_fbm(position: [f32; 3], octaves: f32, roughness: f32, scale: f32) -> f32 {
    let mut total = 0.0;
    let mut amplitude = 1.0;
    let mut frequency = 1.0;
    let mut normalisation = 0.0;
    let octaves = octaves.max(1.0);
    for octave in 0..8 {
        if (octave as f32) < octaves {
            let point = position.map(|component| component * scale * frequency);
            total += graph_value_noise(point) * amplitude;
            normalisation += amplitude;
        }
        frequency *= 2.0;
        amplitude *= roughness.clamp(0.0, 1.0);
    }
    if normalisation > 0.0 {
        total / normalisation
    } else {
        0.0
    }
}

fn graph_safe_normalize(vector: [f32; 3]) -> [f32; 3] {
    let length = vector.iter().map(|value| value * value).sum::<f32>().sqrt();
    if length > 0.000001 {
        vector.map(|value| value / length)
    } else {
        [0.0, 1.0, 0.0]
    }
}

fn face_color(
    normal: [f32; 3],
    base: [f32; 4],
    top: [f32; 4],
    side: [f32; 4],
    bottom: [f32; 4],
) -> [f32; 4] {
    if normal[1] > 0.5 {
        top
    } else if normal[1] < -0.5 {
        bottom
    } else {
        let _ = base;
        side
    }
}

fn face_scalar(normal: [f32; 3], base: f32, top: f32, side: f32, bottom: f32) -> f32 {
    if normal[1] > 0.5 {
        top
    } else if normal[1] < -0.5 {
        bottom
    } else {
        let _ = base;
        side
    }
}

pub fn compile(
    graph: &GraphAsset,
    registry: &NodeRegistry,
) -> Result<MaterialGraphProgram, MaterialGraphError> {
    let resolved = graph.resolve(registry);
    let diagnostics: Vec<_> = resolved
        .diagnostics
        .into_iter()
        .filter(|item| item.severity == DiagnosticSeverity::Error)
        .collect();
    if !diagnostics.is_empty() {
        return Err(MaterialGraphError::InvalidGraph(diagnostics));
    }
    if graph.kind != crate::graph::GraphKind::Material {
        return Err(MaterialGraphError::WrongGraphKind);
    }
    let surface_chain = resolve_material_surface_chain(graph, registry).map_err(|error| {
        if let LayerGraphError::OutputCount { count } = error {
            MaterialGraphError::OutputCount(count)
        } else {
            MaterialGraphError::Surface(error)
        }
    })?;
    project_pattern_stack(graph, registry).map_err(MaterialGraphError::Surface)?;
    let mut lowerer = Lowerer {
        graph,
        registry,
        values: Vec::new(),
        cache: BTreeMap::new(),
    };
    let output = MaterialOutput {
        base_color: lowerer.input(&surface_chain.surface, "base_color")?,
        roughness: lowerer.input(&surface_chain.surface, "roughness")?,
        emission: lowerer.input(&surface_chain.surface, "emission")?,
    };
    lowerer.expect(output.base_color, ValueType::Color)?;
    lowerer.expect(output.roughness, ValueType::Scalar)?;
    lowerer.expect(output.emission, ValueType::Color)?;
    let wgsl = emit_wgsl(&lowerer.values, output);
    validate_wgsl(&wgsl)?;
    Ok(MaterialGraphProgram {
        graph_id: graph.id.clone(),
        semantic_hash: graph.resolve(registry).hashes.semantic,
        instructions: lowerer.values,
        output,
        wgsl,
    })
}

struct Lowerer<'a> {
    graph: &'a GraphAsset,
    registry: &'a NodeRegistry,
    values: Vec<MaterialInstruction>,
    cache: BTreeMap<(NodeId, SocketKey), ValueId>,
}
impl<'a> Lowerer<'a> {
    fn push(&mut self, value: MaterialInstruction) -> ValueId {
        let id = ValueId(self.values.len());
        self.values.push(value);
        id
    }
    fn expect(&self, value: ValueId, expected: ValueType) -> Result<(), MaterialGraphError> {
        let found = self.values[value.0].value_type();
        (found == expected)
            .then_some(())
            .ok_or(MaterialGraphError::TypeMismatch { expected, found })
    }
    fn input(&mut self, node: &NodeId, socket: &str) -> Result<ValueId, MaterialGraphError> {
        let key = SocketKey(socket.into());
        if let Some(link) = self
            .graph
            .links
            .values()
            .find(|link| link.to.node == *node && link.to.socket == key)
        {
            return self.output(&link.from.node, &link.from.socket);
        }
        let record = self
            .graph
            .nodes
            .get(node)
            .ok_or_else(|| MaterialGraphError::MissingNode(node.clone()))?;
        let value = record
            .socket_defaults
            .get(&key)
            .cloned()
            .or_else(|| {
                self.registry
                    .find(&record.node_type)
                    .and_then(|declaration| declaration.field(FieldTarget::InputSocket, socket))
                    .map(|field| field.default.value())
            })
            .ok_or_else(|| MaterialGraphError::MissingInputDefault {
                node: node.clone(),
                socket: key.clone(),
            })?;
        self.property_value(value)
    }

    fn field_value(
        &self,
        record: &NodeRecord,
        target: FieldTarget,
        key: &str,
    ) -> Result<PropertyValue, MaterialGraphError> {
        let authored = match target {
            FieldTarget::Property => record.properties.get(key),
            FieldTarget::InputSocket => record.socket_defaults.get(&SocketKey(key.to_string())),
        };
        authored
            .cloned()
            .or_else(|| {
                self.registry
                    .find(&record.node_type)
                    .and_then(|declaration| declaration.field(target, key))
                    .map(|field| field.default.value())
            })
            .ok_or_else(|| MaterialGraphError::MissingFieldDefault {
                node_type: record.node_type.clone(),
                field: key.to_string(),
            })
    }

    fn output(&mut self, node: &NodeId, socket: &SocketKey) -> Result<ValueId, MaterialGraphError> {
        if let Some(value) = self.cache.get(&(node.clone(), socket.clone())) {
            return Ok(*value);
        }
        let record = self
            .graph
            .nodes
            .get(node)
            .ok_or_else(|| MaterialGraphError::MissingNode(node.clone()))?
            .clone();
        let operation = self
            .registry
            .find(&record.node_type)
            .map(|declaration| declaration.operation)
            .ok_or_else(|| MaterialGraphError::UnsupportedNode(record.node_type.0.clone()))?;
        let NodeOperation::Material(operation) = operation else {
            return Err(MaterialGraphError::UnsupportedNode(record.node_type.0));
        };
        let value = match operation {
            MaterialNodeOperation::ConstantScalar | MaterialNodeOperation::ConstantColor => {
                let value = self.field_value(&record, FieldTarget::Property, "value")?;
                self.property_value(value)?
            }
            MaterialNodeOperation::AddScalar => {
                let a = self.input(node, "a")?;
                let b = self.input(node, "b")?;
                self.expect(a, ValueType::Scalar)?;
                self.expect(b, ValueType::Scalar)?;
                self.push(MaterialInstruction::AddScalar(a, b))
            }
            MaterialNodeOperation::MixColor => {
                let a = self.input(node, "a")?;
                let b = self.input(node, "b")?;
                let factor = self.input(node, "factor")?;
                self.expect(a, ValueType::Color)?;
                self.expect(b, ValueType::Color)?;
                self.expect(factor, ValueType::Scalar)?;
                self.push(MaterialInstruction::MixColor { a, b, factor })
            }
            MaterialNodeOperation::EmissionStrength => {
                let color = self.input(node, "color")?;
                let strength = self.input(node, "strength")?;
                self.expect(color, ValueType::Color)?;
                self.expect(strength, ValueType::Scalar)?;
                self.push(MaterialInstruction::ColorScale { color, strength })
            }
            MaterialNodeOperation::FaceColor => {
                let base = self.input(node, "base")?;
                let top = self.input(node, "top")?;
                let side = self.input(node, "side")?;
                let bottom = self.input(node, "bottom")?;
                for input in [base, top, side, bottom] {
                    self.expect(input, ValueType::Color)?;
                }
                self.push(MaterialInstruction::FaceColor {
                    base,
                    top,
                    side,
                    bottom,
                })
            }
            MaterialNodeOperation::FaceRoughness => {
                let base = self.input(node, "base")?;
                let top = self.input(node, "top")?;
                let side = self.input(node, "side")?;
                let bottom = self.input(node, "bottom")?;
                for input in [base, top, side, bottom] {
                    self.expect(input, ValueType::Scalar)?;
                }
                self.push(MaterialInstruction::FaceScalar {
                    base,
                    top,
                    side,
                    bottom,
                })
            }
            MaterialNodeOperation::RemapScalar => {
                let value = self.input(node, "value")?;
                let from_min = self.input(node, "from_min")?;
                let from_max = self.input(node, "from_max")?;
                let to_min = self.input(node, "to_min")?;
                let to_max = self.input(node, "to_max")?;
                for input in [value, from_min, from_max, to_min, to_max] {
                    self.expect(input, ValueType::Scalar)?;
                }
                let clamp = match self.field_value(&record, FieldTarget::Property, "clamp")? {
                    PropertyValue::Boolean(value) => value,
                    value => return Err(MaterialGraphError::InvalidProperty(value)),
                };
                self.push(MaterialInstruction::RemapScalar {
                    value,
                    from_min,
                    from_max,
                    to_min,
                    to_max,
                    clamp,
                })
            }
            MaterialNodeOperation::Noise => {
                let position = self.input(node, "position")?;
                let scale = self.input(node, "scale")?;
                let detail = self.input(node, "detail")?;
                let roughness = self.input(node, "roughness")?;
                self.expect(position, ValueType::Vector3)?;
                for input in [scale, detail, roughness] {
                    self.expect(input, ValueType::Scalar)?;
                }
                if socket.0 == "color" {
                    self.push(MaterialInstruction::NoiseColor {
                        position,
                        scale,
                        detail,
                        roughness,
                    })
                } else {
                    self.push(MaterialInstruction::Noise {
                        position,
                        scale,
                        detail,
                        roughness,
                    })
                }
            }
            MaterialNodeOperation::Fbm => {
                let position = self.input(node, "position")?;
                let scale = self.input(node, "scale")?;
                let octaves = self.input(node, "octaves")?;
                let roughness = self.input(node, "roughness")?;
                self.expect(position, ValueType::Vector3)?;
                for input in [scale, octaves, roughness] {
                    self.expect(input, ValueType::Scalar)?;
                }
                self.push(MaterialInstruction::Fbm {
                    position,
                    scale,
                    octaves,
                    roughness,
                })
            }
            MaterialNodeOperation::ColorRamp => {
                let factor = self.input(node, "factor")?;
                let color_a = self.input(node, "color_a")?;
                let color_b = self.input(node, "color_b")?;
                let position_a = self.input(node, "position_a")?;
                let position_b = self.input(node, "position_b")?;
                self.expect(factor, ValueType::Scalar)?;
                self.expect(color_a, ValueType::Color)?;
                self.expect(color_b, ValueType::Color)?;
                self.expect(position_a, ValueType::Scalar)?;
                self.expect(position_b, ValueType::Scalar)?;
                self.push(MaterialInstruction::ColorRamp {
                    factor,
                    color_a,
                    color_b,
                    position_a,
                    position_b,
                })
            }
            MaterialNodeOperation::ClampScalar => {
                let value = self.input(node, "value")?;
                let minimum = self.input(node, "minimum")?;
                let maximum = self.input(node, "maximum")?;
                self.expect(value, ValueType::Scalar)?;
                self.expect(minimum, ValueType::Scalar)?;
                self.expect(maximum, ValueType::Scalar)?;
                self.push(MaterialInstruction::ClampScalar {
                    value,
                    minimum,
                    maximum,
                })
            }
            MaterialNodeOperation::Position => self.push(MaterialInstruction::Position),
            MaterialNodeOperation::Normal => self.push(MaterialInstruction::Normal),
            MaterialNodeOperation::VectorAdd => {
                let a = self.input(node, "a")?;
                let b = self.input(node, "b")?;
                self.expect(a, ValueType::Vector3)?;
                self.expect(b, ValueType::Vector3)?;
                self.push(MaterialInstruction::VectorAdd(a, b))
            }
            MaterialNodeOperation::VectorScale => {
                let vector = self.input(node, "vector")?;
                let scale = self.input(node, "scale")?;
                self.expect(vector, ValueType::Vector3)?;
                self.expect(scale, ValueType::Scalar)?;
                self.push(MaterialInstruction::VectorScale { vector, scale })
            }
            MaterialNodeOperation::NormalizeVector => {
                let vector = self.input(node, "vector")?;
                self.expect(vector, ValueType::Vector3)?;
                self.push(MaterialInstruction::NormalizeVector(vector))
            }
            MaterialNodeOperation::DotVector => {
                let a = self.input(node, "a")?;
                let b = self.input(node, "b")?;
                self.expect(a, ValueType::Vector3)?;
                self.expect(b, ValueType::Vector3)?;
                self.push(MaterialInstruction::DotVector(a, b))
            }
            MaterialNodeOperation::PositionComponent => {
                let axis = self.component_axis(&record)?;
                let vector = self.push(MaterialInstruction::Position);
                self.push(MaterialInstruction::Component { vector, axis })
            }
            MaterialNodeOperation::NormalComponent => {
                let axis = self.component_axis(&record)?;
                let vector = self.push(MaterialInstruction::Normal);
                self.push(MaterialInstruction::Component { vector, axis })
            }
            MaterialNodeOperation::PassthroughScalar | MaterialNodeOperation::RerouteScalar => {
                let value = self.input(node, "value")?;
                self.expect(value, ValueType::Scalar)?;
                self.push(MaterialInstruction::PassthroughScalar(value))
            }
            MaterialNodeOperation::RerouteColor => {
                let value = self.input(node, "value")?;
                self.expect(value, ValueType::Color)?;
                self.push(MaterialInstruction::PassthroughColor(value))
            }
            MaterialNodeOperation::RerouteVector => {
                let value = self.input(node, "vector")?;
                self.expect(value, ValueType::Vector3)?;
                self.push(MaterialInstruction::PassthroughVector(value))
            }
            MaterialNodeOperation::Output
            | MaterialNodeOperation::Surface
            | MaterialNodeOperation::PatternLayer
            | MaterialNodeOperation::PatternFlat
            | MaterialNodeOperation::PatternNoise
            | MaterialNodeOperation::PatternSpeckle => {
                return Err(MaterialGraphError::UnsupportedNode(record.node_type.0))
            }
        };
        self.cache.insert((node.clone(), socket.clone()), value);
        Ok(value)
    }
    fn property_value(&mut self, value: PropertyValue) -> Result<ValueId, MaterialGraphError> {
        match value {
            PropertyValue::Scalar(value) if value.is_finite() => {
                Ok(self.push(MaterialInstruction::Scalar(value)))
            }
            PropertyValue::Color(value) if value.iter().all(|value| value.is_finite()) => {
                Ok(self.push(MaterialInstruction::Color(value)))
            }
            PropertyValue::Vector3(value) if value.iter().all(|value| value.is_finite()) => {
                Ok(self.push(MaterialInstruction::Vector3(value)))
            }
            value => Err(MaterialGraphError::InvalidProperty(value)),
        }
    }

    fn component_axis(&self, record: &NodeRecord) -> Result<u8, MaterialGraphError> {
        match self.field_value(record, FieldTarget::Property, "axis")? {
            PropertyValue::Integer(value) => Ok(value.clamp(0, 2) as u8),
            value => Err(MaterialGraphError::InvalidProperty(value)),
        }
    }
}

fn emit_wgsl(values: &[MaterialInstruction], output: MaterialOutput) -> String {
    let face_color_active = values
        .iter()
        .any(|value| matches!(value, MaterialInstruction::FaceColor { .. }));
    let face_roughness_active = values
        .iter()
        .any(|value| matches!(value, MaterialInstruction::FaceScalar { .. }));
    let mut source = format!(
        "{GRAPH_MATERIAL_STRUCT}fn graph_material(position: vec3<f32>, normal: vec3<f32>) -> GraphMaterial {{\n"
    );
    for (index, value) in values.iter().enumerate() {
        let value_name = |value: ValueId| format!("v{}", value.0);
        let expression = match value {
            MaterialInstruction::Scalar(value) => format_float(*value),
            MaterialInstruction::Color(value) => format!(
                "vec4<f32>({}, {}, {}, {})",
                format_float(value[0]),
                format_float(value[1]),
                format_float(value[2]),
                format_float(value[3])
            ),
            MaterialInstruction::Vector3(value) => format!(
                "vec3<f32>({}, {}, {})",
                format_float(value[0]),
                format_float(value[1]),
                format_float(value[2])
            ),
            MaterialInstruction::AddScalar(a, b) => {
                format!("{} + {}", value_name(*a), value_name(*b))
            }
            MaterialInstruction::RemapScalar {
                value,
                from_min,
                from_max,
                to_min,
                to_max,
                clamp,
            } => {
                let ratio = format!(
                    "select(({} - {}) / ({} - {}), 0.0, abs({} - {}) < 0.000001)",
                    value_name(*value),
                    value_name(*from_min),
                    value_name(*from_max),
                    value_name(*from_min),
                    value_name(*from_max),
                    value_name(*from_min)
                );
                let ratio = if *clamp {
                    format!("clamp({ratio}, 0.0, 1.0)")
                } else {
                    ratio
                };
                format!(
                    "{} + {} * ({} - {})",
                    value_name(*to_min),
                    ratio,
                    value_name(*to_max),
                    value_name(*to_min)
                )
            }
            MaterialInstruction::MixColor { a, b, factor } => format!(
                "mix({}, {}, {})",
                value_name(*a),
                value_name(*b),
                value_name(*factor)
            ),
            MaterialInstruction::ColorScale { color, strength } => {
                format!("{} * {}", value_name(*color), value_name(*strength))
            }
            MaterialInstruction::FaceColor {
                base,
                top,
                side,
                bottom,
            } => format!(
                "graph_face_color(normal, {}, {}, {}, {})",
                value_name(*base),
                value_name(*top),
                value_name(*side),
                value_name(*bottom)
            ),
            MaterialInstruction::FaceScalar {
                base,
                top,
                side,
                bottom,
            } => format!(
                "graph_face_scalar(normal, {}, {}, {}, {})",
                value_name(*base),
                value_name(*top),
                value_name(*side),
                value_name(*bottom)
            ),
            MaterialInstruction::ColorRamp {
                factor,
                color_a,
                color_b,
                position_a,
                position_b,
            } => format!(
                "mix({}, {}, clamp(({} - {}) / max({} - {}, 0.000001), 0.0, 1.0))",
                value_name(*color_a),
                value_name(*color_b),
                value_name(*factor),
                value_name(*position_a),
                value_name(*position_b),
                value_name(*position_a)
            ),
            MaterialInstruction::ClampScalar {
                value,
                minimum,
                maximum,
            } => format!(
                "clamp({}, {}, {})",
                value_name(*value),
                value_name(*minimum),
                value_name(*maximum)
            ),
            MaterialInstruction::Noise {
                position,
                scale,
                detail,
                roughness,
            } => format!(
                "graph_fbm({} * {}, {}, {})",
                value_name(*position),
                value_name(*scale),
                value_name(*detail),
                value_name(*roughness)
            ),
            MaterialInstruction::NoiseColor {
                position,
                scale,
                detail,
                roughness,
            } => format!(
                "vec4<f32>(graph_fbm({} * {}, {}, {}), graph_fbm({} * {}, {}, {}), graph_fbm({} * {}, {}, {}), 1.0)",
                value_name(*position),
                value_name(*scale),
                value_name(*detail),
                value_name(*roughness),
                value_name(*position),
                value_name(*scale),
                value_name(*detail),
                value_name(*roughness),
                value_name(*position),
                value_name(*scale),
                value_name(*detail),
                value_name(*roughness)
            ),
            MaterialInstruction::Fbm {
                position,
                scale,
                octaves,
                roughness,
            } => format!(
                "graph_fbm({} * {}, {}, {})",
                value_name(*position),
                value_name(*scale),
                value_name(*octaves),
                value_name(*roughness)
            ),
            MaterialInstruction::PassthroughScalar(value)
            | MaterialInstruction::PassthroughColor(value)
            | MaterialInstruction::PassthroughVector(value) => value_name(*value),
            MaterialInstruction::Position => "position".into(),
            MaterialInstruction::Normal => "normal".into(),
            MaterialInstruction::VectorAdd(a, b) => {
                format!("{} + {}", value_name(*a), value_name(*b))
            }
            MaterialInstruction::VectorScale { vector, scale } => {
                format!("{} * {}", value_name(*vector), value_name(*scale))
            }
            MaterialInstruction::NormalizeVector(vector) => {
                format!("graph_safe_normalize({})", value_name(*vector))
            }
            MaterialInstruction::DotVector(a, b) => {
                format!("dot({}, {})", value_name(*a), value_name(*b))
            }
            MaterialInstruction::Component { vector, axis } => {
                format!("{}.{}", value_name(*vector), ["x", "y", "z"][*axis as usize])
            }
        };
        let ty = match value.value_type() {
            ValueType::Scalar => "f32",
            ValueType::Color => "vec4<f32>",
            ValueType::Vector3 => "vec3<f32>",
        };
        source.push_str(&format!("  let v{index}: {ty} = {expression};\n"));
    }
    source.push_str(&format!(
        "  return GraphMaterial(v{}, v{}, v{}, true, {}, {});\n}}\n",
        output.base_color.0,
        output.roughness.0,
        output.emission.0,
        face_color_active,
        face_roughness_active,
    ));
    source
}
fn format_float(value: f32) -> String {
    if value.fract() == 0.0 {
        format!("{value:.1}")
    } else {
        value.to_string()
    }
}
fn validate_wgsl(source: &str) -> Result<(), MaterialGraphError> {
    let module = naga::front::wgsl::parse_str(source)
        .map_err(|error| MaterialGraphError::Wgsl(error.emit_to_string(source)))?;
    naga::valid::Validator::new(
        naga::valid::ValidationFlags::all(),
        naga::valid::Capabilities::all(),
    )
    .validate(&module)
    .map_err(|error| MaterialGraphError::Wgsl(error.to_string()))?;
    Ok(())
}

#[derive(Debug)]
pub enum MaterialGraphError {
    WrongGraphKind,
    InvalidGraph(Vec<crate::graph::Diagnostic>),
    OutputCount(usize),
    Surface(LayerGraphError),
    MissingNode(NodeId),
    MissingInputDefault {
        node: NodeId,
        socket: SocketKey,
    },
    MissingFieldDefault {
        node_type: NodeTypeId,
        field: String,
    },
    UnsupportedNode(String),
    TypeMismatch {
        expected: ValueType,
        found: ValueType,
    },
    InvalidProperty(PropertyValue),
    Wgsl(String),
}

/// UI state for the first Graph Studio canvas. The semantic graph remains
/// authoritative; egui only edits it through [`GraphCommand`].
#[derive(Clone, Debug)]
pub enum ConnectorDrag {
    /// The pointer started on an output and is looking for an input.
    FromOutput(OutputPin),
    /// The pointer started on an input and is looking for an output.
    FromInput(InputPin),
}

#[derive(Clone, Debug)]
struct GraphClipboardFragment {
    nodes: BTreeMap<NodeId, NodeRecord>,
    positions: BTreeMap<NodeId, [f32; 2]>,
    links: Vec<LinkRecord>,
    anchor: [f32; 2],
}

pub struct GraphEditorState {
    pub visible: bool,
    /// Height of the expanded bottom drawer in logical pixels.
    pub drawer_height: f32,
    pub drawer_resize_last_y: Option<f32>,
    pub material_slot: u8,
    /// Set by the Graph Studio material picker; the platform layer loads or
    /// creates the corresponding graph after the UI frame finishes.
    pub material_select_requested: Option<u8>,
    pub graph: GraphAsset,
    pub history: GraphHistory,
    pub selected_node: Option<NodeId>,
    /// All selected nodes. `selected_node` remains the active/inspected node
    /// for compatibility with the inspector and material bridge.
    pub selected_nodes: BTreeSet<NodeId>,
    pub search: String,
    pub node_type: String,
    pub pan: [f32; 2],
    pub zoom: f32,
    /// Latched middle-button canvas navigation. The latch lets a pan continue
    /// when the pointer crosses a node or briefly leaves the canvas bounds.
    pub canvas_middle_pan_active: bool,
    pub canvas_middle_pan_last_pointer: Option<[f32; 2]>,
    pub pending_output: Option<OutputPin>,
    pub connector_drag: Option<ConnectorDrag>,
    pub connector_menu_filter: String,
    pub connector_menu_position: Option<[f32; 2]>,
    pub status: String,
    pub diagnostics: Vec<Diagnostic>,
    pub compile_requested: bool,
    pub save_requested: bool,
    pub open_requested: bool,
    pub duplicate_requested: bool,
    pub reset_requested: bool,
    pub undo_requested: bool,
    pub redo_requested: bool,
    pub drag_start_positions: BTreeMap<NodeId, [f32; 2]>,
    pub dragging_node: Option<NodeId>,
    pub drag_pointer_start: Option<[f32; 2]>,
    pub box_select_start: Option<[f32; 2]>,
    pub box_select_current: Option<[f32; 2]>,
    pub frame_all_requested: bool,
    pub frame_selection_requested: bool,
    pub collapsed_nodes: BTreeSet<NodeId>,
    clipboard: Option<GraphClipboardFragment>,
}

impl GraphEditorState {
    pub fn new(material_slot: u8) -> Self {
        Self {
            visible: false,
            // Keep the editor compact on first open; the user can grow it from
            // the top edge when more graph space is needed.
            drawer_height: 280.0,
            drawer_resize_last_y: None,
            material_slot,
            material_select_requested: None,
            graph: new_material_graph("Material Graph"),
            history: GraphHistory::default(),
            selected_node: None,
            selected_nodes: BTreeSet::new(),
            search: String::new(),
            node_type: "material.output".to_string(),
            pan: [0.0, 0.0],
            zoom: 1.0,
            canvas_middle_pan_active: false,
            canvas_middle_pan_last_pointer: None,
            pending_output: None,
            connector_drag: None,
            connector_menu_filter: String::new(),
            connector_menu_position: None,
            status: "Create a node graph or open a saved graph".to_string(),
            diagnostics: Vec::new(),
            compile_requested: false,
            save_requested: false,
            open_requested: false,
            duplicate_requested: false,
            reset_requested: false,
            undo_requested: false,
            redo_requested: false,
            drag_start_positions: BTreeMap::new(),
            dragging_node: None,
            drag_pointer_start: None,
            box_select_start: None,
            box_select_current: None,
            frame_all_requested: false,
            frame_selection_requested: false,
            collapsed_nodes: BTreeSet::new(),
            clipboard: None,
        }
    }

    pub fn apply(&mut self, command: GraphCommand, registry: &NodeRegistry) -> bool {
        match self.history.apply(&mut self.graph, registry, command) {
            Ok(_) => {
                self.compile_requested = true;
                self.status = "Graph changed — compiling".to_string();
                true
            }
            Err(error) => {
                self.status = error.to_string();
                false
            }
        }
    }

    pub fn undo(&mut self, registry: &NodeRegistry) {
        match self.history.undo(&mut self.graph, registry) {
            Ok(Some(_)) => self.compile_requested = true,
            Ok(None) => self.status = "Nothing to undo".to_string(),
            Err(error) => self.status = error.to_string(),
        }
    }

    pub fn redo(&mut self, registry: &NodeRegistry) {
        match self.history.redo(&mut self.graph, registry) {
            Ok(Some(_)) => self.compile_requested = true,
            Ok(None) => self.status = "Nothing to redo".to_string(),
            Err(error) => self.status = error.to_string(),
        }
    }

    pub fn open_graph(&mut self, material_slot: u8, graph: GraphAsset) {
        self.material_slot = material_slot;
        self.material_select_requested = None;
        self.graph = graph;
        self.history = GraphHistory::default();
        self.selected_node = None;
        self.selected_nodes.clear();
        self.pending_output = None;
        self.canvas_middle_pan_active = false;
        self.canvas_middle_pan_last_pointer = None;
        self.drawer_resize_last_y = None;
        self.connector_drag = None;
        self.connector_menu_filter.clear();
        self.connector_menu_position = None;
        self.dragging_node = None;
        self.drag_pointer_start = None;
        self.drag_start_positions.clear();
        self.box_select_start = None;
        self.box_select_current = None;
        self.frame_all_requested = false;
        self.frame_selection_requested = false;
        self.collapsed_nodes.clear();
        self.clipboard = None;
        self.diagnostics.clear();
        self.compile_requested = true;
        self.status = "Graph opened — compiling".to_string();
    }

    pub fn reset_graph(&mut self) {
        self.graph = new_material_graph("Material Graph");
        self.history = GraphHistory::default();
        self.selected_node = None;
        self.selected_nodes.clear();
        self.pending_output = None;
        self.canvas_middle_pan_active = false;
        self.canvas_middle_pan_last_pointer = None;
        self.drawer_resize_last_y = None;
        self.material_select_requested = None;
        self.connector_drag = None;
        self.connector_menu_filter.clear();
        self.connector_menu_position = None;
        self.dragging_node = None;
        self.drag_pointer_start = None;
        self.drag_start_positions.clear();
        self.box_select_start = None;
        self.box_select_current = None;
        self.frame_all_requested = false;
        self.frame_selection_requested = false;
        self.collapsed_nodes.clear();
        self.clipboard = None;
        self.diagnostics.clear();
        self.compile_requested = true;
        self.status = "New graph".to_string();
    }

    pub fn add_node(&mut self, node_type: NodeTypeId, registry: &NodeRegistry) {
        if registry.find(&node_type).is_some_and(|declaration| {
            declaration.operation == NodeOperation::Material(MaterialNodeOperation::PatternLayer)
        }) {
            self.add_pattern_layer(registry);
            return;
        }
        let column = self.graph.nodes.len() % 4;
        let row = self.graph.nodes.len() / 4;
        let position = [80.0 + column as f32 * 250.0, 80.0 + row as f32 * 180.0];
        self.add_node_at(node_type, position, registry);
    }

    /// Add a node at a graph-space position. Connector-drop insertion uses this
    /// instead of the toolbar's deterministic grid placement.
    pub fn add_node_at(
        &mut self,
        node_type: NodeTypeId,
        position: [f32; 2],
        registry: &NodeRegistry,
    ) -> Option<NodeId> {
        let id = NodeId::new();
        if !self.apply(
            GraphCommand::AddNode {
                id: id.clone(),
                node_type: node_type.clone(),
                position,
            },
            registry,
        ) {
            return None;
        }
        self.selected_node = Some(id.clone());
        self.selected_nodes.clear();
        self.selected_nodes.insert(id.clone());
        Some(id)
    }

    /// Insert a pattern node immediately before Material Output. The semantic
    /// operation and typed surface sockets come from the registry; the editor
    /// only orchestrates graph commands.
    pub fn add_pattern_layer(&mut self, registry: &NodeRegistry) -> Option<NodeId> {
        let chain = match resolve_material_surface_chain(&self.graph, registry) {
            Ok(chain) => chain,
            Err(error) => {
                self.status = error.to_string();
                return None;
            }
        };
        if chain.layers.len() >= MAX_PATTERN_LAYERS {
            self.status = format!("A material supports at most {MAX_PATTERN_LAYERS} layers");
            return None;
        }
        let predecessor = chain.layers.last().unwrap_or(&chain.surface).clone();
        let Some(link_id) = self
            .graph
            .links
            .iter()
            .find(|(_, link)| {
                link.from.node == predecessor
                    && link.from.socket.0 == "surface"
                    && link.to.node == chain.output
                    && link.to.socket.0 == "surface"
            })
            .map(|(id, _)| id.clone())
        else {
            self.status = "The material surface chain is incomplete".to_string();
            return None;
        };
        let predecessor_position = self
            .graph
            .layout
            .positions
            .get(&predecessor)
            .copied()
            .unwrap_or([440.0, 160.0]);
        let output_position = self
            .graph
            .layout
            .positions
            .get(&chain.output)
            .copied()
            .unwrap_or([predecessor_position[0] + 260.0, predecessor_position[1]]);
        let layer_position = [
            predecessor_position[0] + 260.0,
            predecessor_position[1].min(output_position[1]),
        ];
        let layer = NodeId::new();
        let generator = NodeId::new();
        let mut commands = vec![
            GraphCommand::Disconnect { id: link_id },
            GraphCommand::AddNode {
                id: layer.clone(),
                node_type: NodeTypeId(PATTERN_LAYER_NODE.to_string()),
                position: layer_position,
            },
            GraphCommand::AddNode {
                id: generator.clone(),
                node_type: NodeTypeId(PATTERN_NOISE_NODE.to_string()),
                position: [layer_position[0], layer_position[1] - 280.0],
            },
            GraphCommand::Connect {
                id: LinkId::new(),
                from: OutputPin {
                    node: generator.clone(),
                    socket: SocketKey("pattern".into()),
                },
                to: InputPin {
                    node: layer.clone(),
                    socket: SocketKey("pattern".into()),
                },
            },
            GraphCommand::Connect {
                id: LinkId::new(),
                from: OutputPin {
                    node: predecessor,
                    socket: SocketKey("surface".into()),
                },
                to: InputPin {
                    node: layer.clone(),
                    socket: SocketKey("surface".into()),
                },
            },
            GraphCommand::Connect {
                id: LinkId::new(),
                from: OutputPin {
                    node: layer.clone(),
                    socket: SocketKey("surface".into()),
                },
                to: InputPin {
                    node: chain.output.clone(),
                    socket: SocketKey("surface".into()),
                },
            },
        ];
        if output_position[0] < layer_position[0] + 240.0 {
            commands.push(GraphCommand::MoveNodes {
                positions: vec![(
                    chain.output,
                    [layer_position[0] + 260.0, output_position[1]],
                )],
            });
        }
        if !self.apply(GraphCommand::Transaction { commands }, registry) {
            return None;
        }
        self.selected_node = Some(layer.clone());
        self.selected_nodes.clear();
        self.selected_nodes.insert(layer.clone());
        self.status = format!("Layer {} added", chain.layers.len() + 1);
        Some(layer)
    }

    pub fn remove_nodes(&mut self, mut nodes: Vec<NodeId>, registry: &NodeRegistry) {
        let mut removed: BTreeSet<_> = nodes.iter().cloned().collect();
        let chain = resolve_material_surface_chain(&self.graph, registry).ok();
        let reconnect = chain.filter(|chain| {
            removed
                .iter()
                .all(|node| chain.layers.iter().any(|layer| layer == node))
        });
        if let Some(chain) = reconnect {
            let orphaned_generators = self
                .graph
                .links
                .values()
                .filter(|link| link.to.socket.0 == "pattern" && removed.contains(&link.to.node))
                .map(|link| link.from.node.clone())
                .filter(|generator| {
                    self.graph.links.values().all(|link| {
                        link.from.node != *generator
                            || link.from.socket.0 != "pattern"
                            || removed.contains(&link.to.node)
                    })
                })
                .collect::<Vec<_>>();
            for generator in orphaned_generators {
                if removed.insert(generator.clone()) {
                    nodes.push(generator);
                }
            }
            let chain_nodes = std::iter::once(&chain.surface)
                .chain(chain.layers.iter())
                .chain(std::iter::once(&chain.output))
                .cloned()
                .collect::<BTreeSet<_>>();
            let links = self
                .graph
                .links
                .iter()
                .filter(|(_, link)| {
                    chain_nodes.contains(&link.from.node)
                        && chain_nodes.contains(&link.to.node)
                        && link.from.socket.0 == "surface"
                        && link.to.socket.0 == "surface"
                })
                .map(|(id, _)| id.clone())
                .collect::<Vec<_>>();
            let surface = chain.surface.clone();
            let output = chain.output.clone();
            let remaining_layers = chain
                .layers
                .iter()
                .filter(|node| !removed.contains(*node))
                .cloned()
                .collect::<Vec<_>>();
            let mut commands = Vec::new();
            for id in links {
                commands.push(GraphCommand::Disconnect { id });
            }
            commands.push(GraphCommand::RemoveNodes { nodes });
            let mut previous = surface;
            for layer in remaining_layers {
                commands.push(Self::connect_surface_command(previous, layer.clone()));
                previous = layer;
            }
            commands.push(Self::connect_surface_command(previous, output));
            self.apply(GraphCommand::Transaction { commands }, registry);
        } else {
            self.apply(GraphCommand::RemoveNodes { nodes }, registry);
        }
    }

    fn connect_surface_command(from: NodeId, to: NodeId) -> GraphCommand {
        GraphCommand::Connect {
            id: LinkId::new(),
            from: OutputPin {
                node: from,
                socket: SocketKey("surface".into()),
            },
            to: InputPin {
                node: to,
                socket: SocketKey("surface".into()),
            },
        }
    }

    pub fn visible_node_types(
        &self,
        registry: &NodeRegistry,
    ) -> Vec<&'static crate::graph::NodeDeclaration> {
        registry
            .declarations()
            .iter()
            .filter(|node| {
                node.kinds.contains(&self.graph.kind)
                    && self
                        .graph
                        .can_add_node_type(registry, &NodeTypeId(node.id.into()))
                    && (self.search.is_empty()
                        || format!("{} {} {}", node.id, node.title, node.category.label())
                            .to_ascii_lowercase()
                            .contains(&self.search.to_ascii_lowercase()))
            })
            .collect()
    }

    pub fn copy_selected(&mut self) -> bool {
        let mut ids = self.selected_nodes.clone();
        if ids.is_empty() {
            if let Some(node_id) = self.selected_node.clone() {
                ids.insert(node_id);
            }
        }
        if ids.is_empty() {
            self.status = "Select a node to copy".to_string();
            return false;
        }
        let nodes = ids
            .iter()
            .filter_map(|id| {
                self.graph
                    .nodes
                    .get(id)
                    .cloned()
                    .map(|record| (id.clone(), record))
            })
            .collect::<BTreeMap<_, _>>();
        if nodes.len() != ids.len() {
            self.status = "Selection contains a missing node".to_string();
            return false;
        }
        let positions = ids
            .iter()
            .map(|id| {
                (
                    id.clone(),
                    self.graph
                        .layout
                        .positions
                        .get(id)
                        .copied()
                        .unwrap_or([0.0, 0.0]),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let anchor = positions
            .values()
            .fold([f32::INFINITY; 2], |mut anchor, position| {
                anchor[0] = anchor[0].min(position[0]);
                anchor[1] = anchor[1].min(position[1]);
                anchor
            });
        let links = self
            .graph
            .links
            .values()
            .filter(|link| ids.contains(&link.from.node) && ids.contains(&link.to.node))
            .cloned()
            .collect();
        self.clipboard = Some(GraphClipboardFragment {
            nodes,
            positions,
            links,
            anchor,
        });
        self.status = format!(
            "{} node{} copied",
            ids.len(),
            if ids.len() == 1 { "" } else { "s" }
        );
        true
    }

    pub fn can_paste(&self) -> bool {
        self.clipboard.is_some()
    }

    pub fn paste_clipboard(&mut self, registry: &NodeRegistry) -> Option<NodeId> {
        let clipboard = self.clipboard.clone()?;
        let mut remap = BTreeMap::new();
        let mut new_ids = BTreeSet::new();
        let mut commands = Vec::new();
        for (old_id, record) in &clipboard.nodes {
            let id = NodeId::new();
            let old_position = clipboard
                .positions
                .get(old_id)
                .copied()
                .unwrap_or(clipboard.anchor);
            let position = [
                old_position[0] + 40.0 - clipboard.anchor[0],
                old_position[1] + 40.0 - clipboard.anchor[1],
            ];
            commands.push(GraphCommand::AddNode {
                id: id.clone(),
                node_type: record.node_type.clone(),
                position,
            });
            for (property, value) in &record.properties {
                commands.push(GraphCommand::SetProperty {
                    node: id.clone(),
                    property: property.clone(),
                    value: value.clone(),
                });
            }
            for (socket, value) in &record.socket_defaults {
                commands.push(GraphCommand::SetSocketDefault {
                    node: id.clone(),
                    socket: socket.clone(),
                    value: value.clone(),
                });
            }
            remap.insert(old_id.clone(), id.clone());
            new_ids.insert(id);
        }
        for link in &clipboard.links {
            let (Some(from_node), Some(to_node)) =
                (remap.get(&link.from.node), remap.get(&link.to.node))
            else {
                continue;
            };
            commands.push(GraphCommand::Connect {
                id: LinkId::new(),
                from: OutputPin {
                    node: from_node.clone(),
                    socket: link.from.socket.clone(),
                },
                to: InputPin {
                    node: to_node.clone(),
                    socket: link.to.socket.clone(),
                },
            });
        }
        if !self.apply(GraphCommand::Transaction { commands }, registry) {
            return None;
        }
        self.selected_nodes = new_ids.clone();
        self.selected_node = new_ids.iter().next().cloned();
        self.status = format!(
            "{} node{} pasted",
            new_ids.len(),
            if new_ids.len() == 1 { "" } else { "s" }
        );
        new_ids.iter().next().cloned()
    }
}
impl fmt::Display for MaterialGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(formatter, "material graph compilation failed: {self:?}")
    }
}
impl std::error::Error for MaterialGraphError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::graph::{GraphCommand, GraphHistory, GraphKind, LinkId, NodeTypeId, OutputPin};
    fn node(value: &str) -> NodeId {
        NodeId(value.into())
    }
    fn graph_with_output() -> (GraphAsset, NodeId) {
        let mut graph = GraphAsset::new("test", GraphKind::Material);
        let output = node("output");
        let surface = node("surface");
        graph.nodes.insert(
            output.clone(),
            crate::graph::NodeRecord {
                node_type: NodeTypeId("material.output".into()),
                node_type_version: 1,
                properties: BTreeMap::new(),
                socket_defaults: BTreeMap::new(),
                unknown_payload: None,
            },
        );
        graph.nodes.insert(
            surface.clone(),
            crate::graph::NodeRecord {
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
    #[test]
    fn one_ir_drives_cpu_preview_and_naga_valid_wgsl() {
        let registry = NodeRegistry;
        let (mut graph, output) = graph_with_output();
        graph
            .nodes
            .get_mut(&output)
            .unwrap()
            .socket_defaults
            .insert(
                SocketKey("base_color".into()),
                PropertyValue::Color([0.2, 0.4, 0.6, 1.0]),
            );
        graph
            .nodes
            .get_mut(&output)
            .unwrap()
            .socket_defaults
            .insert(SocketKey("roughness".into()), PropertyValue::Scalar(0.25));
        let program = compile(&graph, &registry).unwrap();
        let sample = program.evaluate(MaterialSampleContext {
            position: [1.0, 2.0, 3.0],
            normal: [0.0, 1.0, 0.0],
        });
        assert_eq!(sample.base_color, [0.2, 0.4, 0.6, 1.0]);
        assert_eq!(sample.roughness, 0.25);
        assert!(program.wgsl.contains("fn graph_material"));
    }
    #[test]
    fn linked_math_is_evaluated_once_in_both_backends() {
        let registry = NodeRegistry;
        let (mut graph, output) = graph_with_output();
        let mut history = GraphHistory::default();
        let add = node("add");
        history
            .apply(
                &mut graph,
                &registry,
                GraphCommand::AddNode {
                    id: add.clone(),
                    node_type: NodeTypeId("material.add_scalar".into()),
                    position: [0.0, 0.0],
                },
            )
            .unwrap();
        graph
            .nodes
            .get_mut(&add)
            .unwrap()
            .socket_defaults
            .insert(SocketKey("a".into()), PropertyValue::Scalar(0.2));
        graph
            .nodes
            .get_mut(&add)
            .unwrap()
            .socket_defaults
            .insert(SocketKey("b".into()), PropertyValue::Scalar(0.3));
        history
            .apply(
                &mut graph,
                &registry,
                GraphCommand::Connect {
                    id: crate::graph::LinkId("roughness".into()),
                    from: OutputPin {
                        node: add,
                        socket: SocketKey("value".into()),
                    },
                    to: crate::graph::InputPin {
                        node: output,
                        socket: SocketKey("roughness".into()),
                    },
                },
            )
            .unwrap();
        let program = compile(&graph, &registry).unwrap();
        assert_eq!(
            program
                .evaluate(MaterialSampleContext {
                    position: [0.0; 3],
                    normal: [0.0; 3]
                })
                .roughness,
            0.5
        );
    }

    #[test]
    fn invalid_edit_keeps_the_last_known_good_program_active() {
        let registry = NodeRegistry;
        let (mut graph, output) = graph_with_output();
        graph
            .nodes
            .get_mut(&output)
            .unwrap()
            .socket_defaults
            .insert(SocketKey("roughness".into()), PropertyValue::Scalar(0.2));
        let mut library = MaterialGraphLibrary::default();
        library.try_activate(&graph, &registry).unwrap();
        graph.nodes.remove(&output);
        assert!(library.try_activate(&graph, &registry).is_err());
        assert_eq!(
            library
                .active(&graph.id)
                .unwrap()
                .evaluate(MaterialSampleContext {
                    position: [0.0; 3],
                    normal: [0.0; 3]
                })
                .roughness,
            0.2
        );
    }

    #[test]
    fn graph_dispatch_injects_a_slot_branch_into_the_full_dda_source() {
        let registry = NodeRegistry;
        let (graph, _) = graph_with_output();
        let program = compile(&graph, &registry).unwrap();
        let mut set = MaterialGraphShaderSet::default();
        set.insert(6, program);
        let source = crate::passes::dda::build_shader_source_with_material_graphs(
            &crate::variants::RenderQuality::default(),
            &set,
        );
        assert!(source.contains("if (material == 6u)"));
        assert!(source.contains("fn graph_material_6("));
        let module = naga::front::wgsl::parse_str(&source).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn procedural_catalog_lowers_noise_fbm_ramp_and_vectors_for_preview_and_gpu() {
        let registry = NodeRegistry;
        let (mut graph, output) = graph_with_output();
        let position = node("position");
        let noise = node("noise");
        let ramp = node("ramp");
        let fbm = node("fbm");
        graph.nodes.extend([
            (
                position.clone(),
                crate::graph::NodeRecord {
                    node_type: NodeTypeId("material.position".into()),
                    node_type_version: 1,
                    properties: BTreeMap::new(),
                    socket_defaults: BTreeMap::new(),
                    unknown_payload: None,
                },
            ),
            (
                noise.clone(),
                crate::graph::NodeRecord {
                    node_type: NodeTypeId("material.noise".into()),
                    node_type_version: 1,
                    properties: BTreeMap::new(),
                    socket_defaults: BTreeMap::from([
                        (SocketKey("scale".into()), PropertyValue::Scalar(1.5)),
                        (SocketKey("detail".into()), PropertyValue::Scalar(3.0)),
                        (SocketKey("roughness".into()), PropertyValue::Scalar(0.55)),
                    ]),
                    unknown_payload: None,
                },
            ),
            (
                ramp.clone(),
                crate::graph::NodeRecord {
                    node_type: NodeTypeId("material.color_ramp".into()),
                    node_type_version: 1,
                    properties: BTreeMap::new(),
                    socket_defaults: BTreeMap::from([
                        (
                            SocketKey("color_a".into()),
                            PropertyValue::Color([0.02, 0.1, 0.01, 1.0]),
                        ),
                        (
                            SocketKey("color_b".into()),
                            PropertyValue::Color([0.5, 0.8, 0.08, 1.0]),
                        ),
                    ]),
                    unknown_payload: None,
                },
            ),
            (
                fbm.clone(),
                crate::graph::NodeRecord {
                    node_type: NodeTypeId("material.fbm".into()),
                    node_type_version: 1,
                    properties: BTreeMap::new(),
                    socket_defaults: BTreeMap::new(),
                    unknown_payload: None,
                },
            ),
        ]);
        graph.links.extend([
            (
                LinkId("position-noise".into()),
                crate::graph::LinkRecord {
                    from: OutputPin {
                        node: position.clone(),
                        socket: SocketKey("vector".into()),
                    },
                    to: InputPin {
                        node: noise.clone(),
                        socket: SocketKey("position".into()),
                    },
                    order: 0,
                },
            ),
            (
                LinkId("noise-ramp".into()),
                crate::graph::LinkRecord {
                    from: OutputPin {
                        node: noise.clone(),
                        socket: SocketKey("factor".into()),
                    },
                    to: InputPin {
                        node: ramp.clone(),
                        socket: SocketKey("factor".into()),
                    },
                    order: 0,
                },
            ),
            (
                LinkId("ramp-base".into()),
                crate::graph::LinkRecord {
                    from: OutputPin {
                        node: ramp.clone(),
                        socket: SocketKey("color".into()),
                    },
                    to: InputPin {
                        node: output.clone(),
                        socket: SocketKey("base_color".into()),
                    },
                    order: 0,
                },
            ),
            (
                LinkId("noise-emission".into()),
                crate::graph::LinkRecord {
                    from: OutputPin {
                        node: noise.clone(),
                        socket: SocketKey("color".into()),
                    },
                    to: InputPin {
                        node: output.clone(),
                        socket: SocketKey("emission".into()),
                    },
                    order: 0,
                },
            ),
            (
                LinkId("position-fbm".into()),
                crate::graph::LinkRecord {
                    from: OutputPin {
                        node: position,
                        socket: SocketKey("vector".into()),
                    },
                    to: InputPin {
                        node: fbm.clone(),
                        socket: SocketKey("position".into()),
                    },
                    order: 0,
                },
            ),
            (
                LinkId("fbm-roughness".into()),
                crate::graph::LinkRecord {
                    from: OutputPin {
                        node: fbm,
                        socket: SocketKey("value".into()),
                    },
                    to: InputPin {
                        node: output,
                        socket: SocketKey("roughness".into()),
                    },
                    order: 0,
                },
            ),
        ]);
        let program = compile(&graph, &registry).unwrap();
        let sample = program.evaluate(MaterialSampleContext {
            position: [1.25, 2.5, -0.75],
            normal: [0.0, 1.0, 0.0],
        });
        assert!(sample.base_color.iter().all(|value| value.is_finite()));
        assert!((0.0..=1.0).contains(&sample.roughness));
        assert!(program.wgsl.contains("graph_fbm"));

        let mut shaders = MaterialGraphShaderSet::default();
        shaders.insert(3, program.clone());
        shaders.insert(4, program);
        let source = crate::passes::dda::build_shader_source_with_material_graphs(
            &crate::variants::RenderQuality::default(),
            &shaders,
        );
        let module = naga::front::wgsl::parse_str(&source).unwrap();
        naga::valid::Validator::new(
            naga::valid::ValidationFlags::all(),
            naga::valid::Capabilities::all(),
        )
        .validate(&module)
        .unwrap();
    }

    #[test]
    fn canonical_material_graph_preserves_face_roles_in_graph_preview() {
        let registry = NodeRegistry;
        let material = crate::material::MATERIALS[1];
        let graph = graph_from_material(&material);
        assert_eq!(
            graph
                .nodes
                .values()
                .find(|node| node.node_type.0 == "material.face_color")
                .map(|_| true),
            Some(true)
        );
        let program = compile(&graph, &registry).unwrap();
        let top = program.evaluate(MaterialSampleContext {
            position: [0.0; 3],
            normal: [0.0, 1.0, 0.0],
        });
        let side = program.evaluate(MaterialSampleContext {
            position: [0.0; 3],
            normal: [1.0, 0.0, 0.0],
        });
        let roles = material.face_roles.unwrap();
        assert_eq!(top.base_color[0], roles.top.albedo[0]);
        assert_eq!(side.base_color[0], roles.side.albedo[0]);
        assert_eq!(top.roughness, roles.top.roughness);
        assert_eq!(side.roughness, roles.side.roughness);
    }

    #[test]
    fn every_compiled_material_has_an_openable_graph_representation() {
        let registry = NodeRegistry;
        for (slot, material) in crate::material::MATERIALS.iter().enumerate() {
            let graph = graph_from_material(material);
            let program = compile(&graph, &registry).unwrap_or_else(|error| {
                panic!("material slot {slot} ({}) failed: {error}", material.name)
            });
            let sample = program.evaluate(MaterialSampleContext {
                position: [0.25, 0.5, -0.75],
                normal: [0.0, 1.0, 0.0],
            });
            assert!(sample.base_color.iter().all(|value| value.is_finite()));
            assert!(sample.roughness.is_finite());
            assert!(sample.emission.iter().all(|value| value.is_finite()));
        }
    }

    #[test]
    fn editor_adds_nodes_through_commands_with_inspectable_defaults() {
        let registry = NodeRegistry;
        let mut editor = GraphEditorState::new(6);
        let baseline_nodes = editor.graph.nodes.len();
        editor.add_node(NodeTypeId("material.constant_scalar".into()), &registry);
        let node = editor.selected_node.clone().unwrap();
        assert_eq!(editor.graph.nodes.len(), baseline_nodes + 1);
        assert_eq!(
            editor.graph.nodes[&node].properties.get("value"),
            Some(&PropertyValue::Scalar(0.5))
        );
        editor.undo(&registry);
        assert_eq!(editor.graph.nodes.len(), baseline_nodes);
    }

    #[test]
    fn editor_adds_editable_defaults_for_procedural_nodes() {
        let registry = NodeRegistry;
        let mut editor = GraphEditorState::new(6);
        editor.add_node(NodeTypeId("material.noise".into()), &registry);
        let noise = editor.selected_node.clone().unwrap();
        assert_eq!(
            editor.graph.nodes[&noise].socket_defaults[&SocketKey("detail".into())],
            PropertyValue::Scalar(3.0)
        );
        editor.add_node(NodeTypeId("material.color_ramp".into()), &registry);
        let ramp = editor.selected_node.clone().unwrap();
        assert!(matches!(
            editor.graph.nodes[&ramp].socket_defaults[&SocketKey("color_a".into())],
            PropertyValue::Color(_)
        ));
    }

    #[test]
    fn node_catalog_hides_types_at_their_declared_instance_limit() {
        let registry = NodeRegistry::builtin();
        let editor = GraphEditorState::new(6);
        let visible = editor
            .visible_node_types(&registry)
            .into_iter()
            .map(|declaration| declaration.id)
            .collect::<BTreeSet<_>>();
        assert!(!visible.contains("material.output"));
        assert!(!visible.contains("material.surface"));
        assert!(visible.contains("material.pattern_layer"));
    }

    #[test]
    fn editor_inserts_and_removes_layers_in_the_typed_surface_chain() {
        let registry = NodeRegistry;
        let graph = graph_from_material(&crate::material::MATERIALS[6]);
        let mut editor = GraphEditorState::new(6);
        editor.open_graph(6, graph);

        let first = editor
            .add_pattern_layer(&registry)
            .expect("first layer inserts");
        let second = editor
            .add_pattern_layer(&registry)
            .expect("second layer inserts");
        let chain = resolve_material_surface_chain(&editor.graph, &registry).unwrap();
        assert_eq!(chain.layers, vec![first.clone(), second.clone()]);
        assert_eq!(
            editor.graph.nodes[&first].properties.get("enabled"),
            Some(&PropertyValue::Boolean(true))
        );
        let first_generator = editor
            .graph
            .links
            .values()
            .find(|link| link.to.node == first && link.to.socket.0 == "pattern")
            .map(|link| link.from.node.clone())
            .expect("new layer has a pattern source");
        assert_eq!(
            editor.graph.nodes[&first_generator].node_type.0,
            PATTERN_NOISE_NODE
        );

        editor.remove_nodes(vec![first], &registry);
        let chain = resolve_material_surface_chain(&editor.graph, &registry).unwrap();
        assert_eq!(chain.layers, vec![second]);
        assert!(!editor.graph.nodes.contains_key(&first_generator));
        compile(&editor.graph, &registry).unwrap();
    }

    #[test]
    fn layer_insertion_is_one_atomic_undoable_edit() {
        let registry = NodeRegistry::builtin();
        let graph = graph_from_material(&crate::material::MATERIALS[6]);
        let mut editor = GraphEditorState::new(6);
        editor.open_graph(6, graph);
        let baseline_nodes = editor.graph.nodes.len();

        editor.add_pattern_layer(&registry).unwrap();
        assert_eq!(
            resolve_material_surface_chain(&editor.graph, &registry)
                .unwrap()
                .layers
                .len(),
            1
        );
        assert_eq!(editor.graph.nodes.len(), baseline_nodes + 2);

        editor.undo(&registry);
        assert!(resolve_material_surface_chain(&editor.graph, &registry)
            .unwrap()
            .layers
            .is_empty());
        assert_eq!(editor.graph.nodes.len(), baseline_nodes);
        editor.redo(&registry);
        let layer = resolve_material_surface_chain(&editor.graph, &registry)
            .unwrap()
            .layers
            .into_iter()
            .next()
            .unwrap();
        editor.remove_nodes(vec![layer], &registry);
        assert!(resolve_material_surface_chain(&editor.graph, &registry)
            .unwrap()
            .layers
            .is_empty());
        editor.undo(&registry);
        assert_eq!(
            resolve_material_surface_chain(&editor.graph, &registry)
                .unwrap()
                .layers
                .len(),
            1
        );
        compile(&editor.graph, &registry).unwrap();
    }

    #[test]
    fn reroute_nodes_preserve_typed_values_for_preview_and_gpu() {
        let registry = NodeRegistry;
        let (mut graph, output) = graph_with_output();
        let color = node("color");
        let reroute = node("reroute");
        graph.nodes.extend([
            (
                color.clone(),
                crate::graph::NodeRecord {
                    node_type: NodeTypeId("material.constant_color".into()),
                    node_type_version: 1,
                    properties: BTreeMap::from([(
                        "value".into(),
                        PropertyValue::Color([0.15, 0.35, 0.75, 1.0]),
                    )]),
                    socket_defaults: BTreeMap::new(),
                    unknown_payload: None,
                },
            ),
            (
                reroute.clone(),
                crate::graph::NodeRecord {
                    node_type: NodeTypeId("material.reroute_color".into()),
                    node_type_version: 1,
                    properties: BTreeMap::new(),
                    socket_defaults: BTreeMap::new(),
                    unknown_payload: None,
                },
            ),
        ]);
        graph.links.extend([
            (
                LinkId("color-reroute".into()),
                crate::graph::LinkRecord {
                    from: OutputPin {
                        node: color,
                        socket: SocketKey("color".into()),
                    },
                    to: InputPin {
                        node: reroute.clone(),
                        socket: SocketKey("value".into()),
                    },
                    order: 0,
                },
            ),
            (
                LinkId("reroute-output".into()),
                crate::graph::LinkRecord {
                    from: OutputPin {
                        node: reroute,
                        socket: SocketKey("color".into()),
                    },
                    to: InputPin {
                        node: output,
                        socket: SocketKey("base_color".into()),
                    },
                    order: 0,
                },
            ),
        ]);
        let program = compile(&graph, &registry).unwrap();
        assert_eq!(
            program
                .evaluate(MaterialSampleContext {
                    position: [0.0; 3],
                    normal: [0.0, 1.0, 0.0],
                })
                .base_color,
            [0.15, 0.35, 0.75, 1.0]
        );
        assert!(program.wgsl.contains("v"));
    }

    #[test]
    fn editor_can_copy_and_paste_a_node_with_defaults() {
        let registry = NodeRegistry;
        let mut editor = GraphEditorState::new(6);
        editor.add_node(NodeTypeId("material.constant_color".into()), &registry);
        let original = editor.selected_node.clone().unwrap();
        assert!(editor.copy_selected());
        let pasted = editor.paste_clipboard(&registry).unwrap();
        assert_ne!(original, pasted);
        assert_eq!(
            editor.graph.nodes[&pasted].properties.get("value"),
            Some(&PropertyValue::Color([0.8, 0.8, 0.8, 1.0]))
        );
        assert_eq!(editor.selected_node, Some(pasted));
    }
}
