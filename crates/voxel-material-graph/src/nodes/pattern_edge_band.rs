//! `material.pattern_edge_band` — a pixel-stepped band along a block edge.
//!
//! The generator is intentionally a mask source rather than a special grass
//! material. A side-only Pattern Layer can use it to tint any material's upper
//! edge: grass over dirt, snow over stone, moss over a wall, or wetness below a
//! ledge.

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const PATTERN_EDGE_BAND_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "A pixel-stepped band along the top or bottom of vertical faces, 0..1.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

const PATTERN_EDGE_BAND_FIELDS: &[FieldDeclarationStatic] = &[
    PATTERN_TEXELS_FIELD,
    PATTERN_VARIATION_FIELD,
    PATTERN_EDGE_DIRECTION_FIELD,
    PATTERN_EDGE_WIDTH_FIELD,
    PATTERN_EDGE_JAGGEDNESS_FIELD,
];

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_edge_band",
    MaterialNodeOperation::PatternEdgeBand,
    "Edge Band",
    "Pixel-stepped material along the top or bottom of a vertical face.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_EDGE_BAND_OUT,
    PATTERN_EDGE_BAND_FIELDS,
    TemporalDependence::Inherited,
);
