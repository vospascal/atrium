//! What a socket is: its value type, when it is evaluated, and how many wires it takes.
//!
//! [`SocketType`] and [`EvaluationRate`] together decide whether one socket may feed
//! another — see [`SocketDeclarationStatic::can_feed`], which is the single rule the editor
//! and the validator both consult.

use serde::{Deserialize, Serialize};

use crate::id::SocketKey;

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SocketType {
    Scalar,
    Integer,
    Vector3,
    Color,
    Boolean,
    Text,
    Asset,
    MaterialSurface,
    MaterialRole,
    ScalarField,
    MaskField,
    VoxelField,
    PointField,
    SplineField,
    BiomeField,
    BiomeDefinition,
    SurfaceProfile,
    SurfaceRule,
    MaterialBinding,
    Environment,
    FeatureSet,
    AudioSignal,
    AnimationSignal,
    QualityProfile,
    RenderTarget,
}

impl SocketType {
    /// Human name for the socket's value type. Where a type has an obvious
    /// counterpart in Blender's shader editor the wording is borrowed from it,
    /// so someone arriving from that editor reads the same words here.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Scalar => "Float",
            Self::Integer => "Int",
            Self::Vector3 => "Vector",
            Self::Color => "Float Color",
            Self::Boolean => "Boolean",
            Self::Text => "String",
            Self::Asset => "Asset",
            Self::MaterialSurface => "Material Surface",
            Self::MaterialRole => "Material Role",
            Self::ScalarField => "Scalar Field",
            Self::MaskField => "Mask Field",
            Self::VoxelField => "Voxel Field",
            Self::PointField => "Point Field",
            Self::SplineField => "Spline Field",
            Self::BiomeField => "Biome Field",
            Self::BiomeDefinition => "Biome Definition",
            Self::SurfaceProfile => "Surface Profile",
            Self::SurfaceRule => "Surface Rule",
            Self::MaterialBinding => "Material Binding",
            Self::Environment => "Environment",
            Self::FeatureSet => "Feature Set",
            Self::AudioSignal => "Audio Signal",
            Self::AnimationSignal => "Animation Signal",
            Self::QualityProfile => "Quality Profile",
            Self::RenderTarget => "Render Target",
        }
    }
}

/// How a node's OWN operation depends on the clock — the axis
/// [`EvaluationRate`] does not have.
///
/// **Why this is a second axis and not another rung of the rate ladder.**
/// `EvaluationRate` is purely SPATIAL and it is ORDERED: `can_feed` is
/// `self <= destination`, so a coarser value may feed a finer one. Time is not
/// comparable with that. An oscillator is spatially [`EvaluationRate::Uniform`]
/// — the same value at every point in the world — and yet it changes every
/// frame. Folding time into the ladder would either break the ordering or
/// force every animated value to claim `PerSample`, which would be a lie about
/// where it can be evaluated.
///
/// Declared on every node rather than defaulted, because the safe answer is not
/// obvious from a node's other metadata and a wrong one silently claims that a
/// surface can be cached when it cannot.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TemporalDependence {
    /// Time-invariant in itself: the output changes over time only if an input
    /// does. Every pure math, mix, ramp and pattern-generator node.
    Inherited,
    /// Reads the animation clock. The output changes every frame regardless of
    /// its inputs.
    Clock,
    /// Reads the world-event field. Changes when an entity moves or an event is
    /// raised, which is not the clock but is equally not cacheable.
    Events,
}

impl TemporalDependence {
    /// Whether this node introduces time-dependence on its own, i.e. whether a
    /// taint pass should SEED at it rather than merely propagate through it.
    pub fn is_source(self) -> bool {
        !matches!(self, TemporalDependence::Inherited)
    }
}

/// How a time-varying value entering a socket combines with the cacheable
/// spatial field the node owns — the question that decides whether an ANIMATED
/// material can still be cached.
///
/// Only meaningful on a node that owns such a field; everywhere else the
/// conservative [`Separable::None`] is correct and is what `socket!` produces.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Separable {
    /// Time here changes the field itself, so nothing about it can be cached.
    None,
    /// Time here multiplies the field's contribution AFTER it is sampled, so the
    /// field caches and the scalar is applied per pixel.
    Scale,
    /// Time here translates the sample COORDINATE, so the field caches and the
    /// clock only moves where it is read. Exact for a pattern layer, because
    /// `pattern_drift_meters` quantises the offset to a whole number of texels —
    /// an integer index shift in the cache's own address space.
    Translate,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum EvaluationRate {
    Uniform,
    PerMaterial,
    PerVoxel,
    PerSample,
}

impl EvaluationRate {
    pub fn can_feed(self, destination: Self) -> bool {
        self <= destination
    }

    /// Human name for how often the value is recomputed.
    pub const fn display_name(self) -> &'static str {
        match self {
            Self::Uniform => "Uniform",
            Self::PerMaterial => "Per Material",
            Self::PerVoxel => "Per Voxel",
            Self::PerSample => "Per Sample",
        }
    }
}

#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct SocketDeclaration {
    pub key: SocketKey,
    pub label: String,
    pub description: String,
    pub value_type: SocketType,
    pub rate: EvaluationRate,
    pub cardinality: Cardinality,
}

/// Inclusive connection/instance bounds used by sockets and graph contracts.
/// `maximum: None` means unbounded. Keeping the same vocabulary at both
/// levels lets validation and UI affordances derive from one model.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
pub struct Cardinality {
    pub minimum: usize,
    pub maximum: Option<usize>,
}

impl Cardinality {
    pub const ANY: Self = Self::new(0, None);
    pub const OPTIONAL_SINGLE: Self = Self::new(0, Some(1));
    pub const REQUIRED_SINGLE: Self = Self::new(1, Some(1));
    pub const EXACTLY_ONE: Self = Self::REQUIRED_SINGLE;

    pub const fn new(minimum: usize, maximum: Option<usize>) -> Self {
        Self { minimum, maximum }
    }

    pub const fn up_to(maximum: usize) -> Self {
        Self::new(0, Some(maximum))
    }

    pub fn accepts(self, count: usize) -> bool {
        count >= self.minimum && self.maximum.is_none_or(|maximum| count <= maximum)
    }

    /// Whether the current occupancy leaves room for one more link/instance.
    /// A saturated single-link socket is still connectable: the connection
    /// planner replaces its existing link instead of exceeding this bound.
    pub const fn accepts_additional(self, count: usize) -> bool {
        match self.maximum {
            Some(maximum) => count < maximum,
            None => true,
        }
    }

    pub const fn allows_many(self) -> bool {
        !matches!(self.maximum, Some(0 | 1))
    }

    pub fn description(self) -> String {
        match (self.minimum, self.maximum) {
            (0, None) => "any number".to_string(),
            (minimum, None) => format!("at least {minimum}"),
            (minimum, Some(maximum)) if minimum == maximum => format!("exactly {minimum}"),
            (minimum, Some(maximum)) => format!("between {minimum} and {maximum}"),
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct SocketDeclarationStatic {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub value_type: SocketType,
    pub rate: EvaluationRate,
    pub cardinality: Cardinality,
    /// How a time-varying value arriving here combines with the node's cacheable
    /// field. `socket!` produces [`Separable::None`]; `socket_separable!` is the
    /// opt-in, and only a node that owns such a field may use it.
    pub separable: Separable,
}

impl SocketDeclarationStatic {
    pub fn can_feed(self, destination: Self) -> bool {
        self.value_type == destination.value_type && self.rate.can_feed(destination.rate)
    }
}
