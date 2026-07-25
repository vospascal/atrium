//! Voxel plateau sandbox — prototype for the Atrium visual layer.
//!
//! A procedural floating-island diorama: rolling voxel terrain with a
//! lush→desert biome gradient, a carved river, slab-canopy trees, and a
//! sculpted rock underside, hovering above a volumetric fog sea.
//!
//! Run:    `cargo run -p voxel-sandbox`
//! Keys:   `Tab` switch orbit ↔ first-person view · `R` new island
//!         `F` release a firefly swarm · `Shift+F` clear swarms
//!         `C` place a campfire · `Shift+C` clear campfires
//!         hold `N` to fast-forward the day/night cycle
//!         `V` tuning panels (view + time & weather)
//!         `P` performance overlay (or start with `VOXEL_PERF=1`)
//! Orbit:  left-drag orbit · right-drag pan · scroll zoom · WASD pan
//! Walk:   left-drag look around · WASD walk · Shift run · Space jump
//!         (on land) / dive (in water; release to float up)
//! Water:  hold `G` to pour water where you look · `H` to scoop it away
//!         (the fluid sim flows/pools/spills what you add)
//!
//! `VOXEL_TIME=0.0..1.0` sets the starting time of day (0 = midnight,
//! 0.25 = sunrise, 0.5 = noon, 0.75 = sunset); `VOXEL_MOON=0.0..1.0` the
//! moon phase (0.5 = full). Weather is scriptable too: `VOXEL_CLOUDS`,
//! `VOXEL_CLOUD_TYPE` (0 stratus · 1 cumulus · 2 cirrus), `VOXEL_WIND`,
//! `VOXEL_WIND_DIRECTION`, `VOXEL_FOG`, `VOXEL_PRECIP=rain|snow`,
//! `VOXEL_PRECIP_INTENSITY`. `VOXEL_SEASON=0.0..1.0` picks the foliage
//! season (0 = high summer, 1 = deep autumn; also a panel slider).
//!
//! Set `VOXEL_SCREENSHOT_PATH=/some/dir/shot.png` to capture one frame and
//! exit (automated visual verification); add `VOXEL_START_FIRST_PERSON=1`
//! to capture from the walking view. `VOXEL_FORCE_UNDERWATER=1` forces the
//! submerged look (tint + fog) from any camera, to preview/tune it.

mod campfire;
mod day_night;
mod fireflies;
mod flame;
mod fluid;
mod fog_ring;
mod grass;
mod mesh;
mod sky;
mod tweak_panel;
mod vox_import;
mod voxel_material;
mod water;
mod weather;

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::light::{CascadeShadowConfigBuilder, GlobalAmbientLight, NotShadowCaster};
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::post_process::dof::{DepthOfField, DepthOfFieldMode};
use bevy::prelude::*;
use bevy::render::storage::ShaderStorageBuffer;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};
use bevy::render::view::ColorGrading;

use std::path::Path;

use crate::flame::{FlameLight, FlameMaterial};
use crate::tweak_panel::ViewTweaks;
use voxel_core::world::{Voxel, VoxelWorld, VOXEL_SIZE, WATER_LEVEL, WORLD_SIZE_X, WORLD_SIZE_Z};

/// Pale haze that washes out the distance, like the reference shots.
const HAZE_COLOR: Color = Color::srgb(0.80, 0.82, 0.79);

/// Where terrain comes from: the built-in generator, or a Blender export
/// (`cargo run -p voxel-sandbox -- path/to/name.terrain.json`).
#[derive(Resource)]
enum WorldSource {
    Procedural,
    Imported(voxel_core::terrain_import::ImportedTerrain),
}

fn main() {
    let start_first_person = std::env::var("VOXEL_START_FIRST_PERSON").is_ok();
    let world_source = match std::env::args().nth(1) {
        Some(terrain_path) => {
            match voxel_core::terrain_import::load_terrain(Path::new(&terrain_path)) {
                Ok(terrain) => {
                    println!("loaded Blender terrain from {terrain_path}");
                    WorldSource::Imported(terrain)
                }
                Err(error) => {
                    eprintln!("failed to load terrain: {error}");
                    std::process::exit(1);
                }
            }
        }
        None => WorldSource::Procedural,
    };

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Atrium — Voxel Plateau Sandbox".to_string(),
                // VSync locks the frame rate to the monitor's refresh (a
                // 60 Hz display reads as "60 fps" no matter how fast the
                // engine is). `VOXEL_NO_VSYNC=1` uncaps it so the `P`
                // overlay shows real headroom when benchmarking.
                present_mode: if std::env::var("VOXEL_NO_VSYNC").is_ok() {
                    bevy::window::PresentMode::AutoNoVsync
                } else {
                    bevy::window::PresentMode::AutoVsync
                },
                ..default()
            }),
            ..default()
        }))
        .insert_resource(ClearColor(HAZE_COLOR))
        .insert_resource(GlobalAmbientLight {
            color: Color::srgb(0.85, 0.88, 0.90),
            brightness: 650.0,
            ..default()
        })
        .insert_resource(WorldSeed(1))
        .insert_resource(Season(
            std::env::var("VOXEL_SEASON")
                .ok()
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(0.0)
                .clamp(0.0, 1.0),
        ))
        .init_resource::<RegenerateRequest>()
        .insert_resource(world_source)
        .insert_resource(if start_first_person {
            ViewMode::FirstPerson
        } else {
            ViewMode::Orbit
        })
        .init_resource::<OrbitCameraState>()
        .init_resource::<FirstPersonState>()
        .init_resource::<day_night::DayNightCycle>()
        .init_resource::<day_night::CelestialState>()
        .init_resource::<weather::WeatherState>()
        .init_resource::<fireflies::FireflySettings>()
        .init_resource::<water::ReflectionTarget>()
        .init_resource::<water::ReflectionSettings>()
        .init_resource::<ViewTweaks>()
        .init_resource::<tweak_panel::UnderwaterTint>()
        .init_resource::<water::SurfaceTuning>()
        .init_resource::<Submerged>()
        .init_resource::<RenderQuality>()
        .insert_resource(tweak_panel::PerfOverlay {
            visible: std::env::var("VOXEL_PERF").is_ok(),
        })
        .add_plugins(bevy::diagnostic::FrameTimeDiagnosticsPlugin::default())
        .add_plugins(bevy::diagnostic::EntityCountDiagnosticsPlugin::default())
        .add_plugins(MaterialPlugin::<voxel_material::VoxelTerrainMaterial>::default())
        .add_plugins(MaterialPlugin::<voxel_material::GrassMaterial>::default())
        .add_plugins(MaterialPlugin::<FlameMaterial>::default())
        .add_plugins(MaterialPlugin::<sky::SkyMaterial>::default())
        .add_plugins(MaterialPlugin::<weather::PrecipitationMaterial>::default())
        .add_plugins(MaterialPlugin::<fog_ring::FogSeaMaterial>::default())
        .add_plugins(MaterialPlugin::<water::WaterMaterial>::default())
        .add_plugins(MaterialPlugin::<fireflies::FireflyMaterial>::default())
        .add_plugins(MaterialPlugin::<campfire::FireMaterial>::default())
        // With two cameras (main + water reflection), bevy_egui must not
        // guess which one hosts the UI — it picked the reflection camera
        // and drew the panels INTO the water. The main camera carries
        // `PrimaryEguiContext` explicitly instead.
        .insert_resource(bevy_egui::EguiGlobalSettings {
            auto_create_primary_context: false,
            ..default()
        })
        .add_plugins(bevy_egui::EguiPlugin::default())
        .add_systems(
            bevy_egui::EguiPrimaryContextPass,
            (tweak_panel::view_tweak_panel, tweak_panel::perf_overlay).chain(),
        )
        .add_systems(Update, tweak_panel::toggle_panel)
        .add_systems(
            Startup,
            (
                setup_scene,
                initial_world_system,
                sky::spawn_sky,
                weather::spawn_precipitation,
                fog_ring::spawn_fog_ring,
                water::spawn_reflection_camera,
                fireflies::setup_fireflies,
                campfire::setup_campfires,
                spawn_underwater_overlay,
            ),
        )
        .add_systems(
            Update,
            (
                toggle_view_mode,
                camera_system,
                weather::ease_weather,
                day_night::advance_day_night,
                // After day_night sets the sky-driven fog, so it wins when submerged.
                underwater_fog,
                sky::update_sky,
                fog_ring::update_fog_ring,
                weather::update_precipitation,
                water::update_reflection_camera,
                water::update_water,
                fireflies::firefly_controls,
                fireflies::spawn_env_fireflies,
                fireflies::update_fireflies,
                campfire::campfire_controls,
                campfire::spawn_env_campfires,
                regenerate_system,
                flame::flicker_flame_lights,
                // Nested to stay under Bevy's 20-system tuple limit; order
                // among these three is irrelevant.
                (
                    grass::update_grass_wind_time,
                    fluid::water_interaction,
                    fluid::step_fluid_water,
                    fluid::update_water_heights,
                    screenshot_system,
                ),
            )
                .chain(),
        )
        .run();
}

#[derive(Resource)]
struct WorldSeed(u32);

/// Stats from the last world build, shown in the `P` performance overlay.
#[derive(Resource)]
struct WorldStats {
    generation: std::time::Duration,
    meshing: std::time::Duration,
    chunk_count: usize,
    total_vertices: usize,
    rle_runs: usize,
    rle_bytes: usize,
}

/// Foliage season, 0.0 (high summer) to 1.0 (deep autumn). Baked into the
/// vertex colors, so changing it rebuilds the island.
#[derive(Resource)]
struct Season(f32);

/// Set by the panel (season slider) or the `R` key to rebuild the world on
/// the next frame.
#[derive(Resource, Default)]
struct RegenerateRequest {
    requested: bool,
    /// `R` rolls a fresh island; the season slider keeps the current one.
    bump_seed: bool,
}

/// Marker for everything despawned and rebuilt on regeneration.
#[derive(Component)]
pub(crate) struct WorldMesh;

#[derive(Resource, Clone, Copy, PartialEq)]
enum ViewMode {
    /// Tilt-shift diorama overview.
    Orbit,
    /// Walking on the plateau at eye height.
    FirstPerson,
}

/// Scene bloom. Kept at Bevy's default strength — a shared helper so the
/// tweak-panel toggle re-inserts exactly this, not a divergent value.
pub(crate) fn scene_bloom() -> bevy::post_process::bloom::Bloom {
    bevy::post_process::bloom::Bloom::default()
}

fn setup_scene(mut commands: Commands, camera_state: Res<OrbitCameraState>) {
    commands.spawn((
        Camera3d::default(),
        // 2× MSAA: 4× roughly doubles the frametime on this fill-bound GPU for
        // little visual gain, and 2× is the sweet spot (Bevy defaults to 4×).
        // SSAO (mutually exclusive with MSAA) is opt-in via the P-overlay toggle.
        Msaa::Sample2,
        bevy::render::view::Hdr,
        // The UI belongs to this camera (see EguiGlobalSettings above). With two
        // cameras (main + reflection), bevy_ui also needs to be told which one
        // renders UI nodes (e.g. the underwater tint overlay).
        bevy_egui::PrimaryEguiContext,
        bevy::ui::IsDefaultUiCamera,
        // The volumetric fog sea reads scene depth to march up to terrain.
        bevy::core_pipeline::prepass::DepthPrepass,
        // Long lens + far camera: compressed perspective like tilt-shift
        // miniature photography (the wide-angle default kept everything
        // in focus and made the diorama look like a fisheye game map).
        Projection::Perspective(PerspectiveProjection {
            fov: 22_f32.to_radians(),
            ..default()
        }),
        camera_state.transform(),
        // Tilt-shift depth of field: focus follows the orbit target, so the
        // fore/background melt away and the diorama reads as a miniature.
        DepthOfField {
            mode: DepthOfFieldMode::Bokeh,
            focal_distance: camera_state.distance,
            // Physically absurd, deliberately: at diorama scale the focus
            // band must be a few meters wide to sell the miniature look.
            aperture_f_stops: 0.06,
            max_depth: 300.0,
            ..default()
        },
        DistanceFog {
            color: HAZE_COLOR,
            falloff: FogFalloff::Linear {
                start: 45.0,
                end: 190.0,
            },
            ..default()
        },
        // HDR flame voxels bloom into a glow.
        scene_bloom(),
        // Neutral filmic grade — identity by default (exposure 0, shadow lift
        // 0, mid-contrast 1, saturation 1). The `V` panel exposes live sliders
        // so the look is dialed by eye, not guessed.
        ColorGrading::default(),
    ));

    // Sun by day, moon by night; the day/night system drives rotation,
    // color, and intensity every frame.
    commands.spawn((
        DirectionalLight {
            illuminance: 10_000.0,
            color: Color::srgb(1.0, 0.96, 0.87),
            shadows_enabled: true,
            ..default()
        },
        Transform::from_rotation(Quat::from_euler(EulerRot::YXZ, 0.8, -0.75, 0.0)),
        CascadeShadowConfigBuilder {
            maximum_distance: 280.0,
            first_cascade_far_bound: 12.0,
            ..default()
        }
        .build(),
        // The sun must also light the reflection camera's view.
        water::reflective_layers(),
        day_night::SunLight,
    ));
}

#[allow(clippy::too_many_arguments)]
fn initial_world_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut terrain_materials: ResMut<Assets<voxel_material::VoxelTerrainMaterial>>,
    mut grass_materials: ResMut<Assets<voxel_material::GrassMaterial>>,
    mut storage_buffers: ResMut<Assets<ShaderStorageBuffer>>,
    mut flame_materials: ResMut<Assets<FlameMaterial>>,
    mut water_materials: ResMut<Assets<water::WaterMaterial>>,
    reflection_target: Res<water::ReflectionTarget>,
    seed: Res<WorldSeed>,
    season: Res<Season>,
    source: Res<WorldSource>,
) {
    spawn_world(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut terrain_materials,
        &mut grass_materials,
        &mut storage_buffers,
        &mut flame_materials,
        &mut water_materials,
        &reflection_target,
        &source,
        seed.0,
        season.0,
    );
}

#[allow(clippy::too_many_arguments)]
fn spawn_world(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    terrain_materials: &mut Assets<voxel_material::VoxelTerrainMaterial>,
    grass_materials: &mut Assets<voxel_material::GrassMaterial>,
    storage_buffers: &mut Assets<ShaderStorageBuffer>,
    flame_materials: &mut Assets<FlameMaterial>,
    water_materials: &mut Assets<water::WaterMaterial>,
    reflection_target: &water::ReflectionTarget,
    source: &WorldSource,
    seed: u32,
    season: f32,
) {
    let generation_start = std::time::Instant::now();
    let voxel_world = match source {
        WorldSource::Procedural => VoxelWorld::generate(seed, season),
        WorldSource::Imported(terrain) => VoxelWorld::from_imported(terrain, seed, season),
    };
    let generation_elapsed = generation_start.elapsed();
    let (run_count, rle_bytes) = voxel_world.memory_stats();
    info!(
        "world RLE: {run_count} runs, {:.1} MB resident (dense grid was 256 MB)",
        rle_bytes as f32 / 1e6
    );

    let meshing_start = std::time::Instant::now();
    let chunk_meshes = mesh::build_all_chunk_meshes(&voxel_world, seed, season);
    let count_bucket = |select: fn(&mesh::ChunkMeshes) -> &Option<Mesh>| {
        chunk_meshes
            .iter()
            .map(|chunk| select(chunk).as_ref().map_or(0, Mesh::count_vertices))
            .sum::<usize>()
    };
    let meshing_elapsed = meshing_start.elapsed();
    let total_vertices = count_bucket(|chunk| &chunk.terrain_above_water)
        + count_bucket(|chunk| &chunk.meadow_cover)
        + count_bucket(|chunk| &chunk.terrain_below_water)
        + count_bucket(|chunk| &chunk.canopy)
        + count_bucket(|chunk| &chunk.canopy_solid)
        + count_bucket(|chunk| &chunk.water);
    info!(
        "world generated in {generation_elapsed:.2?}, meshed in {meshing_elapsed:.2?} \
         ({} chunks: terrain {} + meadow {} + underwater {} + canopy {} verts, water {} verts)",
        chunk_meshes.len(),
        count_bucket(|chunk| &chunk.terrain_above_water),
        count_bucket(|chunk| &chunk.meadow_cover),
        count_bucket(|chunk| &chunk.terrain_below_water),
        count_bucket(|chunk| &chunk.canopy),
        count_bucket(|chunk| &chunk.water),
    );
    commands.insert_resource(WorldStats {
        generation: generation_elapsed,
        meshing: meshing_elapsed,
        chunk_count: chunk_meshes.len(),
        total_vertices,
        rle_runs: run_count,
        rle_bytes,
    });

    let ground_heights = GroundHeights::from_world(&voxel_world);
    // Trunk colliders for first-person tree collision.
    commands.insert_resource(TreeColliders::from_world(&voxel_world));
    // Live water simulation (F2), bounded to the wet region. When present it
    // drives a dynamic surface mesh that *replaces* the static chunk water
    // (spawned below), so the water flows and settles. Built here but inserted
    // as a resource after the mesh spawn, so the wet region is known while
    // deciding whether to emit the static water.
    let fluid_water = fluid::FluidWater::from_world(&voxel_world);
    let has_fluid = fluid_water.is_some();
    if !has_fluid {
        commands.remove_resource::<fluid::FluidWater>();
        commands.remove_resource::<fluid::WaterHeightBuffer>();
    }
    // The reflection system scales mirror quality by camera-to-water
    // distance; give it a coarse copy of the water-distance field.
    commands.insert_resource(water::WaterProximity::from_world(&voxel_world));

    // Log the shoreline nearest the island center and the widest open
    // water, so screenshot runs can aim at them (`VOXEL_POSITION`/`VOXEL_LOOK`).
    const BUCKET_VOXELS: usize = 50;
    let buckets_x = WORLD_SIZE_X / BUCKET_VOXELS;
    let buckets_z = WORLD_SIZE_Z / BUCKET_VOXELS;
    let mut bucket_water_counts = vec![0_u32; buckets_x * buckets_z];
    let mut nearest_water: Option<(f32, f32, f32)> = None;
    for z in 0..WORLD_SIZE_Z as i32 {
        for x in 0..WORLD_SIZE_X as i32 {
            if voxel_world.get(x, WATER_LEVEL, z) != Voxel::Water {
                continue;
            }
            bucket_water_counts
                [(z as usize / BUCKET_VOXELS) * buckets_x + (x as usize / BUCKET_VOXELS)] += 1;
            let render_x = (x as f32 - WORLD_SIZE_X as f32 / 2.0) * VOXEL_SIZE;
            let render_z = (z as f32 - WORLD_SIZE_Z as f32 / 2.0) * VOXEL_SIZE;
            let distance_squared = render_x * render_x + render_z * render_z;
            if nearest_water.is_none_or(|(_, _, best)| distance_squared < best) {
                nearest_water = Some((render_x, render_z, distance_squared));
            }
        }
    }
    if let Some((render_x, render_z, _)) = nearest_water {
        info!("nearest water to center: ({render_x:.1}, {render_z:.1})");
    }
    if let Some(densest_bucket) =
        (0..bucket_water_counts.len()).max_by_key(|&bucket_index| bucket_water_counts[bucket_index])
    {
        let bucket_center_x =
            ((densest_bucket % buckets_x) * BUCKET_VOXELS + BUCKET_VOXELS / 2) as f32;
        let bucket_center_z =
            ((densest_bucket / buckets_x) * BUCKET_VOXELS + BUCKET_VOXELS / 2) as f32;
        info!(
            "widest open water around: ({:.1}, {:.1})",
            (bucket_center_x - WORLD_SIZE_X as f32 / 2.0) * VOXEL_SIZE,
            (bucket_center_z - WORLD_SIZE_Z as f32 / 2.0) * VOXEL_SIZE,
        );
    }

    // Waterfalls are no longer faked ribbons — they emerge from the fluid sim
    // (rim lips spill, curtains hang where water actually pours over; see
    // `fluid::build_surface_mesh`).

    // Terrain uses the voxel jitter material (StandardMaterial PBR + the
    // per-fragment brightness speckle). Props keep a plain StandardMaterial —
    // they bake their own jitter in vox_import and aren't greedy-meshed.
    let occupancy = storage_buffers.add(ShaderStorageBuffer::from(
        voxel_world.solid_occupancy_bits(),
    ));
    let terrain_material = terrain_materials.add(voxel_material::VoxelTerrainMaterial {
        base: StandardMaterial {
            base_color: Color::WHITE,
            perceptual_roughness: 0.95,
            ..default()
        },
        extension: voxel_material::voxel_extension(seed, occupancy.clone()),
    });
    let prop_material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.95,
        ..default()
    });

    // Demo prop placement, meshed with the terrain's AO treatment:
    //   VOXEL_PROP=pack.vox                      → gallery row of every model
    //   VOXEL_PROP_LAYOUT="4@0,0;2@1.5,3.2;…"    → place model INDEX@x,z (m)
    if let Ok(prop_path) = std::env::var("VOXEL_PROP") {
        match vox_import::load_vox_models(Path::new(&prop_path)) {
            Ok(models) => {
                let placements: Vec<(usize, f32, f32, f32)> =
                    match std::env::var("VOXEL_PROP_LAYOUT") {
                        Ok(layout) => parse_prop_layout(&layout),
                        Err(_) => {
                            // Gallery: all models in a row, 2 m apart.
                            let mut row_x = -8.0;
                            models
                                .iter()
                                .enumerate()
                                .map(|(index, model)| {
                                    let width = model.dimensions_meters().x;
                                    let center_x = row_x + width / 2.0;
                                    row_x += width + 2.0;
                                    (index, center_x, -10.0, 0.0)
                                })
                                .collect()
                        }
                    };
                info!("placing {} props from {prop_path}", placements.len());
                for (model_index, x, z, lift) in placements {
                    let Some(model) = models.get(model_index) else {
                        warn!("prop layout references missing model {model_index}");
                        continue;
                    };
                    let ground = ground_heights.ground_at(x, z);
                    let prop_meshes =
                        vox_import::build_prop_meshes(model, seed.wrapping_add(model_index as u32));
                    if let Some(solid_mesh) = prop_meshes.solid {
                        commands.spawn((
                            Mesh3d(meshes.add(solid_mesh)),
                            MeshMaterial3d(prop_material.clone()),
                            Transform::from_xyz(x, ground + lift, z),
                            water::reflective_layers(),
                            WorldMesh,
                        ));
                    }
                    if let Some(flame_mesh) = prop_meshes.flame {
                        commands.spawn((
                            Mesh3d(meshes.add(flame_mesh)),
                            MeshMaterial3d(flame_materials.add(FlameMaterial {
                                // height, sway amplitude, sway speed, gain
                                params: Vec4::new(1.0, 0.05, 6.0, 2.8),
                            })),
                            Transform::from_xyz(x, ground + lift, z),
                            NotShadowCaster,
                            water::reflective_layers(),
                            WorldMesh,
                        ));
                        commands.spawn((
                            PointLight {
                                color: Color::srgb(1.0, 0.62, 0.28),
                                intensity: 60_000.0,
                                range: 12.0,
                                shadows_enabled: false,
                                ..default()
                            },
                            FlameLight {
                                base_intensity: 60_000.0,
                            },
                            Transform::from_xyz(x, ground + lift + 0.6, z),
                            water::reflective_layers(),
                            WorldMesh,
                        ));
                    }
                }
            }
            Err(error) => warn!("props skipped: {error}"),
        }
    }

    commands.insert_resource(ground_heights);

    // One entity per chunk bucket, so bevy's per-entity frustum culling
    // trims every pass (main, reflection, shadow cascades) to what each
    // view actually sees. Vertices are in world space; transforms stay
    // identity and the culling AABBs derive from the vertices.
    for chunk in chunk_meshes {
        // Above-water terrain is also on the reflection layer, so the
        // mirrored camera sees it; the meadow carpet and underwater
        // terrain stay main-view only.
        if let Some(chunk_mesh) = chunk.terrain_above_water {
            commands.spawn((
                Mesh3d(meshes.add(chunk_mesh)),
                MeshMaterial3d(terrain_material.clone()),
                water::reflective_layers(),
                WorldMesh,
            ));
        }
        // Neither the grass carpet nor anything below the waterline casts
        // a shadow worth seeing — keeping them out of the cascade shadow
        // passes (which render per camera view) is a large win.
        if let Some(chunk_mesh) = chunk.meadow_cover {
            commands.spawn((
                Mesh3d(meshes.add(chunk_mesh)),
                MeshMaterial3d(terrain_material.clone()),
                NotShadowCaster,
                WorldMesh,
            ));
        }
        if let Some(chunk_mesh) = chunk.terrain_below_water {
            commands.spawn((
                Mesh3d(meshes.add(chunk_mesh)),
                MeshMaterial3d(terrain_material.clone()),
                NotShadowCaster,
                WorldMesh,
            ));
        }
        // Canopy confetti: reflection-visible (trees in the mirror), but NOT a
        // shadow caster — 1.8M little cubes rendered through 4 shadow cascades
        // ×2 camera views cost ~30 fps. Shadow cascades render per view, so
        // the confetti's cast shade isn't worth that; the ambient + directional
        // lighting on the leaves still reads fine.
        if let Some(chunk_mesh) = chunk.canopy {
            commands.spawn((
                Mesh3d(meshes.add(chunk_mesh)),
                MeshMaterial3d(terrain_material.clone()),
                NotShadowCaster,
                CanopyConfetti,
                water::reflective_layers(),
                WorldMesh,
            ));
        }
        // The solid inner canopy IS the tree shadow caster (cheap, ~0.3M),
        // behind the confetti. Reflection-visible + shadow-casting.
        if let Some(chunk_mesh) = chunk.canopy_solid {
            commands.spawn((
                Mesh3d(meshes.add(chunk_mesh)),
                MeshMaterial3d(terrain_material.clone()),
                water::reflective_layers(),
                WorldMesh,
            ));
        }
        // The static per-chunk water mesh (`chunk.water`) is intentionally not
        // spawned: any world with water has a fluid sim, whose dynamic surface
        // (below) covers the same region. It exists only when there's no water,
        // in which case `chunk.water` is `None` anyway.
        let _ = &chunk.water;
    }

    // Dynamic water surface: a STATIC grid mesh built once, displaced every sim
    // tick by the GPU from the `heights` storage buffer (F4). The sim only
    // re-uploads that small buffer — the mesh itself never changes.
    if let Some(fluid_water) = fluid_water {
        let surface_mesh = meshes.add(fluid::build_static_surface_mesh(&fluid_water));
        let heights = storage_buffers.add(ShaderStorageBuffer::from(
            fluid::corner_heights(&fluid_water).as_slice(),
        ));
        info!(
            "fluid sim: {}×{} cells, static surface {} verts, heights buffer {} floats",
            fluid_water.sim.size_x,
            fluid_water.sim.size_z,
            meshes.get(&surface_mesh).map_or(0, Mesh::count_vertices),
            (fluid_water.sim.size_x + 1) * (fluid_water.sim.size_z + 1),
        );
        let water_material = water_materials.add(water::WaterMaterial {
            water: water::WaterUniform::default(),
            reflection: reflection_target.image.clone(),
            reflection_clip_from_world: Mat4::IDENTITY,
            heights: heights.clone(),
        });
        commands.spawn((
            Mesh3d(surface_mesh),
            MeshMaterial3d(water_material),
            NotShadowCaster,
            fluid::DynamicWaterSurface,
            WorldMesh,
        ));
        commands.insert_resource(fluid::WaterHeightBuffer(heights));
        commands.insert_resource(fluid_water);
    }

    // Grass is instanced (auto-instancing over a small variant palette), not
    // baked into the chunk meshes — the mesher skips TallGrass.
    grass::spawn_instanced_grass(commands, meshes, grass_materials, &voxel_world, seed);
}

/// Parse `"index@x,z"` or `"index@x,z,lift"` prop placements (meters,
/// world-centered; `lift` raises the prop above the ground, e.g. spray).
fn parse_prop_layout(layout: &str) -> Vec<(usize, f32, f32, f32)> {
    layout
        .split(';')
        .filter_map(|entry| {
            let (index_text, position_text) = entry.trim().split_once('@')?;
            let mut numbers = position_text.split(',');
            let x = numbers.next()?.trim().parse().ok()?;
            let z = numbers.next()?.trim().parse().ok()?;
            let lift = match numbers.next() {
                Some(lift_text) => lift_text.trim().parse().ok()?,
                None => 0.0,
            };
            Some((index_text.trim().parse().ok()?, x, z, lift))
        })
        .collect()
}

/// Walkable ground height per column, in render space. Trees, tall grass,
/// and clouds are not ground: you walk under canopies and through tufts.
#[derive(Resource)]
struct GroundHeights {
    /// Surface clamped UP to the water plane — used for prop placement.
    heights: Vec<f32>,
    /// True terrain floor (river/lake bed), NOT clamped to water — the
    /// walker wades on this, so it sinks into water instead of standing on it.
    bed: Vec<f32>,
}

impl GroundHeights {
    fn from_world(voxel_world: &VoxelWorld) -> Self {
        let mut heights = vec![PLATEAU_FLOOR_RENDER; WORLD_SIZE_X * WORLD_SIZE_Z];
        let mut bed = vec![PLATEAU_FLOOR_RENDER; WORLD_SIZE_X * WORLD_SIZE_Z];
        let water_plane = (WATER_LEVEL + 1) as f32 * VOXEL_SIZE;
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                // Topmost walkable ground, straight off the column's RLE
                // runs (a handful per column) instead of 256 cell reads.
                let mut ground_top: Option<i32> = None;
                for (voxel, y_start, length) in voxel_world.column_runs(x, z) {
                    if matches!(
                        voxel,
                        Voxel::Grass | Voxel::Dirt | Voxel::Sand | Voxel::Sediment | Voxel::Stone
                    ) {
                        ground_top = Some(y_start + length - 1);
                    }
                }
                if let Some(y) = ground_top {
                    let index = (z as usize) * WORLD_SIZE_X + x as usize;
                    let floor = (y + 1) as f32 * VOXEL_SIZE;
                    bed[index] = floor;
                    heights[index] = floor.max(water_plane);
                }
            }
        }
        Self { heights, bed }
    }

    fn column_index(x: f32, z: f32) -> Option<usize> {
        let column_x = (x / VOXEL_SIZE + WORLD_SIZE_X as f32 / 2.0) as i32;
        let column_z = (z / VOXEL_SIZE + WORLD_SIZE_Z as f32 / 2.0) as i32;
        if column_x < 0
            || column_z < 0
            || column_x >= WORLD_SIZE_X as i32
            || column_z >= WORLD_SIZE_Z as i32
        {
            return None;
        }
        Some((column_z as usize) * WORLD_SIZE_X + column_x as usize)
    }

    /// Water-clamped surface height at a render-space position (for placement).
    fn ground_at(&self, x: f32, z: f32) -> f32 {
        match Self::column_index(x, z) {
            Some(index) => self.heights[index],
            None => PLATEAU_FLOOR_RENDER,
        }
    }

    /// True terrain-floor (bed) height — the walker wades on this.
    fn bed_at(&self, x: f32, z: f32) -> f32 {
        match Self::column_index(x, z) {
            Some(index) => self.bed[index],
            None => PLATEAU_FLOOR_RENDER,
        }
    }
}

/// Half-width of the walker's collision circle (m). Keeps a small gap off trunks.
const PLAYER_RADIUS: f32 = 0.32;

/// Which columns contain a tree trunk, so the walker can be pushed out of them
/// (trees are still baked voxels — this just adds collision, no instancing).
/// Leaves are passable; only `Trunk`/`TrunkBirch` block.
#[derive(Resource)]
struct TreeColliders {
    trunk: Vec<bool>,
}

impl TreeColliders {
    fn from_world(voxel_world: &VoxelWorld) -> Self {
        let mut trunk = vec![false; WORLD_SIZE_X * WORLD_SIZE_Z];
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                let is_trunk = voxel_world
                    .column_runs(x, z)
                    .any(|(voxel, _, _)| matches!(voxel, Voxel::Trunk | Voxel::TrunkBirch));
                if is_trunk {
                    trunk[(z as usize) * WORLD_SIZE_X + x as usize] = true;
                }
            }
        }
        Self { trunk }
    }

    /// Push a circle of `radius` at render-space `(x, z)` out of any trunk cell
    /// it overlaps (circle-vs-AABB), returning the resolved `(x, z)`.
    fn resolve(&self, x: f32, z: f32, radius: f32) -> (f32, f32) {
        let half_x = WORLD_SIZE_X as f32 / 2.0;
        let half_z = WORLD_SIZE_Z as f32 / 2.0;
        // Work in voxel-space where each trunk cell is the unit square [c, c+1].
        let mut px = x / VOXEL_SIZE + half_x;
        let mut pz = z / VOXEL_SIZE + half_z;
        let r = radius / VOXEL_SIZE;

        let min_cx = (px - r - 1.0).floor() as i32;
        let max_cx = (px + r + 1.0).ceil() as i32;
        let min_cz = (pz - r - 1.0).floor() as i32;
        let max_cz = (pz + r + 1.0).ceil() as i32;
        for cz in min_cz..=max_cz {
            for cx in min_cx..=max_cx {
                if cx < 0 || cz < 0 || cx >= WORLD_SIZE_X as i32 || cz >= WORLD_SIZE_Z as i32 {
                    continue;
                }
                if !self.trunk[(cz as usize) * WORLD_SIZE_X + cx as usize] {
                    continue;
                }
                let cell_x = cx as f32;
                let cell_z = cz as f32;
                let nearest_x = px.clamp(cell_x, cell_x + 1.0);
                let nearest_z = pz.clamp(cell_z, cell_z + 1.0);
                let dx = px - nearest_x;
                let dz = pz - nearest_z;
                let dist_squared = dx * dx + dz * dz;
                if dist_squared >= r * r {
                    continue;
                }
                if dist_squared > 1e-6 {
                    // Outside the cell but within the circle: push along the
                    // normal away from the nearest edge.
                    let dist = dist_squared.sqrt();
                    let push = r - dist;
                    px += dx / dist * push;
                    pz += dz / dist * push;
                } else {
                    // Center inside the cell: eject along the shallowest axis.
                    let left = px - cell_x;
                    let right = cell_x + 1.0 - px;
                    let back = pz - cell_z;
                    let front = cell_z + 1.0 - pz;
                    let min_pen = left.min(right).min(back).min(front);
                    if min_pen == left {
                        px = cell_x - r;
                    } else if min_pen == right {
                        px = cell_x + 1.0 + r;
                    } else if min_pen == back {
                        pz = cell_z - r;
                    } else {
                        pz = cell_z + 1.0 + r;
                    }
                }
            }
        }
        ((px - half_x) * VOXEL_SIZE, (pz - half_z) * VOXEL_SIZE)
    }
}

const PLATEAU_FLOOR_RENDER: f32 = voxel_core::world::PLATEAU_FLOOR as f32 * VOXEL_SIZE;

/// Press `R` for a fresh plateau; the panel's season slider requests a
/// rebuild of the current one.
#[allow(clippy::too_many_arguments)]
fn regenerate_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut request: ResMut<RegenerateRequest>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut terrain_materials: ResMut<Assets<voxel_material::VoxelTerrainMaterial>>,
    mut grass_materials: ResMut<Assets<voxel_material::GrassMaterial>>,
    mut storage_buffers: ResMut<Assets<ShaderStorageBuffer>>,
    mut flame_materials: ResMut<Assets<FlameMaterial>>,
    mut water_materials: ResMut<Assets<water::WaterMaterial>>,
    reflection_target: Res<water::ReflectionTarget>,
    mut seed: ResMut<WorldSeed>,
    season: Res<Season>,
    source: Res<WorldSource>,
    existing_meshes: Query<Entity, With<WorldMesh>>,
) {
    if keyboard.just_pressed(KeyCode::KeyR) {
        request.requested = true;
        request.bump_seed = true;
    }
    if !request.requested {
        return;
    }
    if request.bump_seed {
        seed.0 = seed.0.wrapping_add(1);
    }
    *request = RegenerateRequest::default();

    for entity in &existing_meshes {
        commands.entity(entity).despawn();
    }
    // Imported terrain keeps its heights; a new seed reshuffles the
    // decoration (grass, flowers, procedural trees) and waterfalls.
    spawn_world(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut terrain_materials,
        &mut grass_materials,
        &mut storage_buffers,
        &mut flame_materials,
        &mut water_materials,
        &reflection_target,
        &source,
        seed.0,
        season.0,
    );
}

#[derive(Resource)]
struct OrbitCameraState {
    focus: Vec3,
    yaw: f32,
    /// Angle above the horizon, radians.
    pitch: f32,
    distance: f32,
}

impl Default for OrbitCameraState {
    fn default() -> Self {
        Self {
            // The island's surface sits around y = 12 since the world was
            // raised to make room for the sculpted underside.
            focus: Vec3::new(0.0, 12.5, 0.0),
            yaw: 0.7,
            // `VOXEL_ORBIT_PITCH` (radians) frames screenshots — low values
            // show the island silhouette side-on.
            pitch: std::env::var("VOXEL_ORBIT_PITCH")
                .ok()
                .and_then(|value| value.parse().ok())
                .unwrap_or(0.55),
            distance: 130.0,
        }
    }
}

impl OrbitCameraState {
    fn transform(&self) -> Transform {
        let rotation = Quat::from_euler(EulerRot::YXZ, self.yaw, -self.pitch, 0.0);
        Transform::from_translation(self.focus + rotation * Vec3::Z * self.distance)
            .looking_at(self.focus, Vec3::Y)
    }
}

#[derive(Resource)]
struct FirstPersonState {
    position: Vec3,
    yaw: f32,
    /// Positive looks up, radians.
    pitch: f32,
    /// Vertical speed (m/s) for gravity + jumping; 0 while grounded.
    vertical_velocity: f32,
    /// True when the walker is resting on the ground (can jump).
    grounded: bool,
}

impl Default for FirstPersonState {
    fn default() -> Self {
        // `VOXEL_LOOK=yaw,pitch` (radians) aims the starting first-person
        // view — used by screenshot runs to frame the moon or clouds.
        let (yaw, pitch) = std::env::var("VOXEL_LOOK")
            .ok()
            .and_then(|value| {
                let (yaw_text, pitch_text) = value.split_once(',')?;
                Some((
                    yaw_text.trim().parse().ok()?,
                    pitch_text.trim().parse().ok()?,
                ))
            })
            .unwrap_or((0.7, -0.05));
        // `VOXEL_POSITION=x,z` (render-space meters) places the walker —
        // pair with the "nearest water" log to shoot shoreline close-ups.
        let (start_x, start_z) = std::env::var("VOXEL_POSITION")
            .ok()
            .and_then(|value| {
                let (x_text, z_text) = value.split_once(',')?;
                Some((x_text.trim().parse().ok()?, z_text.trim().parse().ok()?))
            })
            .unwrap_or((0.0, 0.0));
        Self {
            position: Vec3::new(start_x, 13.0, start_z),
            yaw,
            pitch,
            vertical_velocity: 0.0,
            grounded: false,
        }
    }
}

/// `Tab` switches orbit ↔ first-person; each mode resumes where the other
/// left off (walk somewhere, Tab out, and you orbit around that spot).
fn toggle_view_mode(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut view_mode: ResMut<ViewMode>,
    mut orbit_state: ResMut<OrbitCameraState>,
    mut walk_state: ResMut<FirstPersonState>,
) {
    if !keyboard.just_pressed(KeyCode::Tab) {
        return;
    }
    *view_mode = match *view_mode {
        ViewMode::Orbit => {
            walk_state.position.x = orbit_state.focus.x;
            walk_state.position.z = orbit_state.focus.z;
            walk_state.yaw = orbit_state.yaw;
            walk_state.pitch = -0.05;
            ViewMode::FirstPerson
        }
        ViewMode::FirstPerson => {
            orbit_state.focus = walk_state.position;
            orbit_state.yaw = walk_state.yaw;
            ViewMode::Orbit
        }
    };
}

#[allow(clippy::too_many_arguments)]
fn camera_system(
    view_mode: Res<ViewMode>,
    tweaks: Res<ViewTweaks>,
    weather: Res<weather::WeatherState>,
    mut orbit_state: ResMut<OrbitCameraState>,
    mut walk_state: ResMut<FirstPersonState>,
    ground_heights: Option<Res<GroundHeights>>,
    tree_colliders: Option<Res<TreeColliders>>,
    mut camera_query: Query<
        (
            &mut Transform,
            &mut Projection,
            // Optional: the perf overlay's GPU levers may strip the DoF
            // component entirely to measure its cost.
            Option<&mut DepthOfField>,
            &mut DistanceFog,
        ),
        With<Camera3d>,
    >,
    mouse_buttons: Res<ButtonInput<MouseButton>>,
    keyboard: Res<ButtonInput<KeyCode>>,
    mouse_motion: Res<AccumulatedMouseMotion>,
    mouse_scroll: Res<AccumulatedMouseScroll>,
    time: Res<Time>,
) {
    let Ok((mut camera_transform, mut projection, mut depth_of_field, mut fog)) =
        camera_query.single_mut()
    else {
        return;
    };

    // The tweak panel owns the mouse while the cursor is over it.
    let mouse_free = !tweaks.pointer_over_panel;

    // Weather fog pulls the falloff band in toward the camera; the curve
    // makes the slider's mid-range already feel misty.
    let fog_amount = weather.current.fog.clamp(0.0, 1.0).powf(0.75);
    let fog_range = |clear: f32, thick: f32| clear + (thick - clear) * fog_amount;

    match *view_mode {
        ViewMode::Orbit => {
            orbit_update(
                &mut orbit_state,
                &mouse_buttons,
                &keyboard,
                &mouse_motion,
                &mouse_scroll,
                &time,
                mouse_free,
            );
            *camera_transform = orbit_state.transform();
            if let Some(depth_of_field) = depth_of_field.as_mut() {
                depth_of_field.focal_distance = orbit_state.distance;
                depth_of_field.aperture_f_stops = tweaks.orbit_aperture_f_stops;
            }
            set_fov(&mut projection, tweaks.orbit_fov_degrees);
            // Haze must stay behind the diorama from up here, or the whole
            // scene washes out; only the far rim picks up a little depth —
            // until weather fog rolls in and swallows the far half.
            fog.falloff = FogFalloff::Linear {
                start: fog_range(orbit_state.distance * 1.1, orbit_state.distance * 0.30),
                end: fog_range(orbit_state.distance * 3.0, orbit_state.distance * 1.15),
            };
        }
        ViewMode::FirstPerson => {
            first_person_update(
                &mut walk_state,
                ground_heights.as_deref(),
                tree_colliders.as_deref(),
                &mouse_buttons,
                &keyboard,
                &mouse_motion,
                &time,
                tweaks.eye_height,
                mouse_free,
            );
            *camera_transform = Transform::from_translation(walk_state.position).with_rotation(
                Quat::from_euler(EulerRot::YXZ, walk_state.yaw, walk_state.pitch, 0.0),
            );
            if let Some(depth_of_field) = depth_of_field.as_mut() {
                depth_of_field.focal_distance = tweaks.walk_focal_distance;
                depth_of_field.aperture_f_stops = tweaks.walk_aperture_f_stops;
            }
            set_fov(&mut projection, tweaks.first_person_fov_degrees);
            fog.falloff = FogFalloff::Linear {
                start: fog_range(35.0, 2.5),
                end: fog_range(170.0, 26.0),
            };
        }
    }
}

fn set_fov(projection: &mut Projection, fov_degrees: f32) {
    if let Projection::Perspective(perspective) = projection {
        perspective.fov = fov_degrees.to_radians();
    }
}

fn orbit_update(
    camera_state: &mut OrbitCameraState,
    mouse_buttons: &ButtonInput<MouseButton>,
    keyboard: &ButtonInput<KeyCode>,
    mouse_motion: &AccumulatedMouseMotion,
    mouse_scroll: &AccumulatedMouseScroll,
    time: &Time,
    mouse_free: bool,
) {
    let motion_delta = if mouse_free {
        mouse_motion.delta
    } else {
        Vec2::ZERO
    };

    if mouse_buttons.pressed(MouseButton::Left) && motion_delta != Vec2::ZERO {
        camera_state.yaw -= motion_delta.x * 0.005;
        camera_state.pitch = (camera_state.pitch + motion_delta.y * 0.005).clamp(0.05, 1.5);
    }

    let scroll_amount = if !mouse_free {
        0.0
    } else {
        match mouse_scroll.unit {
            MouseScrollUnit::Line => mouse_scroll.delta.y,
            MouseScrollUnit::Pixel => mouse_scroll.delta.y / 50.0,
        }
    };
    if scroll_amount != 0.0 {
        camera_state.distance =
            (camera_state.distance * (1.0 - scroll_amount * 0.1)).clamp(8.0, 500.0);
    }

    let yaw_rotation = Quat::from_rotation_y(camera_state.yaw);
    let forward = yaw_rotation * Vec3::NEG_Z;
    let right = yaw_rotation * Vec3::X;

    let mut pan_direction = Vec3::ZERO;
    if keyboard.pressed(KeyCode::KeyW) {
        pan_direction += forward;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        pan_direction -= forward;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        pan_direction += right;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        pan_direction -= right;
    }
    let keyboard_pan_speed = camera_state.distance * 0.5;
    let mut pan = pan_direction * keyboard_pan_speed * time.delta_secs();

    if mouse_buttons.pressed(MouseButton::Right) && motion_delta != Vec2::ZERO {
        let mouse_pan_speed = camera_state.distance * 0.0012;
        pan += (right * -motion_delta.x + forward * motion_delta.y) * mouse_pan_speed;
    }
    camera_state.focus += pan;
}

/// When the first-person camera drops below the water surface, drown the view
/// in a murky blue-green with short visibility. Runs LAST in the frame (after
/// `day_night` sets the sky-driven fog) so it gets the final say on the fog,
/// and reverts automatically the moment the eye surfaces.
/// Whether the eye is currently underwater. Set by `underwater_fog`, read by
/// the sky (to drop clouds) and anything else that should change when submerged.
#[derive(Resource, Default)]
pub struct Submerged(pub bool);

/// The canopy "confetti" leaf cubes (~1.8M) — a P-overlay toggle hides them to
/// A/B their cost (they render through the reflection view too).
#[derive(Component)]
pub struct CanopyConfetti;

/// Live render-quality / optimization levers, each toggleable from the P-overlay
/// so their cost can be A/B'd. Defaults reproduce the current look.
#[derive(Resource)]
pub struct RenderQuality {
    /// Fog-sea raymarch steps (fill-bound hog). Fewer = cheaper, dither hides it.
    pub fog_steps: u32,
    /// Sky cloud march steps (runs for main + reflection views).
    pub cloud_steps: u32,
    /// Re-render the planar reflection only every Nth frame (1 = every frame).
    /// The reflection is the single biggest GPU cost; the wave wobble hides
    /// staleness, so higher = much cheaper.
    pub reflection_interval: u32,
    /// Extra multiplier on the reflection's dynamic-resolution tier (0.25–1.0).
    /// The wave distortion hides low res, so this trades mirror sharpness for
    /// fill directly on the biggest cost.
    pub reflection_scale: f32,
}

impl Default for RenderQuality {
    fn default() -> Self {
        Self {
            fog_steps: 12,
            cloud_steps: 10,
            reflection_interval: 2,
            reflection_scale: 1.0,
        }
    }
}

/// Full-screen tint quad shown while the eye is underwater — copies KUDA's
/// whole-view `color * waterColor` so *near* geometry is tinted too (distance
/// fog alone left it "too clear"). Spawned once, transparent; `underwater_fog`
/// drives its color/alpha.
#[derive(Component)]
struct UnderwaterOverlay;

fn spawn_underwater_overlay(mut commands: Commands) {
    commands.spawn((
        Node {
            position_type: PositionType::Absolute,
            width: Val::Percent(100.0),
            height: Val::Percent(100.0),
            ..default()
        },
        BackgroundColor(Color::NONE),
        // Above the 3D scene, below the egui panels (their own pass) so tuning
        // stays readable; ignore pointer so it never eats camera drags.
        GlobalZIndex(5),
        bevy::picking::Pickable::IGNORE,
        UnderwaterOverlay,
    ));
}

#[allow(clippy::too_many_arguments)]
fn underwater_fog(
    view_mode: Res<ViewMode>,
    walk_state: Res<FirstPersonState>,
    tint: Res<tweak_panel::UnderwaterTint>,
    mut fog_query: Query<&mut DistanceFog, With<Camera3d>>,
    mut overlay: Query<&mut BackgroundColor, With<UnderwaterOverlay>>,
    mut ambient: ResMut<bevy::light::GlobalAmbientLight>,
    mut fog_sea: Query<&mut Visibility, With<fog_ring::FogSea>>,
    mut submerged_state: ResMut<Submerged>,
) {
    // `VOXEL_FORCE_UNDERWATER=1` previews the submerged look from any camera
    // (you otherwise have to hold a dive, which a screenshot can't).
    let submerged = (*view_mode == ViewMode::FirstPerson
        && walk_state.position.y < water::water_surface_y())
        || std::env::var("VOXEL_FORCE_UNDERWATER").is_ok();
    submerged_state.0 = submerged;

    // Whole-screen tint (KUDA `color * waterColor`): the dominant "you're
    // underwater" cue, and what fixes near geometry looking too clear.
    // The fog-sea dome is a custom-shader mesh (ignores lighting); from below it
    // reads as dark shards against the surface. Hide it while submerged — you're
    // under the fog, not looking across it.
    if let Ok(mut fog_visibility) = fog_sea.single_mut() {
        *fog_visibility = if submerged {
            Visibility::Hidden
        } else {
            Visibility::Inherited
        };
    }

    if let Ok(mut background) = overlay.single_mut() {
        background.0 = if submerged {
            Color::srgba(
                tint.screen_color[0],
                tint.screen_color[1],
                tint.screen_color[2],
                tint.screen_strength,
            )
        } else {
            Color::NONE
        };
    }

    if !submerged {
        return;
    }
    // Optional ambient lift/tint for submerged solids (0 = leave day_night's
    // value; the near bed already reads fine, so this is opt-in via the panel).
    // `day_night` resets ambient every frame, so it only holds while submerged.
    if tint.ambient_brightness > 0.0 {
        ambient.brightness = tint.ambient_brightness;
        ambient.color = Color::srgb(
            tint.inscattering_color[0],
            tint.inscattering_color[1],
            tint.inscattering_color[2],
        );
    }

    let Ok(mut fog) = fog_query.single_mut() else {
        return;
    };
    // Per-channel Beer–Lambert depth gradient on top of the screen tint: red
    // extinguishes fastest so distance drifts green→blue, fading to the water's
    // in-scatter colour. Colours + visibility are live in the V-panel.
    let color = |c: [f32; 3]| Color::srgb(c[0], c[1], c[2]);
    fog.falloff = FogFalloff::from_visibility_colors(
        tint.visibility,
        color(tint.extinction_color),
        color(tint.inscattering_color),
    );
}

/// Gravity for the first-person walker (m/s²) — a touch snappier than real
/// gravity for a game feel.
const GRAVITY: f32 = 22.0;
/// Jump launch speed (m/s): ≈ `sqrt(2·GRAVITY·height)`, ~1.3 m apex.
const JUMP_SPEED: f32 = 7.5;
/// Eye rest height relative to the water surface while swimming (m): slightly
/// above, so you bob head-out by default and dive under by holding Space.
const SWIM_EYE_OFFSET: f32 = 0.1;
/// Buoyancy spring stiffness pulling the swimmer toward the float line.
const BUOYANCY_SPRING: f32 = 6.0;
/// Dive acceleration from holding Space while swimming (m/s²); buoyancy floats
/// you back to the surface on release.
const SWIM_ACCEL: f32 = 20.0;
/// Water drag damping the swimmer's vertical velocity (per second).
const WATER_DRAG: f32 = 4.0;

#[allow(clippy::too_many_arguments)]
fn first_person_update(
    walk_state: &mut FirstPersonState,
    ground_heights: Option<&GroundHeights>,
    tree_colliders: Option<&TreeColliders>,
    mouse_buttons: &ButtonInput<MouseButton>,
    keyboard: &ButtonInput<KeyCode>,
    mouse_motion: &AccumulatedMouseMotion,
    time: &Time,
    eye_height: f32,
    mouse_free: bool,
) {
    let motion_delta = if mouse_free {
        mouse_motion.delta
    } else {
        Vec2::ZERO
    };
    if mouse_buttons.pressed(MouseButton::Left) && motion_delta != Vec2::ZERO {
        walk_state.yaw -= motion_delta.x * 0.003;
        walk_state.pitch = (walk_state.pitch - motion_delta.y * 0.003).clamp(-1.4, 1.4);
    }

    let yaw_rotation = Quat::from_rotation_y(walk_state.yaw);
    let forward = yaw_rotation * Vec3::NEG_Z;
    let right = yaw_rotation * Vec3::X;

    let mut walk_direction = Vec3::ZERO;
    if keyboard.pressed(KeyCode::KeyW) {
        walk_direction += forward;
    }
    if keyboard.pressed(KeyCode::KeyS) {
        walk_direction -= forward;
    }
    if keyboard.pressed(KeyCode::KeyD) {
        walk_direction += right;
    }
    if keyboard.pressed(KeyCode::KeyA) {
        walk_direction -= right;
    }
    let walk_speed = if keyboard.pressed(KeyCode::ShiftLeft) {
        12.0
    } else {
        4.5
    };
    walk_state.position += walk_direction.normalize_or_zero() * walk_speed * time.delta_secs();

    // Push out of tree trunks (XZ only — leaves stay passable).
    if let Some(colliders) = tree_colliders {
        let (resolved_x, resolved_z) =
            colliders.resolve(walk_state.position.x, walk_state.position.z, PLAYER_RADIUS);
        walk_state.position.x = resolved_x;
        walk_state.position.z = resolved_z;
    }

    // Vertical motion: on land it's gravity + jump with the terrain as the
    // floor; in deep water it switches to buoyant swimming (Space up/Ctrl down).
    if let Some(heights) = ground_heights {
        let dt = time.delta_secs();
        let water_surface = water::water_surface_y();
        // Wade on the bed, not the water surface — the camera sinks into water
        // as it deepens instead of standing on top.
        let floor_eye = heights.bed_at(walk_state.position.x, walk_state.position.z) + eye_height;
        // Deep enough that standing on the bed would submerge the eye → swim.
        let swimming = floor_eye < water_surface - 0.05;

        if swimming {
            // Buoyancy: a damped spring floats the eye toward just above the
            // surface. Hold Space to swim up (out of the water), Ctrl to dive;
            // the bed still stops a dive at the bottom.
            let float_target = water_surface + SWIM_EYE_OFFSET;
            walk_state.vertical_velocity +=
                (float_target - walk_state.position.y) * BUOYANCY_SPRING * dt;
            // In water, hold Space to dive; release and buoyancy floats you
            // back up to the surface (no separate up key needed).
            if keyboard.pressed(KeyCode::Space) {
                walk_state.vertical_velocity -= SWIM_ACCEL * dt;
            }
            walk_state.vertical_velocity *= (1.0 - WATER_DRAG * dt).max(0.0);
            walk_state.position.y += walk_state.vertical_velocity * dt;
            if walk_state.position.y < floor_eye {
                walk_state.position.y = floor_eye;
                walk_state.vertical_velocity = walk_state.vertical_velocity.max(0.0);
            }
            walk_state.grounded = false;
        } else {
            // On land: gravity + jump, with the terrain as the floor.
            if walk_state.grounded && keyboard.just_pressed(KeyCode::Space) {
                walk_state.vertical_velocity = JUMP_SPEED;
                walk_state.grounded = false;
            }
            walk_state.vertical_velocity -= GRAVITY * dt;
            walk_state.position.y += walk_state.vertical_velocity * dt;
            // Land on (or step up to) the floor.
            if walk_state.position.y <= floor_eye {
                walk_state.position.y = floor_eye;
                walk_state.vertical_velocity = 0.0;
                walk_state.grounded = true;
            } else {
                walk_state.grounded = false;
            }
        }
        // Debug: pin the eye below the surface so screenshots can inspect the
        // underwater / look-up (Snell's window) view regardless of buoyancy.
        if std::env::var("VOXEL_FORCE_UNDERWATER").is_ok() {
            walk_state.position.y = water_surface - 1.0;
        }
    }
}

/// With `VOXEL_SCREENSHOT_PATH` set: let the scene settle, save one frame
/// to that path, then exit. No OS screen-recording permission needed.
fn screenshot_system(
    mut commands: Commands,
    mut frame_counter: Local<u32>,
    mut capture_time: Local<Option<std::time::Instant>>,
    mut exit: MessageWriter<AppExit>,
) {
    let Ok(path) = std::env::var("VOXEL_SCREENSHOT_PATH") else {
        return;
    };
    *frame_counter += 1;
    if *frame_counter == 30 {
        commands
            .spawn(Screenshot::primary_window())
            .observe(save_to_disk(path));
        *capture_time = Some(std::time::Instant::now());
    }
    if *frame_counter >= 90 {
        if let Some(started) = *capture_time {
            let average_fps = 60.0 / started.elapsed().as_secs_f64();
            info!("frames 30..90 averaged {average_fps:.1} fps");
        }
        exit.write(AppExit::Success);
    }
}
