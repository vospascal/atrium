//! Material Graph asset lifecycle.
//!
//! This module keeps project-file concerns out of the window/platform layer.
//! Rendering only consumes a [`MaterialGraphShaderSet`]; Graph Studio edits a
//! [`GraphAsset`]; this service owns the boundary between those two worlds:
//! resolving required references, compiling them, and committing a graph plus
//! its material reference. Invalid authored data is an error, never a fallback.

use std::path::{Path, PathBuf};

use crate::graph::{GraphAsset, NodeRegistry};
use crate::material_graph::{compile, MaterialGraphShaderSet};
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
    use crate::studio_assets::{AssetId, AssetReference, MaterialAsset, ProjectManifest};

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
