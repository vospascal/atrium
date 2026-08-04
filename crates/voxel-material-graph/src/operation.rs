//! What a material node does, as a typed value.
//!
//! `voxel-graph` never sees these variants — a declaration carries an
//! [`OperationTag`](voxel_graph::OperationTag) and this converts. `tag()` is `const`, so a
//! catalogue can be a `static`; `from_tag` is the reverse and returns `None` for a label this
//! build does not know, which is the same condition as an unrecognised node type and never a
//! reason to panic.

use voxel_graph::OperationTag;

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

    /// Reverse of [`Self::tag`].
    pub fn from_tag(tag: OperationTag) -> Option<Self> {
        Self::from_label(tag.0)
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
