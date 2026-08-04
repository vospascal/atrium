//! `material.reroute_color` — Color Reroute.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

const REROUTE_COLOR_IN: &[SocketDeclarationStatic] = &[socket!(
    "value",
    "Color",
    "The color whose wire is being redirected.",
    SocketType::Color,
    EvaluationRate::PerSample,
    Cardinality::OPTIONAL_SINGLE
)];

const REROUTE_COLOR_OUT: &[SocketDeclarationStatic] = &[socket!(
    "color",
    "Color",
    "The same color; only the wire's path through the editor differs.",
    SocketType::Color,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

const COLOR_INPUT_FIELDS: &[FieldDeclarationStatic] = &[field(
    "value",
    "Value",
    "Input color.",
    FieldTarget::InputSocket,
    FieldDefault::Color([0.0; 4]),
    NONE,
    NONE,
    None,
    EMPTY_CHOICES,
    false,
)];

pub const DECLARATION: NodeDeclaration = node!(
    "material.reroute_color",
    MaterialNodeOperation::RerouteColor,
    "Color Reroute",
    "Reroutes a color connection.",
    NodeCategory::Utilities,
    NodePreview::None,
    MATERIAL,
    REROUTE_COLOR_IN,
    REROUTE_COLOR_OUT,
    COLOR_INPUT_FIELDS,
    TemporalDependence::Inherited,
);
