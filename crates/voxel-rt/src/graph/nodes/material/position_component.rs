//! `material.position_component` — Position Component.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const POSITION_COMPONENT_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The chosen axis of the world-space sample position, in metres.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pub const DECLARATION: NodeDeclaration = node!(
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
);
