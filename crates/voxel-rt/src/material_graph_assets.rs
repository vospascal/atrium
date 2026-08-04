//! Material Graph asset lifecycle.
//!
//! This module keeps project-file concerns out of the window/platform layer.
//! Rendering only consumes a [`MaterialGraphShaderSet`]; Graph Studio edits a
//! [`GraphAsset`]; this service owns the boundary between those two worlds:
//! resolving required references, compiling them, and committing a graph plus
//! its material reference. Invalid authored data is an error, never a fallback.

use std::path::{Path, PathBuf};

use crate::graph::{GraphAsset, NodeRegistry};
use crate::material_graph::{compile, MaterialGraphShaderSet, MaterialSampleContext};
use crate::material_graph_layers::sync_pattern_layers_from_graph;
use crate::material_table::MaterialTable;
use crate::studio_assets::{AssetError, StudioProject, StudioProjectStore};

/// Result of selecting a material in Graph Studio.
#[derive(Clone, Debug)]
pub struct OpenedMaterialGraph {
    pub graph: GraphAsset,
    pub status: String,
}

/// The material graph asset boundary. Stateless by design: active compiled
/// programs remain owned by the renderer, while this service performs short
/// project reads/writes and returns values to the caller.
pub struct MaterialGraphAssetService;

impl MaterialGraphAssetService {
    /// Resolve every material graph into a shader program. A material without a
    /// graph, a dangling graph ID, or a compile failure rejects the project.
    pub fn load_shader_set(
        project_path: &Path,
        project: &StudioProject,
        _material_table: &MaterialTable,
    ) -> Result<MaterialGraphShaderSet, AssetError> {
        let store = StudioProjectStore::new(project_path);
        let graphs = project.load_graph_assets(&store)?;
        let mut shaders = MaterialGraphShaderSet::default();
        let registry = NodeRegistry::builtin();
        for (slot_key, reference) in &project.manifest.material_assignments {
            let slot = slot_key.parse::<u8>().map_err(|_| {
                AssetError::InvalidMaterial(format!(
                    "material assignment key `{slot_key}` is not a u8"
                ))
            })?;
            let material = store.load_material(&reference.path)?;
            let graph_id = material.graph;
            let graph = graphs.get(&graph_id).ok_or_else(|| {
                AssetError::InvalidGraph(format!(
                    "material `{}` references missing graph `{graph_id}`",
                    material.id
                ))
            })?;
            let program = compile(graph, &registry).map_err(|error| {
                AssetError::InvalidGraph(format!("graph `{graph_id}` failed: {error}"))
            })?;
            shaders.insert(slot, program);
        }
        Ok(shaders)
    }

    /// Resolve material graphs independently for a live editing session.
    ///
    /// Strict validation deliberately remains all-or-nothing in
    /// [`Self::load_shader_set`].  The running editor, however, must keep every
    /// valid project material active when one graph is being repaired.  A failed
    /// slot falls back only to its loaded material row, while the valid slots
    /// retain their graph programs.  The graph also projects its representative
    /// surface values and pattern chain into the live table, which is the source
    /// used by the world bindings and CAGI.
    pub fn load_shader_set_for_editing(
        project_path: &Path,
        project: &StudioProject,
        material_table: &mut MaterialTable,
    ) -> (MaterialGraphShaderSet, Vec<String>) {
        let store = StudioProjectStore::new(project_path);
        let registry = NodeRegistry::builtin();
        let mut shaders = MaterialGraphShaderSet::default();
        let mut diagnostics = Vec::new();

        for (slot_key, material_reference) in &project.manifest.material_assignments {
            let result = (|| -> Result<(), AssetError> {
                let slot = slot_key.parse::<u8>().map_err(|_| {
                    AssetError::InvalidMaterial(format!(
                        "material assignment key `{slot_key}` is not a u8"
                    ))
                })?;
                let material = store.load_material(&material_reference.path)?;
                let graph_reference = project
                    .manifest
                    .graph_assets
                    .iter()
                    .find(|reference| reference.id == material.graph)
                    .ok_or_else(|| {
                        AssetError::InvalidGraph(format!(
                            "material `{}` references missing graph `{}`",
                            material.id, material.graph
                        ))
                    })?;
                let graph = store.load_graph(&graph_reference.path)?;
                if graph.id != graph_reference.id {
                    return Err(AssetError::InvalidGraph(format!(
                        "graph asset `{}` does not match its manifest identity",
                        graph_reference.path.display()
                    )));
                }
                let program = compile(&graph, &registry).map_err(|error| {
                    AssetError::InvalidGraph(format!("graph `{}` failed: {error}", graph.id))
                })?;
                let row = material_table.row_mut(slot).ok_or_else(|| {
                    AssetError::InvalidMaterial(format!("material slot {slot} is out of range"))
                })?;
                sync_pattern_layers_from_graph(&graph, row).map_err(|error| {
                    AssetError::InvalidGraph(format!(
                        "graph `{}` pattern chain failed: {error}",
                        graph.id
                    ))
                })?;
                let _ = material_table.apply_graph_sample(
                    slot,
                    &program,
                    MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0]),
                );
                shaders.insert(slot, program);
                Ok(())
            })();
            if let Err(error) = result {
                diagnostics.push(format!(
                    "Material slot {slot_key} is using its basic material fallback: {error}"
                ));
            }
        }

        (shaders, diagnostics)
    }

    /// Load a slot's required canonical graph.
    pub fn open(
        project_path: &Path,
        project: &StudioProject,
        material_table: &MaterialTable,
        slot: u8,
    ) -> Result<Option<OpenedMaterialGraph>, AssetError> {
        let Some(_row) = material_table.row(slot).copied() else {
            return Ok(None);
        };
        let store = StudioProjectStore::new(project_path);
        let reference = project
            .manifest
            .material_assignments
            .get(&slot.to_string())
            .cloned();
        let reference = reference.ok_or_else(|| {
            AssetError::InvalidMaterial(format!("material slot {slot} is not assigned"))
        })?;
        let material = store.load_material(&reference.path)?;
        let graph_id = material.graph;
        let graphs = project.load_graph_assets(&store)?;
        let graph = graphs.get(&graph_id).cloned().ok_or_else(|| {
            AssetError::InvalidGraph(format!(
                "material `{}` references missing graph `{graph_id}`",
                material.id
            ))
        })?;
        Ok(Some(OpenedMaterialGraph {
            graph,
            status: String::new(),
        }))
    }

    /// Persist one graph and attach it to the selected material asset.
    pub fn save(
        project_path: &Path,
        project: &mut StudioProject,
        slot: u8,
        graph: &GraphAsset,
    ) -> Result<(), AssetError> {
        let registry = NodeRegistry::builtin();
        compile(graph, &registry).map_err(|error| {
            AssetError::InvalidGraph(format!("graph `{}` failed: {error}", graph.id))
        })?;
        let store = StudioProjectStore::new(project_path);
        let path = PathBuf::from(format!("graphs/material-{slot:02}.vgraph.json"));
        project.save_graph_asset(&store, path, graph)?;
        let reference = project
            .manifest
            .material_assignments
            .get(&slot.to_string())
            .ok_or_else(|| {
                AssetError::InvalidMaterial(format!(
                    "material slot {slot} has no manifest assignment"
                ))
            })?;
        let mut material = store.load_material(&reference.path)?;
        material.graph = graph.id.clone();
        store.save_material(&reference.path, &material)?;
        store.save_manifest(&project.manifest)
    }
}

#[cfg(test)]
mod tests {
    use std::fs;

    use super::*;
    use crate::material::{material_id, MATERIALS};
    use crate::studio_assets::{AssetReference, MaterialAsset, ProjectManifest};
    use voxel_graph::AssetId;

    #[test]
    fn checked_in_project_compiles_every_assigned_material_graph() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../studio-project");
        let store = StudioProjectStore::new(&root);
        let project = StudioProject {
            manifest: store.load_manifest().unwrap(),
        };
        let shaders =
            MaterialGraphAssetService::load_shader_set(&root, &project, &MaterialTable::default())
                .unwrap();
        assert_eq!(shaders.len(), project.manifest.material_assignments.len());
    }

    /// The authored lava must actually MOVE, and the authored glow block must
    /// actually react. Compiling is not the same as animating: a drift socket
    /// left at zero, or a gain wired to the wrong layer, compiles perfectly and
    /// renders a still surface. This walks the real checked-in assets.
    #[test]
    fn the_authored_lava_drifts_and_the_authored_glow_block_reacts() {
        use crate::animation_clock::AnimationClock;
        use crate::material_graph::MaterialSampleContext;
        use crate::pattern::{LayerAnimationSample, PatternSample};
        use crate::world_event::{GpuWorldEvent, CHANNEL_PRESENCE};

        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../studio-project");
        let store = StudioProjectStore::new(&root);
        let project = StudioProject {
            manifest: store.load_manifest().unwrap(),
        };
        let shaders =
            MaterialGraphAssetService::load_shader_set(&root, &project, &MaterialTable::default())
                .unwrap();

        // ---- lava: the crust creeps ----------------------------------------
        let program = shaders.program(26).expect("lava has a compiled graph");
        // Evaluated, not pattern-matched: the drift may be authored as a bare
        // vector, a direction node or an oscillated one, and none of that is
        // this test's business — only that the surface ends up moving.
        let animation = program
            .evaluate_layer_animation(MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0]))
            .first()
            .copied()
            .expect("lava authors a pattern layer");
        let drift_velocity = animation.drift_velocity;
        assert_ne!(
            drift_velocity, [0.0; 3],
            "lava authors no drift, so its crust cannot creep"
        );
        // Direction is deliberately NOT pinned. Which way lava runs is a taste
        // decision that gets retuned per look — down a wall, level across a
        // lake — and a test that fails when someone dials an angle is a test
        // that trains people to edit tests. That it MOVES is the contract.

        // The layer as PROJECTED FROM THE GRAPH, not the compiled const table.
        // The graph is what the renderer uploads, and the two differ the moment
        // anyone re-authors — a test reading the const would keep passing while
        // the shipped material changed underneath it.
        let lava_graph = store
            .load_graph(std::path::Path::new("graphs/material-26.vgraph.json"))
            .expect("lava's graph loads");
        let layer = crate::material_graph_layers::project_pattern_stack(
            &lava_graph,
            &NodeRegistry::builtin(),
        )
        .expect("lava's pattern chain projects")
        .active()
        .next()
        .copied()
        .expect("lava authors a pattern layer");
        let sample = PatternSample {
            world_meters: [12.3, 4.5, 6.7],
            voxel: [98, 36, 53],
            axis: 1,
            axis_sign: -1.0,
            distance_meters: 3.0,
        };
        let at = |seconds: f32| {
            layer.generator_value_animated(
                &sample,
                LayerAnimationSample {
                    gain: 1.0,
                    drift_velocity,
                    time_seconds: seconds,
                },
            )
        };
        let still = at(0.0);
        assert!(
            (0..40).any(|step| (at(step as f32 * 0.5) - still).abs() > 1e-6),
            "lava's pattern never changed over 20 seconds of drift"
        );

        // ---- glow block: dim at rest, brighter with something near ----------
        let program = shaders
            .program(24)
            .expect("the glow block has a compiled graph");
        let mut clock = AnimationClock::new();
        clock.advance(4.0, 1.0);

        let resting = program
            .evaluate(MaterialSampleContext {
                clock: clock.sample(),
                ..MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0])
            })
            .emission;
        assert_eq!(
            [resting[0], resting[1], resting[2]],
            [0.0; 3],
            "the glow block is authored OFF at rest — the look Pascal asked for on \
             2026-08-02. It is only safe because of S3b: the volume stores the \
             TRIGGERED peak and scales down, so a row resting at zero is still an \
             emitter and still lights the room on approach. Before S3b this same \
             graph would have handed the volume a black emitter and lit nothing"
        );

        let nearby = [GpuWorldEvent {
            position_meters: [0.0, 0.0, 1.0],
            radius_meters: 12.0,
            started_epoch: 0.0,
            started_remainder_seconds: 0.0,
            ended_epoch: 0.0,
            ended_remainder_seconds: 0.0,
            channel: CHANNEL_PRESENCE,
            strength: 1.0,
            open: 1.0,
            _pad_row2: 0.0,
        }];
        let lit = program
            .evaluate(MaterialSampleContext {
                clock: clock.sample(),
                events: &nearby,
                ..MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0])
            })
            .emission;
        assert!(
            lit[0] > 0.0,
            "the glow block did not light up with something 1 m away: resting \
             {resting:?} vs lit {lit:?}. The likeliest cause is a channel with no \
             producer — the camera raises presence on CHANNEL_PRESENCE and nothing \
             else raises anything, so a sensor on any other channel is inert"
        );

        // ---- S3b: and the room reacts with it -------------------------------
        // The surface brightening is P2. What this asserts is P4: that the
        // brightening reaches the LIGHT VOLUME, so the floor in front of the
        // block lifts too instead of the block being a decal.
        let response = program
            .emission_event_response(MaterialSampleContext {
                clock: clock.sample(),
                ..MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0])
            })
            .expect("the glow block gates its emission on an event sensor");
        assert!(
            response.triggered[0] > response.resting[0],
            "the response's two ends do not bracket the surface's: {response:?}"
        );
        // The authored amount reaches the response INTACT, past 16.
        //
        // 16 is not an arbitrary number to test against: it is
        // `pattern::MAX_EMISSION_INTENSITY`, the hard clamp an emission pattern
        // layer applies in `target_value()`. The graph route has no such clamp —
        // `ColorScale` is a bare multiply — so a `multiply_scalar` in the chain
        // is how a material asks for more emission than a pattern layer can
        // express. The exact figure is deliberately not pinned; that it clears
        // the pattern ceiling is the contract.
        assert!(
            response.triggered[0] > crate::pattern::MAX_EMISSION_INTENSITY,
            "the glow block's authored amount did not survive to the response, or \
             it no longer demonstrates the uncapped graph route: {response:?}"
        );

        let mut table = MaterialTable::default();
        assert!(table.apply_graph_sample(
            24,
            program,
            MaterialSampleContext::still([0.0; 3], [0.0, 1.0, 0.0]),
        ));
        let attributes = table.cagi_attributes();
        let slot = (attributes.word(24) & crate::cagi::CELL_EVENT_RESPONSE_MASK)
            >> crate::cagi::CELL_EVENT_RESPONSE_SHIFT;
        assert_ne!(
            slot, 0,
            "the glow block reached the volume with no response slot, so its \
             cells would inject a constant emission and the room would not follow"
        );
        let row = attributes.responses()[slot as usize];
        assert!(
            row.resting_scale[0] < row.triggered_scale[0],
            "the volume's response is the wrong way round: {row:?}"
        );
        assert_eq!(attributes.event_response_overflow(), 0);
    }

    #[test]
    fn dangling_material_graph_is_a_project_error() {
        let root = std::env::temp_dir().join(format!("voxel-rt-strict-{}", AssetId::new()));
        let store = StudioProjectStore::new(&root);
        store.create_layout().unwrap();
        let slot = material_id(voxel_core::world::Voxel::Stone);
        let material = MaterialAsset::from_material(
            slot,
            &MATERIALS[slot as usize],
            AssetId("missing-graph".into()),
        );
        let path = PathBuf::from("materials/stone.vmat.json");
        store.save_material(&path, &material).unwrap();
        let mut manifest = ProjectManifest::new("strict");
        manifest.material_assignments.insert(
            slot.to_string(),
            AssetReference {
                id: material.id,
                path,
            },
        );
        store.save_manifest(&manifest).unwrap();
        let project = StudioProject { manifest };
        assert!(matches!(
            MaterialGraphAssetService::load_shader_set(&root, &project, &MaterialTable::default()),
            Err(AssetError::InvalidGraph(_))
        ));
        fs::remove_dir_all(root).unwrap();
    }

    #[test]
    fn invalid_graph_is_rejected_before_save_touches_the_project() {
        let root = std::env::temp_dir().join(format!("voxel-rt-invalid-save-{}", AssetId::new()));
        let mut project = StudioProject {
            manifest: ProjectManifest::new("test"),
        };
        let mut graph = crate::material_graph::new_material_graph("invalid");
        graph
            .links
            .retain(|_, link| link.from.socket.0 != "surface");

        assert!(matches!(
            MaterialGraphAssetService::save(&root, &mut project, 0, &graph),
            Err(AssetError::InvalidGraph(_))
        ));
        assert!(!root.exists());
    }
}
