//! What one node type IS — its schema, not its behaviour.
//!
//! A [`NodeDeclaration`] is pure data, declared in a `static` by whichever crate owns that
//! family of nodes. This crate never evaluates one.
//!
//! [`OperationTag`] is the part worth reading about: see its own docs for why a declaration
//! carries a label here rather than the operation enum it used to.

use std::collections::BTreeMap;
use std::fmt;

use serde::{Deserialize, Serialize};

use crate::document::NodeRecord;
use crate::field::{FieldDeclarationStatic, FieldTarget};
use crate::id::{NodeTypeId, SocketKey};
use crate::socket::{SocketDeclarationStatic, TemporalDependence};

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum GraphKind {
    World,
    Material,
    MaterialFunction,
    Geometry,
    Environment,
    Biome,
    SurfaceRule,
    WorldModifier,
    Feature,
    Audio,
    Animation,
    Quality,
    RenderPipeline,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodeCategory {
    MaterialOutput,
    Inputs,
    Layers,
    Procedural,
    Coordinates,
    Utilities,
    Conditions,
    Environment,
    Biomes,
    Surface,
    Features,
    Audio,
    Animation,
    Quality,
    Render,
}

impl NodeCategory {
    pub const ALL: &'static [Self] = &[
        Self::MaterialOutput,
        Self::Inputs,
        Self::Layers,
        Self::Procedural,
        Self::Coordinates,
        Self::Utilities,
        Self::Conditions,
        Self::Environment,
        Self::Biomes,
        Self::Surface,
        Self::Features,
        Self::Audio,
        Self::Animation,
        Self::Quality,
        Self::Render,
    ];

    pub const fn label(self) -> &'static str {
        match self {
            Self::MaterialOutput => "Material Output",
            Self::Inputs => "Inputs",
            Self::Layers => "Layers",
            Self::Procedural => "Procedural",
            Self::Coordinates => "Coordinates & Vectors",
            Self::Utilities => "Utilities",
            Self::Conditions => "Conditions",
            Self::Environment => "Environment",
            Self::Biomes => "Biomes",
            Self::Surface => "Surface Composition",
            Self::Features => "Features",
            Self::Audio => "Audio",
            Self::Animation => "Animation",
            Self::Quality => "Quality",
            Self::Render => "Render",
        }
    }

    pub const fn color(self) -> [u8; 3] {
        match self {
            Self::MaterialOutput => [137, 57, 68],
            Self::Inputs => [125, 61, 76],
            Self::Layers => [84, 118, 144],
            Self::Procedural => [161, 91, 47],
            Self::Coordinates => [55, 119, 149],
            Self::Utilities => [161, 127, 47],
            Self::Conditions => [126, 93, 51],
            Self::Environment => [55, 128, 121],
            Self::Biomes => [69, 132, 78],
            Self::Surface => [104, 126, 50],
            Self::Features => [109, 105, 58],
            Self::Audio => [116, 72, 139],
            Self::Animation => [139, 72, 111],
            Self::Quality => [72, 101, 157],
            Self::Render => [77, 84, 101],
        }
    }
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum NodePreview {
    None,
    Value,
    ColorWheel,
    MaterialSphere,
    Noise,
    ColorRamp,
}

/// What a node *does*, as an opaque comparable label.
///
/// # Why this is a string and not an enum
///
/// It used to be `NodeOperation` — an enum listing every node kind in the project, 62
/// variants across seven families, 47 of them material. Holding that enum here would mean
/// this crate knows what `MixColor` and `PatternLayer` are, and adding a texture node would
/// mean editing this crate. That is the coupling the crate split exists to remove.
///
/// It can be a label because the mechanics never ask what an operation *means*. Across all
/// of validation they do exactly two things with it: compare two of them, and print one
/// into an author-facing diagnostic (`"graph contains 2 material.output node(s), expected
/// 1"`). Neither needs the variants.
///
/// The catalogue keeps its real enum and its exhaustive `match`, and converts at the
/// boundary. Nothing is lost on either side — a domain still cannot forget a node, because
/// its own `match` is still exhaustive.
///
/// # The one thing you give up
///
/// A typo'd label compiles. Two nodes could claim the same one. That is why a catalogue owes
/// a test asserting every declared tag maps back to a known operation; `voxel-material-graph`
/// has one. Use the node's own type id as the tag text — `"material.mix_color"` — so a
/// collision is visible on sight.
#[derive(Clone, Copy, Debug, Eq, PartialEq, Ord, PartialOrd, Hash)]
pub struct OperationTag(pub &'static str);

impl fmt::Display for OperationTag {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0.fmt(formatter)
    }
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct NodeDeclaration {
    pub id: &'static str,
    pub version: u32,
    pub title: &'static str,
    pub description: &'static str,
    pub category: NodeCategory,
    pub preview: NodePreview,
    pub operation: OperationTag,
    /// How this node's own operation depends on the clock. Declared explicitly on
    /// every node — see [`TemporalDependence`] for why it is not defaulted.
    pub temporal: TemporalDependence,
    pub kinds: &'static [GraphKind],
    pub inputs: &'static [SocketDeclarationStatic],
    pub outputs: &'static [SocketDeclarationStatic],
    pub fields: &'static [FieldDeclarationStatic],
}

impl NodeDeclaration {
    pub fn input(&self, key: &SocketKey) -> Option<SocketDeclarationStatic> {
        self.inputs
            .iter()
            .copied()
            .find(|socket| socket.key == key.0)
    }
    pub fn output(&self, key: &SocketKey) -> Option<SocketDeclarationStatic> {
        self.outputs
            .iter()
            .copied()
            .find(|socket| socket.key == key.0)
    }

    pub fn field(&self, target: FieldTarget, key: &str) -> Option<FieldDeclarationStatic> {
        self.fields
            .iter()
            .copied()
            .find(|field| field.target == target && field.key == key)
    }

    pub fn new_record(&self) -> NodeRecord {
        let mut properties = BTreeMap::new();
        let mut socket_defaults = BTreeMap::new();
        for field in self.fields {
            match field.target {
                FieldTarget::Property => {
                    properties.insert(field.key.to_string(), field.default.value());
                }
                FieldTarget::InputSocket => {
                    socket_defaults.insert(SocketKey(field.key.to_string()), field.default.value());
                }
            }
        }
        NodeRecord {
            node_type: NodeTypeId(self.id.to_string()),
            node_type_version: self.version,
            properties,
            socket_defaults,
            unknown_payload: None,
        }
    }
}
