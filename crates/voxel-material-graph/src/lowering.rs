//! Material-graph lowering shared by the CPU preview and generated WGSL.
//!
//! The editable [`GraphAsset`](voxel_graph::GraphAsset) is never evaluated
//! directly. It first becomes this small typed IR, which gives CPU preview and
//! GPU code generation the same node semantics.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::cacheability::{analyse, CacheReport};
use crate::layers::{
    project_pattern_stack, resolve_material_surface_chain, LayerGraphError, DISPLACEMENT_NODE,
    PATTERN_LAYER_NODE, PATTERN_NOISE_NODE,
};
use crate::operation::MaterialNodeOperation;
use voxel_graph::AssetId;
use voxel_graph::{
    Diagnostic, DiagnosticSeverity, FieldTarget, GraphAsset, GraphCommand, GraphHistory, GraphKind,
    InputPin, LinkId, LinkRecord, NodeId, NodeRecord, NodeRegistry, NodeTypeId, OutputPin,
    PropertyValue, SocketKey,
};
use voxel_material::animation_clock::{fract, AnimationClockSample, EPOCH_SECONDS};
use voxel_material::material::Material;
use voxel_material::pattern::MAX_PATTERN_LAYERS;
use voxel_material::world_event::{GpuWorldEvent, MAX_EVENT_LIFETIME_SECONDS};

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ValueType {
    Scalar,
    Color,
    Vector3,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd)]
pub struct ValueId(pub usize);

/// The oscillator's waveform. There is deliberately no `Square`: it would be
/// `Pulse` at duty 0.5, i.e. a second name for one behaviour.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum OscillatorWave {
    Sine,
    Triangle,
    Saw,
    Pulse,
    /// Sample-and-hold: one random level per cycle, HELD until the next. It
    /// snaps, which is what reads as a failing lamp; interpolated noise over
    /// time is just a wobblier sine.
    Flicker,
}

impl OscillatorWave {
    fn shader_value(self) -> u32 {
        match self {
            Self::Sine => 0,
            Self::Triangle => 1,
            Self::Saw => 2,
            Self::Pulse => 3,
            Self::Flicker => 4,
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "sine" => Self::Sine,
            "triangle" => Self::Triangle,
            "saw" => Self::Saw,
            "pulse" => Self::Pulse,
            "flicker" => Self::Flicker,
            _ => return None,
        })
    }
}

/// Whether two blocks of one material beat together.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhaseSync {
    Global,
    /// Offset per authored ONE-METRE block, not per traversal voxel.
    PerVoxel,
    PerFace,
    PerMaterial,
}

impl PhaseSync {
    fn shader_value(self) -> u32 {
        match self {
            Self::Global => 0,
            Self::PerVoxel => 1,
            Self::PerFace => 2,
            Self::PerMaterial => 3,
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "global" => Self::Global,
            "per_voxel" => Self::PerVoxel,
            "per_face" => Self::PerFace,
            "per_material" => Self::PerMaterial,
            _ => return None,
        })
    }
}

/// How a sensor's nearness falls off across its radius.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SensorFalloff {
    Smoothstep,
    Linear,
    InverseSquare,
    Step,
}

impl SensorFalloff {
    /// The `WORLD_EVENT_FALLOFF_*` value `world.wgsl` switches on. Public
    /// because the light volume's per-material response table carries it too.
    pub fn shader_value(self) -> u32 {
        match self {
            Self::Smoothstep => 0,
            Self::Linear => 1,
            Self::InverseSquare => 2,
            Self::Step => 3,
        }
    }

    fn parse(text: &str) -> Option<Self> {
        Some(match text {
            "smoothstep" => Self::Smoothstep,
            "linear" => Self::Linear,
            "inverse_square" => Self::InverseSquare,
            "step" => Self::Step,
            _ => return None,
        })
    }
}

/// Which of the sensor's three outputs an instruction selects. All three come
/// from ONE winning event, so they stay mutually consistent.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SensorOutput {
    Signal,
    Nearness,
    Envelope,
}

/// A sensor's authored configuration. Every value is a property rather than a
/// socket, which is what lets the `hold + release` budget be validated at
/// compile time against [`MAX_EVENT_LIFETIME_SECONDS`].
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EventSensorConfig {
    pub channel: u32,
    pub radius_meters: f32,
    pub falloff: SensorFalloff,
    pub attack_seconds: f32,
    pub hold_seconds: f32,
    pub release_seconds: f32,
    pub invert: bool,
}

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
    MultiplyScalar(ValueId, ValueId),
    /// Speed and two angles to a velocity vector.
    Direction {
        speed: ValueId,
        azimuth_degrees: ValueId,
        elevation_degrees: ValueId,
    },
    /// Monotone seconds since start.
    Time,
    Oscillator {
        wave: OscillatorWave,
        sync: PhaseSync,
        seed: u32,
        rate_hz: ValueId,
        phase: ValueId,
        duty: ValueId,
        low: ValueId,
        high: ValueId,
    },
    EventSensor {
        config: EventSensorConfig,
    },
}

impl MaterialInstruction {
    /// Every value this instruction reads. The one place the operand shape of
    /// each variant is written down, so a dataflow walk cannot silently miss an
    /// edge when a variant gains an input — the compiler names the new field.
    fn for_each_operand(&self, mut visit: impl FnMut(ValueId)) {
        match *self {
            Self::Scalar(_)
            | Self::Color(_)
            | Self::Vector3(_)
            | Self::Position
            | Self::Normal
            | Self::Time
            | Self::EventSensor { .. } => {}
            Self::PassthroughScalar(value)
            | Self::PassthroughColor(value)
            | Self::PassthroughVector(value)
            | Self::NormalizeVector(value)
            | Self::Component { vector: value, .. } => visit(value),
            Self::AddScalar(a, b)
            | Self::VectorAdd(a, b)
            | Self::DotVector(a, b)
            | Self::MultiplyScalar(a, b) => {
                visit(a);
                visit(b);
            }
            Self::RemapScalar {
                value,
                from_min,
                from_max,
                to_min,
                to_max,
                clamp: _,
            } => {
                visit(value);
                visit(from_min);
                visit(from_max);
                visit(to_min);
                visit(to_max);
            }
            Self::MixColor { a, b, factor } => {
                visit(a);
                visit(b);
                visit(factor);
            }
            Self::ColorScale { color, strength } => {
                visit(color);
                visit(strength);
            }
            Self::FaceColor {
                base,
                top,
                side,
                bottom,
            }
            | Self::FaceScalar {
                base,
                top,
                side,
                bottom,
            } => {
                visit(base);
                visit(top);
                visit(side);
                visit(bottom);
            }
            Self::ColorRamp {
                factor,
                color_a,
                color_b,
                position_a,
                position_b,
            } => {
                visit(factor);
                visit(color_a);
                visit(color_b);
                visit(position_a);
                visit(position_b);
            }
            Self::ClampScalar {
                value,
                minimum,
                maximum,
            } => {
                visit(value);
                visit(minimum);
                visit(maximum);
            }
            Self::Noise {
                position,
                scale,
                detail,
                roughness,
            }
            | Self::NoiseColor {
                position,
                scale,
                detail,
                roughness,
            } => {
                visit(position);
                visit(scale);
                visit(detail);
                visit(roughness);
            }
            Self::Fbm {
                position,
                scale,
                octaves,
                roughness,
            } => {
                visit(position);
                visit(scale);
                visit(octaves);
                visit(roughness);
            }
            Self::VectorScale { vector, scale } => {
                visit(vector);
                visit(scale);
            }
            Self::Direction {
                speed,
                azimuth_degrees,
                elevation_degrees,
            } => {
                visit(speed);
                visit(azimuth_degrees);
                visit(elevation_degrees);
            }
            Self::Oscillator {
                wave: _,
                sync: _,
                seed: _,
                rate_hz,
                phase,
                duty,
                low,
                high,
            } => {
                visit(rate_hz);
                visit(phase);
                visit(duty);
                visit(low);
                visit(high);
            }
        }
    }

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
            | Self::MultiplyScalar(..)
            | Self::Time
            | Self::Oscillator { .. }
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
            | Self::Direction { .. }
            | Self::EventSensor { .. }
            | Self::NormalizeVector(_)
            | Self::PassthroughVector(_) => ValueType::Vector3,
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialOutput {
    pub base_color: ValueId,
    pub roughness: ValueId,
    pub emission: ValueId,
    pub specular: ValueId,
    pub ambient_occlusion: ValueId,
    pub normal: ValueId,
    pub specular_active: bool,
    /// S3 — one entry per ACTIVE pattern slot, in surface-chain order, matching
    /// the slot order the material row uploads. Disabled layers occupy no slot,
    /// exactly as `project_pattern_stack` skips them, so the indices line up
    /// with `materials[m].patterns[slot]` on the GPU.
    pub layer_animation: Vec<LayerAnimation>,
}

/// S3b — how a compiled program's EMISSION answers the world event field,
/// reduced to the two things the light volume can act on: which sensor gates it,
/// and what the emission is at each end of that sensor's range.
///
/// ## Why two endpoints rather than the graph itself
///
/// The CA cannot run a material graph — it has one thread per cell and no
/// surface, no face, no pattern coordinate. What it *can* do is evaluate one
/// sensor per cell and interpolate. So a graph of arbitrary shape between the
/// sensor and the emission output is reduced to the straight line through its
/// two endpoints. That is an approximation, and a named one: a graph that is
/// non-monotone in between (emission peaking at half signal, say) reaches the
/// volume as the linear blend, while the SURFACE still shades the real curve.
///
/// It is well inside the error the volume already carries — half-metre cells, a
/// quarter-fill solidity threshold, one representative voxel per cell — and it
/// buys the property that matters: the room brightens and dims with the wall.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct EmissionEventResponse {
    /// The first sensor on the emission output's dataflow path. A graph with
    /// several is reduced to this one; the others hold at their resting value.
    pub sensor: EventSensorConfig,
    /// Emission with no event in range.
    pub resting: [f32; 3],
    /// Emission with a saturating event of this sensor's channel at the sample
    /// point.
    pub triggered: [f32; 3],
}

/// The two animation values a pattern layer's sockets supply.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LayerAnimation {
    pub gain: ValueId,
    pub drift_velocity: ValueId,
}

/// Deterministic samples per oscillator period when averaging a response
/// endpoint. See [`MaterialGraphProgram::emission_oscillator_window`].
const EMISSION_RESPONSE_PHASES: usize = 16;

#[derive(Clone, Debug)]
pub struct MaterialGraphProgram {
    pub graph_id: AssetId,
    pub semantic_hash: u64,
    pub instructions: Vec<MaterialInstruction>,
    pub output: MaterialOutput,
    pub wgsl: String,
    /// Which of this graph's pattern layers could have their field evaluated
    /// once instead of per pixel per frame.
    ///
    /// Carried on the program rather than recomputed by each caller: it is
    /// derived from the same graph the program was compiled from, so computing
    /// it anywhere else risks answering for a graph that has since changed. The
    /// editor turns [`CacheReport::diagnostics`] into author-facing warnings;
    /// `cargo run --release -p voxel-rt --example cache_report` prints it for the
    /// whole checked-in project.
    pub cache: CacheReport,
}

/// The prefix every generated program carries: the graph ABI plus the shared
/// helpers, taken from the SAME file the DDA source concatenates.
///
/// It was a hand-maintained duplicate of `shaders/graph_prelude.wgsl` until S3;
/// the two would have had to be edited in lockstep forever, and a silent
/// divergence between them is a CPU/GPU mismatch that no test would name.
/// [`MaterialGraphProgram::wgsl_function`] strips this prefix before injecting a
/// function into the DDA source, where the real definitions already exist.
const GRAPH_PRELUDE: &str = include_str!("../shaders/graph_prelude.wgsl");

/// Host declarations the prelude reads, restated so a compiled program can be
/// validated by `naga` on its own — without the brickmap, the lighting uniform
/// or the event field the renderer supplies.
///
/// These are stubs, and they are stripped along with the prelude, so they never
/// reach the GPU. Their only contract is to match the real declarations' SHAPE:
/// if one drifts, standalone validation passes and the injected build fails,
/// which is exactly the failure the assembled-source test at the bottom of this
/// module exists to catch.
const GRAPH_HOST_STUBS: &str = concat!(
    "// ---- validation-only stubs; stripped before injection ----\n",
    "const BRICK_SIZE: f32 = 8.0;\n",
    "struct Lighting { animation_params: vec4<f32>, event_params: vec4<f32>, };\n",
    "var<private> lighting: Lighting;\n",
    "struct BrickmapMeta { voxel_size_meters: f32, };\n",
    "var<private> brickmap: BrickmapMeta;\n",
    "struct PatternAnimation { gain: vec4<f32>, drift_velocity: array<vec4<f32>, 4>, };\n",
    "fn pattern_animation_identity() -> PatternAnimation {\n",
    "    return PatternAnimation(vec4<f32>(1.0), array<vec4<f32>, 4>(\n",
    "        vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0), vec4<f32>(0.0)));\n",
    "}\n",
    "const ANIMATION_EPOCH_SECONDS: f32 = 64.0;\n",
    // W2 moved the split-clock oscillator phase into `world.wgsl`, beside the epoch
    // const, because it grew a second consumer: the water wave field, which is water
    // physics and must not import this prelude to ask what time it is. Same treatment
    // as `world_event_sense` below — a shape-only stub for the seam that crosses.
    "fn animation_oscillator_phase(rate_hz: f32) -> f32 {\n",
    "    return fract(rate_hz * lighting.animation_params.x);\n",
    "}\n",
    // S3b moved the event field and its sensing into `world.wgsl`, shared with
    // the CA pass. The prelude no longer names the buffer at all — only this
    // one function — so the stub shrank to the seam that actually crosses.
    "fn world_event_sense(point_meters: vec3<f32>, channel: u32,\n",
    "    radius_meters: f32, falloff: u32, attack_seconds: f32,\n",
    "    hold_seconds: f32, release_seconds: f32) -> vec3<f32> {\n",
    "    return vec3<f32>(0.0);\n",
    "}\n",
);

fn graph_program_prefix() -> String {
    format!("{GRAPH_HOST_STUBS}{GRAPH_PRELUDE}")
}

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

    pub fn program(&self, material_slot: u8) -> Option<&MaterialGraphProgram> {
        self.programs.get(&material_slot)
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

/// Everything a material program is evaluated against.
///
/// It carries the RAW inputs — the clock and the live event list — rather than
/// a precomputed answer, because a graph may hold several sensors on different
/// channels with different radii and envelopes, and no single scalar can stand
/// for all of them. Each sensor instruction evaluates its own configuration,
/// exactly as the shader does.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialSampleContext<'a> {
    /// TRAVERSAL VOXEL units — the same units the shader's `position` argument
    /// carries, so both backends do identical arithmetic. Parity is the point:
    /// the CPU backend exists to be comparable with the generated WGSL, so it
    /// takes the shader's arguments and makes the shader's conversions rather
    /// than a pre-converted variant.
    pub position: [f32; 3],
    pub normal: [f32; 3],
    /// Traversal voxel size in metres, for the conversions that need it.
    pub voxel_size_meters: f32,
    pub clock: AnimationClockSample,
    /// The live events, in upload order — the same slice the GPU sees.
    pub events: &'a [GpuWorldEvent],
}

impl MaterialSampleContext<'_> {
    /// The still, empty world: a frozen clock and no events. What a preview,
    /// a parity test and the material table's representative GI sample all use.
    pub fn still(position: [f32; 3], normal: [f32; 3]) -> Self {
        Self {
            position,
            normal,
            voxel_size_meters: voxel_core::world::VOXEL_SIZE,
            clock: AnimationClockSample::FROZEN,
            events: &[],
        }
    }
}
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct MaterialSample {
    pub base_color: [f32; 4],
    pub roughness: f32,
    pub emission: [f32; 4],
    pub specular: f32,
    pub ambient_occlusion: f32,
    pub normal: [f32; 3],
}

/// Create the smallest valid material graph entirely from the registered node
/// declarations. New/reset documents therefore obey the same schema as loaded
/// documents and can accept a layer immediately.
pub fn new_material_graph(name: impl Into<String>) -> GraphAsset {
    let registry = crate::CATALOGUE;
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
    crate::layers::append_pattern_layer_nodes(&mut graph, &surface, &output, &material.patterns);
    graph
}

/// Chris Wellons' lowbias32 — the mirror of `graph_hash_u32` in
/// `shaders/graph_prelude.wgsl`. Rust's `wrapping_mul` and WGSL's `u32`
/// multiply are both mod 2^32 and `>>` is logical in both, so the two agree bit
/// for bit. `pattern.rs` picked this hash for the same reason.
fn graph_hash_u32(value: u32) -> u32 {
    let mut hashed = value;
    hashed ^= hashed >> 16;
    hashed = hashed.wrapping_mul(0x7feb_352d);
    hashed ^= hashed >> 15;
    hashed = hashed.wrapping_mul(0x846c_a68b);
    hashed ^= hashed >> 16;
    hashed
}

fn graph_hash_to_unit(value: u32) -> f32 {
    (graph_hash_u32(value) >> 8) as f32 / 16_777_216.0
}

/// Mirrors `graph_direction` in `shaders/graph_prelude.wgsl`.
///
/// Azimuth around the vertical axis, 0 along +X and 90 along +Z; elevation
/// above horizontal. The same meaning `SunSettings` gives those words, reused
/// so there is one definition of an angle pair in the codebase.
fn graph_direction(speed: f32, azimuth_degrees: f32, elevation_degrees: f32) -> [f32; 3] {
    let azimuth = azimuth_degrees.to_radians();
    let elevation = elevation_degrees.to_radians();
    let (sin_elevation, cos_elevation) = elevation.sin_cos();
    let (sin_azimuth, cos_azimuth) = azimuth.sin_cos();
    [
        cos_elevation * cos_azimuth * speed,
        sin_elevation * speed,
        cos_elevation * sin_azimuth * speed,
    ]
}

/// Detail cells per authored one-metre block. Mirrors `BRICK_SIZE` in
/// `shaders/world.wgsl`.
const GRAPH_BRICK_SIZE: f32 = 8.0;

/// Mirrors `graph_phase_offset`. `position` is in traversal voxel units, so
/// `PerVoxel` divides by the brick size first — hashing the raw coordinate
/// would de-sync each 12.5 cm detail cell instead of each authored block.
fn graph_phase_offset(sync: PhaseSync, seed: u32, position: [f32; 3], normal: [f32; 3]) -> f32 {
    match sync {
        PhaseSync::Global => 0.0,
        // The golden-ratio conjugate, written to the SAME number of digits as
        // the WGSL literal so both parse to the identical f32. Successive seeds
        // land far apart in phase rather than marching in a visible progression.
        PhaseSync::PerMaterial => fract(seed as f32 * 0.618_034),
        PhaseSync::PerVoxel | PhaseSync::PerFace => {
            let block = position.map(|axis| (axis / GRAPH_BRICK_SIZE).floor() as i32);
            let mut mixed = (block[0] as u32).wrapping_mul(0x27d4_eb2d)
                ^ (block[1] as u32).wrapping_mul(0x9e37_79b9)
                ^ (block[2] as u32).wrapping_mul(0x85eb_ca6b)
                ^ seed.wrapping_mul(0xc2b2_ae35);
            if sync == PhaseSync::PerFace {
                let face = if normal[1].abs() > 0.5 {
                    if normal[1] > 0.0 {
                        3
                    } else {
                        2
                    }
                } else if normal[0].abs() > 0.5 {
                    if normal[0] > 0.0 {
                        1
                    } else {
                        0
                    }
                } else if normal[2] > 0.0 {
                    5
                } else {
                    4
                };
                mixed ^= (face as u32 + 1).wrapping_mul(0x1656_67b1);
            }
            graph_hash_to_unit(mixed)
        }
    }
}

/// Mirrors `graph_wave`. Returns `[0, 1]` before the low/high remap.
fn graph_wave(wave: OscillatorWave, phase: f32, duty: f32, salt: u32) -> f32 {
    let cycle = fract(phase);
    match wave {
        OscillatorWave::Triangle => 1.0 - (cycle * 2.0 - 1.0).abs(),
        OscillatorWave::Saw => cycle,
        OscillatorWave::Pulse => {
            if cycle < duty.clamp(0.0, 1.0) {
                1.0
            } else {
                0.0
            }
        }
        OscillatorWave::Flicker => {
            graph_hash_to_unit((phase.floor() as i32) as u32 ^ salt ^ 0x9e37_79b9)
        }
        OscillatorWave::Sine => 0.5 - 0.5 * (cycle * std::f32::consts::TAU).cos(),
    }
}

/// Mirrors `world_event_falloff`.
pub fn world_event_falloff(kind: SensorFalloff, normalised_distance: f32) -> f32 {
    let t = normalised_distance.clamp(0.0, 1.0);
    match kind {
        SensorFalloff::Linear => 1.0 - t,
        SensorFalloff::InverseSquare => {
            let falloff = 1.0 / (1.0 + 8.0 * t * t);
            let edge = 1.0 / 9.0;
            ((falloff - edge) / (1.0 - edge)).clamp(0.0, 1.0)
        }
        SensorFalloff::Step => {
            if t < 1.0 {
                1.0
            } else {
                0.0
            }
        }
        SensorFalloff::Smoothstep => {
            let smooth = 1.0 - t;
            smooth * smooth * (3.0 - 2.0 * smooth)
        }
    }
}

/// Mirrors `world_event_ramp`.
fn world_event_ramp(value: f32, length_seconds: f32) -> f32 {
    if length_seconds <= 0.0 {
        return if value >= 0.0 { 1.0 } else { 0.0 };
    }
    (value / length_seconds).clamp(0.0, 1.0)
}

/// Mirrors `world_event_envelope`.
///
/// Attack and release are MULTIPLIED, not switched between: an event that opens
/// and closes inside one frame then ramps up and down at once and yields a
/// smooth shortened blip rather than a step.
fn world_event_envelope(
    event: &GpuWorldEvent,
    clock: AnimationClockSample,
    config: &EventSensorConfig,
) -> f32 {
    let attack_factor = world_event_ramp(event.elapsed_since_start(clock), config.attack_seconds);
    if event.is_open() {
        return attack_factor;
    }
    let since_end = event.elapsed_since_end(clock);
    let release_factor =
        1.0 - world_event_ramp(since_end - config.hold_seconds, config.release_seconds);
    attack_factor * release_factor
}

/// Mirrors `world_event_sense` in `world.wgsl`. Returns
/// `(signal, nearness, envelope)`, all three from ONE winning event so they
/// describe a state that actually existed.
///
/// Takes a point in METRES and the raw field, not a `MaterialSampleContext`,
/// because the light volume senses the same field from a cell centre and there
/// is no surface, no normal and no pattern coordinate there. One definition,
/// two tiers — exactly the split the WGSL side makes.
///
/// `invert` is NOT applied here: it belongs to the sensor node, and the volume's
/// response carries the inversion in its two scales instead.
pub fn sense_world_events(
    config: &EventSensorConfig,
    point_meters: [f32; 3],
    clock: AnimationClockSample,
    events: &[GpuWorldEvent],
) -> (f32, f32, f32) {
    let mut best = (0.0_f32, 0.0_f32, 0.0_f32);
    let mut found = false;
    for event in events {
        if event.channel != config.channel {
            continue;
        }
        let reach = config.radius_meters.min(event.radius_meters);
        if reach <= 0.0 {
            continue;
        }
        let offset = [
            event.position_meters[0] - point_meters[0],
            event.position_meters[1] - point_meters[1],
            event.position_meters[2] - point_meters[2],
        ];
        let distance_squared =
            offset[0] * offset[0] + offset[1] * offset[1] + offset[2] * offset[2];
        if distance_squared >= reach * reach {
            continue;
        }
        let nearness = world_event_falloff(config.falloff, distance_squared.sqrt() / reach);
        let envelope = world_event_envelope(event, clock, config);
        let signal = nearness * envelope * event.strength.clamp(0.0, 1.0);
        if !found || signal > best.0 {
            best = (signal, nearness, envelope);
            found = true;
        }
    }
    best
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
            .strip_prefix(&graph_program_prefix())
            .unwrap_or(&self.wgsl)
            .replacen("fn graph_material(", &format!("fn {name}("), 1)
    }

    pub fn evaluate(&self, context: MaterialSampleContext<'_>) -> MaterialSample {
        let values = self.run(context);
        MaterialSample {
            base_color: values[self.output.base_color.0].color(),
            roughness: values[self.output.roughness.0].scalar(),
            emission: values[self.output.emission.0].color(),
            specular: values[self.output.specular.0].scalar(),
            ambient_occlusion: values[self.output.ambient_occlusion.0].scalar(),
            normal: values[self.output.normal.0].vector(),
        }
    }

    /// S3b — how this program's emission answers events, or `None` when it does
    /// not answer them at all.
    ///
    /// `None` is decided by BEHAVIOUR, not by topology: the program is evaluated
    /// once with an empty field and once with a saturating event, and a graph
    /// whose emission comes out identical does not get a response even if it
    /// holds a sensor. A sensor wired only into base colour is exactly that case,
    /// and the light volume has only seven response slots to spend.
    ///
    /// The probe event sits AT the sample point with the sensor's own radius and
    /// full strength, started one epoch ago so any authored attack has completed.
    /// The clock is `context`'s, unchanged, so oscillators read the same value in
    /// both evaluations and cannot pollute the comparison.
    pub fn emission_event_response(
        &self,
        context: MaterialSampleContext<'_>,
    ) -> Option<EmissionEventResponse> {
        let sensor = self.emission_event_sensor()?;
        let probe = [GpuWorldEvent {
            position_meters: [
                context.position[0] * context.voxel_size_meters,
                context.position[1] * context.voxel_size_meters,
                context.position[2] * context.voxel_size_meters,
            ],
            radius_meters: sensor.radius_meters,
            started_epoch: context.clock.epoch - 1.0,
            started_remainder_seconds: context.clock.remainder_seconds,
            ended_epoch: 0.0,
            ended_remainder_seconds: 0.0,
            channel: sensor.channel,
            strength: 1.0,
            open: 1.0,
            pad_row_b: 0.0,
        }];
        let window = self.emission_oscillator_window(&self.emission_reachable());
        let endpoint = |events: &[GpuWorldEvent]| -> [f32; 3] {
            let Some((period_seconds, phases)) = window else {
                let emission = self
                    .evaluate(MaterialSampleContext { events, ..context })
                    .emission;
                return [emission[0], emission[1], emission[2]];
            };
            let mut total = [0.0_f64; 3];
            for phase in 0..phases {
                let seconds = context.clock.remainder_seconds
                    + period_seconds * (phase as f32 + 0.5) / phases as f32;
                let epochs = (seconds / EPOCH_SECONDS).floor();
                let emission = self
                    .evaluate(MaterialSampleContext {
                        events,
                        clock: AnimationClockSample {
                            epoch: context.clock.epoch + epochs,
                            remainder_seconds: seconds - epochs * EPOCH_SECONDS,
                        },
                        ..context
                    })
                    .emission;
                for (channel, total) in total.iter_mut().enumerate() {
                    *total += f64::from(emission[channel]);
                }
            }
            std::array::from_fn(|channel| (total[channel] / phases as f64) as f32)
        };
        let resting = endpoint(&[]);
        let triggered = endpoint(&probe);
        if resting == triggered {
            return None;
        }
        Some(EmissionEventResponse {
            sensor,
            resting,
            triggered,
        })
    }

    /// The sole event sensor on the emission output's dataflow path.
    ///
    /// A reachability walk rather than a scan of every instruction: a sensor
    /// that drives base colour or a pattern drift must not claim one of the
    /// volume's response slots, and only the edges say which output it feeds.
    /// Multiple reachable sensors deliberately return `None`: the volume has
    /// one response index per cell and cannot faithfully choose an arbitrary
    /// lowering-order sensor while the surface responds to both.
    fn emission_event_sensor(&self) -> Option<EventSensorConfig> {
        let reached = self.emission_reachable();
        let mut sensors =
            self.instructions
                .iter()
                .zip(&reached)
                .filter_map(|(instruction, &reached)| match instruction {
                    MaterialInstruction::EventSensor { config } if reached => Some(*config),
                    _ => None,
                });
        let sensor = sensors.next()?;
        sensors.next().is_none().then_some(sensor)
    }

    /// Which values the emission output actually depends on.
    fn emission_reachable(&self) -> Vec<bool> {
        let mut reached = vec![false; self.instructions.len()];
        let mut pending = vec![self.output.emission];
        while let Some(value) = pending.pop() {
            if reached[value.0] {
                continue;
            }
            reached[value.0] = true;
            self.instructions[value.0].for_each_operand(|operand| pending.push(operand));
        }
        reached
    }

    /// The window an endpoint must be AVERAGED over, or `None` when emission
    /// holds no oscillator and one sample is the whole answer.
    ///
    /// ## Why an average, and why this was a real defect
    ///
    /// An endpoint sampled at one instant reports the oscillator at whatever
    /// phase that instant happens to be. `MaterialSampleContext::still` freezes
    /// the clock at zero, and a sine at phase 0 sits at its TROUGH — so the
    /// authored glow block, whose surface swings 0.45..1.25, handed the light
    /// volume a flat 0.45 and lit the room at 56% of what the block looked like.
    /// Not a rounding error: a visible mismatch between an emitter and its light.
    ///
    /// The mean is the honest stand-in. The CA cannot follow a 1.4 Hz pulse — it
    /// has no clock and no oscillator — so the only question is WHICH constant it
    /// holds, and the time-average of the surface is the one that conserves the
    /// light leaving it.
    ///
    /// Returns `(period, phases)`:
    ///
    /// * **period** — one cycle of the SLOWEST reachable oscillator, so the sweep
    ///   covers the longest thing present. A rate that is not a constant (an
    ///   oscillator driving another oscillator's rate) falls back to 1 Hz: one
    ///   period of a plausible rate still beats one arbitrary instant. Genuinely
    ///   incommensurate rates are not covered by any single period, and that
    ///   limit is inherent rather than a shortcut taken here.
    /// * **phases** — [`EMISSION_RESPONSE_PHASES`], raised for a narrow pulse. A
    ///   `duty` of 0.05 occupies a twentieth of the period, and 16 evenly spaced
    ///   samples would miss it entirely and report the emitter dark.
    fn emission_oscillator_window(&self, reached: &[bool]) -> Option<(f32, usize)> {
        let constant = |value: ValueId| match self.instructions[value.0] {
            MaterialInstruction::Scalar(scalar) => Some(scalar),
            _ => None,
        };
        let mut slowest_period_seconds: Option<f32> = None;
        let mut phases = EMISSION_RESPONSE_PHASES;
        for (instruction, _) in self
            .instructions
            .iter()
            .zip(reached)
            .filter(|(_, reached)| **reached)
        {
            let MaterialInstruction::Oscillator {
                wave,
                rate_hz,
                duty,
                ..
            } = *instruction
            else {
                continue;
            };
            let period_seconds = constant(rate_hz)
                .filter(|rate| *rate > 0.0)
                .map_or(1.0, |rate| 1.0 / rate);
            slowest_period_seconds = Some(
                slowest_period_seconds
                    .map_or(period_seconds, |slowest: f32| slowest.max(period_seconds)),
            );
            if wave == OscillatorWave::Pulse {
                if let Some(duty) = constant(duty).filter(|duty| *duty > 0.0) {
                    phases = phases.max((4.0 / duty).ceil() as usize);
                }
            }
        }
        slowest_period_seconds.map(|period_seconds| (period_seconds, phases))
    }

    /// The per-slot animation values this program produces, in the same slot
    /// order the material row uploads its pattern layers.
    ///
    /// The pattern reference evaluator takes these directly, which is what lets
    /// a CPU consumer reproduce an animated surface without re-deriving how a
    /// gain or a drift was authored — it may be a constant, an oscillator, or a
    /// direction node, and none of that is its business.
    pub fn evaluate_layer_animation(
        &self,
        context: MaterialSampleContext<'_>,
    ) -> Vec<voxel_material::pattern::LayerAnimationSample> {
        let values = self.run(context);
        self.output
            .layer_animation
            .iter()
            .map(|animation| voxel_material::pattern::LayerAnimationSample {
                gain: values[animation.gain.0].scalar(),
                drift_velocity: values[animation.drift_velocity.0].vector(),
                time_seconds: context.clock.monotone_seconds(),
            })
            .collect()
    }

    fn run(&self, context: MaterialSampleContext<'_>) -> Vec<RuntimeValue> {
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
                MaterialInstruction::MultiplyScalar(a, b) => {
                    RuntimeValue::Scalar(values[a.0].scalar() * values[b.0].scalar())
                }
                MaterialInstruction::Direction {
                    speed,
                    azimuth_degrees,
                    elevation_degrees,
                } => RuntimeValue::Vector3(graph_direction(
                    values[speed.0].scalar(),
                    values[azimuth_degrees.0].scalar(),
                    values[elevation_degrees.0].scalar(),
                )),
                MaterialInstruction::Time => RuntimeValue::Scalar(context.clock.monotone_seconds()),
                MaterialInstruction::Oscillator {
                    wave,
                    sync,
                    seed,
                    rate_hz,
                    phase,
                    duty,
                    low,
                    high,
                } => {
                    let rate_hz = values[rate_hz.0].scalar();
                    let sync_offset =
                        graph_phase_offset(sync, seed, context.position, context.normal);
                    let total_phase = context.clock.oscillator_phase(rate_hz)
                        + values[phase.0].scalar()
                        + sync_offset;
                    let shape = graph_wave(wave, total_phase, values[duty.0].scalar(), seed);
                    let low = values[low.0].scalar();
                    let high = values[high.0].scalar();
                    RuntimeValue::Scalar(low + (high - low) * shape)
                }
                MaterialInstruction::EventSensor { config } => {
                    let (signal, nearness, envelope) = sense_world_events(
                        &config,
                        context
                            .position
                            .map(|axis| axis * context.voxel_size_meters),
                        context.clock,
                        context.events,
                    );
                    // Invert applies to the SIGNAL only. Nearness and envelope
                    // keep their literal meanings so they stay usable as
                    // diagnostics — and so the volume's two scales, which are
                    // where inversion lives one tier down, cannot double-apply it.
                    let signal = if config.invert { 1.0 - signal } else { signal };
                    RuntimeValue::Vector3([signal, nearness, envelope])
                }
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
        values
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
    if graph.kind != voxel_graph::GraphKind::Material {
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
        event_sensors: BTreeMap::new(),
    };
    let base_color = lowerer.input(&surface_chain.surface, "base_color")?;
    let roughness = lowerer.input(&surface_chain.surface, "roughness")?;
    let emission = lowerer.input(&surface_chain.surface, "emission")?;
    let specular = lowerer.input(&surface_chain.surface, "specular")?;
    let ambient_occlusion = lowerer.input(&surface_chain.surface, "ambient_occlusion")?;
    let normal = lowerer.input(&surface_chain.surface, "normal")?;
    // Slot order must match the uploaded row, and a DISABLED layer occupies no
    // slot — `project_pattern_stack` skips it — so this walk skips it too. Any
    // other rule here would silently animate the wrong layer.
    let mut layer_animation = Vec::new();
    for layer_id in &surface_chain.layers {
        if !is_pattern_layer(graph, registry, layer_id) {
            continue;
        }
        if !layer_is_enabled(graph, layer_id) {
            continue;
        }
        if layer_animation.len() >= MAX_PATTERN_LAYERS {
            break;
        }
        let gain = lowerer.input(layer_id, "animation_gain")?;
        let drift_velocity = lowerer.input(layer_id, "drift_velocity")?;
        lowerer.expect(gain, ValueType::Scalar)?;
        lowerer.expect(drift_velocity, ValueType::Vector3)?;
        layer_animation.push(LayerAnimation {
            gain,
            drift_velocity,
        });
    }
    let output = MaterialOutput {
        base_color,
        roughness,
        emission,
        specular,
        ambient_occlusion,
        normal,
        specular_active: surface_socket_connected(graph, &surface_chain.surface, "specular"),
        layer_animation,
    };
    lowerer.expect(output.base_color, ValueType::Color)?;
    lowerer.expect(output.roughness, ValueType::Scalar)?;
    lowerer.expect(output.emission, ValueType::Color)?;
    lowerer.expect(output.specular, ValueType::Scalar)?;
    lowerer.expect(output.ambient_occlusion, ValueType::Scalar)?;
    lowerer.expect(output.normal, ValueType::Vector3)?;
    let wgsl = emit_wgsl(&lowerer.values, &output);
    validate_wgsl(&wgsl)?;
    Ok(MaterialGraphProgram {
        graph_id: graph.id.clone(),
        semantic_hash: graph.resolve(registry).hashes.semantic,
        instructions: lowerer.values,
        output,
        wgsl,
        cache: analyse(graph, registry),
    })
}

struct Lowerer<'a> {
    graph: &'a GraphAsset,
    registry: &'a NodeRegistry,
    values: Vec<MaterialInstruction>,
    cache: BTreeMap<(NodeId, SocketKey), ValueId>,
    event_sensors: BTreeMap<NodeId, ValueId>,
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
            // A disabled node is REMOVED, not held still: the link is ignored
            // and the socket falls through to its own default below, exactly as
            // if nothing were connected. That is what makes a per-node toggle
            // mean "as it was before I added this" without anyone having to
            // author a neutral value — the neutral value belongs to the
            // consumer, and a layer gain, an emission strength and a mix factor
            // do not share one. It also costs nothing at runtime: the bypass
            // happens here, at lowering, so no code is emitted for the node.
            .filter(|link| !self.node_is_disabled(&link.from.node))
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

    /// Whether a node has been switched off by its `enabled` property.
    ///
    /// Scoped to the nodes that DECLARE the property, which today is the
    /// oscillator. Pattern layers carry an `enabled` of their own with a
    /// different meaning — out of the uploaded stack rather than out of the
    /// value graph — and `project_pattern_stack` owns that one.
    fn node_is_disabled(&self, node: &NodeId) -> bool {
        let Some(record) = self.graph.nodes.get(node) else {
            return false;
        };
        let bypassable = matches!(
            self.registry
                .find(&record.node_type)
                .and_then(|declaration| MaterialNodeOperation::from_tag(declaration.operation)),
            Some(MaterialNodeOperation::Oscillator | MaterialNodeOperation::EventSensor)
        );
        if !bypassable {
            return false;
        }
        matches!(
            record.properties.get("enabled"),
            Some(PropertyValue::Boolean(false))
        )
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
            .and_then(|declaration| MaterialNodeOperation::from_tag(declaration.operation))
            .ok_or_else(|| MaterialGraphError::UnsupportedNode(record.node_type.0.clone()))?;
        let value = match operation {
            MaterialNodeOperation::ConstantScalar | MaterialNodeOperation::ConstantColor => {
                let value = self.field_value(&record, FieldTarget::Property, "value")?;
                self.property_value(value)?
            }
            MaterialNodeOperation::MultiplyScalar => {
                let a = self.input(node, "a")?;
                let b = self.input(node, "b")?;
                self.expect(a, ValueType::Scalar)?;
                self.expect(b, ValueType::Scalar)?;
                self.push(MaterialInstruction::MultiplyScalar(a, b))
            }
            MaterialNodeOperation::Direction => {
                let speed = self.input(node, "speed")?;
                let azimuth_degrees = self.input(node, "azimuth_degrees")?;
                let elevation_degrees = self.input(node, "elevation_degrees")?;
                for input in [speed, azimuth_degrees, elevation_degrees] {
                    self.expect(input, ValueType::Scalar)?;
                }
                self.push(MaterialInstruction::Direction {
                    speed,
                    azimuth_degrees,
                    elevation_degrees,
                })
            }
            MaterialNodeOperation::Time => self.push(MaterialInstruction::Time),
            MaterialNodeOperation::Oscillator => {
                let rate_hz = self.input(node, "rate_hz")?;
                let phase = self.input(node, "phase")?;
                let duty = self.input(node, "duty")?;
                let low = self.input(node, "low")?;
                let high = self.input(node, "high")?;
                for input in [rate_hz, phase, duty, low, high] {
                    self.expect(input, ValueType::Scalar)?;
                }
                let wave = self.choice(&record, "wave", OscillatorWave::parse)?;
                let sync = self.choice(&record, "sync", PhaseSync::parse)?;
                let seed = self
                    .integer_field(&record, "seed")?
                    .clamp(0, u32::MAX as i64) as u32;
                self.push(MaterialInstruction::Oscillator {
                    wave,
                    sync,
                    seed,
                    rate_hz,
                    phase,
                    duty,
                    low,
                    high,
                })
            }
            MaterialNodeOperation::EventSensor => {
                let config = self.event_sensor_config(&record)?;
                let output = match socket.0.as_str() {
                    "nearness" => SensorOutput::Nearness,
                    "envelope" => SensorOutput::Envelope,
                    _ => SensorOutput::Signal,
                };
                let sensor = if let Some(value) = self.event_sensors.get(node) {
                    *value
                } else {
                    let value = self.push(MaterialInstruction::EventSensor { config });
                    self.event_sensors.insert(node.clone(), value);
                    value
                };
                self.push(MaterialInstruction::Component {
                    vector: sensor,
                    axis: match output {
                        SensorOutput::Signal => 0,
                        SensorOutput::Nearness => 1,
                        SensorOutput::Envelope => 2,
                    },
                })
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
            // Every pattern GENERATOR node, and the layer node itself, are read by
            // `material_graph_layers`'s projection into the uploaded row rather than
            // lowered into the IR — the stack is table data, not shader expressions.
            // Reaching one here means a graph wired a generator somewhere a scalar
            // was expected, which is an authoring error and says so.
            | MaterialNodeOperation::PatternLayer
            | MaterialNodeOperation::Displacement
            | MaterialNodeOperation::PatternFlat
            | MaterialNodeOperation::PatternNoise
            | MaterialNodeOperation::PatternSpeckle
            | MaterialNodeOperation::PatternPerlin
            | MaterialNodeOperation::PatternSimplex
            | MaterialNodeOperation::PatternRidged
            | MaterialNodeOperation::PatternTurbulence
            | MaterialNodeOperation::PatternWorley
            | MaterialNodeOperation::PatternWorleyEdge
            | MaterialNodeOperation::PatternWorleySmooth
            | MaterialNodeOperation::PatternWave
            | MaterialNodeOperation::PatternChecker
            | MaterialNodeOperation::PatternTileTone
            | MaterialNodeOperation::PatternTileEdge
            | MaterialNodeOperation::PatternEdgeBand
            // The tessellation is not a value either — it is where the tiles are,
            // read by the projection and packed into each layer's row.
            | MaterialNodeOperation::Tessellation => {
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

    /// Read a text field and map it through a declared choice set.
    fn choice<T>(
        &self,
        record: &NodeRecord,
        key: &str,
        parse: fn(&str) -> Option<T>,
    ) -> Result<T, MaterialGraphError> {
        match self.field_value(record, FieldTarget::Property, key)? {
            PropertyValue::Text(text) => parse(&text).ok_or(MaterialGraphError::InvalidProperty(
                PropertyValue::Text(text),
            )),
            value => Err(MaterialGraphError::InvalidProperty(value)),
        }
    }

    fn integer_field(&self, record: &NodeRecord, key: &str) -> Result<i64, MaterialGraphError> {
        match self.field_value(record, FieldTarget::Property, key)? {
            PropertyValue::Integer(value) => Ok(value),
            value => Err(MaterialGraphError::InvalidProperty(value)),
        }
    }

    fn scalar_field(&self, record: &NodeRecord, key: &str) -> Result<f32, MaterialGraphError> {
        match self.field_value(record, FieldTarget::Property, key)? {
            PropertyValue::Scalar(value) if value.is_finite() => Ok(value),
            value => Err(MaterialGraphError::InvalidProperty(value)),
        }
    }

    fn boolean_field(&self, record: &NodeRecord, key: &str) -> Result<bool, MaterialGraphError> {
        match self.field_value(record, FieldTarget::Property, key)? {
            PropertyValue::Boolean(value) => Ok(value),
            value => Err(MaterialGraphError::InvalidProperty(value)),
        }
    }

    /// A sensor's authored configuration, with the one cross-field rule the
    /// runtime cannot recover from.
    ///
    /// `hold + release` bounds how long after an event CLOSES a sensor is still
    /// fading, but the event field reclaims a closed slot after
    /// [`MAX_EVENT_LIFETIME_SECONDS`] and cannot know any sensor's envelope.
    /// Exceed the budget and the surface would snap dark mid-fade when the
    /// event vanished underneath it. Capping each field on its own is not
    /// enough — it is the SUM that matters — so it is checked here, at
    /// authoring time, and the runtime never has to.
    fn event_sensor_config(
        &self,
        record: &NodeRecord,
    ) -> Result<EventSensorConfig, MaterialGraphError> {
        let hold_seconds = self.scalar_field(record, "hold_seconds")?.max(0.0);
        let release_seconds = self.scalar_field(record, "release_seconds")?.max(0.0);
        if hold_seconds + release_seconds > MAX_EVENT_LIFETIME_SECONDS {
            return Err(MaterialGraphError::EventEnvelopeTooLong {
                hold_seconds,
                release_seconds,
                budget_seconds: MAX_EVENT_LIFETIME_SECONDS,
            });
        }
        Ok(EventSensorConfig {
            channel: self
                .integer_field(record, "channel")?
                .clamp(0, u32::MAX as i64) as u32,
            radius_meters: self.scalar_field(record, "radius_meters")?.max(0.0),
            falloff: self.choice(record, "falloff", SensorFalloff::parse)?,
            attack_seconds: self.scalar_field(record, "attack_seconds")?.max(0.0),
            hold_seconds,
            release_seconds,
            invert: self.boolean_field(record, "invert")?,
        })
    }

    fn component_axis(&self, record: &NodeRecord) -> Result<u8, MaterialGraphError> {
        match self.field_value(record, FieldTarget::Property, "axis")? {
            PropertyValue::Integer(value) => Ok(value.clamp(0, 2) as u8),
            value => Err(MaterialGraphError::InvalidProperty(value)),
        }
    }
}

fn emit_wgsl(values: &[MaterialInstruction], output: &MaterialOutput) -> String {
    let face_color_active = values
        .iter()
        .any(|value| matches!(value, MaterialInstruction::FaceColor { .. }));
    let face_roughness_active = values
        .iter()
        .any(|value| matches!(value, MaterialInstruction::FaceScalar { .. }));
    let mut source = format!(
        "{}fn graph_material(position: vec3<f32>, normal: vec3<f32>) -> GraphMaterial {{\n",
        graph_program_prefix()
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
            MaterialInstruction::MultiplyScalar(a, b) => {
                format!("{} * {}", value_name(*a), value_name(*b))
            }
            MaterialInstruction::Direction {
                speed,
                azimuth_degrees,
                elevation_degrees,
            } => format!(
                "graph_direction({}, {}, {})",
                value_name(*speed),
                value_name(*azimuth_degrees),
                value_name(*elevation_degrees)
            ),
            MaterialInstruction::Time => "graph_animation_seconds()".to_string(),
            MaterialInstruction::Oscillator {
                wave,
                sync,
                seed,
                rate_hz,
                phase,
                duty,
                low,
                high,
            } => format!(
                "graph_oscillator({}u, {}u, {}u, {}, {}, {}, {}, {}, position, normal)",
                wave.shader_value(),
                sync.shader_value(),
                seed,
                value_name(*rate_hz),
                value_name(*phase),
                value_name(*duty),
                value_name(*low),
                value_name(*high)
            ),
            MaterialInstruction::EventSensor { config } => {
                format!(
                    "graph_event_sensor({}u, {}, {}u, {}, {}, {}, {}, position)",
                    config.channel,
                    format_float(config.radius_meters),
                    config.falloff.shader_value(),
                    format_float(config.attack_seconds),
                    format_float(config.hold_seconds),
                    format_float(config.release_seconds),
                    config.invert,
                )
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
    // Per-slot animation, padded to the fixed array the GPU struct carries.
    // Unused slots are the identity (gain 1, no drift), so a row with fewer
    // layers than the maximum behaves exactly as it did before S3.
    let gains: Vec<String> = (0..MAX_PATTERN_LAYERS)
        .map(|slot| match output.layer_animation.get(slot) {
            Some(animation) => format!("v{}", animation.gain.0),
            None => "1.0".to_string(),
        })
        .collect();
    let drifts: Vec<String> = (0..MAX_PATTERN_LAYERS)
        .map(|slot| match output.layer_animation.get(slot) {
            Some(animation) => format!("vec4<f32>(v{}, 0.0)", animation.drift_velocity.0),
            None => "vec4<f32>(0.0)".to_string(),
        })
        .collect();
    source.push_str(&format!(
        "  let animation = PatternAnimation(vec4<f32>({}), array<vec4<f32>, {}>({}));\n",
        gains.join(", "),
        MAX_PATTERN_LAYERS,
        drifts.join(", ")
    ));
    source.push_str(&format!(
        "  return GraphMaterial(v{}, v{}, v{}, v{}, v{}, v{}, {}, true, {}, {}, animation);\n}}\n",
        output.base_color.0,
        output.roughness.0,
        output.emission.0,
        output.specular.0,
        output.ambient_occlusion.0,
        output.normal.0,
        output.specular_active,
        face_color_active,
        face_roughness_active,
    ));
    source
}

/// Whether a pattern layer occupies an uploaded slot. Mirrors the `enabled`
/// test in `project_pattern_stack`, so animation indices and row slots agree.
fn layer_is_enabled(graph: &GraphAsset, layer: &NodeId) -> bool {
    graph
        .nodes
        .get(layer)
        .and_then(|record| record.properties.get("enabled"))
        .map(|value| matches!(value, PropertyValue::Boolean(true)))
        .unwrap_or(true)
}

fn surface_socket_connected(graph: &GraphAsset, surface: &NodeId, socket: &str) -> bool {
    graph
        .links
        .values()
        .any(|link| link.to.node == *surface && link.to.socket.0 == socket)
}

fn is_pattern_layer(graph: &GraphAsset, registry: &NodeRegistry, node: &NodeId) -> bool {
    graph
        .nodes
        .get(node)
        .and_then(|record| registry.find(&record.node_type))
        .is_some_and(|declaration| {
            declaration.operation == MaterialNodeOperation::PatternLayer.tag()
        })
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
    InvalidGraph(Vec<voxel_graph::Diagnostic>),
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
    /// An event sensor would still be fading after its event had been
    /// reclaimed. See `Lowerer::event_sensor_config` for why the SUM is the
    /// thing that has to be bounded.
    EventEnvelopeTooLong {
        hold_seconds: f32,
        release_seconds: f32,
        budget_seconds: f32,
    },
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

/// Case-insensitive substring test that never allocates.
///
/// The palette runs this once per searchable field per node per keystroke, so
/// lowercasing a fresh `String` for every field would be pure garbage churn.
/// `needle_lowercase` must already be lowercased by the caller.
fn contains_ignore_ascii_case(haystack: &str, needle_lowercase: &str) -> bool {
    if needle_lowercase.is_empty() {
        return true;
    }
    let haystack_bytes = haystack.as_bytes();
    let needle_bytes = needle_lowercase.as_bytes();
    if needle_bytes.len() > haystack_bytes.len() {
        return false;
    }
    haystack_bytes
        .windows(needle_bytes.len())
        .any(|window| window.eq_ignore_ascii_case(needle_bytes))
}

/// Does this node type match a palette search term?
///
/// The haystack is everything the node exposes in prose: its identifier, title,
/// category, description, and the label and description of every input and
/// output socket. Searching the sockets is the point — typing "roughness"
/// surfaces nodes that merely *expose* a roughness socket, not only the handful
/// with "roughness" in their title.
///
/// `search_lowercase` must already be lowercased by the caller.
fn node_matches_search(node: &voxel_graph::NodeDeclaration, search_lowercase: &str) -> bool {
    let matches = |haystack: &str| contains_ignore_ascii_case(haystack, search_lowercase);
    matches(node.id)
        || matches(node.title)
        || matches(node.description)
        || matches(node.category.label())
        || node
            .inputs
            .iter()
            .chain(node.outputs.iter())
            .any(|socket| matches(socket.label) || matches(socket.description))
}

fn is_pattern_generator(operation: MaterialNodeOperation) -> bool {
    matches!(
        operation,
        MaterialNodeOperation::PatternFlat
            | MaterialNodeOperation::PatternNoise
            | MaterialNodeOperation::PatternSpeckle
            | MaterialNodeOperation::PatternPerlin
            | MaterialNodeOperation::PatternSimplex
            | MaterialNodeOperation::PatternRidged
            | MaterialNodeOperation::PatternTurbulence
            | MaterialNodeOperation::PatternWorley
            | MaterialNodeOperation::PatternWorleyEdge
            | MaterialNodeOperation::PatternWorleySmooth
            | MaterialNodeOperation::PatternWave
            | MaterialNodeOperation::PatternChecker
            | MaterialNodeOperation::PatternTileTone
            | MaterialNodeOperation::PatternTileEdge
            | MaterialNodeOperation::PatternEdgeBand
    )
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
    /// The large, explicit inspection view in Graph Studio. Keeping this in
    /// editor state makes the preview survive redraws without coupling the
    /// graph model to egui.
    pub preview_open: bool,
    pub preview_target: GraphPreviewTarget,
    pub preview_node: Option<NodeId>,
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

/// What Graph Studio should show in its inspection view.
///
/// `SelectedNode` is deliberately separate from the material channels: it is
/// the way to inspect an intermediate noise, ramp, or displacement step before
/// it reaches the surface output.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum GraphPreviewTarget {
    SelectedNode,
    Final,
    BaseColor,
    Specular,
    AmbientOcclusion,
    Displacement,
    Roughness,
    Normal,
    Emission,
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
            preview_open: false,
            preview_target: GraphPreviewTarget::SelectedNode,
            preview_node: None,
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
        self.preview_node = None;
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
        // Saved graphs carry authored layout coordinates, which may be far outside
        // the compact drawer's initial viewport. Frame the complete graph on open
        // so loading a valid asset never presents an apparently empty canvas.
        self.frame_all_requested = true;
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
        self.preview_node = None;
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
        self.frame_all_requested = true;
        self.frame_selection_requested = false;
        self.collapsed_nodes.clear();
        self.clipboard = None;
        self.diagnostics.clear();
        self.compile_requested = true;
        self.status = "New graph".to_string();
    }

    pub fn add_node(&mut self, node_type: NodeTypeId, registry: &NodeRegistry) {
        let operation = registry
            .find(&node_type)
            .and_then(|declaration| MaterialNodeOperation::from_tag(declaration.operation));
        if operation == Some(MaterialNodeOperation::PatternLayer) {
            self.add_pattern_layer(registry);
            return;
        }
        if operation == Some(MaterialNodeOperation::Displacement) {
            self.add_displacement(registry);
            return;
        }
        let column = self.graph.nodes.len() % 4;
        let row = self.graph.nodes.len() / 4;
        let position = [80.0 + column as f32 * 250.0, 80.0 + row as f32 * 180.0];
        if operation.is_some_and(is_pattern_generator) {
            self.add_pattern_layer_with_generator(registry, node_type, Some(position));
            return;
        }
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
        let operation = registry
            .find(&node_type)
            .and_then(|declaration| MaterialNodeOperation::from_tag(declaration.operation));
        if operation.is_some_and(is_pattern_generator) {
            return self.add_pattern_layer_with_generator(registry, node_type, Some(position));
        }
        if operation == Some(MaterialNodeOperation::Displacement) {
            return self.add_displacement(registry);
        }
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
        self.add_pattern_layer_with_generator(
            registry,
            NodeTypeId(PATTERN_NOISE_NODE.to_string()),
            None,
        )
    }

    /// Insert a displacement modifier after the current surface chain and feed it
    /// from the most recently authored pattern source. The editor never leaves a
    /// required surface or height socket dangling.
    pub fn add_displacement(&mut self, registry: &NodeRegistry) -> Option<NodeId> {
        let mut chain = match resolve_material_surface_chain(&self.graph, registry) {
            Ok(chain) => chain,
            Err(error) => {
                self.status = error.to_string();
                return None;
            }
        };
        let pattern_layer = chain.layers.iter().rev().find(|node| {
            self.graph
                .nodes
                .get(*node)
                .and_then(|record| registry.find(&record.node_type))
                .is_some_and(|declaration| {
                    declaration.operation == MaterialNodeOperation::PatternLayer.tag()
                })
        });
        if pattern_layer.is_none() {
            self.add_pattern_layer(registry)?;
            chain = resolve_material_surface_chain(&self.graph, registry).ok()?;
        }
        let pattern_layer = chain.layers.iter().rev().find(|node| {
            self.graph
                .nodes
                .get(*node)
                .and_then(|record| registry.find(&record.node_type))
                .is_some_and(|declaration| {
                    declaration.operation == MaterialNodeOperation::PatternLayer.tag()
                })
        })?;
        let generator = self
            .graph
            .links
            .values()
            .find(|link| link.to.node == *pattern_layer && link.to.socket.0 == "pattern")
            .map(|link| link.from.node.clone())?;
        let predecessor = chain.layers.last().unwrap_or(&chain.surface).clone();
        let output_link = self
            .graph
            .links
            .iter()
            .find(|(_, link)| {
                link.from.node == predecessor
                    && link.from.socket.0 == "surface"
                    && link.to.node == chain.output
                    && link.to.socket.0 == "surface"
            })
            .map(|(id, _)| id.clone())?;
        let predecessor_position = self
            .graph
            .layout
            .positions
            .get(&predecessor)
            .copied()
            .unwrap_or([440.0, 160.0]);
        let displacement = NodeId::new();
        let position = [
            predecessor_position[0] + 260.0,
            predecessor_position[1] + 120.0,
        ];
        let commands = vec![
            GraphCommand::Disconnect { id: output_link },
            GraphCommand::AddNode {
                id: displacement.clone(),
                node_type: NodeTypeId(DISPLACEMENT_NODE.to_string()),
                position,
            },
            GraphCommand::Connect {
                id: LinkId::new(),
                from: OutputPin {
                    node: predecessor,
                    socket: SocketKey("surface".into()),
                },
                to: InputPin {
                    node: displacement.clone(),
                    socket: SocketKey("surface".into()),
                },
            },
            GraphCommand::Connect {
                id: LinkId::new(),
                from: OutputPin {
                    node: generator,
                    socket: SocketKey("pattern".into()),
                },
                to: InputPin {
                    node: displacement.clone(),
                    socket: SocketKey("height".into()),
                },
            },
            GraphCommand::Connect {
                id: LinkId::new(),
                from: OutputPin {
                    node: displacement.clone(),
                    socket: SocketKey("surface".into()),
                },
                to: InputPin {
                    node: chain.output.clone(),
                    socket: SocketKey("surface".into()),
                },
            },
        ];
        if !self.apply(GraphCommand::Transaction { commands }, registry) {
            return None;
        }
        self.selected_node = Some(displacement.clone());
        self.selected_nodes.clear();
        self.selected_nodes.insert(displacement.clone());
        self.status = "Displacement added — height mask connected".to_string();
        Some(displacement)
    }

    /// Add a generator together with the Pattern Layer that consumes it, inserting
    /// both into the typed surface chain. Pattern generators have no useful
    /// standalone meaning, so the editor never leaves one dangling.
    fn add_pattern_layer_with_generator(
        &mut self,
        registry: &NodeRegistry,
        generator_type: NodeTypeId,
        requested_generator_position: Option<[f32; 2]>,
    ) -> Option<NodeId> {
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
        let generator_position =
            requested_generator_position.unwrap_or([layer_position[0], layer_position[1] - 280.0]);
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
                node_type: generator_type,
                position: generator_position,
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
        self.status = format!(
            "Layer {} added — pattern generator connected",
            chain.layers.len() + 1
        );
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
    ) -> Vec<&'static voxel_graph::NodeDeclaration> {
        let search_lowercase = self.search.to_ascii_lowercase();
        registry
            .declarations()
            .filter(|node| {
                node.kinds.contains(&self.graph.kind)
                    && self
                        .graph
                        .can_add_node_type(registry, &NodeTypeId(node.id.into()))
                    && (search_lowercase.is_empty() || node_matches_search(node, &search_lowercase))
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
        match self {
            Self::EventEnvelopeTooLong {
                hold_seconds,
                release_seconds,
                budget_seconds,
            } => write!(
                formatter,
                "event sensor hold ({hold_seconds}s) + release ({release_seconds}s) exceeds the \
                 {budget_seconds}s event lifetime, so the event would be reclaimed while the \
                 sensor is still fading"
            ),
            _ => write!(formatter, "material graph compilation failed: {self:?}"),
        }
    }
}
impl std::error::Error for MaterialGraphError {}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::test_support::{graph_driving_roughness, graph_with_output, node};
    use voxel_graph::{GraphCommand, GraphHistory, GraphKind, LinkId, NodeTypeId, OutputPin};
    #[test]
    fn one_ir_drives_cpu_preview_and_naga_valid_wgsl() {
        let registry = crate::CATALOGUE;
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
        let sample = program.evaluate(MaterialSampleContext::still(
            [1.0, 2.0, 3.0],
            [0.0, 1.0, 0.0],
        ));
        assert_eq!(sample.base_color, [0.2, 0.4, 0.6, 1.0]);
        assert_eq!(sample.roughness, 0.25);
        assert!(program.wgsl.contains("fn graph_material"));
    }
    #[test]
    fn linked_math_is_evaluated_once_in_both_backends() {
        let registry = crate::CATALOGUE;
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
                    id: voxel_graph::LinkId("roughness".into()),
                    from: OutputPin {
                        node: add,
                        socket: SocketKey("value".into()),
                    },
                    to: voxel_graph::InputPin {
                        node: output,
                        socket: SocketKey("roughness".into()),
                    },
                },
            )
            .unwrap();
        let program = compile(&graph, &registry).unwrap();
        assert_eq!(
            program
                .evaluate(MaterialSampleContext::still([0.0; 3], [0.0; 3]))
                .roughness,
            0.5
        );
    }

    // ---- S3: animation -------------------------------------------------------

    fn event_at(position: [f32; 3], radius_meters: f32) -> GpuWorldEvent {
        GpuWorldEvent {
            position_meters: position,
            radius_meters,
            started_epoch: 0.0,
            started_remainder_seconds: 0.0,
            ended_epoch: 0.0,
            ended_remainder_seconds: 0.0,
            channel: 0,
            strength: 1.0,
            open: 1.0,
            pad_row_b: 0.0,
        }
    }

    fn clock_at(seconds: f32) -> AnimationClockSample {
        let mut clock = voxel_material::animation_clock::AnimationClock::new();
        clock.advance(seconds, 1.0);
        clock.sample()
    }

    /// Deterministic mode is a frozen clock and an empty event field. It buys
    /// repeatability, and that is what gets asserted — NOT equality with "the
    /// same graph without the animation nodes", which is not a well-defined
    /// comparison: removing a linked node also changes topology and socket
    /// fallback, so the two graphs differ by construction.
    #[test]
    fn a_frozen_clock_and_no_events_make_evaluation_repeatable() {
        let registry = crate::CATALOGUE;
        let (graph, _) = graph_driving_roughness(
            "material.oscillator",
            "value",
            &[("sync", PropertyValue::Text("per_voxel".into()))],
        );
        let program = compile(&graph, &registry).unwrap();
        let context = MaterialSampleContext::still([3.5, 1.5, 9.25], [0.0, 1.0, 0.0]);
        let first = program.evaluate(context).roughness;
        for _ in 0..8 {
            assert_eq!(program.evaluate(context).roughness, first);
        }
    }

    /// `material.time` must be monotone. Returning the clock's remainder would
    /// have made it jump backwards every epoch, quietly breaking any graph
    /// doing arithmetic on it — a lava drift snapping to its origin every 64 s.
    #[test]
    fn time_is_monotone_across_an_epoch_boundary() {
        let registry = crate::CATALOGUE;
        let (graph, _) = graph_driving_roughness("material.time", "value", &[]);
        let program = compile(&graph, &registry).unwrap();
        let epoch_seconds = voxel_material::animation_clock::EPOCH_SECONDS;
        let before = program
            .evaluate(MaterialSampleContext {
                clock: clock_at(epoch_seconds - 0.5),
                ..MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0])
            })
            .roughness;
        let after = program
            .evaluate(MaterialSampleContext {
                clock: clock_at(epoch_seconds + 0.5),
                ..MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0])
            })
            .roughness;
        assert!(
            after > before,
            "time stepped backwards: {before} -> {after}"
        );
    }

    /// `per_voxel` must de-sync AUTHORED BLOCKS, not traversal cells.
    ///
    /// The graph's `position` is in 12.5 cm traversal units, so hashing it
    /// directly would give every detail cell of one block its own phase — a
    /// visible fizz instead of a block-level heartbeat. This pins the
    /// BRICK_SIZE conversion that `pattern_coordinate` also makes.
    #[test]
    fn per_voxel_sync_offsets_whole_blocks_not_detail_cells() {
        let registry = crate::CATALOGUE;
        let (graph, _) = graph_driving_roughness(
            "material.oscillator",
            "value",
            &[("sync", PropertyValue::Text("per_voxel".into()))],
        );
        let program = compile(&graph, &registry).unwrap();
        let sample = |position: [f32; 3]| {
            program
                .evaluate(MaterialSampleContext {
                    clock: clock_at(1.0),
                    ..MaterialSampleContext::still(position, [0.0, 1.0, 0.0])
                })
                .roughness
        };
        // Eight traversal cells inside ONE authored block (BRICK_SIZE = 8).
        let inside = sample([0.5, 0.5, 0.5]);
        for cell in 1..8 {
            assert_eq!(
                sample([cell as f32 + 0.5, 0.5, 0.5]),
                inside,
                "detail cell {cell} de-synced from its own block"
            );
        }
        // The neighbouring block must be somewhere else in the cycle.
        assert_ne!(sample([8.5, 0.5, 0.5]), inside);
    }

    /// A `global` oscillator must NOT vary with position — the single-heartbeat
    /// case a lava lake needs.
    #[test]
    fn global_sync_is_identical_everywhere() {
        let registry = crate::CATALOGUE;
        let (graph, _) = graph_driving_roughness("material.oscillator", "value", &[]);
        let program = compile(&graph, &registry).unwrap();
        let sample = |position: [f32; 3]| {
            program
                .evaluate(MaterialSampleContext {
                    clock: clock_at(0.3),
                    ..MaterialSampleContext::still(position, [0.0, 1.0, 0.0])
                })
                .roughness
        };
        assert_eq!(sample([0.5, 0.5, 0.5]), sample([812.5, 40.5, 3.5]));
    }

    /// The invariant the single-winner rule exists to protect. Taking an
    /// independent maximum per output could pair one event's nearness with
    /// another's envelope — a combination that never existed.
    #[test]
    fn sensor_outputs_all_describe_one_winning_event() {
        let registry = crate::CATALOGUE;
        let mut outputs = Vec::new();
        for socket in ["signal", "nearness", "envelope"] {
            let (graph, _) = graph_driving_roughness(
                "material.event_sensor",
                socket,
                &[
                    ("radius_meters", PropertyValue::Scalar(10.0)),
                    ("attack_seconds", PropertyValue::Scalar(0.0)),
                    ("falloff", PropertyValue::Text("linear".into())),
                ],
            );
            let program = compile(&graph, &registry).unwrap();
            let mut near = event_at([0.0, 0.0, 1.0], 10.0);
            near.strength = 0.5;
            let mut far = event_at([0.0, 0.0, 6.0], 10.0);
            far.strength = 1.0;
            let events = [near, far];
            outputs.push(
                program
                    .evaluate(MaterialSampleContext {
                        clock: clock_at(4.0),
                        events: &events,
                        ..MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0])
                    })
                    .roughness,
            );
        }
        let (signal, nearness, envelope) = (outputs[0], outputs[1], outputs[2]);
        assert!(signal > 0.0 && nearness > 0.0 && envelope > 0.0);
        // The winner is the nearer, weaker event: 0.5 strength beats the far
        // one's 1.0 because nearness dominates here.
        assert!(
            (signal - nearness * envelope * 0.5).abs() < 1e-5,
            "signal {signal} is not nearness {nearness} * envelope {envelope} * strength"
        );
    }

    /// Several sensors with different channels and radii must each evaluate
    /// their OWN configuration. A single precomputed scalar on the context
    /// could not have represented this, which is why the context carries the
    /// raw event list instead.
    #[test]
    fn two_sensors_on_different_channels_evaluate_independently() {
        let registry = crate::CATALOGUE;
        let evaluate = |channel: i64, radius: f32, events: &[GpuWorldEvent]| {
            let (graph, _) = graph_driving_roughness(
                "material.event_sensor",
                "nearness",
                &[
                    ("channel", PropertyValue::Integer(channel)),
                    ("radius_meters", PropertyValue::Scalar(radius)),
                    ("attack_seconds", PropertyValue::Scalar(0.0)),
                    ("falloff", PropertyValue::Text("linear".into())),
                ],
            );
            compile(&graph, &registry)
                .unwrap()
                .evaluate(MaterialSampleContext {
                    clock: clock_at(4.0),
                    events,
                    ..MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0])
                })
                .roughness
        };
        let mut other_channel = event_at([0.0, 0.0, 2.0], 10.0);
        other_channel.channel = 7;
        let events = [event_at([0.0, 0.0, 2.0], 10.0), other_channel];

        // Channel 0 at a generous radius sees the presence event.
        assert!(evaluate(0, 10.0, &events) > 0.0);
        // Channel 7 sees its own, at the same distance.
        assert!(evaluate(7, 10.0, &events) > 0.0);
        // Channel 3 sees neither.
        assert_eq!(evaluate(3, 10.0, &events), 0.0);
        // And a tight radius rejects an event that a wide one accepted, which
        // is the per-sensor radius doing its job.
        assert_eq!(evaluate(0, 1.0, &events), 0.0);
    }

    /// S3b — the light volume's view of a program, built straight from IR so the
    /// dataflow walk is tested rather than a graph editor's plumbing.
    ///
    /// `emission_drives_the_sensor` decides whether the sensor's signal reaches
    /// the emission output or only the roughness one. That is the whole question
    /// [`MaterialGraphProgram::emission_event_sensor`] answers, and the reason it
    /// is a reachability walk instead of a scan.
    fn sensing_program(emission_drives_the_sensor: bool) -> MaterialGraphProgram {
        let config = EventSensorConfig {
            channel: 0,
            radius_meters: 8.0,
            falloff: SensorFalloff::Linear,
            attack_seconds: 0.0,
            hold_seconds: 0.0,
            release_seconds: 0.0,
            invert: false,
        };
        let instructions = vec![
            MaterialInstruction::Color([1.0, 1.0, 1.0, 1.0]),
            MaterialInstruction::EventSensor { config },
            MaterialInstruction::Component {
                vector: ValueId(1),
                axis: 0,
            },
            MaterialInstruction::ColorScale {
                color: ValueId(0),
                strength: ValueId(2),
            },
            MaterialInstruction::Scalar(0.0),
            MaterialInstruction::Vector3([0.0, 0.0, 0.0]),
        ];
        MaterialGraphProgram {
            graph_id: AssetId("sensing".into()),
            semantic_hash: 0,
            instructions,
            output: MaterialOutput {
                base_color: ValueId(0),
                // The sensor drives ONE of these two, never both.
                roughness: ValueId(2),
                emission: if emission_drives_the_sensor {
                    ValueId(3)
                } else {
                    ValueId(0)
                },
                specular: ValueId(4),
                ambient_occlusion: ValueId(4),
                normal: ValueId(5),
                specular_active: false,
                layer_animation: Vec::new(),
            },
            wgsl: String::new(),
            // Hand-built IR fixture: there is no graph to analyse, and these
            // tests are about evaluation, not caching.
            cache: CacheReport::default(),
        }
    }

    /// The positive case: a sensor on the emission path yields both endpoints.
    #[test]
    fn an_emission_gating_sensor_reports_both_ends_of_its_range() {
        let response = sensing_program(true)
            .emission_event_response(MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0]))
            .expect("a sensor feeding emission must produce a response");
        assert_eq!(response.sensor.radius_meters, 8.0);
        assert_eq!(response.resting, [0.0; 3], "no event means no signal");
        assert_eq!(
            response.triggered, [1.0; 3],
            "the probe sits ON the sample point, so the signal must saturate"
        );
    }

    /// An oscillator on the emission path must reach the volume as its MEAN, not
    /// as whatever phase the sampling clock happened to sit at.
    ///
    /// This is a regression test for a real defect, not a formality. The endpoint
    /// used to be one sample at `MaterialSampleContext::still`, whose clock is
    /// frozen at zero — and a sine at phase 0 is at its TROUGH. The authored glow
    /// block swings 0.45..1.25 on the surface and handed the light volume a flat
    /// 0.45, so the room read at 56% of what the block looked like. Nothing caught
    /// it until the emitter was actually looked at.
    #[test]
    fn an_oscillator_on_the_emission_path_reaches_the_volume_as_its_mean() {
        let config = EventSensorConfig {
            channel: 0,
            radius_meters: 8.0,
            falloff: SensorFalloff::Linear,
            attack_seconds: 0.0,
            hold_seconds: 0.0,
            release_seconds: 0.0,
            invert: false,
        };
        // white x (sensor.signal x sine[0.4 .. 1.2] @ 1.4 Hz)
        let instructions = vec![
            MaterialInstruction::Color([1.0, 1.0, 1.0, 1.0]),
            MaterialInstruction::EventSensor { config },
            MaterialInstruction::Component {
                vector: ValueId(1),
                axis: 0,
            },
            MaterialInstruction::Scalar(1.4),
            MaterialInstruction::Scalar(0.0),
            MaterialInstruction::Scalar(0.5),
            MaterialInstruction::Scalar(0.4),
            MaterialInstruction::Scalar(1.2),
            MaterialInstruction::Oscillator {
                wave: OscillatorWave::Sine,
                sync: PhaseSync::Global,
                seed: 0,
                rate_hz: ValueId(3),
                phase: ValueId(4),
                duty: ValueId(5),
                low: ValueId(6),
                high: ValueId(7),
            },
            MaterialInstruction::MultiplyScalar(ValueId(2), ValueId(8)),
            MaterialInstruction::ColorScale {
                color: ValueId(0),
                strength: ValueId(9),
            },
            MaterialInstruction::Scalar(0.0),
            MaterialInstruction::Vector3([0.0, 0.0, 0.0]),
        ];
        let program = MaterialGraphProgram {
            graph_id: AssetId("oscillating".into()),
            semantic_hash: 0,
            instructions,
            output: MaterialOutput {
                base_color: ValueId(0),
                roughness: ValueId(4),
                emission: ValueId(10),
                specular: ValueId(11),
                ambient_occlusion: ValueId(11),
                normal: ValueId(12),
                specular_active: false,
                layer_animation: Vec::new(),
            },
            wgsl: String::new(),
            // Hand-built IR fixture: there is no graph to analyse, and these
            // tests are about evaluation, not caching.
            cache: CacheReport::default(),
        };

        let response = program
            .emission_event_response(MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0]))
            .expect("an oscillator behind a sensor still gates emission");
        assert_eq!(
            response.resting, [0.0; 3],
            "no event means no signal at all"
        );
        // The sine's mean over one period is the midpoint of its range. The old
        // one-sample form returned `low` (0.4) here, which is what this pins.
        let mean = 0.5 * (0.4 + 1.2);
        assert!(
            (response.triggered[0] - mean).abs() < 0.01,
            "expected the sine's mean {mean}, got {:?} — a value near 0.4 means the \
             endpoint went back to a single frozen-clock sample",
            response.triggered
        );
    }

    /// The negative case, and the reason the walk exists: a sensor wired only
    /// into roughness must NOT claim one of the volume's seven response slots.
    /// A scan of every instruction would hand it one.
    #[test]
    fn a_sensor_that_drives_only_roughness_claims_no_emission_response() {
        assert!(sensing_program(false)
            .emission_event_response(MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0]))
            .is_none());
    }

    /// A graph with no sensor at all has no response — the case every shipped
    /// material is in, and the one that keeps the volume bit-identical.
    #[test]
    fn a_graph_without_a_sensor_has_no_emission_response() {
        let registry = crate::CATALOGUE;
        let (graph, _) = graph_with_output();
        assert!(compile(&graph, &registry)
            .unwrap()
            .emission_event_response(MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0]))
            .is_none());
    }

    /// `invert` must touch the signal only. Nearness and envelope keep their
    /// literal meanings so they stay usable as diagnostics.
    #[test]
    fn invert_flips_the_signal_and_leaves_the_diagnostics_literal() {
        let registry = crate::CATALOGUE;
        let read = |socket: &str, invert: bool| {
            let (graph, _) = graph_driving_roughness(
                "material.event_sensor",
                socket,
                &[
                    ("invert", PropertyValue::Boolean(invert)),
                    ("radius_meters", PropertyValue::Scalar(10.0)),
                    ("attack_seconds", PropertyValue::Scalar(0.0)),
                ],
            );
            let events = [event_at([0.0, 0.0, 2.0], 10.0)];
            compile(&graph, &registry)
                .unwrap()
                .evaluate(MaterialSampleContext {
                    clock: clock_at(4.0),
                    events: &events,
                    ..MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0])
                })
                .roughness
        };
        assert!((read("signal", false) + read("signal", true) - 1.0).abs() < 1e-5);
        assert_eq!(read("nearness", false), read("nearness", true));
        assert_eq!(read("envelope", false), read("envelope", true));
    }

    /// The envelope must be continuous — including the awkward case the
    /// multiplied-factor form exists for: an event that opens and closes inside
    /// one frame while its attack is still ramping.
    #[test]
    fn the_envelope_is_continuous_including_an_impulse_that_closes_during_attack() {
        let registry = crate::CATALOGUE;
        let (graph, _) = graph_driving_roughness(
            "material.event_sensor",
            "envelope",
            &[
                ("radius_meters", PropertyValue::Scalar(10.0)),
                ("attack_seconds", PropertyValue::Scalar(0.4)),
                ("hold_seconds", PropertyValue::Scalar(0.2)),
                ("release_seconds", PropertyValue::Scalar(1.0)),
            ],
        );
        let program = compile(&graph, &registry).unwrap();
        // An impulse: opened and closed at t = 0.
        let mut impulse = event_at([0.0, 0.0, 1.0], 10.0);
        impulse.open = 0.0;
        let events = [impulse];
        let sample = |seconds: f32| {
            program
                .evaluate(MaterialSampleContext {
                    clock: clock_at(seconds),
                    events: &events,
                    ..MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0])
                })
                .roughness
        };
        let mut previous = sample(0.0);
        let mut peak: f32 = 0.0;
        let steps = 400;
        for step in 1..=steps {
            let value = sample(step as f32 * 0.005);
            assert!(
                (value - previous).abs() < 0.05,
                "envelope stepped at t={}: {previous} -> {value}",
                step as f32 * 0.005
            );
            peak = peak.max(value);
            previous = value;
        }
        // A shortened blip: it rises, but the release eats it before the attack
        // ever completes, so it never reaches full.
        assert!(peak > 0.0 && peak < 1.0, "impulse peak was {peak}");
        assert!(
            previous < 1e-3,
            "impulse never released, ended at {previous}"
        );
    }

    /// An ongoing event ramps up over its attack and holds there — the "walk
    /// up and stop" case. This is what a distance-only sensor could not do.
    #[test]
    fn an_ongoing_event_ramps_over_the_attack_then_holds() {
        let registry = crate::CATALOGUE;
        let (graph, _) = graph_driving_roughness(
            "material.event_sensor",
            "envelope",
            &[
                ("radius_meters", PropertyValue::Scalar(10.0)),
                ("attack_seconds", PropertyValue::Scalar(0.4)),
            ],
        );
        let program = compile(&graph, &registry).unwrap();
        let events = [event_at([0.0, 0.0, 1.0], 10.0)];
        let sample = |seconds: f32| {
            program
                .evaluate(MaterialSampleContext {
                    clock: clock_at(seconds),
                    events: &events,
                    ..MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0])
                })
                .roughness
        };
        assert!((sample(0.0) - 0.0).abs() < 1e-5);
        assert!(
            (sample(0.2) - 0.5).abs() < 1e-3,
            "midway was {}",
            sample(0.2)
        );
        assert!((sample(0.4) - 1.0).abs() < 1e-5);
        // Standing still holds it at full rather than freezing it mid-ramp.
        assert!((sample(30.0) - 1.0).abs() < 1e-5);
    }

    /// The one cross-field rule: a sensor may not still be fading after its
    /// event has been reclaimed. Capping each field alone would not catch this
    /// — it is the SUM that has to fit the budget.
    #[test]
    fn a_sensor_envelope_longer_than_the_event_lifetime_is_a_compile_error() {
        let registry = crate::CATALOGUE;
        let budget = MAX_EVENT_LIFETIME_SECONDS;
        let (graph, _) = graph_driving_roughness(
            "material.event_sensor",
            "signal",
            &[
                ("hold_seconds", PropertyValue::Scalar(budget * 0.75)),
                ("release_seconds", PropertyValue::Scalar(budget * 0.75)),
            ],
        );
        assert!(matches!(
            compile(&graph, &registry),
            Err(MaterialGraphError::EventEnvelopeTooLong { .. })
        ));

        // Each field is individually legal at three quarters of the budget —
        // proof the check is on the sum and not on either one.
        let (legal, _) = graph_driving_roughness(
            "material.event_sensor",
            "signal",
            &[
                ("hold_seconds", PropertyValue::Scalar(budget * 0.75)),
                ("release_seconds", PropertyValue::Scalar(budget * 0.25)),
            ],
        );
        assert!(compile(&legal, &registry).is_ok());
    }

    /// Gating: a sensor's signal times an oscillator. The composition the whole
    /// "trigger a pulse" feature is made of, and the reason multiply exists.
    #[test]
    fn a_sensor_gates_an_oscillator_through_multiply() {
        let registry = crate::CATALOGUE;
        let (mut graph, output) = graph_with_output();
        let mut history = GraphHistory::default();
        let sensor = node("sensor");
        let oscillator = node("oscillator");
        let gate = node("gate");
        for (id, node_type) in [
            (&sensor, "material.event_sensor"),
            (&oscillator, "material.oscillator"),
            (&gate, "material.multiply_scalar"),
        ] {
            history
                .apply(
                    &mut graph,
                    &registry,
                    GraphCommand::AddNode {
                        id: id.clone(),
                        node_type: NodeTypeId(node_type.into()),
                        position: [0.0, 0.0],
                    },
                )
                .unwrap();
        }
        let sensor_record = graph.nodes.get_mut(&sensor).unwrap();
        sensor_record
            .properties
            .insert("radius_meters".into(), PropertyValue::Scalar(10.0));
        sensor_record
            .properties
            .insert("attack_seconds".into(), PropertyValue::Scalar(0.0));
        for (from, socket, to, input) in [
            (&sensor, "signal", &gate, "a"),
            (&oscillator, "value", &gate, "b"),
        ] {
            history
                .apply(
                    &mut graph,
                    &registry,
                    GraphCommand::Connect {
                        id: voxel_graph::LinkId(format!("{socket}-{input}")),
                        from: OutputPin {
                            node: from.clone(),
                            socket: SocketKey(socket.into()),
                        },
                        to: voxel_graph::InputPin {
                            node: to.clone(),
                            socket: SocketKey(input.into()),
                        },
                    },
                )
                .unwrap();
        }
        history
            .apply(
                &mut graph,
                &registry,
                GraphCommand::Connect {
                    id: voxel_graph::LinkId("gate-roughness".into()),
                    from: OutputPin {
                        node: gate,
                        socket: SocketKey("value".into()),
                    },
                    to: voxel_graph::InputPin {
                        node: output,
                        socket: SocketKey("roughness".into()),
                    },
                },
            )
            .unwrap();
        let program = compile(&graph, &registry).unwrap();
        let near = [event_at([0.0, 0.0, 1.0], 10.0)];

        // Nothing nearby: the gate is shut, whatever the oscillator is doing.
        for step in 0..16 {
            let value = program
                .evaluate(MaterialSampleContext {
                    clock: clock_at(step as f32 * 0.1),
                    ..MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0])
                })
                .roughness;
            assert_eq!(value, 0.0, "the gate leaked with no event present");
        }

        // Something nearby: the oscillator comes through and actually moves.
        let mut minimum = f32::MAX;
        let mut maximum = f32::MIN;
        for step in 0..32 {
            let value = program
                .evaluate(MaterialSampleContext {
                    clock: clock_at(step as f32 * 0.05),
                    events: &near,
                    ..MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0])
                })
                .roughness;
            minimum = minimum.min(value);
            maximum = maximum.max(value);
        }
        assert!(
            maximum - minimum > 0.1,
            "gated oscillator did not move: {minimum}..{maximum}"
        );
    }

    /// Animation must land on the layer the author connected it to, at the slot
    /// the material row uploads it in. Getting the index wrong would animate a
    /// different layer and look like a working feature.
    #[test]
    fn layer_animation_lands_on_the_slot_its_layer_occupies() {
        let registry = crate::CATALOGUE;
        let graph = graph_from_material(&voxel_material::material::MATERIALS[6]);
        let mut editor = GraphEditorState::new(6);
        editor.open_graph(6, graph);
        let first = editor.add_pattern_layer(&registry).expect("first layer");
        let second = editor.add_pattern_layer(&registry).expect("second layer");
        assert_eq!(
            resolve_material_surface_chain(&editor.graph, &registry)
                .unwrap()
                .layers,
            vec![first.clone(), second.clone()]
        );

        // Drive the SECOND layer's gain with a distinctive constant.
        editor
            .graph
            .nodes
            .get_mut(&second)
            .unwrap()
            .socket_defaults
            .insert(
                SocketKey("animation_gain".into()),
                PropertyValue::Scalar(0.375),
            );
        editor
            .graph
            .nodes
            .get_mut(&second)
            .unwrap()
            .socket_defaults
            .insert(
                SocketKey("drift_velocity".into()),
                PropertyValue::Vector3([0.25, 0.0, 0.0]),
            );

        let program = compile(&editor.graph, &registry).unwrap();
        assert_eq!(program.output.layer_animation.len(), 2);
        let gains: Vec<_> = program
            .output
            .layer_animation
            .iter()
            .map(|animation| match program.instructions[animation.gain.0] {
                MaterialInstruction::Scalar(value) => value,
                _ => panic!("gain should have lowered to a constant"),
            })
            .collect();
        assert_eq!(gains, vec![1.0, 0.375], "gain landed on the wrong slot");

        // ...and the emitted function builds the struct in slot order. Checked
        // against the STRIPPED function, not the whole program: the prefix
        // carries `pattern_animation_identity`, whose body would otherwise
        // satisfy a naive text match.
        let body = program.wgsl_function("graph_material_6");
        let built = body
            .lines()
            .find(|line| line.contains("let animation = PatternAnimation("))
            .expect("the generated function builds a PatternAnimation");
        let gain_slots: Vec<&str> = built
            .split("vec4<f32>(")
            .nth(1)
            .and_then(|rest| rest.split(')').next())
            .expect("gain vector")
            .split(", ")
            .collect();
        assert_eq!(gain_slots.len(), 4);
        assert_eq!(
            gain_slots[0],
            format!("v{}", program.output.layer_animation[0].gain.0)
        );
        assert_eq!(
            gain_slots[1],
            format!("v{}", program.output.layer_animation[1].gain.0)
        );
        assert_eq!(gain_slots[2], "1.0", "unused slots must be the identity");
        assert_eq!(gain_slots[3], "1.0");
    }

    /// A DISABLED layer occupies no uploaded slot, so it must occupy no
    /// animation slot either — otherwise every layer after it animates from the
    /// wrong index.
    #[test]
    fn a_disabled_layer_consumes_no_animation_slot() {
        let registry = crate::CATALOGUE;
        let graph = graph_from_material(&voxel_material::material::MATERIALS[6]);
        let mut editor = GraphEditorState::new(6);
        editor.open_graph(6, graph);
        let first = editor.add_pattern_layer(&registry).expect("first layer");
        let second = editor.add_pattern_layer(&registry).expect("second layer");
        editor
            .graph
            .nodes
            .get_mut(&second)
            .unwrap()
            .socket_defaults
            .insert(
                SocketKey("animation_gain".into()),
                PropertyValue::Scalar(0.375),
            );
        editor
            .graph
            .nodes
            .get_mut(&first)
            .unwrap()
            .properties
            .insert("enabled".into(), PropertyValue::Boolean(false));

        let program = compile(&editor.graph, &registry).unwrap();
        let stack = crate::layers::project_pattern_stack(&editor.graph, &registry).unwrap();
        assert_eq!(stack.active_count(), 1, "the disabled layer still uploaded");
        assert_eq!(
            program.output.layer_animation.len(),
            1,
            "the disabled layer still claimed an animation slot"
        );
        match program.instructions[program.output.layer_animation[0].gain.0] {
            MaterialInstruction::Scalar(value) => assert_eq!(
                value, 0.375,
                "the surviving layer's gain shifted to the wrong slot"
            ),
            _ => panic!("gain should have lowered to a constant"),
        }
    }

    /// An un-animated graph must emit the identity, so a material authored
    /// before S3 behaves exactly as it did.
    #[test]
    fn a_graph_without_animation_sockets_emits_the_identity() {
        let registry = crate::CATALOGUE;
        let graph = graph_from_material(&voxel_material::material::MATERIALS[26]);
        let program = compile(&graph, &registry).unwrap();
        assert_eq!(
            program.output.layer_animation.len(),
            1,
            "lava authors exactly one pattern layer"
        );
        for animation in &program.output.layer_animation {
            assert!(
                matches!(
                    program.instructions[animation.gain.0],
                    MaterialInstruction::Scalar(value) if value == 1.0
                ),
                "an unconnected gain must lower to exactly 1.0"
            );
            assert!(
                matches!(
                    program.instructions[animation.drift_velocity.0],
                    MaterialInstruction::Vector3(value) if value == [0.0; 3]
                ),
                "an unconnected drift must lower to zero"
            );
        }
    }

    /// A disabled oscillator is REMOVED, not frozen: whatever it fed falls back
    /// to that socket's own default. That is what makes the toggle mean "as it
    /// was before I added this", and it is why there is no authored
    /// value-while-disabled — the neutral value belongs to the consumer, and a
    /// layer gain (1.0) and a mix factor (0.0) do not share one.
    #[test]
    fn a_disabled_oscillator_leaves_its_consumer_on_the_socket_default() {
        let registry = crate::CATALOGUE;
        let build = |enabled: bool| {
            let (graph, _) = graph_driving_roughness(
                "material.oscillator",
                "value",
                // `low`/`high` are input SOCKETS, not properties; their
                // declared defaults are already 0.0 and 1.0.
                &[("enabled", PropertyValue::Boolean(enabled))],
            );
            compile(&graph, &registry).unwrap()
        };

        // Enabled, the roughness moves with the clock.
        let running = build(true);
        let sample = |program: &MaterialGraphProgram, seconds: f32| {
            program
                .evaluate(MaterialSampleContext {
                    clock: clock_at(seconds),
                    ..MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0])
                })
                .roughness
        };
        assert!(
            (0..16).any(|step| sample(&running, step as f32 * 0.05) != sample(&running, 0.0)),
            "an enabled oscillator did not animate"
        );

        // Disabled, the surface's roughness socket falls back to its default
        // and stops moving entirely.
        let stopped = build(false);
        let held = sample(&stopped, 0.0);
        for step in 0..16 {
            assert_eq!(sample(&stopped, step as f32 * 0.37), held);
        }
        // ...and the node is gone from the emitted code, so it costs nothing.
        assert!(
            !stopped
                .wgsl_function("graph_material_6")
                .contains("graph_oscillator("),
            "a disabled oscillator still emitted shader code"
        );
    }

    /// The case that matters for the authored lava: disabling the oscillator on
    /// a layer's gain must leave the layer at its AUTHORED amount, not at the
    /// bottom of the oscillator's range.
    #[test]
    fn disabling_a_layer_gain_oscillator_restores_the_authored_amount() {
        let registry = crate::CATALOGUE;
        let build = |enabled: bool| {
            let graph = graph_from_material(&voxel_material::material::MATERIALS[6]);
            let mut editor = GraphEditorState::new(6);
            editor.open_graph(6, graph);
            let layer = editor.add_pattern_layer(&registry).expect("layer");
            let oscillator = node("oscillator");
            let mut history = GraphHistory::default();
            history
                .apply(
                    &mut editor.graph,
                    &registry,
                    GraphCommand::AddNode {
                        id: oscillator.clone(),
                        node_type: NodeTypeId("material.oscillator".into()),
                        position: [0.0, 0.0],
                    },
                )
                .unwrap();
            let record = editor.graph.nodes.get_mut(&oscillator).unwrap();
            record
                .properties
                .insert("enabled".into(), PropertyValue::Boolean(enabled));
            record
                .socket_defaults
                .insert(SocketKey("low".into()), PropertyValue::Scalar(0.7));
            record
                .socket_defaults
                .insert(SocketKey("high".into()), PropertyValue::Scalar(1.15));
            history
                .apply(
                    &mut editor.graph,
                    &registry,
                    GraphCommand::Connect {
                        id: voxel_graph::LinkId("gain".into()),
                        from: OutputPin {
                            node: oscillator,
                            socket: SocketKey("value".into()),
                        },
                        to: voxel_graph::InputPin {
                            node: layer,
                            socket: SocketKey("animation_gain".into()),
                        },
                    },
                )
                .unwrap();
            compile(&editor.graph, &registry).unwrap()
        };

        let disabled = build(false);
        let gain = disabled.output.layer_animation[0].gain;
        assert!(
            matches!(
                disabled.instructions[gain.0],
                MaterialInstruction::Scalar(value) if value == 1.0
            ),
            "a disabled gain oscillator must leave the layer at unit gain, not \
             at the bottom of its own range"
        );

        // Sanity: enabled, the gain is a live expression rather than a constant.
        let enabled = build(true);
        let gain = enabled.output.layer_animation[0].gain;
        assert!(!matches!(
            enabled.instructions[gain.0],
            MaterialInstruction::Scalar(_)
        ));
    }

    /// Speed and angles must mean what the field descriptions say, in both
    /// backends. -90 elevation is straight down; azimuth 0 is +X and 90 is +Z.
    #[test]
    fn direction_turns_speed_and_angles_into_the_documented_vector() {
        let registry = crate::CATALOGUE;
        let read = |azimuth: f32, elevation: f32, speed: f32| {
            let (mut graph, output) = graph_with_output();
            let mut history = GraphHistory::default();
            let direction = node("direction");
            // `position_component` reads the POSITION, not an arbitrary vector,
            // so a dot with a unit basis vector is how a component is read out.
            let component = node("component");
            for (id, node_type) in [
                (&direction, "material.direction"),
                (&component, "material.dot_vector"),
            ] {
                history
                    .apply(
                        &mut graph,
                        &registry,
                        GraphCommand::AddNode {
                            id: id.clone(),
                            node_type: NodeTypeId(node_type.into()),
                            position: [0.0, 0.0],
                        },
                    )
                    .unwrap();
            }
            let record = graph.nodes.get_mut(&direction).unwrap();
            for (key, value) in [
                ("azimuth_degrees", azimuth),
                ("elevation_degrees", elevation),
                ("speed", speed),
            ] {
                record
                    .socket_defaults
                    .insert(SocketKey(key.into()), PropertyValue::Scalar(value));
            }
            // Read each axis out through a component node.
            let mut axes = [0.0_f32; 3];
            for (axis, slot) in axes.iter_mut().enumerate() {
                let mut probe = graph.clone();
                let mut basis = [0.0_f32; 3];
                basis[axis] = 1.0;
                probe
                    .nodes
                    .get_mut(&component)
                    .unwrap()
                    .socket_defaults
                    .insert(SocketKey("b".into()), PropertyValue::Vector3(basis));
                let mut history = GraphHistory::default();
                history
                    .apply(
                        &mut probe,
                        &registry,
                        GraphCommand::Connect {
                            id: voxel_graph::LinkId("dir".into()),
                            from: OutputPin {
                                node: direction.clone(),
                                socket: SocketKey("vector".into()),
                            },
                            to: voxel_graph::InputPin {
                                node: component.clone(),
                                socket: SocketKey("a".into()),
                            },
                        },
                    )
                    .unwrap();
                history
                    .apply(
                        &mut probe,
                        &registry,
                        GraphCommand::Connect {
                            id: voxel_graph::LinkId("out".into()),
                            from: OutputPin {
                                node: component.clone(),
                                socket: SocketKey("value".into()),
                            },
                            to: voxel_graph::InputPin {
                                node: output.clone(),
                                socket: SocketKey("roughness".into()),
                            },
                        },
                    )
                    .unwrap();
                *slot = compile(&probe, &registry)
                    .unwrap()
                    .evaluate(MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0]))
                    .roughness;
            }
            axes
        };

        let close = |got: [f32; 3], want: [f32; 3]| {
            assert!(
                (0..3).all(|axis| (got[axis] - want[axis]).abs() < 1e-4),
                "got {got:?}, want {want:?}"
            );
        };
        // Straight down at 0.25 m/s — the lava case.
        close(read(0.0, -90.0, 0.25), [0.0, -0.25, 0.0]);
        // Level, azimuth 0 is +X.
        close(read(0.0, 0.0, 1.0), [1.0, 0.0, 0.0]);
        // Level, azimuth 90 is +Z.
        close(read(90.0, 0.0, 1.0), [0.0, 0.0, 1.0]);
        // Zero speed is a standstill whatever the angles say.
        close(read(37.0, 12.0, 0.0), [0.0, 0.0, 0.0]);
    }

    #[test]
    fn invalid_edit_keeps_the_last_known_good_program_active() {
        let registry = crate::CATALOGUE;
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
                .evaluate(MaterialSampleContext::still([0.0; 3], [0.0; 3]))
                .roughness,
            0.2
        );
    }

    #[test]
    fn canonical_material_graph_preserves_face_roles_in_graph_preview() {
        let registry = crate::CATALOGUE;
        let material = voxel_material::material::MATERIALS[1];
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
        let top = program.evaluate(MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0]));
        let side = program.evaluate(MaterialSampleContext::still([0.0; 3], [1.0, 0.0, 0.0]));
        let roles = material.face_roles.unwrap();
        assert_eq!(top.base_color[0], roles.top.albedo[0]);
        assert_eq!(side.base_color[0], roles.side.albedo[0]);
        assert_eq!(top.roughness, roles.top.roughness);
        assert_eq!(side.roughness, roles.side.roughness);
    }

    #[test]
    fn every_compiled_material_has_an_openable_graph_representation() {
        let registry = crate::CATALOGUE;
        for (slot, material) in voxel_material::material::MATERIALS.iter().enumerate() {
            let graph = graph_from_material(material);
            let program = compile(&graph, &registry).unwrap_or_else(|error| {
                panic!("material slot {slot} ({}) failed: {error}", material.name)
            });
            let sample = program.evaluate(MaterialSampleContext::still(
                [0.25, 0.5, -0.75],
                [0.0, 1.0, 0.0],
            ));
            assert!(sample.base_color.iter().all(|value| value.is_finite()));
            assert!(sample.roughness.is_finite());
            assert!(sample.emission.iter().all(|value| value.is_finite()));
        }
    }

    #[test]
    fn editor_adds_nodes_through_commands_with_inspectable_defaults() {
        let registry = crate::CATALOGUE;
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
        let registry = crate::CATALOGUE;
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
    fn editor_connects_pattern_generators_into_a_surface_layer() {
        let registry = crate::CATALOGUE;
        let mut editor = GraphEditorState::new(6);
        editor.add_node(NodeTypeId("material.pattern_edge_band".into()), &registry);

        let chain = resolve_material_surface_chain(&editor.graph, &registry).unwrap();
        assert_eq!(chain.layers.len(), 1);
        let layer = &chain.layers[0];
        let generator = editor
            .graph
            .links
            .values()
            .find(|link| link.to.node == *layer && link.to.socket.0 == "pattern")
            .map(|link| link.from.node.clone())
            .unwrap();
        assert_eq!(
            editor.graph.nodes[&generator].node_type,
            NodeTypeId("material.pattern_edge_band".into())
        );
        assert!(editor
            .graph
            .links
            .values()
            .any(|link| { link.from.node == *layer && link.to.node == chain.output }));
    }

    #[test]
    fn node_catalog_hides_types_at_their_declared_instance_limit() {
        let registry = crate::CATALOGUE;
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

    /// A node type whose only occurrence of "gloss" and "hemisphere" is on its
    /// sockets — the id, title, description and category deliberately avoid
    /// both words, so a match can only come from socket prose.
    static SOCKET_PROSE_NODES: &[voxel_graph::NodeDeclaration] = &[voxel_graph::NodeDeclaration {
        id: "test.socket_prose",
        version: 1,
        title: "Widget",
        description: "A node declared only for palette search tests.",
        category: voxel_graph::NodeCategory::Inputs,
        preview: voxel_graph::NodePreview::Value,
        operation: MaterialNodeOperation::ConstantScalar.tag(),
        temporal: voxel_graph::TemporalDependence::Inherited,
        kinds: &[GraphKind::Material],
        inputs: &[voxel_graph::SocketDeclarationStatic {
            key: "gloss",
            label: "Gloss",
            description: "How tight the specular highlight stays.",
            value_type: voxel_graph::SocketType::Scalar,
            rate: voxel_graph::EvaluationRate::Uniform,
            cardinality: voxel_graph::Cardinality::OPTIONAL_SINGLE,
            separable: voxel_graph::Separable::None,
        }],
        outputs: &[voxel_graph::SocketDeclarationStatic {
            key: "value",
            label: "Result",
            description: "Sampled over the hemisphere above the surface.",
            value_type: voxel_graph::SocketType::Scalar,
            rate: voxel_graph::EvaluationRate::Uniform,
            cardinality: voxel_graph::Cardinality::ANY,
            separable: voxel_graph::Separable::None,
        }],
        fields: &[],
    }];

    #[test]
    fn node_palette_search_matches_socket_label_and_description() {
        static PROSE_FAMILIES: &[&[voxel_graph::NodeDeclaration]] = &[SOCKET_PROSE_NODES];
        static PROSE_CONTRACTS: &[&[voxel_graph::GraphContractStatic]] = &[crate::CONTRACTS];
        let registry = NodeRegistry::new(PROSE_FAMILIES, PROSE_CONTRACTS);
        let mut editor = GraphEditorState::new(6);

        // "Gloss" is only an input socket label; "hemisphere" is only inside an
        // output socket description. Both must still surface the node, and the
        // match must be case-insensitive.
        for term in ["gloss", "Hemisphere"] {
            editor.search = term.to_string();
            let visible = editor
                .visible_node_types(&registry)
                .into_iter()
                .map(|declaration| declaration.id)
                .collect::<Vec<_>>();
            assert_eq!(visible, vec!["test.socket_prose"], "search term {term:?}");
        }

        editor.search = "unrelated".to_string();
        assert!(
            editor.visible_node_types(&registry).is_empty(),
            "an unrelated term must match nothing"
        );
    }

    #[test]
    fn editor_inserts_and_removes_layers_in_the_typed_surface_chain() {
        let registry = crate::CATALOGUE;
        let graph = graph_from_material(&voxel_material::material::MATERIALS[6]);
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
        let registry = crate::CATALOGUE;
        let graph = graph_from_material(&voxel_material::material::MATERIALS[6]);
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
        let registry = crate::CATALOGUE;
        let (mut graph, output) = graph_with_output();
        let color = node("color");
        let reroute = node("reroute");
        graph.nodes.extend([
            (
                color.clone(),
                voxel_graph::NodeRecord {
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
                voxel_graph::NodeRecord {
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
                voxel_graph::LinkRecord {
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
                voxel_graph::LinkRecord {
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
                .evaluate(MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0]))
                .base_color,
            [0.15, 0.35, 0.75, 1.0]
        );
        assert!(program.wgsl.contains("v"));
    }

    #[test]
    fn editor_can_copy_and_paste_a_node_with_defaults() {
        let registry = crate::CATALOGUE;
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
