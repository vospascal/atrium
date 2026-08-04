//! `material.output` — Material Output.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const MATERIAL_OUTPUT_IN: &[SocketDeclarationStatic] = &[socket!(
    "surface",
    "Surface",
    "The finished surface the renderer shades with; every material graph must \
     terminate here.",
    SocketType::MaterialSurface,
    EvaluationRate::PerMaterial,
    Cardinality::REQUIRED_SINGLE
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.output",
    NodeOperation::Material(MaterialNodeOperation::Output),
    "Material Output",
    "Final material surface consumed by the renderer.",
    NodeCategory::MaterialOutput,
    NodePreview::MaterialSphere,
    MATERIAL,
    MATERIAL_OUTPUT_IN,
    &[],
    &[],
    TemporalDependence::Inherited,
);
