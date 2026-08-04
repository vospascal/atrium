//! `material.normal` — Normal.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

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
    MaterialNodeOperation::Normal,
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
