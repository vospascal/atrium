//! Shared project-backed runtime for the normal voxel world and the isolated
//! material Studio. Platform code supplies a mode; this module owns the common
//! project/material/graph loading and turns a registered world graph into a
//! brickmap.

use std::path::{Path, PathBuf};

use voxel_core::world::VoxelWorld;

use crate::brickmap::Brickmap;
use crate::environment::{RuntimeEnvironmentState, Season};
use crate::graph::lowering::GenerationProgram;
use crate::material_graph_assets::MaterialGraphAssetService;
use crate::material_table::MaterialTable;
use crate::studio::StudioScene;
use crate::studio_assets::{AssetError, StudioProject, StudioProjectStore};
use crate::variants::RenderQuality;
use voxel_graph::{GraphAsset, GraphKind};
use voxel_material_graph::lowering::MaterialGraphShaderSet;

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum RuntimeMode {
    WorldEdit,
    StudioEdit,
}

impl RuntimeMode {
    pub fn parse(value: &str) -> Option<Self> {
        match value {
            "world" => Some(Self::WorldEdit),
            "studio" => Some(Self::StudioEdit),
            _ => None,
        }
    }
}

#[derive(Clone, Debug)]
pub struct VoxelEngineConfig {
    pub mode: RuntimeMode,
    pub project_root: PathBuf,
    /// Replace the spawn with L0's rainbow corridor and stand the camera inside
    /// it. `None` is the normal island.
    ///
    /// A launch flag rather than a key binding, because entering the fixture also
    /// re-aims the sun and zeroes the ambient floor
    /// ([`crate::light_fixture::RainbowCorridor::sun`]) — that is a different
    /// lighting environment, not a place you walk to.
    pub light_fixture: Option<crate::light_fixture::NotchState>,
}

impl Default for VoxelEngineConfig {
    fn default() -> Self {
        Self {
            mode: RuntimeMode::WorldEdit,
            project_root: PathBuf::from("studio-project"),
            light_fixture: None,
        }
    }
}

impl VoxelEngineConfig {
    /// `--mode world|studio` is the canonical switch. `--studio` remains a
    /// deliberately small compatibility alias so existing launch commands work.
    pub fn from_args(arguments: &[String]) -> Result<Self, String> {
        let mut config = Self::default();
        let mut index = 1;
        while index < arguments.len() {
            match arguments[index].as_str() {
                "--studio" => config.mode = RuntimeMode::StudioEdit,
                "--mode" => {
                    index += 1;
                    let value = arguments.get(index).ok_or("--mode needs world or studio")?;
                    config.mode = RuntimeMode::parse(value).ok_or_else(|| {
                        format!("unknown runtime mode `{value}`; use world or studio")
                    })?;
                }
                "--project" => {
                    index += 1;
                    config.project_root =
                        PathBuf::from(arguments.get(index).ok_or("--project needs a path")?);
                }
                // `--light-fixture` alone is the sealed room, which is the one to
                // judge colour bleed in: exactly one light path, so anything on the
                // ceiling came off a wall. `open` cuts the far-end notch for the
                // corner-seal case.
                "--light-fixture" => {
                    let variant = arguments
                        .get(index + 1)
                        .filter(|value| !value.starts_with("--"));
                    config.light_fixture = Some(match variant.map(String::as_str) {
                        None | Some("sealed") => crate::light_fixture::NotchState::Sealed,
                        Some("open") => crate::light_fixture::NotchState::Open,
                        Some(other) => {
                            return Err(format!(
                                "unknown --light-fixture variant `{other}`; use sealed or open"
                            ))
                        }
                    });
                    if variant.is_some() {
                        index += 1;
                    }
                }
                _ => {}
            }
            index += 1;
        }
        Ok(config)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ProjectDiagnostic {
    pub message: String,
    pub fallback_active: bool,
}

pub struct ProjectRuntime {
    pub root: PathBuf,
    pub project: StudioProject,
    pub materials: MaterialTable,
    pub quality: RenderQuality,
    pub material_graphs: MaterialGraphShaderSet,
    pub diagnostics: Vec<ProjectDiagnostic>,
}

impl ProjectRuntime {
    /// Editing is resilient: broken project input never crashes a live authoring
    /// session. The renderer receives compiled defaults and diagnostics instead.
    pub(crate) fn load_for_editing(root: impl AsRef<Path>) -> Self {
        let root = root.as_ref().to_path_buf();
        let store = StudioProjectStore::new(&root);
        let mut materials = MaterialTable::default();
        let mut quality = RenderQuality::default();
        let mut diagnostics = Vec::new();
        let project = match StudioProject::load_live_state(&store, &mut materials, &mut quality) {
            Ok((project, warnings)) => {
                diagnostics.extend(warnings.into_iter().map(|message| ProjectDiagnostic {
                    message,
                    fallback_active: false,
                }));
                project
            }
            Err(error) => {
                diagnostics.push(ProjectDiagnostic {
                    message: format!("Project fallback: {error}"),
                    fallback_active: true,
                });
                StudioProject::new("Voxel Project")
            }
        };
        let (material_graphs, material_diagnostics) =
            MaterialGraphAssetService::load_shader_set_for_editing(&root, &project, &mut materials);
        diagnostics.extend(
            material_diagnostics
                .into_iter()
                .map(|message| ProjectDiagnostic {
                    message,
                    fallback_active: true,
                }),
        );
        if project.manifest.active_world_graph.is_some() {
            match project
                .load_active_world_graph(&store)
                .map_err(|error| error.to_string())
                .and_then(|graph| graph.ok_or_else(|| "active world graph disappeared".to_string()))
                .and_then(|graph| CompiledWorldProgram::compile(&graph))
            {
                Ok(_) => {}
                Err(error) => diagnostics.push(ProjectDiagnostic {
                    message: format!("World graph fallback: {error}"),
                    fallback_active: true,
                }),
            }
        }
        Self {
            root,
            project,
            materials,
            quality,
            material_graphs,
            diagnostics,
        }
    }

    pub fn validate_strict(root: impl AsRef<Path>) -> Result<(), Vec<ProjectDiagnostic>> {
        let runtime = Self::load_for_editing(root);
        let mut errors: Vec<_> = runtime
            .diagnostics
            .into_iter()
            .filter(|item| item.fallback_active)
            .collect();
        // Editing may use a per-slot fallback, but the command-line content
        // gate is intentionally uncompromising: every assigned material graph
        // and every registered graph asset must be sound.
        let store = StudioProjectStore::new(&runtime.root);
        if let Err(error) = MaterialGraphAssetService::load_shader_set(
            &runtime.root,
            &runtime.project,
            &runtime.materials,
        ) {
            errors.push(ProjectDiagnostic::from(error));
        }
        if let Err(error) = runtime.project.load_graph_assets(&store) {
            errors.push(ProjectDiagnostic::from(error));
        }
        if errors.is_empty() {
            Ok(())
        } else {
            Err(errors)
        }
    }
}

/// What one world build produced: the totals, and what each stage did.
///
/// One value rather than two returned side by side, because every consumer wants
/// both and threading them separately is how a readout ends up showing totals from
/// one build next to stages from another.
#[derive(Clone, Debug)]
pub struct GenerationOutcome {
    pub stats: voxel_core::chunk::GenerationStats,
    pub report: voxel_core::stage_report::StageReport,
}

/// What the active world graph compiled to.
///
/// One `Generated` variant carrying a whole [`GenerationProgram`], rather than the
/// previous `Generated`/`GeneratedWithCaves` pair. The pair forced every consumer
/// to match on whether caves existed, and `apply_biome_graph` used that to
/// silently promote a cave-less world into a cave-bearing one — a graph edit in a
/// different tab changing what the world graph said. Caves on or off is now a flag
/// inside the program, where the graph put it.
#[derive(Clone, Debug)]
pub enum CompiledWorldProgram {
    Generated(GenerationProgram),
    StudioPreview,
}

impl CompiledWorldProgram {
    pub fn compile(graph: &GraphAsset) -> Result<Self, String> {
        if graph.kind != GraphKind::World {
            return Err("active world graph is not a World graph".to_string());
        }
        let resolved = graph.resolve(&crate::graph::CATALOGUE);
        if let Some(error) = resolved
            .diagnostics
            .iter()
            .find(|item| item.severity == voxel_graph::DiagnosticSeverity::Error)
        {
            return Err(error.message.clone());
        }
        let node_types = graph
            .nodes
            .values()
            .map(|node| node.node_type.0.as_str())
            .collect::<Vec<_>>();
        if node_types.contains(&"world.studio_preview") {
            Ok(Self::StudioPreview)
        } else if node_types.contains(&"world.generated_terrain") {
            Ok(Self::Generated(crate::graph::lowering::generation_program(
                graph,
            )))
        } else {
            Err("world graph has no registered world source".to_string())
        }
    }

    /// The generation program, for a graph-backed world.
    pub fn generation_program(&self) -> Option<&GenerationProgram> {
        match self {
            Self::Generated(program) => Some(program),
            Self::StudioPreview => None,
        }
    }

    pub fn build(&self, studio_scene: &StudioScene, world_seed: u64) -> Brickmap {
        self.build_reporting(studio_scene, world_seed).0
    }

    /// Build the world and report what generation produced.
    ///
    /// The stats are measured during generation, so the generation window shows
    /// the world on screen rather than a second pass that could disagree with it.
    /// `None` for the studio preview, which generates no world at all.
    pub fn build_reporting(
        &self,
        studio_scene: &StudioScene,
        world_seed: u64,
    ) -> (Brickmap, Option<GenerationOutcome>) {
        match self {
            Self::Generated(program) => {
                let (world, stats, report) = VoxelWorld::from_chunk_generator_reporting(
                    &program.chunk_generator(world_seed),
                );
                (
                    Brickmap::build(&world),
                    Some(GenerationOutcome { stats, report }),
                )
            }
            Self::StudioPreview => (studio_scene.build(), None),
        }
    }

    /// Apply the authored biome cave recipe to the executable cave pass.
    ///
    /// One call into the shared lowering, which the Studio previews use too — the
    /// preview and the built world cannot disagree about what a biome graph means
    /// because there is only one mapping left.
    pub fn apply_biome_graph(&mut self, graph: &GraphAsset) -> Result<(), String> {
        if graph.kind != GraphKind::Biome {
            return Err("biome update received a non-biome graph".to_string());
        }
        let resolved = graph.resolve(&crate::graph::CATALOGUE);
        if let Some(error) = resolved
            .diagnostics
            .iter()
            .find(|item| item.severity == voxel_graph::DiagnosticSeverity::Error)
        {
            return Err(error.message.clone());
        }
        match self {
            Self::Generated(program) => crate::graph::lowering::apply_biome_graph(program, graph),
            Self::StudioPreview => Ok(()),
        }
    }
}

pub struct VoxelEngineRuntime {
    pub config: VoxelEngineConfig,
    pub project: ProjectRuntime,
    pub environment: RuntimeEnvironmentState,
    pub program: CompiledWorldProgram,
}

impl VoxelEngineRuntime {
    pub fn load(config: VoxelEngineConfig) -> Self {
        let project = ProjectRuntime::load_for_editing(&config.project_root);
        let program = match config.mode {
            RuntimeMode::StudioEdit => CompiledWorldProgram::StudioPreview,
            RuntimeMode::WorldEdit => {
                // A project with no readable world graph still gets a world: the
                // default generation program is the registered fallback.
                Self::load_world_program(&project).unwrap_or_else(|_| {
                    CompiledWorldProgram::Generated(GenerationProgram::default())
                })
            }
        };
        Self {
            config,
            project,
            environment: RuntimeEnvironmentState {
                season: Season::Summer,
                ..RuntimeEnvironmentState::default()
            },
            program,
        }
    }

    fn load_world_program(project: &ProjectRuntime) -> Result<CompiledWorldProgram, String> {
        // Schema-v3 projects record an active world graph. Until a new project is
        // saved, the registered generated-world fallback remains safe and visible.
        let store = StudioProjectStore::new(&project.root);
        let Some(graph) = project
            .project
            .load_active_world_graph(&store)
            .map_err(|error| error.to_string())?
        else {
            return Ok(CompiledWorldProgram::Generated(GenerationProgram::default()));
        };
        let mut program = CompiledWorldProgram::compile(&graph)?;
        // A registered biome graph is authored data like any other, so it applies
        // at load. Before this, the biome graph was applied only by the Biomes
        // tab's Update World button and lived purely in memory: the caves you
        // tuned and saved were silently replaced by the world graph's own recipe
        // on the next launch.
        if let Some(biome_graph) = project
            .project
            .load_active_biome_graph(&store)
            .map_err(|error| error.to_string())?
        {
            program.apply_biome_graph(&biome_graph)?;
        }
        Ok(program)
    }

    pub fn build_world(&self, studio_scene: &StudioScene) -> Brickmap {
        self.build_world_reporting(studio_scene).0
    }

    /// Build the world and report what generation produced, for the debug window.
    pub fn build_world_reporting(
        &self,
        studio_scene: &StudioScene,
    ) -> (Brickmap, Option<GenerationOutcome>) {
        self.program
            .build_reporting(studio_scene, self.project.project.manifest.world_seed)
    }
}

impl From<AssetError> for ProjectDiagnostic {
    fn from(error: AssetError) -> Self {
        Self {
            message: error.to_string(),
            fallback_active: true,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn modes_and_project_path_are_independent() {
        let args = vec![
            "voxel-rt".into(),
            "--mode".into(),
            "studio".into(),
            "--project".into(),
            "my-project".into(),
        ];
        let config = VoxelEngineConfig::from_args(&args).unwrap();
        assert_eq!(config.mode, RuntimeMode::StudioEdit);
        assert_eq!(config.project_root, PathBuf::from("my-project"));
    }

    #[test]
    fn legacy_studio_flag_is_an_alias() {
        let args = vec!["voxel-rt".into(), "--studio".into()];
        assert_eq!(
            VoxelEngineConfig::from_args(&args).unwrap().mode,
            RuntimeMode::StudioEdit
        );
    }

    fn parse(arguments: &[&str]) -> Result<VoxelEngineConfig, String> {
        let owned: Vec<String> = std::iter::once("voxel-rt".to_string())
            .chain(arguments.iter().map(|value| value.to_string()))
            .collect();
        VoxelEngineConfig::from_args(&owned)
    }

    /// The bare flag has to mean the SEALED room. That is the configuration with
    /// exactly one light path, so it is the one where anything on the ceiling
    /// must have come off a wall — the default should be the readable case.
    #[test]
    fn light_fixture_defaults_to_the_sealed_room() {
        assert_eq!(
            parse(&["--light-fixture"]).unwrap().light_fixture,
            Some(crate::light_fixture::NotchState::Sealed)
        );
        assert_eq!(
            parse(&["--light-fixture", "sealed"]).unwrap().light_fixture,
            Some(crate::light_fixture::NotchState::Sealed)
        );
        assert_eq!(
            parse(&["--light-fixture", "open"]).unwrap().light_fixture,
            Some(crate::light_fixture::NotchState::Open)
        );
        assert_eq!(parse(&[]).unwrap().light_fixture, None);
    }

    /// The bare flag must not swallow a following flag as its variant, or
    /// `--light-fixture --studio` would silently lose the studio.
    #[test]
    fn light_fixture_does_not_consume_a_following_flag() {
        let config = parse(&["--light-fixture", "--studio"]).unwrap();
        assert_eq!(
            config.light_fixture,
            Some(crate::light_fixture::NotchState::Sealed)
        );
        assert_eq!(config.mode, RuntimeMode::StudioEdit);
    }

    #[test]
    fn an_unknown_light_fixture_variant_is_an_error() {
        let error = parse(&["--light-fixture", "rainbow"]).unwrap_err();
        assert!(error.contains("rainbow"), "unhelpful message: {error}");
    }

    #[test]
    fn checked_in_project_has_a_registered_executable_world_graph() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../studio-project");
        let config = VoxelEngineConfig {
            mode: RuntimeMode::WorldEdit,
            project_root: root,
            light_fixture: None,
        };
        let runtime = VoxelEngineRuntime::load(config);
        let program = runtime
            .program
            .generation_program()
            .expect("the checked-in graph is a generated world");
        assert!(program.caves_enabled, "the checked-in graph carves caves");
        assert_eq!(
            program.cave_program.shape,
            voxel_core::cave::CaveShape::CellularAutomata
        );
        assert!(runtime
            .project
            .diagnostics
            .iter()
            .all(|item| !item.fallback_active));
    }

    #[test]
    fn checked_in_biome_graph_lowers_cellular_controls_into_the_world_program() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../studio-project");
        let json = std::fs::read_to_string(root.join("graphs/biomes.vgraph.json")).unwrap();
        let graph: GraphAsset = serde_json::from_str(&json).unwrap();
        let mut program =
            CompiledWorldProgram::Generated(crate::graph::lowering::GenerationProgram::default());
        program.apply_biome_graph(&graph).unwrap();
        let lowered = program
            .generation_program()
            .expect("applying a biome graph keeps a generated world");
        let cave_program = &lowered.cave_program;
        assert!(lowered.caves_enabled);
        assert_eq!(
            cave_program.shape,
            voxel_core::cave::CaveShape::CellularAutomata
        );
        assert_eq!(cave_program.cellular_iterations, 8);
        assert_eq!(cave_program.cellular_cleanup_iterations, 2);
        assert_eq!(cave_program.cellular_cell_size, 2);
        assert!((cave_program.cellular_fill - 0.40).abs() < f32::EPSILON);
        assert!((cave_program.cellular_worley_scale - 0.05).abs() < f32::EPSILON);
        assert!((cave_program.cellular_billow_scale - 0.08).abs() < f32::EPSILON);
        assert!((cave_program.cellular_billow_strength - 0.40).abs() < f32::EPSILON);
        assert_eq!(cave_program.cellular_billow_octaves, 3);
    }

    /// The registered biome graph must apply at LOAD, not only when the Biomes
    /// tab's button is pressed. Before it was registered, a tuned and saved cave
    /// recipe silently stopped affecting the world after a restart: startup
    /// compiled the world graph alone (4 iterations) while the biome graph the
    /// author had been working against says 8.
    #[test]
    fn the_registered_biome_graph_is_applied_at_startup() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../studio-project");
        let runtime = VoxelEngineRuntime::load(VoxelEngineConfig {
            mode: RuntimeMode::WorldEdit,
            project_root: root.clone(),
            light_fixture: None,
        });
        let program = runtime
            .program
            .generation_program()
            .expect("the checked-in project is a generated world");
        let world_graph_iterations = 4;
        assert_ne!(
            program.cave_program.cellular_iterations, world_graph_iterations,
            "the biome graph was not applied at load"
        );
        assert_eq!(program.cave_program.cellular_iterations, 8);
        // ...and it must not have relocated the cave band the world graph owns.
        let world_json = std::fs::read_to_string(root.join("graphs/world.vgraph.json")).unwrap();
        let world_graph: GraphAsset = serde_json::from_str(&world_json).unwrap();
        let authored = crate::graph::lowering::generation_program(&world_graph);
        assert_eq!(program.cave_program.min_y, authored.cave_program.min_y);
        assert_eq!(program.cave_program.max_y, authored.cave_program.max_y);
        assert_eq!(program.cave_fluids, authored.cave_fluids);
    }

    /// Pass two must be authored by the checked-in graph, not merely defaulted on.
    #[test]
    fn the_checked_in_graph_authors_generation_pass_two() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../studio-project");
        let json = std::fs::read_to_string(root.join("graphs/world.vgraph.json")).unwrap();
        let graph: GraphAsset = serde_json::from_str(&json).unwrap();
        assert!(
            graph
                .nodes
                .values()
                .any(|node| node.node_type.0 == "world.scatter"),
            "the world graph has no scatter node, so decoration is not authored"
        );
        let program = crate::graph::lowering::generation_program(&graph);
        assert!(program.scatter.enabled);
        assert!(program.cave_fluids.enabled);
        assert!(
            program.terrain.base_height - program.terrain.relief / 2 < program.terrain.sea_level,
            "the authored terrain never reaches below sea level, so it has no coastline"
        );
    }

    #[test]
    fn checked_in_world_uses_the_same_graph_backed_stone_material_as_studio() {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../studio-project");
        let runtime = VoxelEngineRuntime::load(VoxelEngineConfig {
            mode: RuntimeMode::WorldEdit,
            project_root: root,
            light_fixture: None,
        });
        let stone = voxel_material::material::material_id(voxel_core::world::Voxel::Stone);
        assert_eq!(
            runtime.project.material_graphs.len(),
            runtime.project.project.manifest.material_assignments.len()
        );
        assert!(
            runtime
                .project
                .materials
                .row(stone)
                .unwrap()
                .patterns
                .active_count()
                > 0
        );
    }
}
