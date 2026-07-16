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
use crate::world::{VOXEL_SIZE, WATER_LEVEL};

/// Reflections render at half resolution — the wave perturbation hides it.
const REFLECTION_WIDTH: u32 = 960;
const REFLECTION_HEIGHT: u32 = 540;

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
}

impl Default for WaterUniform {
    fn default() -> Self {
        Self {
            zenith: Vec4::new(0.09, 0.22, 0.57, 1.0),
            horizon: Vec4::new(0.60, 0.64, 0.60, 0.0),
            light_direction: Vec4::new(0.0, 1.0, 0.0, 1.0),
            light_color: Vec4::new(1.0, 0.96, 0.87, 0.2),
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
pub fn update_reflection_camera(
    main_camera: Query<
        (&Transform, &Projection),
        (
            With<bevy::core_pipeline::prepass::DepthPrepass>,
            Without<ReflectionCamera>,
        ),
    >,
    mut reflection_camera: Query<(&mut Transform, &mut Projection), With<ReflectionCamera>>,
    mut materials: ResMut<Assets<WaterMaterial>>,
) {
    let Ok((main_transform, main_projection)) = main_camera.single() else {
        return;
    };
    let Ok((mut mirror_transform, mut mirror_projection)) = reflection_camera.single_mut() else {
        return;
    };

    let plane_y = water_surface_y();
    let position = main_transform.translation;
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
