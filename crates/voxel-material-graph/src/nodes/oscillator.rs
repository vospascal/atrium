//! `material.oscillator` — Oscillator.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

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

pub const DECLARATION: NodeDeclaration = node!(
    "material.oscillator",
    MaterialNodeOperation::Oscillator,
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
);
