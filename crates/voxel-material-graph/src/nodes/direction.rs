//! `material.direction` — Direction.
//!
//! Its declaration and every constant only it uses. Shared field atoms and the
//! `socket!`/`node!` builders come from [`crate::declare`].

use crate::declare::*;
use crate::operation::MaterialNodeOperation;

/// Speed and angles, all connectable so a flow can itself be animated.
const DIRECTION_IN: &[SocketDeclarationStatic] = &[
    socket!(
        "speed",
        "Speed",
        "Length of the resulting vector; for a pattern drift this is metres per \
         second.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "azimuth_degrees",
        "Azimuth",
        "Heading around the vertical axis in degrees, 0 along +X and 90 along +Z; \
         it has no effect at an elevation of -90 or +90.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
    socket!(
        "elevation_degrees",
        "Elevation",
        "Angle above horizontal in degrees, -90 straight down to +90 straight up.",
        SocketType::Scalar,
        EvaluationRate::PerSample,
        Cardinality::OPTIONAL_SINGLE
    ),
];

const DIRECTION_OUT: &[SocketDeclarationStatic] = &[socket!(
    "vector",
    "Vector",
    "A velocity of length Speed pointing along Azimuth and Elevation, in metres \
     per second.",
    SocketType::Vector3,
    EvaluationRate::PerSample,
    Cardinality::ANY
)];

/// The oscillator's shape. Every numeric control is an input socket, so a
/// sensor can drive the rate or the range and "trigger a pulse" composes out of
/// nodes rather than needing a mode on this one.
/// Azimuth is measured around the vertical axis with 0 degrees along +X and 90
/// along +Z; elevation is the angle above horizontal. That is the same meaning
/// `SunSettings` already gives those words (`lighting.rs`) — reused so the
/// codebase has ONE definition of an angle pair, not because a flow has
/// anything to do with the sun.
const DIRECTION_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "speed",
        "Speed",
        "Length of the resulting vector. For a pattern drift this is metres per \
         second; a texel is 1 m / texels-per-voxel, so 0.25 m/s at 8 texels is \
         two rows a second.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.25),
        WIDE,
        Some(NumericRange::new(0.0, 4.0)),
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "azimuth_degrees",
        "Azimuth",
        "Heading around the vertical axis: 0 points along +X, 90 along +Z. \
         \n\nAT AN ELEVATION OF -90 OR +90 THIS DOES NOTHING: straight down has \
         no horizontal part to steer, so the slider will appear dead. For a \
         diagonal, back the elevation off the pole first — -45 splits the \
         motion evenly between downward and sideways, and the azimuth then \
         chooses which way sideways.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        Some(NumericRange::new(-360.0, 360.0)),
        Some(NumericRange::new(0.0, 360.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "elevation_degrees",
        "Elevation",
        "Angle above horizontal. -90 is straight down a wall, 0 is level across \
         a floor or a lake, and anything between is a diagonal. Note that -90 \
         and +90 are poles where the azimuth stops having any effect.",
        FieldTarget::InputSocket,
        FieldDefault::Scalar(0.0),
        Some(NumericRange::new(-90.0, 90.0)),
        Some(NumericRange::new(-90.0, 90.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
];

pub const DECLARATION: NodeDeclaration = node!(
    "material.direction",
    MaterialNodeOperation::Direction,
    "Direction",
    "Speed and two angles to a velocity vector — the authoring form for a \
         pattern drift, where dialling an angle beats editing three components. \
         Every input is connectable, so an oscillator on the azimuth swirls the \
         flow and one on the speed makes it surge.",
    NodeCategory::Coordinates,
    NodePreview::Value,
    MATERIAL,
    DIRECTION_IN,
    DIRECTION_OUT,
    DIRECTION_FIELDS,
    TemporalDependence::Inherited,
);
