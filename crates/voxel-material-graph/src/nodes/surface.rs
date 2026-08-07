//! `material.surface` — Material Surface.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const MATERIAL_SURFACE_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "base_color",
        "Base Color",
        "Diffuse albedo of the material in authored sRGB RGBA, before pattern layers.",
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
        "specular",
        "Specular",
        "Specular reflectance at normal incidence (F0), before pattern layers.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "ambient_occlusion",
        "Ambient Occlusion",
        "Material-authored occlusion multiplier: 1 is open, 0 is fully occluded.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "normal",
        "Normal",
        "Optional world-space normal override; zero leaves the displacement normal unchanged.",
        SocketType::Vector3,
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
    "PBR Surface",
    "The intrinsic PBR surface, ready for pattern layers or straight into the \
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
        "specular",
        "Specular",
        "Specular reflectance at normal incidence.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.04),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "ambient_occlusion",
        "Ambient Occlusion",
        "Material-authored occlusion multiplier.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(1.0),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "normal",
        "Normal",
        "Optional normal override. A zero vector keeps the geometric normal.",
        FieldTarget::InputSocket,
        FieldDefault::Vector3([0.0; 3]),
        NONE,
        NONE,
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
    MaterialNodeOperation::Surface,
    "PBR Surface",
    "Complete specular-roughness surface state before ordered pattern layers.",
    NodeCategory::MaterialOutput,
    NodePreview::MaterialSphere,
    MATERIAL,
    MATERIAL_SURFACE_IN,
    MATERIAL_SURFACE_OUT,
    MATERIAL_OUTPUT_FIELDS,
    TemporalDependence::Inherited,
);
