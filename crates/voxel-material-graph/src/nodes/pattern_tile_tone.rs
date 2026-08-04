//! `material.pattern_tile_tone` — Tile Tone.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

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
    MaterialNodeOperation::PatternTileTone,
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
