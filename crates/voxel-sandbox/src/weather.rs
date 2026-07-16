//! Weather state and precipitation.
//!
//! [`WeatherState`] holds a `target` (what the panel or, later, a biome
//! script asks for) and a `current` that eases toward it, so cloud cover
//! rolls in and fog thickens gradually instead of snapping. The sky dome,
//! distance fog, sun dimming, and precipitation all read `current`.
//!
//! Rain and snow are one pre-built particle mesh (thousands of quads in a
//! box volume) whose vertex shader wraps each particle's fall and drift —
//! the volume follows the camera so precipitation is always around you.

use bevy::asset::RenderAssetUsages;
use bevy::light::NotShadowCaster;
use bevy::mesh::{Indices, PrimitiveTopology};
use bevy::pbr::Material;
use bevy::prelude::*;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;

use crate::day_night::CelestialState;
use crate::noise::{hash_3d, hash_to_unit};
use crate::ViewMode;

/// Particles in the precipitation volume (each is one billboarded quad).
const PARTICLE_COUNT: usize = 16_000;
/// Half-extent of the volume on x/z, meters (local space, pre-scale).
const VOLUME_HALF_EXTENT: f32 = 28.0;
/// Height of the volume, meters (local space, pre-scale).
const VOLUME_HEIGHT: f32 = 24.0;
/// In orbit view the volume is scaled up to drape the whole diorama.
const ORBIT_VOLUME_SCALE: f32 = 4.0;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Precipitation {
    None,
    Rain,
    Snow,
}

/// The tunable weather values. `WeatherState` keeps two copies: the panel
/// (or a biome script) writes `target`, and `current` eases toward it.
#[derive(Clone, Copy, Debug)]
pub struct WeatherSettings {
    /// 0 = clear sky, 1 = overcast.
    pub cloud_coverage: f32,
    /// 0 = stratus (flat gray sheet), 1 = cumulus (puffy), 2 = cirrus (wisps).
    pub cloud_type: f32,
    /// Wind at cloud level, m/s — drives cloud scroll and precipitation slant.
    pub wind_speed: f32,
    /// Direction the wind blows toward, degrees (0 = +x, 90 = +z).
    pub wind_direction_degrees: f32,
    /// 0 = clear air, 1 = pea-soup fog.
    pub fog: f32,
    /// 0 = none, 1 = downpour / blizzard.
    pub precipitation_intensity: f32,
}

#[derive(Resource)]
pub struct WeatherState {
    pub target: WeatherSettings,
    pub current: WeatherSettings,
    pub precipitation: Precipitation,
    /// Accumulated cloud drift (m) — integrating wind avoids jumps when the
    /// wind speed slider moves.
    pub cloud_scroll: Vec2,
    /// Accumulated precipitation drift (m).
    pub precipitation_drift: Vec2,
}

impl Default for WeatherState {
    fn default() -> Self {
        // Env overrides make weather scriptable for screenshots and, later,
        // biome presets: VOXEL_CLOUDS, VOXEL_CLOUD_TYPE, VOXEL_WIND,
        // VOXEL_FOG, VOXEL_PRECIP=rain|snow, VOXEL_PRECIP_INTENSITY.
        let env_f32 = |name: &str, default: f32| {
            std::env::var(name)
                .ok()
                .and_then(|value| value.parse::<f32>().ok())
                .unwrap_or(default)
        };
        let settings = WeatherSettings {
            cloud_coverage: env_f32("VOXEL_CLOUDS", 0.35).clamp(0.0, 1.0),
            cloud_type: env_f32("VOXEL_CLOUD_TYPE", 0.4).clamp(0.0, 2.0),
            wind_speed: env_f32("VOXEL_WIND", 7.0).clamp(0.0, 60.0),
            wind_direction_degrees: env_f32("VOXEL_WIND_DIRECTION", 40.0),
            fog: env_f32("VOXEL_FOG", 0.0).clamp(0.0, 1.0),
            precipitation_intensity: env_f32("VOXEL_PRECIP_INTENSITY", 0.25).clamp(0.0, 1.0),
        };
        let precipitation = match std::env::var("VOXEL_PRECIP").as_deref() {
            Ok("rain") => Precipitation::Rain,
            Ok("snow") => Precipitation::Snow,
            _ => Precipitation::None,
        };
        Self {
            target: settings,
            current: settings,
            precipitation,
            cloud_scroll: Vec2::ZERO,
            precipitation_drift: Vec2::ZERO,
        }
    }
}

impl WeatherState {
    /// Unit vector of the wind on the ground plane (x, z).
    pub fn wind_direction(&self) -> Vec2 {
        let radians = self.current.wind_direction_degrees.to_radians();
        Vec2::new(radians.cos(), radians.sin())
    }

    /// Cloud coverage as the sky actually shows it: precipitation without
    /// clouds would look wrong, so rain/snow pull coverage up with them.
    pub fn effective_cloud_coverage(&self) -> f32 {
        let precipitation_floor = if self.precipitation == Precipitation::None {
            0.0
        } else {
            self.current.precipitation_intensity * 0.75
        };
        self.current.cloud_coverage.max(precipitation_floor)
    }
}

/// Ease `current` toward `target` and integrate wind drift.
pub fn ease_weather(time: Res<Time>, mut weather: ResMut<WeatherState>) {
    let delta = time.delta_secs();
    // Per-field time constants: clouds roll in slowly, precipitation
    // reacts faster.
    let blend = |tau: f32| 1.0 - (-delta / tau).exp();
    let cloud_blend = blend(6.0);
    let wind_blend = blend(2.5);
    let fog_blend = blend(5.0);
    let precipitation_blend = blend(3.0);

    let target = weather.target;
    let current = &mut weather.current;
    current.cloud_coverage += (target.cloud_coverage - current.cloud_coverage) * cloud_blend;
    current.cloud_type += (target.cloud_type - current.cloud_type) * cloud_blend;
    current.wind_speed += (target.wind_speed - current.wind_speed) * wind_blend;
    current.wind_direction_degrees +=
        (target.wind_direction_degrees - current.wind_direction_degrees) * wind_blend;
    current.fog += (target.fog - current.fog) * fog_blend;
    current.precipitation_intensity +=
        (target.precipitation_intensity - current.precipitation_intensity) * precipitation_blend;

    let wind = weather.wind_direction() * weather.current.wind_speed;
    weather.cloud_scroll += wind * delta;
    weather.precipitation_drift += wind * 0.35 * delta;
}

#[derive(Clone, Copy, Debug, ShaderType)]
pub struct PrecipitationUniform {
    /// xyz = particle velocity (m/s, world), w = intensity 0..1.
    pub velocity: Vec4,
    /// x = quad width, y = quad length, z = sway amplitude, w = snowiness
    /// (0 = rain streak along velocity, 1 = camera-facing flake).
    pub shape: Vec4,
    /// Particle tint; alpha is per-particle opacity.
    pub color: Vec4,
    /// xy = accumulated wind drift (m), z = volume half-extent, w = height.
    pub drift: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Debug, Clone)]
pub struct PrecipitationMaterial {
    #[uniform(0)]
    pub params: PrecipitationUniform,
}

impl Material for PrecipitationMaterial {
    fn vertex_shader() -> ShaderRef {
        "shaders/precipitation.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "shaders/precipitation.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Blend
    }
}

/// Marker for the precipitation volume entity.
#[derive(Component)]
pub struct PrecipitationVolume;

pub fn spawn_precipitation(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<PrecipitationMaterial>>,
) {
    commands.spawn((
        Mesh3d(meshes.add(build_precipitation_mesh())),
        MeshMaterial3d(materials.add(PrecipitationMaterial {
            params: PrecipitationUniform {
                velocity: Vec4::new(0.0, -9.0, 0.0, 0.0),
                shape: Vec4::new(0.012, 0.35, 0.0, 0.0),
                color: Vec4::new(0.75, 0.80, 0.90, 0.32),
                drift: Vec4::new(0.0, 0.0, VOLUME_HALF_EXTENT, VOLUME_HEIGHT),
            },
        })),
        Transform::default(),
        Visibility::Hidden,
        NotShadowCaster,
        PrecipitationVolume,
    ));
}

/// One quad per particle. All four corners carry the particle's base
/// position; UV0 encodes the corner, UV1 two per-particle random seeds.
/// Both windings are emitted so billboarding can never turn a quad inside
/// out (the back-facing copy is culled by the GPU).
fn build_precipitation_mesh() -> Mesh {
    let mut positions = Vec::with_capacity(PARTICLE_COUNT * 4);
    let mut normals = Vec::with_capacity(PARTICLE_COUNT * 4);
    let mut corner_uvs = Vec::with_capacity(PARTICLE_COUNT * 4);
    let mut seed_uvs = Vec::with_capacity(PARTICLE_COUNT * 4);
    let mut indices = Vec::with_capacity(PARTICLE_COUNT * 12);

    for particle in 0..PARTICLE_COUNT as i32 {
        let unit = |salt: i32| hash_to_unit(hash_3d(particle, salt, 71, 9_001));
        let base = [
            (unit(0) * 2.0 - 1.0) * VOLUME_HALF_EXTENT,
            unit(1) * VOLUME_HEIGHT,
            (unit(2) * 2.0 - 1.0) * VOLUME_HALF_EXTENT,
        ];
        let seeds = [unit(3), unit(4)];

        let first_vertex = positions.len() as u32;
        for corner in [[0.0, 0.0], [1.0, 0.0], [1.0, 1.0], [0.0, 1.0]] {
            positions.push(base);
            normals.push([0.0, 1.0, 0.0]);
            corner_uvs.push(corner);
            seed_uvs.push(seeds);
        }
        for triangle in [[0, 1, 2], [0, 2, 3], [0, 2, 1], [0, 3, 2]] {
            indices.extend(triangle.map(|offset| first_vertex + offset));
        }
    }

    Mesh::new(
        PrimitiveTopology::TriangleList,
        RenderAssetUsages::default(),
    )
    .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, positions)
    .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, normals)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, corner_uvs)
    .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, seed_uvs)
    .with_inserted_indices(Indices::U32(indices))
}

/// Drive the precipitation volume: follow the camera (or drape the diorama
/// in orbit view), and refresh the material for the current weather.
#[allow(clippy::too_many_arguments)]
pub fn update_precipitation(
    weather: Res<WeatherState>,
    celestial: Res<CelestialState>,
    view_mode: Res<ViewMode>,
    orbit_state: Res<crate::OrbitCameraState>,
    mut materials: ResMut<Assets<PrecipitationMaterial>>,
    mut volume_query: Query<
        (
            &mut Transform,
            &mut Visibility,
            &MeshMaterial3d<PrecipitationMaterial>,
        ),
        With<PrecipitationVolume>,
    >,
    camera_query: Query<&Transform, (With<Camera3d>, Without<PrecipitationVolume>)>,
) {
    let Ok((mut transform, mut visibility, material_handle)) = volume_query.single_mut() else {
        return;
    };

    let intensity = weather.current.precipitation_intensity;
    let falling = weather.precipitation != Precipitation::None && intensity > 0.01;
    *visibility = if falling {
        Visibility::Inherited
    } else {
        Visibility::Hidden
    };
    if !falling {
        return;
    }

    match *view_mode {
        ViewMode::FirstPerson => {
            if let Ok(camera_transform) = camera_query.single() {
                transform.translation =
                    camera_transform.translation - Vec3::Y * (VOLUME_HEIGHT * 0.55);
                transform.scale = Vec3::ONE;
            }
        }
        ViewMode::Orbit => {
            transform.translation = Vec3::new(
                orbit_state.focus.x,
                orbit_state.focus.y - VOLUME_HEIGHT * ORBIT_VOLUME_SCALE * 0.15,
                orbit_state.focus.z,
            );
            transform.scale = Vec3::splat(ORBIT_VOLUME_SCALE);
        }
    }

    let Some(material) = materials.get_mut(&material_handle.0) else {
        return;
    };

    // Precipitation is lit by whatever the sky offers; keep it faintly
    // visible on moonless nights so heavy rain never fully vanishes.
    let light = 0.25 + 0.75 * celestial.daylight.max(celestial.moonlight * 0.4);
    let wind = weather.wind_direction() * weather.current.wind_speed;
    match weather.precipitation {
        Precipitation::Rain => {
            material.params.velocity = Vec4::new(
                wind.x * 0.6,
                -(9.0 + 5.0 * intensity),
                wind.y * 0.6,
                intensity,
            );
            material.params.shape = Vec4::new(0.016, 0.30 + 0.15 * intensity, 0.0, 0.0);
            material.params.color = Vec4::new(0.72 * light, 0.78 * light, 0.88 * light, 0.38);
        }
        Precipitation::Snow => {
            material.params.velocity = Vec4::new(
                wind.x * 0.35,
                -(0.9 + 0.8 * intensity),
                wind.y * 0.35,
                intensity,
            );
            material.params.shape = Vec4::new(0.055, 0.055, 0.35, 1.0);
            material.params.color = Vec4::new(0.95 * light, 0.96 * light, 1.00 * light, 0.92);
        }
        Precipitation::None => {}
    }
    material.params.drift = Vec4::new(
        weather.precipitation_drift.x,
        weather.precipitation_drift.y,
        VOLUME_HALF_EXTENT,
        VOLUME_HEIGHT,
    );
}
