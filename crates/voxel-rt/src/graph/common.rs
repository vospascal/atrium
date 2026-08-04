//! Declaration builders and the field atoms more than one node uses.
//!
//! The `socket!`, `socket_separable!`, `node!` and `pattern_fields!` macros are what keep a
//! node file to its own content instead of restating the `NodeDeclaration` layout 54 times.
//! Everything else here is shared by construction — `TESSELLATION_IN` alone is used by 14
//! nodes, and the `PATTERN_*_FIELD` atoms are assembled by `pattern_fields!`.

pub use voxel_graph::{
    choice, field, Cardinality, ChoiceDeclaration, EvaluationRate, FieldDeclarationStatic,
    FieldDefault, FieldTarget, GraphKind, NodeCategory, NodeDeclaration, NodePreview, NumericRange,
    Separable, SocketDeclarationStatic, SocketType, TemporalDependence, EMPTY_CHOICES, NONE,
    POSITIVE, SIGNED, UNIT, WIDE,
};

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

pub const MATERIAL: &[GraphKind] = &[GraphKind::Material];

pub const WORLD: &[GraphKind] = &[GraphKind::World];

pub const VECTOR_BINARY_FIELDS: &[FieldDeclarationStatic] = &[
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

pub const COMPONENT_FIELDS: &[FieldDeclarationStatic] = &[field(
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

pub const SCALAR_INPUT_FIELDS: &[FieldDeclarationStatic] = &[field(
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

pub const PATTERN_FRAME_FIELD: FieldDeclarationStatic = field(
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

pub const PATTERN_PERIOD_FIELD: FieldDeclarationStatic = field(
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

pub const PATTERN_TEXELS_FIELD: FieldDeclarationStatic = field(
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

pub const PATTERN_VARIATION_FIELD: FieldDeclarationStatic = field(
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

pub const PATTERN_OCTAVES_FIELD: FieldDeclarationStatic = field(
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

pub const PATTERN_DENSITY_FIELD: FieldDeclarationStatic = field(
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

pub const PATTERN_DISTORTION_FIELD: FieldDeclarationStatic = field(
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
pub const PATTERN_WARP_FIELD: FieldDeclarationStatic = field(
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

pub const PATTERN_SHARPNESS_FIELD: FieldDeclarationStatic = field(
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

pub const TILE_ASPECT_FIELD: FieldDeclarationStatic = field(
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

pub const TILE_BOND_FIELD: FieldDeclarationStatic = field(
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

pub const TILE_GAP_FIELD: FieldDeclarationStatic = field(
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

/// The optional tessellation input every generator node carries.
///
/// OPTIONAL, and on all of them rather than only the tile pair, because the tile
/// FRAME is what most materials will use it for: a noise layer set to `tile` needs
/// to know where the tiles are just as much as a `tile tone` layer does, and a wall
/// whose tone, grout and grain disagreed about the tiling would be a bug with no
/// obvious cause.
pub const TESSELLATION_IN: &[SocketDeclarationStatic] = &[socket!(
    "tessellation",
    "Tessellation",
    "Optional. Where the tiles are, for a `tile`-framed layer or a tile generator.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::OPTIONAL_SINGLE
)];
