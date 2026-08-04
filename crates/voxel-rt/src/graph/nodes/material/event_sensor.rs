//! `material.event_sensor` — Event Sensor.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

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

pub const DECLARATION: NodeDeclaration = node!(
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
);
