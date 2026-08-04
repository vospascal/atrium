//! `material.pattern_layer` — Pattern Layer.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

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

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_layer",
    MaterialNodeOperation::PatternLayer,
    "Pattern Layer",
    "An ordered procedural modification of the incoming surface.",
    NodeCategory::Layers,
    NodePreview::Noise,
    MATERIAL,
    PATTERN_LAYER_IN,
    PATTERN_LAYER_OUT,
    PATTERN_LAYER_FIELDS,
    TemporalDependence::Inherited,
);
