//! `material.position` — Position.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const POSITION_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "World-space position of the point being shaded, in metres.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.position",
    MaterialNodeOperation::Position,
    "Position",
    "World-space sample position.",
    NodeCategory::Coordinates,
    NodePreview::Value,
    MATERIAL,
    &[],
    POSITION_OUT,
    &[],
    TemporalDependence::Inherited,
);
