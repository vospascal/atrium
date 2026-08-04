//! Which parts of a material graph could be evaluated once and cached, decided
//! from the node declarations rather than from a hardcoded list of node names.
//!
//! ## What problem this answers
//!
//! Pattern layers are evaluated per pixel per frame, from scratch, with no cache
//! and no textures. Bench section 11 prices them: one `worley` layer is ~1.0 ms
//! at 2560x1440, and a saturated four-layer stack ~3.1 ms — *every frame, for as
//! long as the surface is on screen*. Caching that is attractive because
//! `pattern_snap_to_texels` already quantises the sample coordinate to a 1.56 cm
//! lattice, so a cache would be EXACT rather than an approximation.
//!
//! The thing that decides whether a given material can use such a cache is
//! whether time reaches the FIELD. That is a static property of the graph, so it
//! can be answered at author time, before anything is built — which is the whole
//! point: an authoring tool can price a layer *and* say whether the price is
//! payable once or every frame.
//!
//! ## Why this is graph-level and not IR-level
//!
//! The obvious place to put a taint pass is the compiled IR, which is flat and
//! has explicit operands. It does not work here: a pattern generator is
//! **projected into the material table row** as authored constants
//! (`material_graph_layers::project_pattern_stack`), not lowered into
//! `MaterialInstruction`s. The IR never sees the field, so an IR pass cannot tell
//! a live field from a static one — it would report every material cacheable and
//! be right today for the wrong reason.
//!
//! The graph does see it, and since S3 every node declares
//! [`TemporalDependence`] and every socket declares [`Separable`], so the answer
//! is derived from declarations. Adding a node cannot silently change the
//! verdict; it has to declare its axis, and a test cross-checks that declaration
//! against what the node actually lowers to.
//!
//! ## The two ways an animated layer stays cacheable
//!
//! Time entering a pattern layer is not automatically fatal, because the two
//! animation sockets combine with the field in ways that factor out:
//!
//! - [`Separable::Scale`] (`animation_gain`) multiplies the layer's contribution
//!   *after* the field is sampled. Cache the field, apply the scalar per pixel.
//! - [`Separable::Translate`] (`drift_velocity`) moves *where* the field is read.
//!   For a pattern layer this is exact, because `pattern_drift_meters` quantises
//!   the offset to a whole number of texels — an integer index shift in the
//!   cache's own address space, with no resampling.
//!
//! The two are read differently, and it matters. `Scale` carries an ordinary
//! value and animates only if its source animates. `Translate` carries a
//! **velocity** that the shader multiplies by the clock itself, so connecting a
//! plain constant vector is enough to make the layer move — which is precisely
//! how lava is authored, with no oscillator in the graph at all.
//!
//! So a flowing, pulsing lava surface is fully cacheable, and its cache never
//! needs to be invalidated by the animation. Only an edit dirties it.

use std::collections::{BTreeMap, BTreeSet, VecDeque};

use voxel_graph::{Diagnostic, GraphAsset, NodeId, NodeRegistry, Separable, SocketKey, SocketType};

/// What a single pattern layer can do about caching its field.
#[derive(Clone, Debug, Eq, PartialEq)]
pub enum LayerCache {
    /// Nothing time-varying reaches this layer at all.
    Static,
    /// The field is static; time enters only through sockets that factor out of
    /// it. The cached field is still valid every frame.
    Separable {
        /// `animation_gain` is driven by a time-varying value.
        gain: bool,
        /// `drift_velocity` is driven by a time-varying value.
        drift: bool,
    },
    /// Time reaches the field itself through these sockets, so the field would
    /// have to be recomputed every frame and there is nothing to cache.
    ///
    /// Not reachable with the node set as it stands — every parameter that
    /// shapes a pattern field is an authored property, which
    /// `nothing_time_varying_can_shape_a_cacheable_pattern_field` pins. It is
    /// represented anyway so that promoting one of those to a socket produces a
    /// verdict instead of a wrong answer.
    Live { sockets: Vec<SocketKey> },
}

impl LayerCache {
    /// Whether the expensive part — the field evaluation — can be done once.
    pub fn is_cacheable(&self) -> bool {
        !matches!(self, LayerCache::Live { .. })
    }

    /// Short phrase for the authoring UI, to sit beside the layer's cost band.
    pub fn summary(&self) -> String {
        match self {
            LayerCache::Static => "cacheable".to_string(),
            LayerCache::Separable { gain, drift } => match (gain, drift) {
                (true, true) => "cacheable (gain + drift)".to_string(),
                (true, false) => "cacheable (gain)".to_string(),
                (false, true) => "cacheable (drift)".to_string(),
                (false, false) => "cacheable".to_string(),
            },
            LayerCache::Live { sockets } => {
                let names: Vec<&str> = sockets.iter().map(|socket| socket.0.as_str()).collect();
                format!("LIVE — time reaches {}", names.join(", "))
            }
        }
    }
}

/// One pattern layer's verdict, keyed by the node that declares it.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct LayerReport {
    pub node: NodeId,
    pub cache: LayerCache,
}

/// What a whole graph can cache.
#[derive(Clone, Debug, Eq, PartialEq, Default)]
pub struct CacheReport {
    /// One entry per `material.pattern_layer`, sorted by node id so the report is
    /// deterministic. NOT in surface-chain order — chain order is
    /// `material_graph_layers`' business and needs the whole chain resolved.
    pub layers: Vec<LayerReport>,
    /// Nodes that introduce time on their own: the clock and event readers.
    pub sources: Vec<NodeId>,
}

impl CacheReport {
    /// Whether every layer's field can be cached. A graph with no layers is
    /// vacuously true, which is correct — there is no per-pixel field work.
    pub fn is_fully_cacheable(&self) -> bool {
        self.layers.iter().all(|layer| layer.cache.is_cacheable())
    }

    /// Layers whose field must be recomputed every frame.
    pub fn live_layers(&self) -> impl Iterator<Item = &LayerReport> {
        self.layers
            .iter()
            .filter(|layer| !layer.cache.is_cacheable())
    }

    /// Author-facing diagnostics. A live layer is a WARNING and not an error: the
    /// graph is perfectly valid and will render correctly, it just cannot use a
    /// cache that does not exist yet. The message names the fix, because there
    /// almost always is one — moving an oscillator from something that shapes the
    /// field onto `animation_gain` usually looks the same and costs nothing.
    pub fn diagnostics(&self) -> Vec<Diagnostic> {
        self.live_layers()
            .map(|layer| {
                let LayerCache::Live { sockets } = &layer.cache else {
                    unreachable!("live_layers yields only Live");
                };
                let names: Vec<&str> = sockets.iter().map(|socket| socket.0.as_str()).collect();
                Diagnostic::warning(
                    "material.layer_not_cacheable",
                    format!(
                        "Pattern layer `{}` cannot be cached: a time-varying value reaches {}, \
                         which shapes the pattern itself, so the field has to be recomputed every \
                         frame. Driving `animation_gain` or `drift_velocity` instead keeps the \
                         field cacheable.",
                        layer.node.0,
                        names.join(", ")
                    ),
                )
            })
            .collect()
    }
}

/// The set of nodes whose output changes over time.
///
/// A node is time-varying if it declares itself a source
/// ([`TemporalDependence::is_source`]) or if anything reaching it is. That is
/// plain forward reachability over the link graph — no fixed point needed,
/// because links form a DAG (`GraphError::Cycle` is rejected at edit time).
fn time_varying_nodes(graph: &GraphAsset, registry: &NodeRegistry) -> BTreeSet<NodeId> {
    let mut downstream: BTreeMap<&NodeId, Vec<&NodeId>> = BTreeMap::new();
    for link in graph.links.values() {
        downstream
            .entry(&link.from.node)
            .or_default()
            .push(&link.to.node);
    }

    let mut tainted: BTreeSet<NodeId> = BTreeSet::new();
    let mut queue: VecDeque<&NodeId> = VecDeque::new();
    for (node_id, record) in &graph.nodes {
        let is_source = registry
            .find(&record.node_type)
            .is_some_and(|declaration| declaration.temporal.is_source());
        if is_source {
            tainted.insert(node_id.clone());
            queue.push_back(node_id);
        }
    }

    while let Some(node_id) = queue.pop_front() {
        let Some(consumers) = downstream.get(node_id) else {
            continue;
        };
        for consumer in consumers {
            if tainted.insert((*consumer).clone()) {
                queue.push_back(consumer);
            }
        }
    }
    tainted
}

/// Classify every pattern layer in `graph`.
pub fn analyse(graph: &GraphAsset, registry: &NodeRegistry) -> CacheReport {
    let tainted = time_varying_nodes(graph, registry);

    // Incoming links per node, so each layer can ask what feeds each socket.
    let mut incoming: BTreeMap<&NodeId, Vec<(&SocketKey, &NodeId)>> = BTreeMap::new();
    for link in graph.links.values() {
        incoming
            .entry(&link.to.node)
            .or_default()
            .push((&link.to.socket, &link.from.node));
    }

    let mut layers = Vec::new();
    for (node_id, record) in &graph.nodes {
        let Some(declaration) = registry.find(&record.node_type) else {
            continue;
        };
        if declaration.id != "material.pattern_layer" {
            continue;
        }

        let mut gain = false;
        let mut drift = false;
        let mut live_sockets = Vec::new();
        for (socket_key, source) in incoming.get(node_id).into_iter().flatten() {
            let Some(socket) = declaration.input(socket_key) else {
                continue;
            };
            // THE TWO SEPARABLE SOCKETS ARE NOT SYMMETRIC, and reading them the
            // same way is wrong.
            //
            // `Scale` carries an ordinary value: it animates only if what feeds
            // it animates, so a constant there is just a constant multiplier.
            //
            // `Translate` carries a VELOCITY, and the shader multiplies it by the
            // clock itself (`pattern_drift_meters`). Connecting a plain constant
            // vector is therefore enough to make the layer move — that is exactly
            // how lava is authored, with no oscillator anywhere in the graph. A
            // taint test alone would report such a layer as `Static` and a cache
            // built on that would never apply the offset, freezing the flow.
            if socket.separable == Separable::Translate {
                drift = true;
                continue;
            }
            if !tainted.contains(*source) {
                continue;
            }
            match socket.separable {
                Separable::Scale => gain = true,
                Separable::Translate => unreachable!("handled above"),
                // The chain input. A time-varying surface arriving here means an
                // UPSTREAM layer animates, which is that layer's verdict to
                // carry — it says nothing about whether this layer's own field
                // is static, so it must not condemn it.
                Separable::None if socket.value_type == SocketType::MaterialSurface => {}
                Separable::None => live_sockets.push((*socket_key).clone()),
            }
        }

        live_sockets.sort();
        let cache = if !live_sockets.is_empty() {
            LayerCache::Live {
                sockets: live_sockets,
            }
        } else if gain || drift {
            LayerCache::Separable { gain, drift }
        } else {
            LayerCache::Static
        };
        layers.push(LayerReport {
            node: node_id.clone(),
            cache,
        });
    }

    layers.sort_by(|a, b| a.node.0.cmp(&b.node.0));
    CacheReport {
        layers,
        sources: tainted
            .iter()
            .filter(|node_id| {
                graph
                    .nodes
                    .get(*node_id)
                    .and_then(|record| registry.find(&record.node_type))
                    .is_some_and(|declaration| declaration.temporal.is_source())
            })
            .cloned()
            .collect(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lowering::new_material_graph;
    use voxel_graph::{GraphCommand, InputPin, LinkId, NodeTypeId, OutputPin};

    fn node(name: &str) -> NodeId {
        NodeId(name.into())
    }

    fn socket(name: &str) -> SocketKey {
        SocketKey(name.into())
    }

    /// Add a node of `node_type` named `name`, through the command layer so it
    /// materialises the declared socket defaults exactly as the editor would.
    fn add(graph: &mut GraphAsset, name: &str, node_type: &str) -> NodeId {
        let id = node(name);
        GraphCommand::AddNode {
            id: id.clone(),
            node_type: NodeTypeId(node_type.into()),
            position: [0.0, 0.0],
        }
        .apply(graph, &crate::CATALOGUE)
        .expect("adding a builtin node");
        id
    }

    fn link(graph: &mut GraphAsset, from: (&NodeId, &str), to: (&NodeId, &str)) {
        GraphCommand::Connect {
            id: LinkId(format!("{}:{}->{}:{}", from.0 .0, from.1, to.0 .0, to.1)),
            from: OutputPin {
                node: from.0.clone(),
                socket: socket(from.1),
            },
            to: InputPin {
                node: to.0.clone(),
                socket: socket(to.1),
            },
        }
        .apply(graph, &crate::CATALOGUE)
        .expect("connecting two builtin sockets");
    }

    /// A layer fed by a plain generator, with nothing animated, is cacheable and
    /// says so without qualification.
    #[test]
    fn a_still_layer_is_static() {
        let mut graph = new_material_graph("still");
        let worley = add(&mut graph, "worley", "material.pattern_worley");
        let layer = add(&mut graph, "layer", "material.pattern_layer");
        link(&mut graph, (&worley, "pattern"), (&layer, "pattern"));

        let report = analyse(&graph, &crate::CATALOGUE);
        assert_eq!(report.layers.len(), 1);
        assert_eq!(report.layers[0].cache, LayerCache::Static);
        assert!(report.is_fully_cacheable());
        assert!(report.diagnostics().is_empty());
    }

    /// An oscillator on `animation_gain` keeps the field cacheable — the whole
    /// point of the separable classification.
    #[test]
    fn an_oscillator_on_the_gain_leaves_the_field_cacheable() {
        let mut graph = new_material_graph("pulse");
        let worley = add(&mut graph, "worley", "material.pattern_worley");
        let layer = add(&mut graph, "layer", "material.pattern_layer");
        let oscillator = add(&mut graph, "osc", "material.oscillator");
        link(&mut graph, (&worley, "pattern"), (&layer, "pattern"));
        link(
            &mut graph,
            (&oscillator, "value"),
            (&layer, "animation_gain"),
        );

        let report = analyse(&graph, &crate::CATALOGUE);
        assert_eq!(
            report.layers[0].cache,
            LayerCache::Separable {
                gain: true,
                drift: false
            }
        );
        assert!(report.is_fully_cacheable());
        assert_eq!(report.sources, vec![oscillator]);
    }

    /// A CONSTANT vector on `drift_velocity` still animates the layer, because
    /// the shader multiplies it by the clock. This is the case a plain taint pass
    /// gets wrong — there is no oscillator anywhere, yet the pattern moves, and a
    /// cache that believed "static" would freeze the flow.
    ///
    /// It is also exactly how lava is authored, so it is not a corner case.
    #[test]
    fn a_constant_drift_velocity_still_counts_as_animation() {
        let mut graph = new_material_graph("flow");
        let worley = add(&mut graph, "worley", "material.pattern_worley");
        let layer = add(&mut graph, "layer", "material.pattern_layer");
        // `material.direction` is how a constant velocity is authored — there is
        // no constant-vector node, and this one is literally documented as "a
        // velocity of length Speed", which is exactly the lava case.
        let direction = add(&mut graph, "direction", "material.direction");
        link(&mut graph, (&worley, "pattern"), (&layer, "pattern"));
        link(
            &mut graph,
            (&direction, "vector"),
            (&layer, "drift_velocity"),
        );

        let report = analyse(&graph, &crate::CATALOGUE);
        assert_eq!(
            report.layers[0].cache,
            LayerCache::Separable {
                gain: false,
                drift: true
            }
        );
        // No node in this graph reads the clock, and yet the layer animates.
        assert!(report.sources.is_empty());
        assert!(report.is_fully_cacheable());
    }

    /// Time arriving on the `surface` chain input means an UPSTREAM layer
    /// animates. It must not condemn the downstream layer's own field, which is
    /// still perfectly static.
    #[test]
    fn animation_upstream_in_the_chain_does_not_condemn_the_next_layer() {
        let mut graph = new_material_graph("chain");
        let first = add(&mut graph, "first", "material.pattern_layer");
        let second = add(&mut graph, "second", "material.pattern_layer");
        let worley_a = add(&mut graph, "worley_a", "material.pattern_worley");
        let worley_b = add(&mut graph, "worley_b", "material.pattern_worley");
        let oscillator = add(&mut graph, "osc", "material.oscillator");
        link(&mut graph, (&worley_a, "pattern"), (&first, "pattern"));
        link(&mut graph, (&worley_b, "pattern"), (&second, "pattern"));
        link(
            &mut graph,
            (&oscillator, "value"),
            (&first, "animation_gain"),
        );
        link(&mut graph, (&first, "surface"), (&second, "surface"));

        let report = analyse(&graph, &crate::CATALOGUE);
        let first_report = report
            .layers
            .iter()
            .find(|entry| entry.node == first)
            .expect("first layer");
        let second_report = report
            .layers
            .iter()
            .find(|entry| entry.node == second)
            .expect("second layer");
        assert_eq!(
            first_report.cache,
            LayerCache::Separable {
                gain: true,
                drift: false
            }
        );
        assert_eq!(
            second_report.cache,
            LayerCache::Static,
            "the second layer's own field is untouched by the first layer's gain"
        );
    }

    /// Taint travels through intermediate maths, not just direct links — the
    /// reason this is a reachability pass and not a check on the layer's
    /// immediate neighbours.
    #[test]
    fn time_propagates_through_intermediate_nodes() {
        let mut graph = new_material_graph("indirect");
        let worley = add(&mut graph, "worley", "material.pattern_worley");
        let layer = add(&mut graph, "layer", "material.pattern_layer");
        let oscillator = add(&mut graph, "osc", "material.oscillator");
        let clamp = add(&mut graph, "clamp", "material.clamp_scalar");
        link(&mut graph, (&worley, "pattern"), (&layer, "pattern"));
        link(&mut graph, (&oscillator, "value"), (&clamp, "value"));
        link(&mut graph, (&clamp, "value"), (&layer, "animation_gain"));

        let report = analyse(&graph, &crate::CATALOGUE);
        assert_eq!(
            report.layers[0].cache,
            LayerCache::Separable {
                gain: true,
                drift: false
            }
        );
    }

    /// The shipped lava material is the motivating case: it should come back
    /// cacheable, because everything it animates is separable.
    #[test]
    fn the_shipped_lava_graph_is_cacheable() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../studio-project/graphs/material-26.vgraph.json");
        let text = std::fs::read_to_string(&path).expect("the checked-in lava graph");
        let graph: GraphAsset = serde_json::from_str(&text).expect("lava graph parses");

        let report = analyse(&graph, &crate::CATALOGUE);
        assert!(
            !report.layers.is_empty(),
            "lava should author at least one pattern layer"
        );
        assert!(
            report.is_fully_cacheable(),
            "lava is not cacheable: {:?}",
            report.live_layers().collect::<Vec<_>>()
        );
        assert!(report.diagnostics().is_empty());
    }

    /// The `Live` verdict is unreachable through the shipped node set, so it is
    /// proved out directly on the type rather than left untested: the summary and
    /// the diagnostic are what an author would actually see, and they should be
    /// correct on the day a generator parameter becomes a socket.
    #[test]
    fn a_live_layer_reports_and_warns() {
        let report = CacheReport {
            layers: vec![LayerReport {
                node: node("layer"),
                cache: LayerCache::Live {
                    sockets: vec![socket("domain_warp")],
                },
            }],
            sources: vec![node("osc")],
        };
        assert!(!report.is_fully_cacheable());
        assert_eq!(report.live_layers().count(), 1);
        assert_eq!(
            report.layers[0].cache.summary(),
            "LIVE — time reaches domain_warp"
        );
        let diagnostics = report.diagnostics();
        assert_eq!(diagnostics.len(), 1);
        assert_eq!(diagnostics[0].code, "material.layer_not_cacheable");
        assert!(diagnostics[0].message.contains("domain_warp"));
    }

    /// Unused but declared: a property-only graph with no layers is vacuously
    /// cacheable, which is the right answer — there is no per-pixel field work to
    /// pay for either way.
    #[test]
    fn a_graph_with_no_layers_is_vacuously_cacheable() {
        let graph = new_material_graph("bare");
        let report = analyse(&graph, &crate::CATALOGUE);
        assert!(report.layers.is_empty());
        assert!(report.is_fully_cacheable());
    }
}
