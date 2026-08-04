//! `material.normal` — Normal.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const NORMAL_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "Unit outward normal of the voxel face being shaded, in world space.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.normal",
    NodeOperation::Material(MaterialNodeOperation::Normal),
    "Normal",
    "World-space surface normal.",
    NodeCategory::Coordinates,
    NodePreview::Value,
    MATERIAL,
    &[],
    NORMAL_OUT,
    &[],
    TemporalDependence::Inherited,
);
