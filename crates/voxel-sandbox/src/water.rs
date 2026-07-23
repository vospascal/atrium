//! Stylized water surface for the river and lakes.
//!
//! The voxel mesher still produces the water boundary mesh; this material
//! replaces its flat translucent blue with a real water look: Beer–Lambert
//! depth absorption (reading true optical depth from the depth prepass),
//! octave-rotated wave normals that calm with distance, sun/moon glints,
//! a noisy foam band that melts the waterline into the shore — and REAL
//! planar reflections.
//!
//! Reflections work because the water plane is globally flat (one world
//! water level): a second camera, mirrored below the plane, renders the
//! above-water world (reflection render layer) into a half-resolution
//! texture every frame. The fragment shader projects each water point
//! through that camera's clip matrix and samples the texture with
//! wave-perturbed coordinates — trees, shores, campfires, and the sky
//! actually mirror in the water. The mirrored camera only ever renders
//! geometry above the plane (the mesher splits the terrain at the
//! waterline), so no oblique-frustum clipping is needed.

use bevy::asset::RenderAssetUsages;
use bevy::camera::visibility::RenderLayers;
use bevy::camera::RenderTarget;
use bevy::image::Image;
use bevy::pbr::Material;
use bevy::prelude::*;
use bevy::render::render_resource::{
    AsBindGroup, Extent3d, ShaderType, TextureDimension, TextureFormat, TextureUsages,
};
use bevy::shader::ShaderRef;

use crate::day_night::CelestialState;
use crate::weather::WeatherState;
use voxel_core::world::{VOXEL_SIZE, WATER_LEVEL};

/// Reflections render at reduced resolution — the wave perturbation hides it,
/// and the mirror pass is the single biggest GPU cost on a fill-bound machine.
const REFLECTION_WIDTH: u32 = 720;
const REFLECTION_HEIGHT: u32 = 405;

/// Re-render the mirror only every Nth frame; the wave distortion hides the
/// staleness. Cuts the mirror's cost proportionally (both fill and draw), so
/// it's the most robust reflection optimisation here. 1 = every frame.
const REFLECTION_UPDATE_INTERVAL: u32 = 2;

/// Entities on this layer are visible to the reflection camera (they still
/// need layer 0 to be visible to the main camera).
pub const REFLECTION_LAYER: usize = 1;

/// Height of the water surface in render space.
pub fn water_surface_y() -> f32 {
    (WATER_LEVEL + 1) as f32 * VOXEL_SIZE
}

/// Layers for everything that should appear in water reflections.
pub fn reflective_layers() -> RenderLayers {
    RenderLayers::from_layers(&[0, REFLECTION_LAYER])
}

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct WaterUniform {
    /// rgb = zenith sky color (linear), w = daylight 0..1.
    pub zenith: Vec4,
    /// rgb = horizon sky color (linear), w = moonlight 0..1.
    pub horizon: Vec4,
    /// xyz = direction to the active light body (sun or moon), w = glint
    /// strength.
    pub light_direction: Vec4,
    /// rgb = light color (linear), w = wave choppiness from the wind.
    pub light_color: Vec4,
    /// xy = fraction of the reflection texture in use (dynamic-resolution
    /// viewport), z = live-mirror strength (0 = procedural fallback).
    pub reflection: Vec4,
}

impl Default for WaterUniform {
    fn default() -> Self {
        Self {
            zenith: Vec4::new(0.09, 0.22, 0.57, 1.0),
            horizon: Vec4::new(0.60, 0.64, 0.60, 0.0),
            light_direction: Vec4::new(0.0, 1.0, 0.0, 1.0),
            light_color: Vec4::new(1.0, 0.96, 0.87, 0.2),
            reflection: Vec4::new(1.0, 1.0, 1.0, 0.0),
        }
    }
}

/// Coarse copy of the world's distance-to-water field (sampled every 8th
/// column, ~1 m grid), kept as a resource so the reflection system can ask
/// "how far is the camera from any water?" long after the full world data
/// is compressed away.
#[derive(Resource)]
pub struct WaterProximity {
    grid: Vec<f32>,
    columns_x: usize,
    columns_z: usize,
}

impl WaterProximity {
    const STEP: usize = 8;

    pub fn from_world(world: &voxel_core::world::VoxelWorld) -> Self {
        let columns_x = voxel_core::world::WORLD_SIZE_X / Self::STEP;
        let columns_z = voxel_core::world::WORLD_SIZE_Z / Self::STEP;
        let mut grid = vec![f32::MAX; columns_x * columns_z];
        for z in 0..columns_z {
            for x in 0..columns_x {
                grid[z * columns_x + x] =
                    world.water_distance_at((x * Self::STEP) as i32, (z * Self::STEP) as i32);
            }
        }
        Self {
            grid,
            columns_x,
            columns_z,
        }
    }

    /// Horizontal distance (meters) from a render-space position to the
    /// nearest water surface.
    pub fn horizontal_distance(&self, render_x: f32, render_z: f32) -> f32 {
        let column_x = ((render_x / VOXEL_SIZE + voxel_core::world::WORLD_SIZE_X as f32 / 2.0)
            / Self::STEP as f32)
            .clamp(0.0, self.columns_x as f32 - 1.0) as usize;
        let column_z = ((render_z / VOXEL_SIZE + voxel_core::world::WORLD_SIZE_Z as f32 / 2.0)
            / Self::STEP as f32)
            .clamp(0.0, self.columns_z as f32 - 1.0) as usize;
        self.grid[column_z * self.columns_x + column_x]
    }
}

/// Runtime knobs + readout for the planar reflection.
#[derive(Resource)]
pub struct ReflectionSettings {
    /// Master switch (perf overlay lever).
    pub enabled: bool,
    /// Written by the update system: current resolution tier (1.0, 0.5,
    /// 0.25) — for the overlay readout.
    pub current_tier: f32,
    /// Written by the update system: live-mirror strength after the
    /// distance fade (0 = camera off, fallback in the shader).
    pub current_strength: f32,
    /// Camera's 3D distance to the nearest water, meters (readout).
    pub current_distance: f32,
    /// Whether any water chunk survived the main view's frustum culling
    /// last frame — no water on screen means no mirror to render.
    pub water_on_screen: bool,
}

impl Default for ReflectionSettings {
    fn default() -> Self {
        Self {
            enabled: true,
            current_tier: 1.0,
            current_strength: 1.0,
            current_distance: 0.0,
            water_on_screen: true,
        }
    }
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct WaterMaterial {
    #[uniform(0)]
    pub water: WaterUniform,
    #[texture(1)]
    #[sampler(2)]
    pub reflection: Handle<Image>,
    /// Clip-from-world matrix of the mirrored reflection camera; the
    /// fragment shader projects water points through it to find their
    /// reflection texels.
    #[uniform(3)]
    pub reflection_clip_from_world: Mat4,
}

impl Material for WaterMaterial {
    fn fragment_shader() -> ShaderRef {
        "shaders/water.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

/// The render target the reflection camera draws into. Built with
/// `FromWorld` so it exists before any startup system runs.
#[derive(Resource)]
pub struct ReflectionTarget {
    pub image: Handle<Image>,
}

impl FromWorld for ReflectionTarget {
    fn from_world(world: &mut World) -> Self {
        let mut image = Image::new_fill(
            Extent3d {
                width: REFLECTION_WIDTH,
                height: REFLECTION_HEIGHT,
                depth_or_array_layers: 1,
            },
            TextureDimension::D2,
            &[0, 0, 0, 255],
            TextureFormat::Bgra8UnormSrgb,
            RenderAssetUsages::default(),
        );
        image.texture_descriptor.usage = TextureUsages::TEXTURE_BINDING
            | TextureUsages::COPY_DST
            | TextureUsages::RENDER_ATTACHMENT;
        let image = world.resource_mut::<Assets<Image>>().add(image);
        Self { image }
    }
}

/// Marker for the mirrored camera that renders the reflection texture.
#[derive(Component)]
pub struct ReflectionCamera;

pub fn spawn_reflection_camera(mut commands: Commands, target: Res<ReflectionTarget>) {
    commands.spawn((
        Camera3d::default(),
        Camera {
            order: -1,
            ..default()
        },
        // HDR + the same tonemapping as the main view: an LDR reflection
        // pass tonemaps the night sky brighter and bluer than the main
        // camera shows it, and the mismatch reads as blue water at night.
        bevy::render::view::Hdr,
        // No MSAA for the mirror — the wave wobble hides the jaggies.
        Msaa::Off,
        RenderTarget::Image(target.image.clone().into()),
        Projection::Perspective(PerspectiveProjection {
            fov: 22_f32.to_radians(),
            aspect_ratio: REFLECTION_WIDTH as f32 / REFLECTION_HEIGHT as f32,
            ..default()
        }),
        Transform::default(),
        RenderLayers::layer(REFLECTION_LAYER),
        ReflectionCamera,
    ));
}

/// Mirror the main camera below the water plane and hand the resulting
/// clip matrix to every water material, so the shader can look up "what
/// does the reflected ray see" in the freshly rendered texture.
///
/// The mirror's cost scales with how close the camera is to water:
/// half-resolution viewport within 60 m, quarter beyond, and past ~120 m the
/// camera switches off entirely while the shader cross-fades to a procedural
/// sky tint (at that range the mirror is mostly sky anyway). It also only
/// re-renders every `REFLECTION_UPDATE_INTERVAL` frames.
#[allow(clippy::too_many_arguments, clippy::type_complexity)]
pub fn update_reflection_camera(
    main_camera: Query<
        (&Transform, &Projection),
        (
            With<bevy::core_pipeline::prepass::DepthPrepass>,
            Without<ReflectionCamera>,
        ),
    >,
    mut reflection_camera: Query<
        (&mut Transform, &mut Projection, &mut Camera),
        With<ReflectionCamera>,
    >,
    proximity: Option<Res<WaterProximity>>,
    water_chunks: Query<&ViewVisibility, With<MeshMaterial3d<WaterMaterial>>>,
    time: Res<Time>,
    mut settings: ResMut<ReflectionSettings>,
    mut materials: ResMut<Assets<WaterMaterial>>,
    mut frame_counter: Local<u32>,
) {
    let Ok((main_transform, main_projection)) = main_camera.single() else {
        return;
    };
    let Ok((mut mirror_transform, mut mirror_projection, mut mirror_camera)) =
        reflection_camera.single_mut()
    else {
        return;
    };

    let plane_y = water_surface_y();
    let position = main_transform.translation;

    // No water chunk survived the main view's frustum culling (last
    // frame's result — water is layer 0 only, so no other view counts):
    // nothing would show the mirror, park the camera entirely.
    let water_on_screen = water_chunks
        .iter()
        .any(|view_visibility| view_visibility.get());
    settings.water_on_screen = water_on_screen;

    // Distance-scaled quality: 3D distance from the camera to the nearest
    // water surface picks the viewport tier and the live-mirror strength.
    let horizontal = proximity
        .map(|proximity| proximity.horizontal_distance(position.x, position.z))
        .unwrap_or(0.0);
    let vertical = (position.y - plane_y).abs();
    let water_distance = (horizontal * horizontal + vertical * vertical).sqrt();
    // Wave distortion hides the low resolution; a full-res mirror is far too
    // expensive on a fill-bound machine, so even the close tier is capped at
    // half.
    let tier = if water_distance < 60.0 { 0.5 } else { 0.25 };
    let target_strength = if settings.enabled && water_on_screen {
        ((125.0 - water_distance) / 25.0).clamp(0.0, 1.0)
    } else {
        0.0
    };
    // Ease toward the target so the mirror fades in when water swings
    // back into frame instead of popping.
    let blend = (time.delta_secs() * 6.0).min(1.0);
    let mut strength =
        settings.current_strength + (target_strength - settings.current_strength) * blend;
    if strength < 0.004 {
        strength = 0.0;
    }
    settings.current_tier = tier;
    settings.current_strength = strength;
    settings.current_distance = water_distance;

    let viewport_width = ((REFLECTION_WIDTH as f32 * tier) as u32).max(1);
    let viewport_height = ((REFLECTION_HEIGHT as f32 * tier) as u32).max(1);
    // Only re-render the mirror every Nth frame; on the skipped frames the
    // render target keeps its last contents and the wave wobble hides it.
    *frame_counter = frame_counter.wrapping_add(1);
    let render_this_frame = frame_counter.is_multiple_of(REFLECTION_UPDATE_INTERVAL);
    mirror_camera.is_active = strength > 0.005 && render_this_frame;
    mirror_camera.viewport = Some(bevy::camera::Viewport {
        physical_position: UVec2::ZERO,
        physical_size: UVec2::new(viewport_width, viewport_height),
        ..default()
    });
    let uv_scale = Vec2::new(
        viewport_width as f32 / REFLECTION_WIDTH as f32,
        viewport_height as f32 / REFLECTION_HEIGHT as f32,
    );
    let mirrored_position = Vec3::new(position.x, 2.0 * plane_y - position.y, position.z);
    let forward = main_transform.forward();
    let up = main_transform.up();
    let mirrored_forward = Vec3::new(forward.x, -forward.y, forward.z);
    let mirrored_up = Vec3::new(up.x, -up.y, up.z);
    *mirror_transform =
        Transform::from_translation(mirrored_position).looking_to(mirrored_forward, mirrored_up);

    if let (Projection::Perspective(main), Projection::Perspective(mirror)) =
        (main_projection, &mut *mirror_projection)
    {
        mirror.fov = main.fov;
        mirror.aspect_ratio = REFLECTION_WIDTH as f32 / REFLECTION_HEIGHT as f32;
    }

    let clip_from_world =
        mirror_projection.get_clip_from_view() * mirror_transform.to_matrix().inverse();
    for (_, material) in materials.iter_mut() {
        material.reflection_clip_from_world = clip_from_world;
        material.water.reflection = Vec4::new(uv_scale.x, uv_scale.y, strength, 0.0);
    }
}

/// Keep the water in sync with the sky it mirrors and the wind that
/// ruffles it.
pub fn update_water(
    celestial: Res<CelestialState>,
    weather: Res<WeatherState>,
    mut materials: ResMut<Assets<WaterMaterial>>,
) {
    let daylight = celestial.daylight;
    let moonlight = celestial.moonlight;
    let (light_direction, glint) = if daylight > 0.02 {
        (celestial.sun_direction, daylight)
    } else {
        (celestial.moon_direction, moonlight * 0.6)
    };
    let choppiness = (weather.current.wind_speed / 40.0).clamp(0.0, 1.0);

    for (_, material) in materials.iter_mut() {
        material.water.zenith = celestial.zenith_color.extend(daylight);
        material.water.horizon = celestial.horizon_color.extend(moonlight);
        material.water.light_direction = light_direction.extend(glint);
        material.water.light_color = celestial.light_color.extend(choppiness);
    }
}
