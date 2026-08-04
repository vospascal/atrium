//! `material.pattern_wave` — Wave Pattern.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const PATTERN_WAVE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "pattern",
    "Pattern",
    "Noise-bent bands along the frame's X, 0..1.",
    SocketType::MaskField,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

pattern_fields!(PATTERN_WAVE_FIELDS, PATTERN_DISTORTION_FIELD);

pub const DECLARATION: NodeDeclaration = node!(
    "material.pattern_wave",
    MaterialNodeOperation::PatternWave,
    "Wave Pattern",
    "Noise-bent bands — wood grain, geological strata, brushed metal.",
    NodeCategory::Procedural,
    NodePreview::Noise,
    MATERIAL,
    TESSELLATION_IN,
    PATTERN_WAVE_OUT,
    PATTERN_WAVE_FIELDS,
    TemporalDependence::Inherited,
);
