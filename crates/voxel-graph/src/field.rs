//! Authored values that live on a node rather than arriving through a wire.
//!
//! A field is the number, colour or choice you type into a node. [`FieldTarget`] decides
//! whether it is a bare property or the default for an unconnected input socket — the
//! distinction matters because the second one disappears the moment a wire lands.
//!
//! [`choice`] and [`field`] are `const fn` builders so a catalogue can declare its nodes in
//! a `static` with no initialisation code at all.

use serde::{Deserialize, Serialize};

use crate::asset::AssetId;
use crate::socket::SocketType;

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", content = "value", rename_all = "snake_case")]
pub enum PropertyValue {
    Scalar(f32),
    Vector3([f32; 3]),
    Color([f32; 4]),
    Boolean(bool),
    Integer(i64),
    Text(String),
    Asset(AssetId),
}

impl PropertyValue {
    pub fn socket_type(&self) -> SocketType {
        match self {
            Self::Scalar(_) => SocketType::Scalar,
            Self::Vector3(_) => SocketType::Vector3,
            Self::Color(_) => SocketType::Color,
            Self::Boolean(_) => SocketType::Boolean,
            Self::Integer(_) => SocketType::Integer,
            Self::Text(_) => SocketType::Text,
            Self::Asset(_) => SocketType::Asset,
        }
    }
}

/// Where an editable field is stored in a node record. Both properties and
/// unconnected input defaults use the same schema and therefore the same UI,
/// validation, and persistence rules.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum FieldTarget {
    Property,
    InputSocket,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum FieldDefault {
    Scalar(f32),
    Integer(i64),
    Vector3([f32; 3]),
    Color([f32; 4]),
    Boolean(bool),
    Text(&'static str),
}

impl FieldDefault {
    pub fn value(self) -> PropertyValue {
        match self {
            Self::Scalar(value) => PropertyValue::Scalar(value),
            Self::Integer(value) => PropertyValue::Integer(value),
            Self::Vector3(value) => PropertyValue::Vector3(value),
            Self::Color(value) => PropertyValue::Color(value),
            Self::Boolean(value) => PropertyValue::Boolean(value),
            Self::Text(value) => PropertyValue::Text(value.to_string()),
        }
    }

    pub fn socket_type(self) -> SocketType {
        self.value().socket_type()
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NumericRange {
    pub min: f32,
    pub max: f32,
}

impl NumericRange {
    pub const fn new(min: f32, max: f32) -> Self {
        Self { min, max }
    }

    pub fn contains(self, value: f32) -> bool {
        value.is_finite() && value >= self.min && value <= self.max
    }
}

/// One selectable option of a text-valued field. `value` is the string that is
/// persisted and that compilers dispatch on; `label` and `description` exist
/// only so the editor can explain the option instead of showing a bare id.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ChoiceDeclaration {
    pub value: &'static str,
    pub label: &'static str,
    pub description: &'static str,
}

pub const fn choice(
    value: &'static str,
    label: &'static str,
    description: &'static str,
) -> ChoiceDeclaration {
    ChoiceDeclaration {
        value,
        label,
        description,
    }
}

/// Canonical editable-field definition. `hard_range` is enforced by graph
/// validation and compilers; `soft_range` controls the ordinary UI widget.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct FieldDeclarationStatic {
    pub key: &'static str,
    pub label: &'static str,
    pub description: &'static str,
    pub target: FieldTarget,
    pub default: FieldDefault,
    pub hard_range: Option<NumericRange>,
    pub soft_range: Option<NumericRange>,
    pub step: Option<f32>,
    pub choices: &'static [ChoiceDeclaration],
    pub read_only: bool,
}

impl FieldDeclarationStatic {
    pub fn accepts(self, value: &PropertyValue) -> bool {
        if self.default.socket_type() != value.socket_type() {
            return false;
        }
        match (self.hard_range, value) {
            (Some(range), PropertyValue::Scalar(value)) => range.contains(*value),
            (Some(range), PropertyValue::Integer(value)) => range.contains(*value as f32),
            (_, PropertyValue::Vector3(value)) => value.iter().all(|value| value.is_finite()),
            (_, PropertyValue::Color(value)) => value.iter().all(|value| value.is_finite()),
            (_, PropertyValue::Text(value)) if !self.choices.is_empty() => self
                .choices
                .iter()
                .any(|choice| choice.value == value.as_str()),
            _ => true,
        }
    }

    /// The declared option carrying this persisted value, if the field offers
    /// choices at all.
    pub fn choice(&self, value: &str) -> Option<&'static ChoiceDeclaration> {
        let choices: &'static [ChoiceDeclaration] = self.choices;
        choices.iter().find(|choice| choice.value == value)
    }
}

pub const NONE: Option<NumericRange> = None;

pub const UNIT: Option<NumericRange> = Some(NumericRange::new(0.0, 1.0));

pub const SIGNED: Option<NumericRange> = Some(NumericRange::new(-1.0, 1.0));

pub const WIDE: Option<NumericRange> = Some(NumericRange::new(-1_000_000.0, 1_000_000.0));

pub const POSITIVE: Option<NumericRange> = Some(NumericRange::new(0.0, 1_000_000.0));

pub const EMPTY_CHOICES: &[ChoiceDeclaration] = &[];

#[allow(clippy::too_many_arguments)]
pub const fn field(
    key: &'static str,
    label: &'static str,
    description: &'static str,
    target: FieldTarget,
    default: FieldDefault,
    hard_range: Option<NumericRange>,
    soft_range: Option<NumericRange>,
    step: Option<f32>,
    choices: &'static [ChoiceDeclaration],
    read_only: bool,
) -> FieldDeclarationStatic {
    FieldDeclarationStatic {
        key,
        label,
        description,
        target,
        default,
        hard_range,
        soft_range,
        step,
        choices,
        read_only,
    }
}
