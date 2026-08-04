//! Atrium's node catalogue: which nodes exist, what sockets and fields each has, and
//! which of them a graph of a given kind must contain.
//!
//! The graph *mechanics* — documents, wiring rules, validation, history — are
//! `voxel-graph`, which knows nothing about materials. This file is the domain half: the
//! 62 operations across seven families, their socket and field declarations, and
//! [`GRAPH_CONTRACTS`].
//!
//! Each operation carries a stable label ([`OperationTag`]) rather than being named
//! directly in `voxel-graph`'s types. `tag()` and [`NodeOperation::from_tag`] convert;
//! `every_operation_tag_round_trips` proves no label is orphaned or duplicated.

use voxel_graph::{
    choice, field, Cardinality, EvaluationRate, FieldDeclarationStatic, FieldDefault, FieldTarget,
    FlowConstraintStatic, GraphContractStatic, GraphKind, NodeCategory, NodeConstraintStatic,
    NodeDeclaration, NodePreview, NodeRegistry, NumericRange, OperationTag, Separable,
    SocketDeclarationStatic, SocketType, TemporalDependence, EMPTY_CHOICES, NONE, POSITIVE, SIGNED,
    UNIT, WIDE,
};

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

impl MaterialNodeOperation {
    /// This operation's stable label. The text matches the node's own type id, so a
    /// collision between two operations is visible on sight.
    pub const fn tag(self) -> OperationTag {
        match self {
            Self::Output => OperationTag("material.output"),
            Self::Surface => OperationTag("material.surface"),
            Self::PatternLayer => OperationTag("material.pattern_layer"),
            Self::PatternFlat => OperationTag("material.pattern_flat"),
            Self::PatternNoise => OperationTag("material.pattern_noise"),
            Self::PatternSpeckle => OperationTag("material.pattern_speckle"),
            Self::PatternPerlin => OperationTag("material.pattern_perlin"),
            Self::PatternSimplex => OperationTag("material.pattern_simplex"),
            Self::PatternRidged => OperationTag("material.pattern_ridged"),
            Self::PatternTurbulence => OperationTag("material.pattern_turbulence"),
            Self::PatternWorley => OperationTag("material.pattern_worley"),
            Self::PatternWorleyEdge => OperationTag("material.pattern_worley_edge"),
            Self::PatternWorleySmooth => OperationTag("material.pattern_worley_smooth"),
            Self::PatternWave => OperationTag("material.pattern_wave"),
            Self::PatternChecker => OperationTag("material.pattern_checker"),
            Self::PatternTileTone => OperationTag("material.pattern_tile_tone"),
            Self::PatternTileEdge => OperationTag("material.pattern_tile_edge"),
            Self::Tessellation => OperationTag("material.tessellation"),
            Self::ConstantScalar => OperationTag("material.constant_scalar"),
            Self::ConstantColor => OperationTag("material.constant_color"),
            Self::AddScalar => OperationTag("material.add_scalar"),
            Self::MixColor => OperationTag("material.mix_color"),
            Self::ClampScalar => OperationTag("material.clamp_scalar"),
            Self::Position => OperationTag("material.position"),
            Self::Normal => OperationTag("material.normal"),
            Self::EmissionStrength => OperationTag("material.emission_strength"),
            Self::FaceColor => OperationTag("material.face_color"),
            Self::FaceRoughness => OperationTag("material.face_roughness"),
            Self::RemapScalar => OperationTag("material.remap_scalar"),
            Self::Noise => OperationTag("material.noise"),
            Self::Fbm => OperationTag("material.fbm"),
            Self::ColorRamp => OperationTag("material.color_ramp"),
            Self::VectorAdd => OperationTag("material.vector_add"),
            Self::VectorScale => OperationTag("material.vector_scale"),
            Self::NormalizeVector => OperationTag("material.normalize_vector"),
            Self::DotVector => OperationTag("material.dot_vector"),
            Self::PositionComponent => OperationTag("material.position_component"),
            Self::NormalComponent => OperationTag("material.normal_component"),
            Self::PassthroughScalar => OperationTag("material.passthrough_scalar"),
            Self::Time => OperationTag("material.time"),
            Self::Oscillator => OperationTag("material.oscillator"),
            Self::EventSensor => OperationTag("material.event_sensor"),
            Self::MultiplyScalar => OperationTag("material.multiply_scalar"),
            Self::Direction => OperationTag("material.direction"),
            Self::RerouteScalar => OperationTag("material.reroute_scalar"),
            Self::RerouteColor => OperationTag("material.reroute_color"),
            Self::RerouteVector => OperationTag("material.reroute_vector"),
        }
    }

    fn from_label(label: &str) -> Option<Self> {
        match label {
            "material.output" => Some(Self::Output),
            "material.surface" => Some(Self::Surface),
            "material.pattern_layer" => Some(Self::PatternLayer),
            "material.pattern_flat" => Some(Self::PatternFlat),
            "material.pattern_noise" => Some(Self::PatternNoise),
            "material.pattern_speckle" => Some(Self::PatternSpeckle),
            "material.pattern_perlin" => Some(Self::PatternPerlin),
            "material.pattern_simplex" => Some(Self::PatternSimplex),
            "material.pattern_ridged" => Some(Self::PatternRidged),
            "material.pattern_turbulence" => Some(Self::PatternTurbulence),
            "material.pattern_worley" => Some(Self::PatternWorley),
            "material.pattern_worley_edge" => Some(Self::PatternWorleyEdge),
            "material.pattern_worley_smooth" => Some(Self::PatternWorleySmooth),
            "material.pattern_wave" => Some(Self::PatternWave),
            "material.pattern_checker" => Some(Self::PatternChecker),
            "material.pattern_tile_tone" => Some(Self::PatternTileTone),
            "material.pattern_tile_edge" => Some(Self::PatternTileEdge),
            "material.tessellation" => Some(Self::Tessellation),
            "material.constant_scalar" => Some(Self::ConstantScalar),
            "material.constant_color" => Some(Self::ConstantColor),
            "material.add_scalar" => Some(Self::AddScalar),
            "material.mix_color" => Some(Self::MixColor),
            "material.clamp_scalar" => Some(Self::ClampScalar),
            "material.position" => Some(Self::Position),
            "material.normal" => Some(Self::Normal),
            "material.emission_strength" => Some(Self::EmissionStrength),
            "material.face_color" => Some(Self::FaceColor),
            "material.face_roughness" => Some(Self::FaceRoughness),
            "material.remap_scalar" => Some(Self::RemapScalar),
            "material.noise" => Some(Self::Noise),
            "material.fbm" => Some(Self::Fbm),
            "material.color_ramp" => Some(Self::ColorRamp),
            "material.vector_add" => Some(Self::VectorAdd),
            "material.vector_scale" => Some(Self::VectorScale),
            "material.normalize_vector" => Some(Self::NormalizeVector),
            "material.dot_vector" => Some(Self::DotVector),
            "material.position_component" => Some(Self::PositionComponent),
            "material.normal_component" => Some(Self::NormalComponent),
            "material.passthrough_scalar" => Some(Self::PassthroughScalar),
            "material.time" => Some(Self::Time),
            "material.oscillator" => Some(Self::Oscillator),
            "material.event_sensor" => Some(Self::EventSensor),
            "material.multiply_scalar" => Some(Self::MultiplyScalar),
            "material.direction" => Some(Self::Direction),
            "material.reroute_scalar" => Some(Self::RerouteScalar),
            "material.reroute_color" => Some(Self::RerouteColor),
            "material.reroute_vector" => Some(Self::RerouteVector),
            _ => None,
        }
    }
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
        if let Some(operation) = MaterialNodeOperation::from_label(tag.0) {
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum MaterialNodeOperation {
    Output,
    Surface,
    PatternLayer,
    PatternFlat,
    PatternNoise,
    PatternSpeckle,
    PatternPerlin,
    PatternSimplex,
    PatternRidged,
    PatternTurbulence,
    PatternWorley,
    PatternWorleyEdge,
    PatternWorleySmooth,
    PatternWave,
    PatternChecker,
    PatternTileTone,
    PatternTileEdge,
    Tessellation,
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

const MATERIAL_SURFACE_INTERMEDIATES: &[OperationTag] =
    &[NodeOperation::Material(MaterialNodeOperation::PatternLayer).tag()];
const MATERIAL_NODE_CONSTRAINTS: &[NodeConstraintStatic] = &[
    NodeConstraintStatic {
        operation: NodeOperation::Material(MaterialNodeOperation::Output).tag(),
        cardinality: Cardinality::EXACTLY_ONE,
    },
    NodeConstraintStatic {
        operation: NodeOperation::Material(MaterialNodeOperation::Surface).tag(),
        cardinality: Cardinality::EXACTLY_ONE,
    },
    NodeConstraintStatic {
        operation: NodeOperation::Material(MaterialNodeOperation::PatternLayer).tag(),
        cardinality: Cardinality::up_to(crate::pattern::MAX_PATTERN_LAYERS),
    },
];
const MATERIAL_FLOWS: &[FlowConstraintStatic] = &[
    FlowConstraintStatic {
        value_type: SocketType::MaterialSurface,
        source: NodeOperation::Material(MaterialNodeOperation::Surface).tag(),
        intermediates: MATERIAL_SURFACE_INTERMEDIATES,
        sink: NodeOperation::Material(MaterialNodeOperation::Output).tag(),
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
    operation: NodeOperation::World(WorldNodeOperation::Output).tag(),
    cardinality: Cardinality::EXACTLY_ONE,
}];
const WORLD_FLOWS: &[FlowConstraintStatic] = &[FlowConstraintStatic {
    value_type: SocketType::VoxelField,
    source: NodeOperation::World(WorldNodeOperation::GeneratedTerrain).tag(),
    intermediates: &[NodeOperation::World(WorldNodeOperation::Compose).tag()],
    sink: NodeOperation::World(WorldNodeOperation::Output).tag(),
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
            separable: Separable::None,
        }
    };
}

/// A socket whose time-varying input can be lifted OUT of a cached field. The
/// default is deliberately the conservative one, so forgetting to reach for this
/// macro under-claims cacheability rather than over-claiming it.
macro_rules! socket_separable {
    ($key:literal, $label:literal, $description:literal, $value_type:expr, $rate:expr,
     $cardinality:expr, $separable:expr $(,)?) => {
        SocketDeclarationStatic {
            key: $key,
            label: $label,
            description: $description,
            value_type: $value_type,
            rate: $rate,
            cardinality: $cardinality,
            separable: $separable,
        }
    };
}

macro_rules! node {
    ($id:literal, $operation:expr, $title:literal, $description:literal, $category:expr, $preview:expr,
     $kinds:expr, $inputs:expr, $outputs:expr, $fields:expr, $temporal:expr $(,)?) => {
        NodeDeclaration {
            id: $id,
            version: 1,
            title: $title,
            description: $description,
            category: $category,
            preview: $preview,
            operation: $operation.tag(),
            temporal: $temporal,
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
    socket_separable!(
        "animation_gain",
        "Animation Gain",
        "Multiplies this layer's Amount, 0 off to 1 as authored; unconnected it is \
         the identity.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE,
        // Applied AFTER the field is sampled, so an oscillator here leaves the
        // cached field untouched.
        Separable::Scale
    ),
    socket_separable!(
        "drift_velocity",
        "Drift",
        "How fast the pattern travels through world space, in metres per second; \
         the shader applies the clock itself.",
        SocketType::Vector3,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE,
        // Moves WHERE the field is read, not what it contains, and
        // `pattern_drift_meters` quantises that to whole texels.
        Separable::Translate
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
const PATTERN_PERLIN_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Fractal Perlin gradient noise, 0..1. No axis bias.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const PATTERN_SIMPLEX_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Fractal simplex gradient noise, 0..1. Four corners per octave.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const PATTERN_RIDGED_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Ridged multifractal, 0..1. Creases at each octave's midline.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const PATTERN_TURBULENCE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Turbulence, 0..1. Creases at each octave's zero crossing.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const PATTERN_WORLEY_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Distance to the nearest feature point, 0..1.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const PATTERN_WORLEY_EDGE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Bright on the boundary between two cells, 0..1.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const PATTERN_WORLEY_SMOOTH_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Worley through a smooth minimum, 0..1. No hard creases.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const PATTERN_WAVE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Noise-bent bands along the frame's X, 0..1.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const PATTERN_TILE_TONE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "One flat value per tile, 0..1.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const PATTERN_TILE_EDGE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "0 in the joint, 1 at the tile's centre.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];
const PATTERN_CHECKER_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Alternating lattice cells: 1 and 0.",
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
    "Constant linear RGBA color. The swatch authors CHROMATICITY in 0-1 only — every \
     colour picker does, and clamping is the widget's, not ours. Magnitude above white \
     comes from a scale downstream (Emission Strength for emitters), which is also the \
     only thing HDR float output can show.",
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
    "Linear emitted color before intensity scaling. A RADIANCE, not a reflectance: \
     1.0 is SDR reference white (100 cd/m²). The picker only authors 0-1, so anything \
     brighter than white comes from Emission Strength, not from here.",
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
        "Emission intensity, in multiples of SDR reference white: 1.0 is a 100 cd/m² \
         white, 4.0 is 400. Above 1.0 only reaches the display in HDR float output — \
         the integer depths tone-map it back under white. Note the GI volume quantises \
         radiance into [0, 1] (cagi.rs quantize_radiance), so past 1.0 the bounced \
         light saturates while the lit surface itself keeps brightening.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        POSITIVE,
        // 64x SDR white = 6400 cd/m², comfortably past the 1600 cd/m² peak of the
        // brightest panel we target, so nothing physically displayable is out of
        // reach. Deliberately NOT the 100x PQ signalling ceiling: that is an encoding
        // limit no display realises, and spending most of the slider on it would make
        // the 0-2 range everything else lives in unusable.
        Some(NumericRange::new(0.0, 64.0)),
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
            "tile",
            "Tile",
            "Subdivide the wall into tiles and sample within one, so the pattern \
             restarts at every joint and each tile draws its own independent copy. \
             Period is the TILE SIZE here; the tessellation input sets the bond.",
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
const PATTERN_DISTORTION_FIELD: FieldDeclarationStatic = field(
    "distortion",
    "Distortion",
    "How far noise bends the bands, in periods. Zero rules perfectly straight \
     lines; a quarter reads as wood grain.",
    FieldTarget::Property,
    FieldDefault::Scalar(0.25),
    Some(NumericRange::new(0.0, 2.0)),
    Some(NumericRange::new(0.0, 2.0)),
    Some(0.01),
    EMPTY_CHOICES,
    false,
);
/// On EVERY generator node, because domain warping composes with all of them —
/// see [`crate::pattern::PatternLayer::domain_warp`]. That is the whole reason it
/// is a shared field rather than a thirteenth generator.
const PATTERN_WARP_FIELD: FieldDeclarationStatic = field(
    "domain_warp",
    "Domain Warp",
    "Pushes the sample point through a noise field before this generator reads it \
     (iq, 'domain warping'). Costs about one extra octave, so it trades against \
     the octave count directly.",
    FieldTarget::Property,
    FieldDefault::Scalar(0.0),
    UNIT,
    UNIT,
    Some(0.01),
    EMPTY_CHOICES,
    false,
);
const PATTERN_SHARPNESS_FIELD: FieldDeclarationStatic = field(
    "sharpness",
    "Edge Sharpness",
    "How abruptly the joint gives way to the tile face. Zero ramps all the way to \
     the tile's centre and reads as pillows; toward one it is a narrow dark line \
     around a flat tile.",
    FieldTarget::Property,
    FieldDefault::Scalar(0.6),
    UNIT,
    UNIT,
    Some(0.01),
    EMPTY_CHOICES,
    false,
);
const TILE_ASPECT_FIELD: FieldDeclarationStatic = field(
    "tile_aspect",
    "Tile Aspect",
    "Tile width over height. 1 is square, 4 is a long brick. Only the `tile` frame \
     reads it.",
    FieldTarget::Property,
    FieldDefault::Scalar(1.0),
    Some(NumericRange::new(
        crate::pattern::MINIMUM_TILE_ASPECT,
        crate::pattern::MAXIMUM_TILE_ASPECT,
    )),
    Some(NumericRange::new(
        crate::pattern::MINIMUM_TILE_ASPECT,
        crate::pattern::MAXIMUM_TILE_ASPECT,
    )),
    Some(0.05),
    EMPTY_CHOICES,
    false,
);
const TILE_BOND_FIELD: FieldDeclarationStatic = field(
    "tile_bond",
    "Bond",
    "How far each course shifts relative to the one below, as a fraction of a tile. \
     0 stacks the joints into continuous vertical lines; 0.5 is a running bond.",
    FieldTarget::Property,
    FieldDefault::Scalar(0.5),
    UNIT,
    UNIT,
    Some(0.01),
    EMPTY_CHOICES,
    false,
);
const TILE_GAP_FIELD: FieldDeclarationStatic = field(
    "tile_gap",
    "Gap",
    "Grout width, as a fraction of the tile's short edge. Taken out of the tile's \
     interior, so widening it opens the joints rather than moving the tiles.",
    FieldTarget::Property,
    FieldDefault::Scalar(0.06),
    Some(NumericRange::new(0.0, crate::pattern::MAXIMUM_TILE_GAP)),
    Some(NumericRange::new(0.0, crate::pattern::MAXIMUM_TILE_GAP)),
    Some(0.005),
    EMPTY_CHOICES,
    false,
);
const TESSELLATION_FIELDS: &[FieldDeclarationStatic] =
    &[TILE_ASPECT_FIELD, TILE_BOND_FIELD, TILE_GAP_FIELD];
/// The optional tessellation input every generator node carries.
///
/// OPTIONAL, and on all of them rather than only the tile pair, because the tile
/// FRAME is what most materials will use it for: a noise layer set to `tile` needs
/// to know where the tiles are just as much as a `tile tone` layer does, and a wall
/// whose tone, grout and grain disagreed about the tiling would be a bug with no
/// obvious cause.
const TESSELLATION_IN: &[SocketDeclarationStatic] = &[socket!(
    "tessellation",
    "Tessellation",
    "Optional. Where the tiles are, for a `tile`-framed layer or a tile generator.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::OPTIONAL_SINGLE
)];
const TESSELLATION_OUT: &[SocketDeclarationStatic] = &[socket!(
    "tessellation",
    "Tessellation",
    "Where the tiles are. Wire it into every generator that should share this \
     tiling — they cannot disagree if they read the same node.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

/// The four fields every generator node carries. Spelled once rather than copied
/// into twelve arrays, so adding a shared field cannot reach eleven of them and
/// miss the twelfth.
macro_rules! pattern_fields {
    ($name:ident) => {
        const $name: &[FieldDeclarationStatic] = &[
            PATTERN_FRAME_FIELD,
            PATTERN_PERIOD_FIELD,
            PATTERN_TEXELS_FIELD,
            PATTERN_VARIATION_FIELD,
            PATTERN_WARP_FIELD,
        ];
    };
    ($name:ident, $extra:expr) => {
        const $name: &[FieldDeclarationStatic] = &[
            PATTERN_FRAME_FIELD,
            PATTERN_PERIOD_FIELD,
            PATTERN_TEXELS_FIELD,
            PATTERN_VARIATION_FIELD,
            PATTERN_WARP_FIELD,
            $extra,
        ];
    };
}
pattern_fields!(PATTERN_FLAT_FIELDS);
pattern_fields!(PATTERN_WORLEY_FIELDS);
pattern_fields!(PATTERN_WORLEY_EDGE_FIELDS);
pattern_fields!(PATTERN_WORLEY_SMOOTH_FIELDS);
pattern_fields!(PATTERN_CHECKER_FIELDS);
pattern_fields!(PATTERN_TILE_TONE_FIELDS);
pattern_fields!(PATTERN_TILE_EDGE_FIELDS, PATTERN_SHARPNESS_FIELD);
pattern_fields!(PATTERN_NOISE_FIELDS, PATTERN_OCTAVES_FIELD);
pattern_fields!(PATTERN_PERLIN_FIELDS, PATTERN_OCTAVES_FIELD);
pattern_fields!(PATTERN_SIMPLEX_FIELDS, PATTERN_OCTAVES_FIELD);
pattern_fields!(PATTERN_RIDGED_FIELDS, PATTERN_OCTAVES_FIELD);
pattern_fields!(PATTERN_TURBULENCE_FIELDS, PATTERN_OCTAVES_FIELD);
pattern_fields!(PATTERN_SPECKLE_FIELDS, PATTERN_DENSITY_FIELD);
pattern_fields!(PATTERN_WAVE_FIELDS, PATTERN_DISTORTION_FIELD);
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
/// Atrium's node catalogue, paired with the contracts its graphs must satisfy.
///
/// The one place both halves are named together. `voxel-graph` has no `builtin()` and no
/// `Default` registry — it owns no nodes, so a default could only have meant "somebody
/// else's catalogue", which is exactly how the contracts came to be read from a hidden
/// module-level static.
pub const CATALOGUE: NodeRegistry = NodeRegistry::new(BUILTIN_NODES, GRAPH_CONTRACTS);

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
        CONSTANT_SCALAR_FIELDS,
        TemporalDependence::Inherited,
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
        &[],
        TemporalDependence::Inherited,
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
        MATERIAL_OUTPUT_FIELDS,
        TemporalDependence::Inherited,
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
        CONSTANT_COLOR_FIELDS,
        TemporalDependence::Inherited,
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
        ADD_SCALAR_FIELDS,
        TemporalDependence::Inherited,
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
        MIX_COLOR_FIELDS,
        TemporalDependence::Inherited,
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
        CLAMP_SCALAR_FIELDS,
        TemporalDependence::Inherited,
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
        &[],
        TemporalDependence::Inherited,
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
        &[],
        TemporalDependence::Inherited,
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
        BASE_COLOR_FIELDS,
        TemporalDependence::Inherited,
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
        ROUGHNESS_FIELDS,
        TemporalDependence::Inherited,
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
        EMISSION_FIELDS,
        TemporalDependence::Inherited,
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
        COLOR_STRENGTH_FIELDS,
        TemporalDependence::Inherited,
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
        FACE_COLOR_FIELDS,
        TemporalDependence::Inherited,
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
        FACE_ROUGHNESS_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_flat",
        NodeOperation::Material(MaterialNodeOperation::PatternFlat),
        "Flat Pattern",
        "One stable value per sampling cell.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_FLAT_OUT,
        PATTERN_FLAT_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_noise",
        NodeOperation::Material(MaterialNodeOperation::PatternNoise),
        "Noise Pattern",
        "Fractal value-noise pattern.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_NOISE_OUT,
        PATTERN_NOISE_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_speckle",
        NodeOperation::Material(MaterialNodeOperation::PatternSpeckle),
        "Speckle Pattern",
        "Scattered specks controlled by cell density.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_SPECKLE_OUT,
        PATTERN_SPECKLE_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_perlin",
        NodeOperation::Material(MaterialNodeOperation::PatternPerlin),
        "Perlin Pattern",
        "Fractal Perlin gradient noise on the cubic lattice — no axis bias.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_PERLIN_OUT,
        PATTERN_PERLIN_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_simplex",
        NodeOperation::Material(MaterialNodeOperation::PatternSimplex),
        "Simplex Pattern",
        "Fractal gradient noise on the tetrahedral lattice — four corners per octave.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_SIMPLEX_OUT,
        PATTERN_SIMPLEX_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_ridged",
        NodeOperation::Material(MaterialNodeOperation::PatternRidged),
        "Ridged Pattern",
        "Ridged multifractal: veins, erosion channels, rock strata.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_RIDGED_OUT,
        PATTERN_RIDGED_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_turbulence",
        NodeOperation::Material(MaterialNodeOperation::PatternTurbulence),
        "Turbulence Pattern",
        "Turbulence: marble veining, smoke, weathering streaks.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_TURBULENCE_OUT,
        PATTERN_TURBULENCE_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_worley",
        NodeOperation::Material(MaterialNodeOperation::PatternWorley),
        "Worley Pattern",
        "Cellular F1 — pebbles, cells, lichen colonies.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_WORLEY_OUT,
        PATTERN_WORLEY_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_worley_edge",
        NodeOperation::Material(MaterialNodeOperation::PatternWorleyEdge),
        "Worley Edge Pattern",
        "Cellular F2 minus F1 — cracked mud, dried paint, mortar.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_WORLEY_EDGE_OUT,
        PATTERN_WORLEY_EDGE_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_worley_smooth",
        NodeOperation::Material(MaterialNodeOperation::PatternWorleySmooth),
        "Smooth Worley Pattern",
        "Cellular F1 through a smooth minimum — merged, blobby cells.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_WORLEY_SMOOTH_OUT,
        PATTERN_WORLEY_SMOOTH_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_wave",
        NodeOperation::Material(MaterialNodeOperation::PatternWave),
        "Wave Pattern",
        "Noise-bent bands — wood grain, geological strata, brushed metal.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_WAVE_OUT,
        PATTERN_WAVE_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_checker",
        NodeOperation::Material(MaterialNodeOperation::PatternChecker),
        "Checker Pattern",
        "Alternating lattice cells — tiles and boards.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_CHECKER_OUT,
        PATTERN_CHECKER_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_tile_tone",
        NodeOperation::Material(MaterialNodeOperation::PatternTileTone),
        "Tile Tone",
        "One flat shade per tile — the tone variation that makes masonry read as blocks.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_TILE_TONE_OUT,
        PATTERN_TILE_TONE_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.pattern_tile_edge",
        NodeOperation::Material(MaterialNodeOperation::PatternTileEdge),
        "Tile Edge",
        "Distance to the nearest tile edge — grout, and the bevel of a raised block.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        TESSELLATION_IN,
        PATTERN_TILE_EDGE_OUT,
        PATTERN_TILE_EDGE_FIELDS,
        TemporalDependence::Inherited,
    ),
    node!(
        "material.tessellation",
        NodeOperation::Material(MaterialNodeOperation::Tessellation),
        "Tessellation",
        "Divides a wall into bonded tiles. Share it between every layer that should \
         agree about where the tiles are.",
        NodeCategory::Procedural,
        NodePreview::Noise,
        MATERIAL,
        &[],
        TESSELLATION_OUT,
        TESSELLATION_FIELDS,
        TemporalDependence::Inherited,
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
        PATTERN_LAYER_FIELDS,
        TemporalDependence::Inherited,
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
        MULTIPLY_SCALAR_FIELDS,
        TemporalDependence::Inherited,
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
        DIRECTION_FIELDS,
        TemporalDependence::Inherited,
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
        &[],
        TemporalDependence::Clock,
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
        OSCILLATOR_FIELDS,
        TemporalDependence::Clock,
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
        EVENT_SENSOR_FIELDS,
        TemporalDependence::Events,
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
        REMAP_FIELDS,
        TemporalDependence::Inherited,
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
        NOISE_FIELDS,
        TemporalDependence::Inherited,
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
        FBM_FIELDS,
        TemporalDependence::Inherited,
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
        COLOR_RAMP_FIELDS,
        TemporalDependence::Inherited,
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
        VECTOR_BINARY_FIELDS,
        TemporalDependence::Inherited,
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
        VECTOR_SCALE_FIELDS,
        TemporalDependence::Inherited,
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
        VECTOR_INPUT_FIELDS,
        TemporalDependence::Inherited,
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
        VECTOR_BINARY_FIELDS,
        TemporalDependence::Inherited,
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
        COMPONENT_FIELDS,
        TemporalDependence::Inherited,
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
        COMPONENT_FIELDS,
        TemporalDependence::Inherited,
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
        SCALAR_INPUT_FIELDS,
        TemporalDependence::Inherited,
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
        SCALAR_INPUT_FIELDS,
        TemporalDependence::Inherited,
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
        COLOR_INPUT_FIELDS,
        TemporalDependence::Inherited,
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
        VECTOR_REROUTE_FIELDS,
        TemporalDependence::Inherited,
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
        &[],
        TemporalDependence::Inherited,
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
        &[],
        TemporalDependence::Inherited,
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
        &[],
        TemporalDependence::Inherited,
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
        &[],
        TemporalDependence::Inherited,
    ),
];

#[cfg(test)]
mod tests {
    use std::collections::{BTreeMap, BTreeSet};

    use super::*;
    use voxel_graph::{
        node_reachability, DiagnosticSeverity, GraphAsset, GraphCommand, GraphCommandError,
        GraphHistory, InputPin, LinkId, LinkRecord, NodeId, NodeRecord, NodeTypeId, OutputPin,
        PropertyValue, SocketKey,
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
        for declaration in BUILTIN_NODES {
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
        for contract in GRAPH_CONTRACTS {
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
        let declaration = CATALOGUE
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
        let registry = crate::graph::CATALOGUE;
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
        let registry = crate::graph::CATALOGUE;
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
        use crate::graph::MaterialNodeOperation;
        for declaration in BUILTIN_NODES {
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
        let mut sources: Vec<&str> = BUILTIN_NODES
            .iter()
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
        for declaration in BUILTIN_NODES {
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
        for declaration in BUILTIN_NODES {
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
        let mut producers: Vec<&str> = BUILTIN_NODES
            .iter()
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

        let strength = BUILTIN_NODES
            .iter()
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
