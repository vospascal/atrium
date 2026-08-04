//! `material.surface` — Material Surface.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const MATERIAL_SURFACE_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "base_color",
        "Base Color",
        "Diffuse albedo of the material in linear RGBA, before pattern layers.",
        SocketType::Color,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "roughness",
        "Roughness",
        "Microsurface roughness before pattern layers, 0 mirror-smooth to 1 fully \
         diffuse.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "emission",
        "Emission",
        "Light the surface gives off, in linear RGBA already scaled by its strength.",
        SocketType::Color,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const MATERIAL_SURFACE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "surface",
    "Surface",
    "The intrinsic surface, ready for pattern layers or straight into the \
     Material Output.",
    SocketType::MaterialSurface,
    EvaluationRate::PerMaterial,
    Cardinality::REQUIRED_SINGLE
)];

const MATERIAL_OUTPUT_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "base_color",
        "Base Color",
        "Surface base color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.8, 0.8, 0.8, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "roughness",
        "Roughness",
        "Surface roughness.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.6),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "emission",
        "Emission",
        "Surface emitted color.",
        FieldTarget::InputSocket,
        FieldDefault::Color([0.0, 0.0, 0.0, 1.0]),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
];

pub const DECLARATION: NodeDeclaration = node!(
    "material.surface",
    NodeOperation::Material(MaterialNodeOperation::Surface),
    "Material Surface",
    "Intrinsic surface values before ordered pattern layers.",
    NodeCategory::MaterialOutput,
    NodePreview::MaterialSphere,
    MATERIAL,
    MATERIAL_SURFACE_IN,
    MATERIAL_SURFACE_OUT,
    MATERIAL_OUTPUT_FIELDS,
    TemporalDependence::Inherited,
);
