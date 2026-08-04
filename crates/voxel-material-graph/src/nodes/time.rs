//! `material.time` — Time.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const TIME_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "Seconds since the session started, counting up and never stepping backwards.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.time",
    MaterialNodeOperation::Time,
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
);
