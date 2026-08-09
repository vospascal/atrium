//! `biome.definition` — a reusable biome definition.

use crate::graph::BiomeNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const OUTPUTS: &[SocketDeclarationStatic] = &[socket!(
    "biome",
    "Biome",
    "A biome definition selected by the world biome graph.",
    SocketType::BiomeDefinition,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];

const BIOME_TYPE_CHOICES: &[ChoiceDeclaration] = &[
    choice(
        "surface",
        "Surface",
        "Above-ground terrain and its surface features.",
    ),
    choice(
        "cave",
        "Cave",
        "Underground cave terrain and its cave features.",
    ),
];

const FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "id",
        "Biome ID",
        "Stable identifier used by world profiles and replayed saves.",
        FieldTarget::Property,
        FieldDefault::Text("surface"),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "name",
        "Name",
        "Human-readable name shown in the biome palette.",
        FieldTarget::Property,
        FieldDefault::Text("Surface"),
        NONE,
        NONE,
        None,
        EMPTY_CHOICES,
        false,
    ),
    field(
        "biome_type",
        "Biome Type",
        "Selects surface or cave generation routing.",
        FieldTarget::Property,
        FieldDefault::Text("surface"),
        NONE,
        NONE,
        None,
        BIOME_TYPE_CHOICES,
        false,
    ),
    field(
        "priority",
        "Priority",
        "Tie-break priority when selectors overlap.",
        FieldTarget::Property,
        FieldDefault::Integer(0),
        Some(NumericRange::new(-1000.0, 1000.0)),
        Some(NumericRange::new(-100.0, 100.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "biome.definition",
    BiomeNodeOperation::Definition,
    "Biome Definition",
    "Provides one authored biome definition to the biome graph.",
    NodeCategory::Biomes,
    NodePreview::None,
    BIOME,
    &[],
    OUTPUTS,
    FIELDS,
    TemporalDependence::Inherited,
);
