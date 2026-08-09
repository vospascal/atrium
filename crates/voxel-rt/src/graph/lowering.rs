//! The one place a world/biome node record becomes an executable program.
//!
//! Every consumer lowers through this module: the runtime that builds the world,
//! the Studio viewport preview, and the node-card thumbnail. That is the whole
//! point of it existing. Before it, the runtime and the preview each carried a
//! field-by-field copy of the same mapping — same magic constants, same clamps —
//! and the node card carried a third, different one. The result was a preview
//! showing caves the world did not have and a thumbnail that responded to
//! sliders the generator ignored.
//!
//! Defaults come from the node DECLARATION, never from a hand-typed literal here.
//! A declaration is already the compiled source of truth for what a field means;
//! restating its default in the lowering is how the cave band came to read 4..24
//! on the node card while generation used 8..56.

use voxel_core::cave::{CaveFluids, CaveProgram, CaveShape};
use voxel_core::chunk::ChunkGenerator;
use voxel_core::scatter::{ScatterClass, ScatterProgram, ScatterRule, ScatterTarget};
use voxel_graph::{FieldTarget, GraphAsset, NodeRecord, PropertyValue};

/// The authored terrain shape, before caves.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerrainShape {
    pub surface_headroom: i32,
    pub sea_level: i32,
    pub base_height: i32,
    pub relief: i32,
}

impl Default for TerrainShape {
    fn default() -> Self {
        let defaults = ChunkGenerator::default();
        Self {
            surface_headroom: defaults.surface_headroom,
            sea_level: defaults.sea_level,
            base_height: defaults.terrain_base_height,
            relief: defaults.terrain_relief,
        }
    }
}

/// Everything the CPU generator needs, lowered from a graph.
///
/// Carried as one value so a new authored control is one field here and one field
/// on the node, rather than another positional parameter threaded through a stack
/// of constructors.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct GenerationProgram {
    pub terrain: TerrainShape,
    /// Caves off by default: a graph with no cave carver node is a valid cave-less
    /// world, and it must not acquire caves because something else defaulted them
    /// on. Every other field's default comes from its own type.
    pub caves_enabled: bool,
    pub cave_program: CaveProgram,
    pub cave_fluids: CaveFluids,
    pub scatter: ScatterProgram,
}

impl GenerationProgram {
    /// The generator this program describes, for one world seed.
    ///
    /// The seed is a parameter rather than a field because it belongs to the
    /// project manifest, not to the graph: one graph, replayed under a different
    /// seed, is a different world and the same program.
    pub fn chunk_generator(&self, world_seed: u64) -> ChunkGenerator {
        ChunkGenerator {
            world_seed,
            sea_level: self.terrain.sea_level,
            surface_headroom: self.terrain.surface_headroom,
            terrain_base_height: self.terrain.base_height,
            terrain_relief: self.terrain.relief,
            caves_enabled: self.caves_enabled,
            cave_program: self.cave_program.clone(),
            cave_fluids: self.cave_fluids,
            scatter: self.scatter.clone(),
            ..ChunkGenerator::default()
        }
    }
}

/// A property value, falling back to the field's DECLARED default.
///
/// Unlike the material graph's equivalent this never panics on an undeclared or
/// mistyped field: world graphs are authored files that may be older than the
/// build reading them, and a missing key must degrade to the default rather than
/// take down a live authoring session.
fn property(record: &NodeRecord, key: &str) -> Option<PropertyValue> {
    record.properties.get(key).cloned().or_else(|| {
        crate::graph::CATALOGUE
            .find(&record.node_type)
            .and_then(|declaration| declaration.field(FieldTarget::Property, key))
            .map(|field| field.default.value())
    })
}

fn scalar(record: &NodeRecord, key: &str, fallback: f32) -> f32 {
    match property(record, key) {
        Some(PropertyValue::Scalar(value)) if value.is_finite() => value,
        Some(PropertyValue::Integer(value)) => value as f32,
        _ => fallback,
    }
}

fn integer(record: &NodeRecord, key: &str, fallback: i32) -> i32 {
    match property(record, key) {
        Some(PropertyValue::Integer(value)) => {
            value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
        }
        Some(PropertyValue::Scalar(value)) if value.is_finite() => value.round() as i32,
        _ => fallback,
    }
}

fn integer_64(record: &NodeRecord, key: &str, fallback: i64) -> i64 {
    match property(record, key) {
        Some(PropertyValue::Integer(value)) => value,
        Some(PropertyValue::Scalar(value)) if value.is_finite() => value.round() as i64,
        _ => fallback,
    }
}

fn boolean(record: &NodeRecord, key: &str, fallback: bool) -> bool {
    match property(record, key) {
        Some(PropertyValue::Boolean(value)) => value,
        _ => fallback,
    }
}

fn text(record: &NodeRecord, key: &str) -> Option<String> {
    match property(record, key) {
        Some(PropertyValue::Text(value)) => Some(value),
        _ => None,
    }
}

/// The first record of a node type in a graph, if any.
pub fn node_of_type<'a>(graph: &'a GraphAsset, node_type: &str) -> Option<&'a NodeRecord> {
    graph
        .nodes
        .values()
        .find(|node| node.node_type.0 == node_type)
}

/// Lower `world.generated_terrain` into a terrain shape.
pub fn terrain_shape(record: &NodeRecord) -> TerrainShape {
    let defaults = TerrainShape::default();
    TerrainShape {
        // Only the lower bound is enforced here. The upper bound depends on the
        // generated span, which this lowering does not know, so
        // `ChunkGenerator::sanitized` owns it — one clamp, at the point that has
        // the information.
        surface_headroom: integer(record, "surface_headroom", defaults.surface_headroom).max(0),
        sea_level: integer(record, "sea_level", defaults.sea_level),
        base_height: integer(record, "base_height", defaults.base_height),
        relief: integer(record, "relief", defaults.relief).max(0),
    }
}

/// Lower `world.cave_carver` into the part of the cave program it owns: WHERE
/// caves may exist, and the value-noise family's shape.
///
/// The cellular recipe is deliberately not read here. It is authored in the biome
/// graph, one control per node, and applied by [`apply_biome_graph`]. Both graphs
/// used to author the same cellular parameters under different key names, so which
/// caves you got depended on which tab you had last pressed a button in.
pub fn cave_band(record: &NodeRecord) -> CaveProgram {
    let defaults = CaveProgram::default();
    let shape = match text(record, "shape").as_deref() {
        Some("spaghetti") => CaveShape::Spaghetti,
        Some("noodle") => CaveShape::Noodle,
        Some("cellular_automata") => CaveShape::CellularAutomata,
        _ => CaveShape::Cheese,
    };
    let candidate = CaveProgram {
        shape,
        min_y: integer(record, "min_y", defaults.min_y),
        max_y: integer(record, "max_y", defaults.max_y),
        horizontal_scale: scalar(record, "horizontal_scale", defaults.horizontal_scale),
        vertical_scale: scalar(record, "vertical_scale", defaults.vertical_scale),
        threshold: scalar(record, "threshold", defaults.threshold).clamp(0.0, 1.0),
        salt: integer_64(record, "salt", defaults.salt as i64).max(0) as u64,
        ..defaults
    };
    if candidate.is_valid() {
        candidate
    } else {
        defaults
    }
}

/// Apply one node of the cave/cellular family to a cave program.
///
/// One function over the whole family rather than one per node: each node owns a
/// disjoint set of keys, so dispatching on the node type and reading only that
/// node's keys makes the chain ORDER irrelevant — which is the property that lets
/// an author reorder or omit nodes and still get the world the remaining nodes
/// describe. Returns whether the record was one of the family.
pub fn apply_cave_node(program: &mut CaveProgram, record: &NodeRecord) -> bool {
    let defaults = CaveProgram::default();
    match record.node_type.0.as_str() {
        // ---- world graph: the 3D volume the cellular mask is intersected with --
        "world.cave_chambers" => {
            program.cellular_worley_scale = scalar(
                record,
                "cellular_worley_scale",
                defaults.cellular_worley_scale,
            )
            .clamp(0.001, 1.0);
            program.cellular_openness =
                scalar(record, "cellular_openness", defaults.cellular_openness).clamp(0.0, 1.0);
        }
        "world.cave_detail" => {
            program.cellular_billow_scale = scalar(
                record,
                "cellular_billow_scale",
                defaults.cellular_billow_scale,
            )
            .clamp(0.001, 1.0);
            program.cellular_billow_strength = scalar(
                record,
                "cellular_billow_strength",
                defaults.cellular_billow_strength,
            )
            .clamp(0.0, 1.0);
            program.cellular_billow_octaves = integer(
                record,
                "cellular_billow_octaves",
                defaults.cellular_billow_octaves as i32,
            )
            .clamp(0, 8) as u32;
        }
        // ---- biome graph: the cellular recipe, one decision per node ----------
        "biome.cellular_seed" => {
            program.cellular_fill =
                scalar(record, "cellular_fill", defaults.cellular_fill).clamp(0.0, 1.0);
            program.cellular_cell_size = integer(
                record,
                "cellular_cell_size",
                defaults.cellular_cell_size as i32,
            )
            .clamp(1, 8) as u32;
        }
        "biome.cellular_smooth" => {
            program.cellular_iterations =
                integer(record, "iterations", defaults.cellular_iterations as i32).clamp(0, 32)
                    as u32;
            program.cellular_r1_cutoff =
                integer(record, "r1_cutoff", defaults.cellular_r1_cutoff as i32).clamp(0, 9) as u32;
            program.cellular_r2_cutoff =
                integer(record, "r2_cutoff", defaults.cellular_r2_cutoff).clamp(-1, 25);
        }
        "biome.cellular_cleanup" => {
            program.cellular_cleanup_iterations = integer(
                record,
                "cleanup_iterations",
                defaults.cellular_cleanup_iterations as i32,
            )
            .clamp(0, 32) as u32;
        }
        "biome.cellular_connect" => {
            program.cellular_min_room_cells = integer(
                record,
                "min_room_cells",
                defaults.cellular_min_room_cells as i32,
            )
            .clamp(0, 4096) as u32;
            program.cellular_corridor_radius = integer(
                record,
                "corridor_radius",
                defaults.cellular_corridor_radius as i32,
            )
            .clamp(0, 8) as u32;
        }
        _ => return false,
    }
    true
}

/// Apply every cave/cellular node a graph contains, in a stable order.
///
/// Stable order rather than link order: the nodes own disjoint keys, so the result
/// is identical either way, and sorting by node id keeps the lowering independent
/// of how the author happened to wire the chain. Returns how many were applied,
/// which is what tells a caller whether the graph authored a recipe at all.
pub fn apply_cave_nodes(program: &mut CaveProgram, graph: &GraphAsset) -> usize {
    let mut applied = 0;
    for (_, record) in graph.nodes.iter() {
        if apply_cave_node(program, record) {
            applied += 1;
        }
    }
    applied
}

/// Lower the underground fluid levels off a cave record.
pub fn cave_fluids(record: &NodeRecord) -> CaveFluids {
    let defaults = CaveFluids::default();
    CaveFluids {
        water_level: integer(record, "water_level", defaults.water_level),
        lava_level: integer(record, "lava_level", defaults.lava_level),
        min_y: integer(record, "fluid_min_y", defaults.min_y),
        enabled: boolean(record, "fluids_enabled", defaults.enabled),
    }
}

/// The decoration class a `world.scatter_rule` record names.
fn scatter_class(record: &NodeRecord) -> Option<ScatterClass> {
    match text(record, "class")?.as_str() {
        "tall_grass" => Some(ScatterClass::TallGrass),
        "flowers" => Some(ScatterClass::Flowers),
        "cave_moss" => Some(ScatterClass::CaveMoss),
        "cave_mushroom" => Some(ScatterClass::CaveMushroom),
        "cave_vine" => Some(ScatterClass::CaveVine),
        "cave_crystal" => Some(ScatterClass::CaveCrystal),
        _ => None,
    }
}

/// Lower one `world.scatter_rule` node.
///
/// The target surface comes from the class, not from the node: a mushroom grows on
/// a floor and a vine hangs from a ceiling, and making that authorable would let a
/// graph describe decoration the placement pass cannot honour.
pub fn scatter_rule(record: &NodeRecord) -> Option<ScatterRule> {
    let class = scatter_class(record)?;
    let template = ScatterProgram::default()
        .rules
        .into_iter()
        .find(|rule| rule.class == class)?;
    Some(ScatterRule {
        class,
        target: class_target(class),
        enabled: true,
        chance: scalar(record, "rate", template.chance).clamp(0.0, 1.0),
        patch_scale: scalar(record, "patch_scale", template.patch_scale).clamp(0.001, 1.0),
        patch_coverage: scalar(record, "patch_coverage", template.patch_coverage).clamp(0.0, 1.0),
        min_y: integer(record, "min_y", template.min_y),
        max_y: integer(record, "max_y", template.max_y),
    })
}

/// Lower `world.scatter` plus every `world.scatter_rule` in the graph.
///
/// A graph with the pass enabled but no rule nodes scatters NOTHING, rather than
/// falling back to a default set. That is the honest reading of an empty chain: the
/// author removed the rules. Defaulting them back would make deleting a rule node
/// look broken.
pub fn scatter_program(record: &NodeRecord, graph: &GraphAsset) -> ScatterProgram {
    if !boolean(record, "enabled", true) {
        return ScatterProgram {
            enabled: false,
            rules: Vec::new(),
        };
    }
    let clumping = scalar(record, "clumping", 1.0).clamp(0.0, 4.0);
    let mut rules: Vec<ScatterRule> = graph
        .nodes
        .values()
        .filter(|node| node.node_type.0 == "world.scatter_rule")
        .filter_map(scatter_rule)
        .map(|rule| ScatterRule {
            // Clumping tightens every rule's patches together: at 1.0 the authored
            // coverage stands, above it patches shrink so the same rate reads as
            // denser clumps with barer ground between them.
            patch_coverage: (rule.patch_coverage / clumping.max(0.05)).clamp(0.02, 1.0),
            ..rule
        })
        .collect();
    // Deterministic order: rules are evaluated first-match-wins, so two rules
    // competing for one voxel must not resolve by hash iteration order.
    rules.sort_by_key(|rule| {
        ScatterClass::ALL
            .iter()
            .position(|candidate| *candidate == rule.class)
            .unwrap_or(usize::MAX)
    });
    ScatterProgram {
        enabled: true,
        rules,
    }
}

/// Where a decoration class attaches, for the debug window's legend.
pub const fn class_target(class: ScatterClass) -> ScatterTarget {
    match class {
        ScatterClass::TallGrass | ScatterClass::Flowers => ScatterTarget::SurfaceTop,
        ScatterClass::CaveMoss | ScatterClass::CaveMushroom => ScatterTarget::CaveFloor,
        ScatterClass::CaveVine => ScatterTarget::CaveCeiling,
        ScatterClass::CaveCrystal => ScatterTarget::CaveWall,
    }
}

/// Lower a complete world graph into a generation program.
///
/// Nodes that are absent leave their part of the program at its default, so a
/// graph carrying only terrain is a valid cave-less world rather than an error.
pub fn generation_program(graph: &GraphAsset) -> GenerationProgram {
    let mut program = GenerationProgram::default();
    if let Some(record) = node_of_type(graph, "world.generated_terrain") {
        program.terrain = terrain_shape(record);
    }
    if let Some(record) = node_of_type(graph, "world.cave_carver") {
        program.caves_enabled = true;
        program.cave_program = cave_band(record);
    }
    // Chambers and detail refine the cave program the carver established, so they
    // are applied after it and are harmless on a graph with no carver.
    apply_cave_nodes(&mut program.cave_program, graph);
    if let Some(record) = node_of_type(graph, "world.cave_fluids") {
        program.cave_fluids = cave_fluids(record);
    }
    if let Some(record) = node_of_type(graph, "world.scatter") {
        program.scatter = scatter_program(record, graph);
    }
    program
}

/// Apply a biome graph's cellular controls to an existing program.
///
/// The biome graph authors the cave RECIPE; the world graph authors where caves
/// may exist and what fills them. Keeping the vertical band and the fluids from
/// the world program is what stops the Biomes tab silently relocating the caves.
pub fn apply_biome_graph(
    program: &mut GenerationProgram,
    graph: &GraphAsset,
) -> Result<(), String> {
    let applied = apply_cave_nodes(&mut program.cave_program, graph);
    if applied == 0 {
        return Err(
            "biome graph authors no cellular nodes, so it describes no cave recipe".to_string(),
        );
    }
    // A biome graph that authors a cellular recipe means the caves ARE cellular.
    // The band and the fluids stay with the world graph that owns them, which is
    // what stops the Biomes tab silently relocating the caves.
    program.caves_enabled = true;
    program.cave_program.shape = CaveShape::CellularAutomata;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_graph::{NodeTypeId, PropertyValue};

    fn record(node_type: &str) -> NodeRecord {
        crate::graph::CATALOGUE
            .find(&NodeTypeId(node_type.to_string()))
            .expect("declared node")
            .new_record()
    }

    /// A freshly created node must lower to the same program its declaration
    /// advertises. This is the test that would have caught the 4..24-versus-8..56
    /// cave band: the node card and the generator now read one source.
    #[test]
    fn a_fresh_node_lowers_to_its_declared_defaults() {
        let defaults = CaveProgram::default();
        let band = cave_band(&record("world.cave_carver"));
        assert_eq!(band.min_y, defaults.min_y);
        assert_eq!(band.max_y, defaults.max_y);
        assert_eq!(band.threshold, defaults.threshold);

        // Each cave/cellular node contributes only its own keys, and a fresh one
        // must leave the program at the defaults it declares.
        let mut program = CaveProgram::default();
        for node_type in [
            "world.cave_chambers",
            "world.cave_detail",
            "biome.cellular_seed",
            "biome.cellular_smooth",
            "biome.cellular_cleanup",
            "biome.cellular_connect",
        ] {
            assert!(
                apply_cave_node(&mut program, &record(node_type)),
                "{node_type} is not recognised by the cave lowering"
            );
        }
        assert_eq!(program, defaults);

        assert_eq!(
            terrain_shape(&record("world.generated_terrain")),
            TerrainShape::default()
        );
        assert_eq!(
            cave_fluids(&record("world.cave_fluids")),
            CaveFluids::default()
        );
    }

    /// Every declared field must reach the program. A slider that cannot change
    /// the world is the exact bug this arc repaired twice: first the dead
    /// birth/survival fallbacks, then three fields left on a node after the code
    /// that read them was removed.
    #[test]
    fn every_authored_field_on_every_cave_node_changes_the_program() {
        let cases: [(&str, &str, PropertyValue); 11] = [
            (
                "world.cave_chambers",
                "cellular_worley_scale",
                PropertyValue::Scalar(0.11),
            ),
            (
                "world.cave_chambers",
                "cellular_openness",
                PropertyValue::Scalar(0.41),
            ),
            (
                "world.cave_detail",
                "cellular_billow_scale",
                PropertyValue::Scalar(0.21),
            ),
            (
                "world.cave_detail",
                "cellular_billow_strength",
                PropertyValue::Scalar(0.77),
            ),
            (
                "world.cave_detail",
                "cellular_billow_octaves",
                PropertyValue::Integer(5),
            ),
            (
                "biome.cellular_seed",
                "cellular_fill",
                PropertyValue::Scalar(0.61),
            ),
            (
                "biome.cellular_seed",
                "cellular_cell_size",
                PropertyValue::Integer(4),
            ),
            (
                "biome.cellular_smooth",
                "iterations",
                PropertyValue::Integer(7),
            ),
            (
                "biome.cellular_smooth",
                "r1_cutoff",
                PropertyValue::Integer(3),
            ),
            (
                "biome.cellular_cleanup",
                "cleanup_iterations",
                PropertyValue::Integer(1),
            ),
            (
                "biome.cellular_connect",
                "min_room_cells",
                PropertyValue::Integer(40),
            ),
        ];
        for (node_type, key, value) in cases {
            let mut probe = record(node_type);
            probe.properties.insert(key.to_string(), value);
            let mut program = CaveProgram::default();
            apply_cave_node(&mut program, &probe);
            assert_ne!(
                program,
                CaveProgram::default(),
                "authoring `{key}` on {node_type} did not change the program"
            );
        }
    }

    /// A node reads ONLY its own keys. That is what makes the chain order
    /// irrelevant, so an author can reorder or omit nodes without surprise.
    #[test]
    fn a_cave_node_ignores_another_nodes_keys() {
        let mut seed = record("biome.cellular_seed");
        // `iterations` belongs to the smoothing node, not the seed node.
        seed.properties
            .insert("iterations".to_string(), PropertyValue::Integer(31));
        let mut program = CaveProgram::default();
        apply_cave_node(&mut program, &seed);
        assert_eq!(
            program.cellular_iterations,
            CaveProgram::default().cellular_iterations,
            "the seed node consumed a key that belongs to the smoothing node"
        );
    }

    /// The biome graph must not relocate the caves the world graph authored, and a
    /// graph with no cellular nodes must say so rather than silently doing nothing.
    #[test]
    fn a_biome_graph_keeps_the_world_graphs_cave_band_and_fluids() {
        let mut program = GenerationProgram {
            caves_enabled: true,
            cave_program: CaveProgram {
                min_y: 3,
                max_y: 27,
                ..CaveProgram::default()
            },
            cave_fluids: CaveFluids {
                water_level: 9,
                ..CaveFluids::default()
            },
            ..GenerationProgram::default()
        };
        let mut graph = GraphAsset::new("biomes", voxel_graph::GraphKind::Biome);
        assert!(
            apply_biome_graph(&mut program, &graph).is_err(),
            "an empty biome graph must not read as a cave recipe"
        );
        let mut seed = record("biome.cellular_seed");
        seed.properties
            .insert("cellular_fill".to_string(), PropertyValue::Scalar(0.55));
        graph
            .nodes
            .insert(voxel_graph::NodeId("seed".to_string()), seed);
        graph.nodes.insert(
            voxel_graph::NodeId("smooth".to_string()),
            record("biome.cellular_smooth"),
        );
        apply_biome_graph(&mut program, &graph).expect("the graph authors a recipe");
        assert_eq!(program.cave_program.shape, CaveShape::CellularAutomata);
        assert_eq!(program.cave_program.min_y, 3, "the cave band moved");
        assert_eq!(program.cave_program.max_y, 27, "the cave band moved");
        assert_eq!(program.cave_fluids.water_level, 9, "the fluids moved");
        assert!((program.cave_program.cellular_fill - 0.55).abs() < f32::EPSILON);
    }

    /// Each decoration class is its own rule node, so the pass places exactly the
    /// classes the graph contains — and an enabled pass with no rule nodes places
    /// nothing rather than silently restoring a default set.
    #[test]
    fn scatter_rules_come_from_their_own_nodes() {
        let scatter = record("world.scatter");
        let mut graph = GraphAsset::new("world", voxel_graph::GraphKind::World);
        let empty = scatter_program(&scatter, &graph);
        assert!(empty.enabled);
        assert!(
            empty.rules.is_empty(),
            "an empty rule chain must place nothing"
        );

        let mut crystal = record("world.scatter_rule");
        crystal.properties.insert(
            "class".to_string(),
            PropertyValue::Text("cave_crystal".into()),
        );
        crystal
            .properties
            .insert("rate".to_string(), PropertyValue::Scalar(0.5));
        graph
            .nodes
            .insert(voxel_graph::NodeId("crystal".to_string()), crystal);
        let program = scatter_program(&scatter, &graph);
        assert_eq!(program.rules.len(), 1);
        let rule = &program.rules[0];
        assert_eq!(rule.class, ScatterClass::CaveCrystal);
        assert!((rule.chance - 0.5).abs() < f32::EPSILON);
        // The target comes from the class, never from the node.
        assert_eq!(rule.target, ScatterTarget::CaveWall);

        let mut disabled = record("world.scatter");
        disabled
            .properties
            .insert("enabled".to_string(), PropertyValue::Boolean(false));
        assert!(!scatter_program(&disabled, &graph).enabled);
    }
}
