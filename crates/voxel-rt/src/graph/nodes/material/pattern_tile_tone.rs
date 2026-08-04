//! `material.pattern_tile_tone` — Tile Tone.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::graph::common`].

use crate::graph::common::*;
use crate::graph::{MaterialNodeOperation, NodeOperation};

const PATTERN_TILE_TONE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "One flat value per tile, 0..1.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_TILE_TONE_FIELDS);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_tile_tone",
    NodeOperation::Material(MaterialNodeOperation::PatternTileTone),
    "Tile Tone",
    "One flat shade per tile — the tone variation that makes masonry read as blocks.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_TILE_TONE_OUT,
    PATTERN_TILE_TONE_FIELDS,
    TemporalDependence::Inherited,
);
