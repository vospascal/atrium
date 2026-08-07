//! `material.displacement` — a standalone height/displacement modifier.
//!
//! The mask remains a reusable input: this node owns the physical response
//! (height and face scope), while `Pattern Layer` owns only
//! albedo/roughness/emission blending. The renderer derives a lighting normal
//! from the height field; topology is intentionally unchanged for voxel faces.

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const DISPLACEMENT_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "surface",
        "Surface",
        "The surface being displaced; keep the modifier in the surface chain.",
        SocketType::MaterialSurface,
        EvaluationRate::PerMaterial,
        Cardinality::REQUIRED_SINGLE
    ),
    socket!(
        "height",
        "Height",
        "The grayscale height mask that lifts the detail.",
        SocketType::MaskField,
        EvaluationRate::PerSample,
        Cardinality::REQUIRED_SINGLE
    ),
];

const DISPLACEMENT_OUT: &[SocketDeclarationStatic] = &[socket!(
    "surface",
    "Surface",
    "The displaced surface, ready for the next modifier or Material Output.",
    SocketType::MaterialSurface,
    EvaluationRate::PerMaterial,
    Cardinality::REQUIRED_SINGLE
)];

const DISPLACEMENT_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "enabled",
        "Enabled",
        "Include this displacement modifier in the material.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "height_meters",
        "Height (m)",
        "Maximum physical lift along the geometric surface normal. The mask is sampled at the same texel resolution as its source pattern.",
        FieldTarget::Property,
        FieldDefault::Scalar(0.0),
        Some(NumericRange::new(0.0, 0.25)),
        Some(NumericRange::new(0.0, 0.25)),
        Some(0.001),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "affects_normal",
        "Affects Normal",
        "Derive a lighting normal from this height field. Turn off to keep the height response without adding bump detail.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "normal_strength",
        "Normal Strength",
        "Multiplier on the derived normal's tilt, independent of the physical height. 1.0 is the true gradient; higher exaggerates the emboss lighting without lifting the surface further.",
        FieldTarget::Property,
        FieldDefault::Scalar(1.0),
        Some(NumericRange::new(
            0.0,
            voxel_material::pattern::MAXIMUM_RELIEF_NORMAL_STRENGTH,
        )),
        Some(NumericRange::new(
            0.0,
            voxel_material::pattern::MAXIMUM_RELIEF_NORMAL_STRENGTH,
        )),
        Some(0.05),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "bevel_fraction",
        "Bevel Width",
        "Half-width of the emboss bevel as a fraction of one texel — the sub-texel step the normal is sampled at. Small is a crisp chisel edge, 0.5 is a full-texel step and smears the relief into a soft roll.",
        FieldTarget::Property,
        FieldDefault::Scalar(voxel_material::pattern::DEFAULT_RELIEF_BEVEL_FRACTION),
        Some(NumericRange::new(
            voxel_material::pattern::MINIMUM_RELIEF_BEVEL_FRACTION,
            voxel_material::pattern::MAXIMUM_RELIEF_BEVEL_FRACTION,
        )),
        Some(NumericRange::new(
            voxel_material::pattern::MINIMUM_RELIEF_BEVEL_FRACTION,
            voxel_material::pattern::MAXIMUM_RELIEF_BEVEL_FRACTION,
        )),
        Some(0.005),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "invert",
        "Invert",
        "Raise where the mask is LOW. Use when the same mask darkens the colour (mix toward black), so the lighter texels are the raised ones.",
        FieldTarget::Property,
        FieldDefault::Boolean(false),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "steps",
        "Steps",
        "Quantise the height mask into this many flat levels. A continuous mask tilts every texel border a little and reads as wash; stepped plateaus concentrate the full height into few, crisp bevels — the normal-map look. 0 keeps the continuous mask; 2 is raised-or-not.",
        FieldTarget::Property,
        FieldDefault::Integer(0),
        Some(NumericRange::new(
            0.0,
            voxel_material::pattern::MAX_RELIEF_STEPS as f32,
        )),
        Some(NumericRange::new(
            0.0,
            voxel_material::pattern::MAX_RELIEF_STEPS as f32,
        )),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "faces_top",
        "Top Faces",
        "Affect upward-facing block faces.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "faces_side",
        "Side Faces",
        "Affect vertical block faces.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "faces_bottom",
        "Bottom Faces",
        "Affect downward-facing block faces.",
        FieldTarget::Property,
        FieldDefault::Boolean(true),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
];

pub const DECLARATION: NodeDeclaration = node!(
    "material.displacement",
    MaterialNodeOperation::Displacement,
    "Displacement",
    "Lifts a mask into voxel-scale surface detail while leaving color-layer scope independent.",
    NodeCategory::MaterialOutput,
    NodePreview::Noise,
    MATERIAL,
    DISPLACEMENT_IN,
    DISPLACEMENT_OUT,
    DISPLACEMENT_FIELDS,
    TemporalDependence::Inherited,
);
