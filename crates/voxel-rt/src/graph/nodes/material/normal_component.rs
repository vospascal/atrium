//! `material.normal_component` — Normal Component.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const NORMAL_COMPONENT_OUT: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Value",
    "The chosen axis of the world-space surface normal, -1..1.",
    SocketType::Scalar,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pub const DECLARATION: NodeDeclaration = node!(
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
);
