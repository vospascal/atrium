//! `world.scatter_rule` — one decoration class, fully described.
//!
//! `world.scatter` used to carry a rate field per class, which meant adding a
//! seventh decoration meant editing the node, and it left every class sharing
//! one patch shape and one height band whether that suited it or not. A rule is
//! a node instead: it chains on the `rules` socket, so a world places as many as
//! it wants, each with its own rate, patch geometry and Y window, and the
//! scatter node itself is back to the two controls that genuinely are global.

use crate::graph::WorldNodeOperation;
use voxel_material_graph::declare::*;
use voxel_material_graph::{node, socket};

const SCATTER_RULE_IN: &[SocketDeclarationStatic] = &[socket!(
    "rules",
    "Rules",
    "An earlier rule in the chain. Leave unconnected to start a new chain.",
    SocketType::FeatureSet,
    EvaluationRate::Uniform,
    Cardinality::OPTIONAL_SINGLE
)];

const SCATTER_RULE_OUT: &[SocketDeclarationStatic] = &[socket!(
    "rules",
    "Rules",
    "This rule appended to the chain, for the next rule or for Scatter.",
    SocketType::FeatureSet,
    EvaluationRate::Uniform,
    Cardinality::ANY
)];

const CLASS_CHOICES: &[ChoiceDeclaration] = &[
    choice(
        "tall_grass",
        "Tall Grass",
        "Grass blades on lit surface voxels.",
    ),
    choice(
        "flowers",
        "Flowers",
        "Flowering plants on lit surface voxels.",
    ),
    choice("cave_moss", "Cave Moss", "Moss on cave floor voxels."),
    choice(
        "cave_mushroom",
        "Cave Mushroom",
        "Mushrooms on cave floor voxels.",
    ),
    choice(
        "cave_vine",
        "Cave Vine",
        "Vines hanging from cave ceiling voxels.",
    ),
    choice(
        "cave_crystal",
        "Cave Crystal",
        "Glowing crystals on cave wall voxels.",
    ),
];

const SCATTER_RULE_FIELDS: &[FieldDeclarationStatic] = &[
    field(
        "class",
        "Class",
        "Which decoration this rule places. The class also decides the surface it \
         attaches to — floor, ceiling, wall or lit surface.",
        FieldTarget::Property,
        FieldDefault::Text("tall_grass"),
        NONE,
        NONE,
        None,
        CLASS_CHOICES,
        false,
    ),
    field(
        "rate",
        "Rate",
        "Chance an eligible voxel inside a patch grows this decoration. For \
         light-emitting classes keep this low: a dense scatter of crystals \
         flattens the darkness that makes a cave read as one.",
        FieldTarget::Property,
        FieldDefault::Scalar(0.20),
        UNIT,
        UNIT,
        Some(0.005),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "patch_scale",
        "Patch Scale",
        "Frequency of the patch field that groups this decoration. Larger values \
         make smaller, more scattered patches.",
        FieldTarget::Property,
        FieldDefault::Scalar(0.06),
        POSITIVE,
        Some(NumericRange::new(0.001, 1.0)),
        Some(0.001),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "patch_coverage",
        "Patch Coverage",
        "Fraction of the world the patches cover. 1.0 covers everything, which \
         removes patchiness entirely and leaves Rate alone deciding density.",
        FieldTarget::Property,
        FieldDefault::Scalar(0.50),
        UNIT,
        UNIT,
        Some(0.01),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "min_y",
        "Minimum Y",
        "Lowest world voxel this rule may place decoration on.",
        FieldTarget::Property,
        FieldDefault::Integer(0),
        Some(NumericRange::new(-4096.0, 4096.0)),
        Some(NumericRange::new(0.0, 64.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
    field(
        "max_y",
        "Maximum Y",
        "Highest world voxel this rule may place decoration on. Set it below \
         Minimum Y and the rule places nothing at all.",
        FieldTarget::Property,
        FieldDefault::Integer(4096),
        Some(NumericRange::new(-4096.0, 4096.0)),
        Some(NumericRange::new(0.0, 64.0)),
        Some(1.0),
        EMPTY_CHOICES,
        false,
    ),
];

pub(crate) const DECLARATION: NodeDeclaration = node!(
    "world.scatter_rule",
    WorldNodeOperation::ScatterRule,
    "Scatter Rule",
    "Describes one decoration class: its rate, patch shape and height band.",
    NodeCategory::Features,
    NodePreview::None,
    WORLD,
    SCATTER_RULE_IN,
    SCATTER_RULE_OUT,
    SCATTER_RULE_FIELDS,
    TemporalDependence::Inherited,
);
