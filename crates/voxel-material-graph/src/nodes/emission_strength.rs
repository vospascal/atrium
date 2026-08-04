//! `material.emission_strength` — Emission Strength.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

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

pub const DECLARATION: NodeDeclaration = node!(
    "material.emission_strength",
    MaterialNodeOperation::EmissionStrength,
    "Emission Strength",
    "Scales emitted color intensity.",
    NodeCategory::MaterialOutput,
    NodePreview::Value,
    MATERIAL,
    COLOR_STRENGTH_IN,
    EMISSION_STRENGTH_OUT,
    COLOR_STRENGTH_FIELDS,
    TemporalDependence::Inherited,
);
