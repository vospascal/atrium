//! Voxel plateau sandbox — prototype for the Atrium visual layer.
//!
//! A procedural floating-island diorama: rolling voxel terrain with a
//! lush→desert biome gradient, a carved river, slab-canopy trees, and a
//! sculpted rock underside, hovering above a volumetric fog sea.
//!
//! Run:    `cargo run -p voxel-sandbox`
//! Keys:   `Tab` switch orbit ↔ first-person view · `R` new island
//!         `F` release a firefly swarm · `Shift+F` clear swarms
//!         hold `N` to fast-forward the day/night cycle
//!         `V` tuning panels (view + time & weather)
//! Orbit:  left-drag orbit · right-drag pan · scroll zoom · WASD pan
//! Walk:   left-drag look around · WASD walk · Shift run
//!
//! `VOXEL_TIME=0.0..1.0` sets the starting time of day (0 = midnight,
//! 0.25 = sunrise, 0.5 = noon, 0.75 = sunset); `VOXEL_MOON=0.0..1.0` the
//! moon phase (0.5 = full). Weather is scriptable too: `VOXEL_CLOUDS`,
//! `VOXEL_CLOUD_TYPE` (0 stratus · 1 cumulus · 2 cirrus), `VOXEL_WIND`,
//! `VOXEL_WIND_DIRECTION`, `VOXEL_FOG`, `VOXEL_PRECIP=rain|snow`,
//! `VOXEL_PRECIP_INTENSITY`.
//!
//! Set `VOXEL_SCREENSHOT_PATH=/some/dir/shot.png` to capture one frame and
//! exit (automated visual verification); add `VOXEL_START_FIRST_PERSON=1`
//! to capture from the walking view.

mod day_night;
mod fireflies;
mod flame;
mod fog_ring;
mod mesh;
mod noise;
mod sky;
mod terrain_import;
mod tweak_panel;
mod vox_import;
mod waterfall;
mod weather;
mod world;

use bevy::input::mouse::{AccumulatedMouseMotion, AccumulatedMouseScroll, MouseScrollUnit};
use bevy::light::{CascadeShadowConfigBuilder, GlobalAmbientLight, NotShadowCaster};
use bevy::pbr::{DistanceFog, FogFalloff};
use bevy::post_process::dof::{DepthOfField, DepthOfFieldMode};
use bevy::prelude::*;
use bevy::render::view::screenshot::{save_to_disk, Screenshot};

use std::path::Path;

use crate::flame::{FlameLight, FlameMaterial};
use crate::tweak_panel::ViewTweaks;
use crate::world::{
    Voxel, VoxelWorld, VOXEL_SIZE, WATER_LEVEL, WORLD_SIZE_X, WORLD_SIZE_Y, WORLD_SIZE_Z,
};

/// Pale haze that washes out the distance, like the reference shots.
const HAZE_COLOR: Color = Color::srgb(0.80, 0.82, 0.79);

/// Where terrain comes from: the built-in generator, or a Blender export
/// (`cargo run -p voxel-sandbox -- path/to/name.terrain.json`).
#[derive(Resource)]
enum WorldSource {
    Procedural,
    Imported(terrain_import::ImportedTerrain),
}

fn main() {
    let start_first_person = std::env::var("VOXEL_START_FIRST_PERSON").is_ok();
    let world_source = match std::env::args().nth(1) {
        Some(terrain_path) => match terrain_import::load_terrain(Path::new(&terrain_path)) {
            Ok(terrain) => {
                println!("loaded Blender terrain from {terrain_path}");
                WorldSource::Imported(terrain)
            }
            Err(error) => {
                eprintln!("failed to load terrain: {error}");
                std::process::exit(1);
            }
        },
        None => WorldSource::Procedural,
    };

    App::new()
        .add_plugins(DefaultPlugins.set(WindowPlugin {
            primary_window: Some(Window {
                title: "Atrium — Voxel Plateau Sandbox".to_string(),
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
        .init_resource::<ViewTweaks>()
        .add_plugins(MaterialPlugin::<FlameMaterial>::default())
        .add_plugins(MaterialPlugin::<sky::SkyMaterial>::default())
        .add_plugins(MaterialPlugin::<weather::PrecipitationMaterial>::default())
        .add_plugins(MaterialPlugin::<fog_ring::FogSeaMaterial>::default())
        .add_plugins(MaterialPlugin::<waterfall::WaterfallMaterial>::default())
        .add_plugins(MaterialPlugin::<fireflies::FireflyMaterial>::default())
        .add_plugins(bevy_egui::EguiPlugin::default())
        .add_systems(
            bevy_egui::EguiPrimaryContextPass,
            tweak_panel::view_tweak_panel,
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
                fireflies::setup_fireflies,
            ),
        )
        .add_systems(
            Update,
            (
                toggle_view_mode,
                camera_system,
                weather::ease_weather,
                day_night::advance_day_night,
                sky::update_sky,
                fog_ring::update_fog_ring,
                weather::update_precipitation,
                waterfall::update_waterfalls,
                fireflies::firefly_controls,
                fireflies::spawn_env_fireflies,
                fireflies::update_fireflies,
                regenerate_system,
                flame::flicker_flame_lights,
                screenshot_system,
            )
                .chain(),
        )
        .run();
}

#[derive(Resource)]
struct WorldSeed(u32);

/// Marker for everything despawned and rebuilt on regeneration.
#[derive(Component)]
struct WorldMesh;

#[derive(Resource, Clone, Copy, PartialEq)]
enum ViewMode {
    /// Tilt-shift diorama overview.
    Orbit,
    /// Walking on the plateau at eye height.
    FirstPerson,
}

fn setup_scene(mut commands: Commands, camera_state: Res<OrbitCameraState>) {
    commands.spawn((
        Camera3d::default(),
        bevy::render::view::Hdr,
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
        bevy::post_process::bloom::Bloom::default(),
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
        day_night::SunLight,
    ));
}

fn initial_world_system(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut flame_materials: ResMut<Assets<FlameMaterial>>,
    mut waterfall_materials: ResMut<Assets<waterfall::WaterfallMaterial>>,
    seed: Res<WorldSeed>,
    source: Res<WorldSource>,
) {
    spawn_world(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut flame_materials,
        &mut waterfall_materials,
        &source,
        seed.0,
    );
}

#[allow(clippy::too_many_arguments)]
fn spawn_world(
    commands: &mut Commands,
    meshes: &mut Assets<Mesh>,
    materials: &mut Assets<StandardMaterial>,
    flame_materials: &mut Assets<FlameMaterial>,
    waterfall_materials: &mut Assets<waterfall::WaterfallMaterial>,
    source: &WorldSource,
    seed: u32,
) {
    let generation_start = std::time::Instant::now();
    let voxel_world = match source {
        WorldSource::Procedural => VoxelWorld::generate(seed),
        WorldSource::Imported(terrain) => VoxelWorld::from_imported(terrain, seed),
    };
    let generation_elapsed = generation_start.elapsed();

    let meshing_start = std::time::Instant::now();
    let world_meshes = mesh::build_meshes(&voxel_world, seed);
    info!(
        "world generated in {generation_elapsed:.2?}, meshed in {:.2?} \
         (terrain {} verts, water {} verts)",
        meshing_start.elapsed(),
        world_meshes.terrain.count_vertices(),
        world_meshes.water.count_vertices(),
    );

    let ground_heights = GroundHeights::from_world(&voxel_world);

    // Waterfalls wherever the river spills over the rim.
    let river_exits = voxel_world.river_rim_exits();
    let waterfall_count =
        waterfall::spawn_waterfalls(commands, meshes, waterfall_materials, &river_exits);
    info!("{waterfall_count} waterfall(s) at the rim");

    let terrain_material = materials.add(StandardMaterial {
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
                            MeshMaterial3d(terrain_material.clone()),
                            Transform::from_xyz(x, ground + lift, z),
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
                            WorldMesh,
                        ));
                    }
                }
            }
            Err(error) => warn!("props skipped: {error}"),
        }
    }

    commands.insert_resource(ground_heights);
    let water_material = materials.add(StandardMaterial {
        base_color: Color::WHITE,
        perceptual_roughness: 0.15,
        alpha_mode: AlphaMode::Blend,
        ..default()
    });

    commands.spawn((
        Mesh3d(meshes.add(world_meshes.terrain)),
        MeshMaterial3d(terrain_material),
        WorldMesh,
    ));
    commands.spawn((
        Mesh3d(meshes.add(world_meshes.water)),
        MeshMaterial3d(water_material),
        NotShadowCaster,
        WorldMesh,
    ));
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
    heights: Vec<f32>,
}

impl GroundHeights {
    fn from_world(voxel_world: &VoxelWorld) -> Self {
        let mut heights = vec![PLATEAU_FLOOR_RENDER; WORLD_SIZE_X * WORLD_SIZE_Z];
        for z in 0..WORLD_SIZE_Z as i32 {
            for x in 0..WORLD_SIZE_X as i32 {
                for y in (0..WORLD_SIZE_Y as i32).rev() {
                    if matches!(
                        voxel_world.get(x, y, z),
                        Voxel::Grass | Voxel::Dirt | Voxel::Sand | Voxel::Stone
                    ) {
                        // Wade on the river surface rather than its bed.
                        let ground = (y + 1).max(WATER_LEVEL + 1) as f32 * VOXEL_SIZE;
                        heights[(z as usize) * WORLD_SIZE_X + x as usize] = ground;
                        break;
                    }
                }
            }
        }
        Self { heights }
    }

    /// Ground height at a render-space position (centered world).
    fn ground_at(&self, x: f32, z: f32) -> f32 {
        let column_x = (x / VOXEL_SIZE + WORLD_SIZE_X as f32 / 2.0) as i32;
        let column_z = (z / VOXEL_SIZE + WORLD_SIZE_Z as f32 / 2.0) as i32;
        if column_x < 0
            || column_z < 0
            || column_x >= WORLD_SIZE_X as i32
            || column_z >= WORLD_SIZE_Z as i32
        {
            return PLATEAU_FLOOR_RENDER;
        }
        self.heights[(column_z as usize) * WORLD_SIZE_X + column_x as usize]
    }
}

const PLATEAU_FLOOR_RENDER: f32 = world::PLATEAU_FLOOR as f32 * VOXEL_SIZE;

/// Press `R` for a fresh plateau.
#[allow(clippy::too_many_arguments)]
fn regenerate_system(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
    mut flame_materials: ResMut<Assets<FlameMaterial>>,
    mut waterfall_materials: ResMut<Assets<waterfall::WaterfallMaterial>>,
    mut seed: ResMut<WorldSeed>,
    source: Res<WorldSource>,
    existing_meshes: Query<Entity, With<WorldMesh>>,
) {
    if !keyboard.just_pressed(KeyCode::KeyR) {
        return;
    }
    seed.0 = seed.0.wrapping_add(1);
    for entity in &existing_meshes {
        commands.entity(entity).despawn();
    }
    // Imported terrain keeps its heights; a new seed reshuffles the
    // decoration (grass, flowers, procedural trees) and waterfalls.
    spawn_world(
        &mut commands,
        &mut meshes,
        &mut materials,
        &mut flame_materials,
        &mut waterfall_materials,
        &source,
        seed.0,
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
        Self {
            position: Vec3::new(0.0, 13.0, 0.0),
            yaw,
            pitch,
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
    mut camera_query: Query<
        (
            &mut Transform,
            &mut Projection,
            &mut DepthOfField,
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
            depth_of_field.focal_distance = orbit_state.distance;
            depth_of_field.aperture_f_stops = tweaks.orbit_aperture_f_stops;
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
            depth_of_field.focal_distance = tweaks.walk_focal_distance;
            depth_of_field.aperture_f_stops = tweaks.walk_aperture_f_stops;
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

#[allow(clippy::too_many_arguments)]
fn first_person_update(
    walk_state: &mut FirstPersonState,
    ground_heights: Option<&GroundHeights>,
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

    // Follow the terrain, smoothed so voxel steps don't jolt the camera.
    if let Some(heights) = ground_heights {
        let target_eye =
            heights.ground_at(walk_state.position.x, walk_state.position.z) + eye_height;
        let blend = (time.delta_secs() * 10.0).min(1.0);
        walk_state.position.y += (target_eye - walk_state.position.y) * blend;
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
