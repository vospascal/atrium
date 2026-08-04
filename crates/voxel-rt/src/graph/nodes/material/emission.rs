//! `material.emission` — Emission.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const EMISSION_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "The emitted color in linear RGBA, before any strength scaling.",
    SocketType::Color,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];

const EMISSION_FIELDS: &[FieldDeclarationStatic] = &[field(
    "value",
    "Emission",
    "Linear emitted color before intensity scaling. A RADIANCE, not a reflectance: \
     1.0 is SDR reference white (100 cd/m²). The picker only authors 0-1, so anything \
     brighter than white comes from Emission Strength, not from here.",
    FieldTarget::Property,
    FieldDefault::Color([0.0, 0.0, 0.0, 1.0]),
    NONE,
    NONE,
    None,
    EMPTY_CHOICES,
    false,
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.emission",
    NodeOperation::Material(MaterialNodeOperation::ConstantColor),
    "Emission",
    "Intrinsic emitted color.",
    NodeCategory::MaterialOutput,
    NodePreview::ColorWheel,
    MATERIAL,
    &[],
    EMISSION_OUT,
    EMISSION_FIELDS,
    TemporalDependence::Inherited,
);
