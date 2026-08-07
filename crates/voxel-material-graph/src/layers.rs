//! Canonical projection from material pattern nodes to the renderer's packed
//! pattern stack.
//!
//! Pattern layers remain evaluated by the renderer's established GPU stack. This
//! module gives that stack a graph-owned authoring surface. Pattern nodes form a
//! typed surface chain between `Material Surface` and `Material Output`; their
//! topology is their evaluation order. The bridge is deliberately separate from
//! graph shading so the node UI cannot create a second, subtly different
//! noise/blend implementation.

use std::collections::{BTreeMap, BTreeSet};
use std::fmt;

use crate::operation::MaterialNodeOperation;
use voxel_graph::{
    FieldTarget, GraphAsset, InputPin, LinkId, LinkRecord, NodeId, NodeRecord, NodeRegistry,
    NodeTypeId, OutputPin, PropertyValue, SocketKey,
};
use voxel_material::material::Material;
use voxel_material::pattern::{
    EdgeBandDirection, PatternBlend, PatternFrame, PatternGenerator, PatternLayer, PatternStack,
    PatternTarget, MAXIMUM_TILE_ASPECT, MAXIMUM_TILE_GAP, MAX_EMISSION_INTENSITY,
    MAX_NOISE_OCTAVES, MAX_PATTERN_LAYERS, MAX_RELIEF_HEIGHT_METERS, MAX_TEXELS_PER_VOXEL,
    MINIMUM_TILE_ASPECT, NO_PATTERNS, TEXEL_RUNGS,
};

pub const MATERIAL_OUTPUT_NODE: &str = "material.output";
pub const MATERIAL_SURFACE_NODE: &str = "material.surface";
pub const PATTERN_LAYER_NODE: &str = "material.pattern_layer";
pub const PATTERN_FLAT_NODE: &str = "material.pattern_flat";
pub const PATTERN_NOISE_NODE: &str = "material.pattern_noise";
pub const PATTERN_SPECKLE_NODE: &str = "material.pattern_speckle";
pub const PATTERN_PERLIN_NODE: &str = "material.pattern_perlin";
pub const PATTERN_SIMPLEX_NODE: &str = "material.pattern_simplex";
pub const PATTERN_RIDGED_NODE: &str = "material.pattern_ridged";
pub const PATTERN_TURBULENCE_NODE: &str = "material.pattern_turbulence";
pub const PATTERN_WORLEY_NODE: &str = "material.pattern_worley";
pub const PATTERN_WORLEY_EDGE_NODE: &str = "material.pattern_worley_edge";
pub const PATTERN_WORLEY_SMOOTH_NODE: &str = "material.pattern_worley_smooth";
pub const PATTERN_WAVE_NODE: &str = "material.pattern_wave";
pub const PATTERN_CHECKER_NODE: &str = "material.pattern_checker";
pub const PATTERN_EDGE_BAND_NODE: &str = "material.pattern_edge_band";
pub const DISPLACEMENT_NODE: &str = "material.displacement";
pub const PATTERN_TILE_TONE_NODE: &str = "material.pattern_tile_tone";
pub const PATTERN_TILE_EDGE_NODE: &str = "material.pattern_tile_edge";
/// The shared tessellation. Not a generator — it produces no pattern of its own;
/// it configures where the tiles ARE, and the layers downstream read it.
pub const TESSELLATION_NODE: &str = "material.tessellation";

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct MaterialSurfaceChain {
    pub output: NodeId,
    pub surface: NodeId,
    pub layers: Vec<NodeId>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayerGraphError {
    OutputCount { count: usize },
    MissingSurfaceConnection { node: NodeId },
    InvalidSurfaceNode { node: NodeId, node_type: NodeTypeId },
    MissingPatternConnection { layer: NodeId },
    InvalidPatternGenerator { node: NodeId, node_type: NodeTypeId },
    SurfaceCycle,
    TooManyLayers { count: usize, maximum: usize },
}

impl fmt::Display for LayerGraphError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::OutputCount { count } => {
                write!(formatter, "material graph needs one output, found {count}")
            }
            Self::MissingSurfaceConnection { node } => {
                write!(formatter, "surface node `{node}` has no surface input")
            }
            Self::InvalidSurfaceNode { node, node_type } => write!(
                formatter,
                "node `{node}` (`{node_type}`) cannot participate in a material surface chain"
            ),
            Self::MissingPatternConnection { layer } => {
                write!(formatter, "pattern layer `{layer}` has no pattern input")
            }
            Self::InvalidPatternGenerator { node, node_type } => write!(
                formatter,
                "node `{node}` (`{node_type}`) is not a pattern generator"
            ),
            Self::SurfaceCycle => formatter.write_str("material surface chain contains a cycle"),
            Self::TooManyLayers { count, maximum } => {
                write!(
                    formatter,
                    "material has {count} layers; maximum is {maximum}"
                )
            }
        }
    }
}

impl std::error::Error for LayerGraphError {}

/// Connect the intrinsic surface through one node per authored pattern and into
/// the output. The chain itself is the stack; no separate count/order metadata
/// exists to drift out of sync.
pub fn append_pattern_layer_nodes(
    graph: &mut GraphAsset,
    surface: &NodeId,
    output: &NodeId,
    stack: &PatternStack,
) {
    // One tessellation node per DISTINCT tiling, shared by every layer that uses it.
    //
    // Not one per layer, and not one for the whole stack. Per layer would defeat the
    // node's purpose the moment you dragged a slider — the layers would silently
    // disagree about where the tiles are. One for the stack would be a lie whenever
    // two layers legitimately tile differently, and would round-trip the second
    // one's numbers into the first's.
    let mut tessellations: Vec<(TileKey, NodeId)> = Vec::new();
    let mut previous = surface.clone();
    for (order, layer) in stack.active().enumerate() {
        let layer_id = NodeId::new();
        let generator_id = NodeId::new();
        graph
            .nodes
            .insert(layer_id.clone(), pattern_layer_node(layer));
        graph
            .nodes
            .insert(generator_id.clone(), pattern_generator_node(layer));
        graph
            .layout
            .positions
            .insert(layer_id.clone(), [700.0 + order as f32 * 260.0, 160.0]);
        graph
            .layout
            .positions
            .insert(generator_id.clone(), [700.0 + order as f32 * 260.0, -120.0]);

        let key = TileKey::of(layer);
        let tessellation_id = match tessellations.iter().find(|(seen, _)| *seen == key) {
            Some((_, id)) => id.clone(),
            None => {
                let id = NodeId::new();
                graph.nodes.insert(id.clone(), tessellation_node(layer));
                graph.layout.positions.insert(
                    id.clone(),
                    [700.0 + tessellations.len() as f32 * 260.0, -360.0],
                );
                tessellations.push((key, id.clone()));
                id
            }
        };
        connect_tessellation(graph, &tessellation_id, &generator_id);

        connect_pattern(graph, &generator_id, &layer_id);
        connect_surface(graph, &previous, &layer_id);
        previous = layer_id;
        if layer.relief_height_meters > 0.0 {
            let displacement_id = NodeId::new();
            graph
                .nodes
                .insert(displacement_id.clone(), displacement_node(layer));
            graph.layout.positions.insert(
                displacement_id.clone(),
                [760.0 + order as f32 * 260.0, 360.0],
            );
            connect_height(graph, &generator_id, &displacement_id);
            connect_surface(graph, &previous, &displacement_id);
            previous = displacement_id;
        }
    }
    connect_surface(graph, &previous, output);
    graph.layout.positions.insert(
        output.clone(),
        [700.0 + stack.active_count() as f32 * 260.0, 160.0],
    );
}

/// Apply layer-node edits back to the live material row. Returning `true` lets
/// the platform layer request the normal material-table upload and GI refresh.
pub fn sync_pattern_layers_from_graph(
    graph: &GraphAsset,
    material: &mut Material,
) -> Result<bool, LayerGraphError> {
    let stack = project_pattern_stack(graph, &crate::CATALOGUE)?;
    if material.patterns != stack {
        material.patterns = stack;
        Ok(true)
    } else {
        Ok(false)
    }
}

/// Compile the connected pattern generators and application nodes into the
/// renderer's packed stack. The graph is canonical; this is a derived runtime
/// projection rather than a second authoring representation.
pub fn project_pattern_stack(
    graph: &GraphAsset,
    registry: &NodeRegistry,
) -> Result<PatternStack, LayerGraphError> {
    let chain = resolve_material_surface_chain(graph, registry)?;
    let mut stack = NO_PATTERNS;
    let mut generator_slots = GeneratorSlots::default();
    for layer_id in chain.layers {
        let layer = graph
            .nodes
            .get(&layer_id)
            .expect("resolved surface-chain nodes remain present");
        let generator_id = incoming_pattern_source(graph, &layer_id)?;
        let generator = graph
            .nodes
            .get(&generator_id)
            .expect("resolved pattern source remains present");
        let layer_operation = registry
            .find(&layer.node_type)
            .and_then(|declaration| MaterialNodeOperation::from_tag(declaration.operation));
        let pattern_generator = pattern_generator_from_node(generator, &generator_id, registry)?;
        if layer_operation == Some(MaterialNodeOperation::Displacement) {
            if !property_bool(layer, "enabled") {
                continue;
            }
            if let Some(slot) = generator_slots.find(&generator_id) {
                if let Some(existing) = stack.layers[slot].as_mut() {
                    apply_displacement_properties(existing, layer);
                }
            } else {
                if stack.active_count() >= MAX_PATTERN_LAYERS {
                    return Err(LayerGraphError::TooManyLayers {
                        count: stack.active_count() + 1,
                        maximum: MAX_PATTERN_LAYERS,
                    });
                }
                let order = stack.active_count();
                let synthetic_node = pattern_layer_node(&PatternLayer {
                    amount: 0.0,
                    ..PatternLayer::IDENTITY
                });
                stack.layers[order] = Some(pattern_layer_from_nodes(
                    &synthetic_node,
                    generator,
                    pattern_generator,
                    incoming_tessellation(graph, &generator_id),
                ));
                let relief = stack.layers[order].as_mut().expect("just inserted");
                apply_displacement_properties(relief, layer);
                generator_slots.push(generator_id, order, true);
            }
            continue;
        }
        if layer_operation != Some(MaterialNodeOperation::PatternLayer) {
            return Err(LayerGraphError::InvalidSurfaceNode {
                node: layer_id,
                node_type: layer.node_type.clone(),
            });
        }
        if !property_bool(layer, "enabled") {
            continue;
        }
        // A displacement earlier in the chain may already hold this generator's
        // slot as a colourless placeholder. The colour layer then completes that
        // slot rather than appending a second one — chain order between a layer
        // and its displacement must not change how many slots the pair costs.
        if let Some(slot) = generator_slots.claim_synthetic(&generator_id) {
            let placeholder = stack.layers[slot].expect("synthetic slot is occupied");
            let mut completed = pattern_layer_from_nodes(
                layer,
                generator,
                pattern_generator,
                incoming_tessellation(graph, &generator_id),
            );
            completed.relief_height_meters = placeholder.relief_height_meters;
            completed.relief_faces = placeholder.relief_faces;
            completed.relief_normal = placeholder.relief_normal;
            completed.relief_invert = placeholder.relief_invert;
            completed.relief_bevel_fraction = placeholder.relief_bevel_fraction;
            completed.relief_normal_strength = placeholder.relief_normal_strength;
            completed.relief_steps = placeholder.relief_steps;
            stack.layers[slot] = Some(completed);
            continue;
        }
        let order = stack.active_count();
        stack.layers[order] = Some(pattern_layer_from_nodes(
            layer,
            generator,
            pattern_generator,
            incoming_tessellation(graph, &generator_id),
        ));
        generator_slots.push(generator_id, order, false);
    }
    Ok(stack)
}

/// The displacement node's authored response, applied onto the pattern slot that
/// shares its generator. One function so the merge path and the synthetic path
/// cannot drift apart field by field.
fn apply_displacement_properties(slot: &mut PatternLayer, displacement: &NodeRecord) {
    slot.relief_height_meters =
        property_scalar(displacement, "height_meters").clamp(0.0, MAX_RELIEF_HEIGHT_METERS);
    slot.relief_faces = voxel_material::pattern::PatternFaces {
        top: property_bool(displacement, "faces_top"),
        side: property_bool(displacement, "faces_side"),
        bottom: property_bool(displacement, "faces_bottom"),
    };
    slot.relief_normal = property_bool(displacement, "affects_normal");
    slot.relief_invert = property_bool(displacement, "invert");
    slot.relief_bevel_fraction = property_scalar(displacement, "bevel_fraction").clamp(
        voxel_material::pattern::MINIMUM_RELIEF_BEVEL_FRACTION,
        voxel_material::pattern::MAXIMUM_RELIEF_BEVEL_FRACTION,
    );
    slot.relief_normal_strength = property_scalar(displacement, "normal_strength")
        .clamp(0.0, voxel_material::pattern::MAXIMUM_RELIEF_NORMAL_STRENGTH);
    slot.relief_steps = property_integer(displacement, "steps")
        .clamp(0, voxel_material::pattern::MAX_RELIEF_STEPS as i64) as u32;
}

/// Which stack slot each generator node landed in, and whether that slot is
/// still a colourless placeholder a displacement created ahead of its layer.
#[derive(Default)]
struct GeneratorSlots {
    slots: Vec<(NodeId, usize, bool)>,
}

impl GeneratorSlots {
    fn find(&self, generator: &NodeId) -> Option<usize> {
        self.slots
            .iter()
            .find(|(source, _, _)| source == generator)
            .map(|(_, slot, _)| *slot)
    }

    /// The generator's slot, if it is a placeholder — and claims it: the caller
    /// is about to complete it with a real colour layer.
    fn claim_synthetic(&mut self, generator: &NodeId) -> Option<usize> {
        self.slots
            .iter_mut()
            .find(|(source, _, synthetic)| source == generator && *synthetic)
            .map(|entry| {
                entry.2 = false;
                entry.1
            })
    }

    fn push(&mut self, generator: NodeId, slot: usize, synthetic: bool) {
        self.slots.push((generator, slot, synthetic));
    }
}

pub fn resolve_material_surface_chain(
    graph: &GraphAsset,
    registry: &NodeRegistry,
) -> Result<MaterialSurfaceChain, LayerGraphError> {
    let outputs = graph
        .nodes
        .iter()
        .filter(|(_, record)| {
            registry.find(&record.node_type).is_some_and(|declaration| {
                declaration.operation == MaterialNodeOperation::Output.tag()
            })
        })
        .map(|(id, _)| id)
        .collect::<Vec<_>>();
    if outputs.len() != 1 {
        return Err(LayerGraphError::OutputCount {
            count: outputs.len(),
        });
    }
    resolve_surface_chain(graph, outputs[0], registry)
}

pub fn resolve_surface_chain(
    graph: &GraphAsset,
    output: &NodeId,
    registry: &NodeRegistry,
) -> Result<MaterialSurfaceChain, LayerGraphError> {
    let mut current = incoming_surface_source(graph, output)?;
    let mut layers = Vec::new();
    let mut visited = BTreeSet::new();
    loop {
        if !visited.insert(current.clone()) {
            return Err(LayerGraphError::SurfaceCycle);
        }
        let record =
            graph
                .nodes
                .get(&current)
                .ok_or_else(|| LayerGraphError::MissingSurfaceConnection {
                    node: current.clone(),
                })?;
        let operation = registry
            .find(&record.node_type)
            .and_then(|declaration| MaterialNodeOperation::from_tag(declaration.operation));
        match operation {
            Some(MaterialNodeOperation::Surface) => {
                if layers.len() > MAX_PATTERN_LAYERS * 2 {
                    return Err(LayerGraphError::TooManyLayers {
                        count: layers.len(),
                        maximum: MAX_PATTERN_LAYERS * 2,
                    });
                }
                layers.reverse();
                return Ok(MaterialSurfaceChain {
                    output: output.clone(),
                    surface: current,
                    layers,
                });
            }
            Some(MaterialNodeOperation::PatternLayer) => {
                layers.push(current.clone());
                current = incoming_surface_source(graph, &current)?;
            }
            Some(MaterialNodeOperation::Displacement) => {
                layers.push(current.clone());
                current = incoming_surface_source(graph, &current)?;
            }
            _ => {
                return Err(LayerGraphError::InvalidSurfaceNode {
                    node: current,
                    node_type: record.node_type.clone(),
                });
            }
        }
    }
}

fn incoming_surface_source(graph: &GraphAsset, node: &NodeId) -> Result<NodeId, LayerGraphError> {
    graph
        .links
        .values()
        .find(|link| link.to.node == *node && link.to.socket.0 == "surface")
        .map(|link| link.from.node.clone())
        .ok_or_else(|| LayerGraphError::MissingSurfaceConnection { node: node.clone() })
}

fn incoming_pattern_source(graph: &GraphAsset, layer: &NodeId) -> Result<NodeId, LayerGraphError> {
    graph
        .links
        .values()
        .find(|link| {
            link.to.node == *layer && matches!(link.to.socket.0.as_str(), "pattern" | "height")
        })
        .map(|link| link.from.node.clone())
        .ok_or_else(|| LayerGraphError::MissingPatternConnection {
            layer: layer.clone(),
        })
}

fn pattern_generator_from_node(
    generator: &NodeRecord,
    generator_id: &NodeId,
    registry: &NodeRegistry,
) -> Result<PatternGenerator, LayerGraphError> {
    let operation = registry
        .find(&generator.node_type)
        .and_then(|declaration| MaterialNodeOperation::from_tag(declaration.operation));
    let octaves =
        || property_integer(generator, "octaves").clamp(1, MAX_NOISE_OCTAVES as i64) as u32;
    match operation {
        Some(MaterialNodeOperation::PatternFlat) => Ok(PatternGenerator::Flat),
        Some(MaterialNodeOperation::PatternNoise) => {
            Ok(PatternGenerator::Noise { octaves: octaves() })
        }
        Some(MaterialNodeOperation::PatternSpeckle) => Ok(PatternGenerator::Speckle {
            density: property_scalar(generator, "density").clamp(0.0, 1.0),
        }),
        Some(MaterialNodeOperation::PatternPerlin) => {
            Ok(PatternGenerator::Perlin { octaves: octaves() })
        }
        Some(MaterialNodeOperation::PatternSimplex) => {
            Ok(PatternGenerator::Simplex { octaves: octaves() })
        }
        Some(MaterialNodeOperation::PatternRidged) => {
            Ok(PatternGenerator::Ridged { octaves: octaves() })
        }
        Some(MaterialNodeOperation::PatternTurbulence) => {
            Ok(PatternGenerator::Turbulence { octaves: octaves() })
        }
        Some(MaterialNodeOperation::PatternWorley) => Ok(PatternGenerator::Worley),
        Some(MaterialNodeOperation::PatternWorleyEdge) => Ok(PatternGenerator::WorleyEdge),
        Some(MaterialNodeOperation::PatternWorleySmooth) => Ok(PatternGenerator::WorleySmooth),
        Some(MaterialNodeOperation::PatternWave) => Ok(PatternGenerator::Wave {
            distortion: property_scalar(generator, "distortion").max(0.0),
        }),
        Some(MaterialNodeOperation::PatternChecker) => Ok(PatternGenerator::Checker),
        Some(MaterialNodeOperation::PatternTileTone) => Ok(PatternGenerator::TileTone),
        Some(MaterialNodeOperation::PatternTileEdge) => Ok(PatternGenerator::TileEdge {
            sharpness: property_scalar(generator, "sharpness").clamp(0.0, 1.0),
        }),
        Some(MaterialNodeOperation::PatternEdgeBand) => {
            let direction = match property_text(generator, "direction").as_str() {
                "bottom" => EdgeBandDirection::Bottom,
                _ => EdgeBandDirection::Top,
            };
            Ok(PatternGenerator::EdgeBand {
                direction,
                width: property_scalar(generator, "width").clamp(0.0, 1.0),
                jaggedness: property_scalar(generator, "jaggedness").clamp(0.0, 1.0),
            })
        }
        _ => Err(LayerGraphError::InvalidPatternGenerator {
            node: generator_id.clone(),
            node_type: generator.node_type.clone(),
        }),
    }
}

/// The tessellation wired into a generator node, if any.
///
/// Optional by design: only a `tile`-framed layer or a tile generator needs one,
/// and a layer without one falls back to the tessellation defaults rather than
/// failing. Those defaults are the identity for every frame that ignores them.
fn incoming_tessellation<'a>(graph: &'a GraphAsset, generator: &NodeId) -> Option<&'a NodeRecord> {
    let source = graph
        .links
        .values()
        .find(|link| link.to.node == *generator && link.to.socket.0 == "tessellation")
        .map(|link| &link.from.node)?;
    graph.nodes.get(source)
}

fn connect_surface(graph: &mut GraphAsset, from: &NodeId, to: &NodeId) {
    graph.links.insert(
        LinkId::new(),
        LinkRecord {
            from: OutputPin {
                node: from.clone(),
                socket: SocketKey("surface".into()),
            },
            to: InputPin {
                node: to.clone(),
                socket: SocketKey("surface".into()),
            },
            order: 0,
        },
    );
}

fn connect_pattern(graph: &mut GraphAsset, from: &NodeId, to: &NodeId) {
    graph.links.insert(
        LinkId::new(),
        LinkRecord {
            from: OutputPin {
                node: from.clone(),
                socket: SocketKey("pattern".into()),
            },
            to: InputPin {
                node: to.clone(),
                socket: SocketKey("pattern".into()),
            },
            order: 0,
        },
    );
}

/// A generator's mask into a displacement modifier. Same pattern output, but the
/// displacement declares its input as `height` — wiring it as `pattern` would
/// leave a generated graph with a socket its node never declared.
fn connect_height(graph: &mut GraphAsset, from: &NodeId, to: &NodeId) {
    graph.links.insert(
        LinkId::new(),
        LinkRecord {
            from: OutputPin {
                node: from.clone(),
                socket: SocketKey("pattern".into()),
            },
            to: InputPin {
                node: to.clone(),
                socket: SocketKey("height".into()),
            },
            order: 0,
        },
    );
}

/// A tiling, compared by bits so two layers authored from the same tessellation
/// node share one on the way back out. `f32` has no `Eq`, and comparing these with
/// a tolerance would merge tilings a user deliberately set a hair apart.
#[derive(Clone, Copy, PartialEq, Eq)]
struct TileKey([u32; 3]);

impl TileKey {
    fn of(layer: &PatternLayer) -> Self {
        TileKey([
            layer.tile_aspect.to_bits(),
            layer.tile_bond.to_bits(),
            layer.tile_gap.to_bits(),
        ])
    }
}

fn tessellation_node(layer: &PatternLayer) -> NodeRecord {
    NodeRecord {
        node_type: NodeTypeId(TESSELLATION_NODE.into()),
        node_type_version: 1,
        properties: [
            (
                "tile_aspect".to_string(),
                PropertyValue::Scalar(layer.tile_aspect),
            ),
            (
                "tile_bond".to_string(),
                PropertyValue::Scalar(layer.tile_bond),
            ),
            (
                "tile_gap".to_string(),
                PropertyValue::Scalar(layer.tile_gap),
            ),
        ]
        .into_iter()
        .collect(),
        socket_defaults: BTreeMap::new(),
        unknown_payload: None,
    }
}

fn connect_tessellation(graph: &mut GraphAsset, from: &NodeId, to: &NodeId) {
    graph.links.insert(
        LinkId::new(),
        LinkRecord {
            from: OutputPin {
                node: from.clone(),
                socket: SocketKey("tessellation".into()),
            },
            to: InputPin {
                node: to.clone(),
                socket: SocketKey("tessellation".into()),
            },
            order: 0,
        },
    );
}

fn pattern_layer_node(layer: &PatternLayer) -> NodeRecord {
    NodeRecord {
        node_type: NodeTypeId(PATTERN_LAYER_NODE.into()),
        node_type_version: 1,
        properties: pattern_layer_properties(layer).into_iter().collect(),
        socket_defaults: BTreeMap::new(),
        unknown_payload: None,
    }
}

fn displacement_node(layer: &PatternLayer) -> NodeRecord {
    NodeRecord {
        node_type: NodeTypeId(DISPLACEMENT_NODE.into()),
        node_type_version: 1,
        properties: [
            ("enabled".to_string(), PropertyValue::Boolean(true)),
            (
                "height_meters".to_string(),
                PropertyValue::Scalar(layer.relief_height_meters),
            ),
            (
                "faces_top".to_string(),
                PropertyValue::Boolean(layer.relief_faces.top),
            ),
            (
                "faces_side".to_string(),
                PropertyValue::Boolean(layer.relief_faces.side),
            ),
            (
                "faces_bottom".to_string(),
                PropertyValue::Boolean(layer.relief_faces.bottom),
            ),
            (
                "affects_normal".to_string(),
                PropertyValue::Boolean(layer.relief_normal),
            ),
            (
                "invert".to_string(),
                PropertyValue::Boolean(layer.relief_invert),
            ),
            (
                "normal_strength".to_string(),
                PropertyValue::Scalar(layer.relief_normal_strength),
            ),
            (
                "bevel_fraction".to_string(),
                PropertyValue::Scalar(layer.relief_bevel_fraction),
            ),
            (
                "steps".to_string(),
                PropertyValue::Integer(layer.relief_steps as i64),
            ),
        ]
        .into_iter()
        .collect(),
        socket_defaults: BTreeMap::new(),
        unknown_payload: None,
    }
}

fn pattern_generator_node(layer: &PatternLayer) -> NodeRecord {
    let node_type = match layer.generator {
        PatternGenerator::Flat => PATTERN_FLAT_NODE,
        PatternGenerator::Noise { .. } => PATTERN_NOISE_NODE,
        PatternGenerator::Speckle { .. } => PATTERN_SPECKLE_NODE,
        PatternGenerator::Perlin { .. } => PATTERN_PERLIN_NODE,
        PatternGenerator::Simplex { .. } => PATTERN_SIMPLEX_NODE,
        PatternGenerator::Ridged { .. } => PATTERN_RIDGED_NODE,
        PatternGenerator::Turbulence { .. } => PATTERN_TURBULENCE_NODE,
        PatternGenerator::Worley => PATTERN_WORLEY_NODE,
        PatternGenerator::WorleyEdge => PATTERN_WORLEY_EDGE_NODE,
        PatternGenerator::WorleySmooth => PATTERN_WORLEY_SMOOTH_NODE,
        PatternGenerator::Wave { .. } => PATTERN_WAVE_NODE,
        PatternGenerator::Checker => PATTERN_CHECKER_NODE,
        PatternGenerator::TileTone => PATTERN_TILE_TONE_NODE,
        PatternGenerator::TileEdge { .. } => PATTERN_TILE_EDGE_NODE,
        PatternGenerator::EdgeBand { .. } => PATTERN_EDGE_BAND_NODE,
    };
    NodeRecord {
        node_type: NodeTypeId(node_type.into()),
        node_type_version: 1,
        properties: pattern_generator_properties(layer).into_iter().collect(),
        socket_defaults: BTreeMap::new(),
        unknown_payload: None,
    }
}

fn pattern_generator_properties(layer: &PatternLayer) -> Vec<(String, PropertyValue)> {
    let mut properties = vec![
        (
            "texels_per_voxel".to_string(),
            PropertyValue::Integer(layer.texels_per_voxel as i64),
        ),
        (
            "vary_per_face".to_string(),
            PropertyValue::Boolean(layer.vary_per_face),
        ),
    ];
    if !matches!(layer.generator, PatternGenerator::EdgeBand { .. }) {
        properties.push((
            "frame".to_string(),
            PropertyValue::Text(frame_name(layer.frame).to_string()),
        ));
        properties.push((
            "period_meters".to_string(),
            PropertyValue::Scalar(layer.period_meters),
        ));
        properties.push((
            "grid_average".to_string(),
            PropertyValue::Boolean(layer.grid_average),
        ));
    }
    // The octave count is asked of the generator rather than matched on, so the
    // fractal family can grow without this function knowing about it.
    if layer.generator.has_octaves() {
        properties.push((
            "octaves".to_string(),
            PropertyValue::Integer(layer.generator.octaves() as i64),
        ));
    }
    match layer.generator {
        PatternGenerator::Speckle { density } => {
            properties.push(("density".to_string(), PropertyValue::Scalar(density)));
        }
        PatternGenerator::Wave { distortion } => {
            properties.push(("distortion".to_string(), PropertyValue::Scalar(distortion)));
        }
        PatternGenerator::TileEdge { sharpness } => {
            properties.push(("sharpness".to_string(), PropertyValue::Scalar(sharpness)));
        }
        PatternGenerator::EdgeBand {
            direction,
            width,
            jaggedness,
        } => {
            properties.push((
                "direction".to_string(),
                PropertyValue::Text(match direction {
                    EdgeBandDirection::Top => "top".to_string(),
                    EdgeBandDirection::Bottom => "bottom".to_string(),
                }),
            ));
            properties.push(("width".to_string(), PropertyValue::Scalar(width)));
            properties.push(("jaggedness".to_string(), PropertyValue::Scalar(jaggedness)));
        }
        _ => {}
    }
    // Orthogonal to the generator, so every generator node carries it.
    if !matches!(layer.generator, PatternGenerator::EdgeBand { .. }) {
        properties.push((
            "domain_warp".to_string(),
            PropertyValue::Scalar(layer.domain_warp),
        ));
    }
    properties
}

fn pattern_layer_properties(layer: &PatternLayer) -> Vec<(String, PropertyValue)> {
    vec![
        ("enabled".to_string(), PropertyValue::Boolean(true)),
        (
            "target".to_string(),
            PropertyValue::Text(target_name(layer.target).to_string()),
        ),
        (
            "blend".to_string(),
            PropertyValue::Text(blend_name(layer.blend).to_string()),
        ),
        ("amount".to_string(), PropertyValue::Scalar(layer.amount)),
        (
            "target_color".to_string(),
            PropertyValue::Color([
                layer.target_color[0],
                layer.target_color[1],
                layer.target_color[2],
                1.0,
            ]),
        ),
        (
            "faces_top".to_string(),
            PropertyValue::Boolean(layer.faces.top),
        ),
        (
            "faces_side".to_string(),
            PropertyValue::Boolean(layer.faces.side),
        ),
        (
            "faces_bottom".to_string(),
            PropertyValue::Boolean(layer.faces.bottom),
        ),
        (
            "emission_intensity".to_string(),
            PropertyValue::Scalar(layer.emission_intensity),
        ),
    ]
}

fn pattern_layer_from_nodes(
    layer: &NodeRecord,
    generator_node: &NodeRecord,
    generator: PatternGenerator,
    tessellation: Option<&NodeRecord>,
) -> PatternLayer {
    let texels = property_integer(generator_node, "texels_per_voxel")
        .clamp(0, MAX_TEXELS_PER_VOXEL as i64) as u32;
    let texels = if TEXEL_RUNGS.contains(&texels) {
        texels
    } else {
        8
    };
    let edge_band = matches!(generator, PatternGenerator::EdgeBand { .. });
    PatternLayer {
        generator,
        frame: if edge_band {
            PatternFrame::Face
        } else {
            match property_text(generator_node, "frame").as_str() {
                "voxel" => PatternFrame::Voxel,
                "face" => PatternFrame::Face,
                "tile" => PatternFrame::Tile,
                _ => PatternFrame::World,
            }
        },
        period_meters: if edge_band {
            1.0
        } else {
            property_scalar(generator_node, "period_meters").clamp(0.005, 4.0)
        },
        target: match property_text(layer, "target").as_str() {
            "roughness" => PatternTarget::Roughness,
            "emission" => PatternTarget::Emission,
            _ => PatternTarget::Albedo,
        },
        blend: match property_text(layer, "blend").as_str() {
            "mix_to_color" => PatternBlend::MixToColor,
            "add" => PatternBlend::Add,
            _ => PatternBlend::Multiply,
        },
        amount: property_scalar(layer, "amount").clamp(0.0, 1.0),
        target_color: property_color(layer, "target_color"),
        faces: voxel_material::pattern::PatternFaces {
            top: property_bool(layer, "faces_top"),
            side: property_bool(layer, "faces_side"),
            bottom: property_bool(layer, "faces_bottom"),
        },
        relief_faces: voxel_material::pattern::PatternFaces::ALL,
        texels_per_voxel: texels,
        vary_per_face: property_bool(generator_node, "vary_per_face"),
        // Edge band declares no grid-average field: its value is a per-column
        // band, already constant across each texel column.
        grid_average: !edge_band && property_bool(generator_node, "grid_average"),
        domain_warp: if edge_band {
            0.0
        } else {
            property_scalar(generator_node, "domain_warp").max(0.0)
        },
        // From the TESSELLATION node when one is wired, so every layer sharing it
        // gets the same numbers and they cannot drift apart. Defaults otherwise.
        tile_aspect: tessellation
            .map(|node| property_scalar(node, "tile_aspect"))
            .unwrap_or(1.0)
            .clamp(MINIMUM_TILE_ASPECT, MAXIMUM_TILE_ASPECT),
        tile_bond: tessellation
            .map(|node| property_scalar(node, "tile_bond"))
            .unwrap_or(0.0)
            .rem_euclid(1.0),
        tile_gap: tessellation
            .map(|node| property_scalar(node, "tile_gap"))
            .unwrap_or(0.0)
            .clamp(0.0, MAXIMUM_TILE_GAP),
        emission_intensity: property_scalar(layer, "emission_intensity")
            .clamp(0.0, MAX_EMISSION_INTENSITY),
        relief_height_meters: 0.0,
        relief_normal: true,
        relief_invert: false,
        relief_bevel_fraction: voxel_material::pattern::DEFAULT_RELIEF_BEVEL_FRACTION,
        relief_normal_strength: 1.0,
        relief_steps: 0,
    }
}

fn frame_name(frame: PatternFrame) -> &'static str {
    match frame {
        PatternFrame::World => "world",
        PatternFrame::Voxel => "voxel",
        PatternFrame::Face => "face",
        PatternFrame::Tile => "tile",
    }
}

fn target_name(target: PatternTarget) -> &'static str {
    match target {
        PatternTarget::Albedo => "albedo",
        PatternTarget::Roughness => "roughness",
        PatternTarget::Emission => "emission",
    }
}

fn blend_name(blend: PatternBlend) -> &'static str {
    match blend {
        PatternBlend::Multiply => "multiply",
        PatternBlend::MixToColor => "mix_to_color",
        PatternBlend::Add => "add",
    }
}

fn property_value(node: &NodeRecord, key: &str) -> PropertyValue {
    node.properties.get(key).cloned().unwrap_or_else(|| {
        crate::CATALOGUE
            .find(&node.node_type)
            .and_then(|declaration| declaration.field(FieldTarget::Property, key))
            .unwrap_or_else(|| panic!("node `{}` has no declared field `{key}`", node.node_type))
            .default
            .value()
    })
}

fn property_text(node: &NodeRecord, key: &str) -> String {
    match property_value(node, key) {
        PropertyValue::Text(value) => value,
        value => panic!("field `{key}` has unexpected value {value:?}"),
    }
}

fn property_scalar(node: &NodeRecord, key: &str) -> f32 {
    match property_value(node, key) {
        PropertyValue::Scalar(value) if value.is_finite() => value,
        value => panic!("field `{key}` has unexpected value {value:?}"),
    }
}

fn property_integer(node: &NodeRecord, key: &str) -> i64 {
    match property_value(node, key) {
        PropertyValue::Integer(value) => value,
        value => panic!("field `{key}` has unexpected value {value:?}"),
    }
}

fn property_bool(node: &NodeRecord, key: &str) -> bool {
    match property_value(node, key) {
        PropertyValue::Boolean(value) => value,
        value => panic!("field `{key}` has unexpected value {value:?}"),
    }
}

fn property_color(node: &NodeRecord, key: &str) -> [f32; 3] {
    match property_value(node, key) {
        PropertyValue::Color(value) if value.iter().all(|value| value.is_finite()) => {
            [value[0], value[1], value[2]]
        }
        value => panic!("field `{key}` has unexpected value {value:?}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use voxel_graph::{GraphAsset, GraphKind};
    use voxel_material::material::MATERIALS;

    fn graph_with_surface_chain(stack: &PatternStack) -> (GraphAsset, NodeId) {
        let registry = crate::CATALOGUE;
        let mut graph = GraphAsset::new("material", GraphKind::Material);
        let surface = NodeId::new();
        let output = NodeId::new();
        graph.nodes.insert(
            surface.clone(),
            registry
                .find(&NodeTypeId(MATERIAL_SURFACE_NODE.into()))
                .unwrap()
                .new_record(),
        );
        graph.nodes.insert(
            output.clone(),
            registry
                .find(&NodeTypeId(MATERIAL_OUTPUT_NODE.into()))
                .unwrap()
                .new_record(),
        );
        append_pattern_layer_nodes(&mut graph, &surface, &output, stack);
        (graph, output)
    }

    #[test]
    fn authored_layers_round_trip_through_graph_nodes() {
        let source = MATERIALS[26];
        assert!(!source.patterns.is_empty());
        let (graph, _output) = graph_with_surface_chain(&source.patterns);
        let mut restored = source;
        restored.patterns = NO_PATTERNS;
        assert!(sync_pattern_layers_from_graph(&graph, &mut restored).unwrap());
        assert_eq!(restored.patterns, source.patterns);
    }

    /// Relief must survive the material -> graph -> material round trip: the
    /// displacement node carries height, face scope, normal opt-out and invert,
    /// and projection folds them back onto the colour layer's slot.
    #[test]
    fn relief_round_trips_through_displacement_nodes() {
        let mut source = MATERIALS[26];
        let mut layer = *source.patterns.active().next().unwrap();
        layer.relief_height_meters = 0.04;
        layer.relief_invert = true;
        layer.relief_normal = false;
        layer.relief_faces = voxel_material::pattern::PatternFaces {
            top: true,
            side: true,
            bottom: false,
        };
        layer.relief_bevel_fraction = 0.2;
        layer.relief_normal_strength = 2.5;
        layer.relief_steps = 3;
        layer.grid_average = true;
        source.patterns = voxel_material::pattern::PatternStack::of(&[layer]);
        let (graph, _output) = graph_with_surface_chain(&source.patterns);
        let mut restored = source;
        restored.patterns = NO_PATTERNS;
        assert!(sync_pattern_layers_from_graph(&graph, &mut restored).unwrap());
        assert_eq!(restored.patterns, source.patterns);
    }

    /// A displacement placed AHEAD of its colour layer in the surface chain must
    /// complete into the same slot the layer takes, not burn a second one —
    /// chain order between the pair is an authoring detail, not a cost.
    #[test]
    fn displacement_ahead_of_its_layer_shares_one_slot() {
        let registry = crate::CATALOGUE;
        let mut graph = GraphAsset::new("material", GraphKind::Material);
        let surface = NodeId::new();
        let output = NodeId::new();
        graph.nodes.insert(
            surface.clone(),
            registry
                .find(&NodeTypeId(MATERIAL_SURFACE_NODE.into()))
                .unwrap()
                .new_record(),
        );
        graph.nodes.insert(
            output.clone(),
            registry
                .find(&NodeTypeId(MATERIAL_OUTPUT_NODE.into()))
                .unwrap()
                .new_record(),
        );
        let generator = NodeId::new();
        graph.nodes.insert(
            generator.clone(),
            registry
                .find(&NodeTypeId(PATTERN_NOISE_NODE.into()))
                .unwrap()
                .new_record(),
        );
        let displacement = NodeId::new();
        let mut displacement_record = registry
            .find(&NodeTypeId(DISPLACEMENT_NODE.into()))
            .unwrap()
            .new_record();
        displacement_record
            .properties
            .insert("height_meters".into(), PropertyValue::Scalar(0.05));
        displacement_record
            .properties
            .insert("invert".into(), PropertyValue::Boolean(true));
        displacement_record
            .properties
            .insert("bevel_fraction".into(), PropertyValue::Scalar(0.3));
        displacement_record
            .properties
            .insert("normal_strength".into(), PropertyValue::Scalar(1.5));
        graph
            .nodes
            .insert(displacement.clone(), displacement_record);
        let layer = NodeId::new();
        graph.nodes.insert(
            layer.clone(),
            registry
                .find(&NodeTypeId(PATTERN_LAYER_NODE.into()))
                .unwrap()
                .new_record(),
        );
        connect_surface(&mut graph, &surface, &displacement);
        connect_surface(&mut graph, &displacement, &layer);
        connect_surface(&mut graph, &layer, &output);
        connect_height(&mut graph, &generator, &displacement);
        connect_pattern(&mut graph, &generator, &layer);

        let stack = project_pattern_stack(&graph, &registry).unwrap();
        assert_eq!(
            stack.active_count(),
            1,
            "the displacement/layer pair must share one slot regardless of order"
        );
        let slot = stack.active().next().unwrap();
        assert_eq!(slot.relief_height_meters, 0.05);
        assert!(slot.relief_invert);
        assert_eq!(slot.relief_bevel_fraction, 0.3);
        assert_eq!(slot.relief_normal_strength, 1.5);
        assert!(
            slot.amount > 0.0,
            "the colour layer must have completed the placeholder slot"
        );
    }

    #[test]
    fn connected_generator_type_defines_the_projected_pattern() {
        let source = MATERIALS[26];
        let (mut graph, _output) = graph_with_surface_chain(&source.patterns);
        let layer_id = graph
            .nodes
            .iter()
            .find(|(_, node)| node.node_type.0 == PATTERN_LAYER_NODE)
            .map(|(id, _)| id.clone())
            .unwrap();
        let generator_id = incoming_pattern_source(&graph, &layer_id).unwrap();
        let mut speckle = crate::CATALOGUE
            .find(&NodeTypeId(PATTERN_SPECKLE_NODE.into()))
            .unwrap()
            .new_record();
        speckle
            .properties
            .insert("density".into(), PropertyValue::Scalar(0.4));
        graph.nodes.insert(generator_id, speckle);

        let stack = project_pattern_stack(&graph, &crate::CATALOGUE).unwrap();
        assert_eq!(
            stack.active().next().unwrap().generator,
            PatternGenerator::Speckle { density: 0.4 }
        );
    }

    #[test]
    fn connected_layer_requires_a_typed_pattern_source() {
        let source = MATERIALS[26];
        let (mut graph, _output) = graph_with_surface_chain(&source.patterns);
        graph.links.retain(|_, link| link.to.socket.0 != "pattern");

        assert!(matches!(
            project_pattern_stack(&graph, &crate::CATALOGUE),
            Err(LayerGraphError::MissingPatternConnection { .. })
        ));
    }

    #[test]
    fn disabling_a_layer_removes_it_without_changing_topology() {
        let source = MATERIALS[26];
        let (mut graph, _output) = graph_with_surface_chain(&source.patterns);
        let layer = graph
            .nodes
            .iter_mut()
            .find(|(_, node)| node.node_type.0 == PATTERN_LAYER_NODE)
            .unwrap();
        layer
            .1
            .properties
            .insert("enabled".into(), PropertyValue::Boolean(false));
        let mut restored = source;
        assert!(sync_pattern_layers_from_graph(&graph, &mut restored).unwrap());
        assert!(restored.patterns.is_empty());
    }

    #[test]
    fn disconnected_layers_are_not_part_of_the_material_stack() {
        let source = MATERIALS[26];
        let (mut graph, _output) = graph_with_surface_chain(&NO_PATTERNS);
        graph.nodes.insert(
            NodeId::new(),
            pattern_layer_node(source.patterns.active().next().unwrap()),
        );
        let mut restored = source;
        assert!(sync_pattern_layers_from_graph(&graph, &mut restored).unwrap());
        assert!(restored.patterns.is_empty());
    }
}
