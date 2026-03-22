//! Weather system with Clear/Rain/Wind/Storm states.
//!
//! Controls ground shader wetness, wind, dynamic fog/lighting,
//! and rain particle spawning. Transitions smoothly between states.

use bevy::prelude::*;
use rand::Rng;

use crate::camera::IsometricCamera;
use crate::grass_material::GrassMaterial;

// ── Weather state ───────────────────────────────────────────────────────────

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub enum WeatherKind {
    #[default]
    Clear,
    Rain,
    Wind,
    Storm,
}

impl WeatherKind {
    /// Target parameter values for each weather state.
    fn targets(&self) -> WeatherTargets {
        match self {
            WeatherKind::Clear => WeatherTargets {
                wetness: 0.0,
                wind_strength: 0.3,
                rain_intensity: 0.0,
                fog_density_multiplier: 1.0,
                fog_color: Vec3::new(0.15, 0.18, 0.20),
                light_illuminance: 10000.0,
                light_color: Vec3::new(1.0, 0.98, 0.95),
                ambient_brightness: 1000.0,
                clear_color: Vec3::new(0.12, 0.20, 0.08),
            },
            WeatherKind::Rain => WeatherTargets {
                wetness: 0.8,
                wind_strength: 0.4,
                rain_intensity: 1.0,
                fog_density_multiplier: 2.5,
                fog_color: Vec3::new(0.12, 0.12, 0.15),
                light_illuminance: 1200.0,
                light_color: Vec3::new(0.7, 0.75, 0.85),
                ambient_brightness: 500.0,
                clear_color: Vec3::new(0.06, 0.07, 0.08),
            },
            WeatherKind::Wind => WeatherTargets {
                wetness: 0.0,
                wind_strength: 1.0,
                rain_intensity: 0.0,
                fog_density_multiplier: 1.5,
                fog_color: Vec3::new(0.10, 0.10, 0.11),
                light_illuminance: 2500.0,
                light_color: Vec3::new(0.9, 0.9, 0.88),
                ambient_brightness: 350.0,
                clear_color: Vec3::new(0.05, 0.07, 0.04),
            },
            WeatherKind::Storm => WeatherTargets {
                wetness: 1.0,
                wind_strength: 1.0,
                rain_intensity: 1.0,
                fog_density_multiplier: 3.5,
                fog_color: Vec3::new(0.08, 0.08, 0.10),
                light_illuminance: 600.0,
                light_color: Vec3::new(0.5, 0.55, 0.65),
                ambient_brightness: 600.0,
                clear_color: Vec3::new(0.04, 0.04, 0.06),
            },
        }
    }
}

struct WeatherTargets {
    wetness: f32,
    wind_strength: f32,
    rain_intensity: f32,
    fog_density_multiplier: f32,
    fog_color: Vec3,
    light_illuminance: f32,
    light_color: Vec3,
    ambient_brightness: f32,
    clear_color: Vec3,
}

/// Current weather parameters (interpolated toward target each frame).
#[derive(Resource)]
pub struct WeatherState {
    pub current: WeatherKind,
    pub wetness: f32,
    pub wind_strength: f32,
    pub wind_direction: Vec2,
    pub rain_intensity: f32,
    pub fog_density_multiplier: f32,
    pub fog_color: Vec3,
    pub light_illuminance: f32,
    pub light_color: Vec3,
    pub ambient_brightness: f32,
    pub clear_color: Vec3,
    /// Base fog density from scene setup (multiplied by weather).
    pub base_fog_density: f32,
}

impl Default for WeatherState {
    fn default() -> Self {
        let targets = WeatherKind::Clear.targets();
        Self {
            current: WeatherKind::Clear,
            wetness: targets.wetness,
            wind_strength: targets.wind_strength,
            wind_direction: Vec2::new(1.0, 0.3).normalize(),
            rain_intensity: targets.rain_intensity,
            fog_density_multiplier: targets.fog_density_multiplier,
            fog_color: targets.fog_color,
            light_illuminance: targets.light_illuminance,
            light_color: targets.light_color,
            ambient_brightness: targets.ambient_brightness,
            clear_color: targets.clear_color,
            base_fog_density: 0.1,
        }
    }
}

impl WeatherState {
    pub fn set_weather(&mut self, kind: WeatherKind) {
        self.current = kind;
    }
}

// ── Transition system ───────────────────────────────────────────────────────

/// Transition speed (fraction per second). 0.5 = ~2 second transitions.
const TRANSITION_SPEED: f32 = 0.5;

fn lerp_f32(current: f32, target: f32, alpha: f32) -> f32 {
    current + (target - current) * alpha
}

fn lerp_vec3(current: Vec3, target: Vec3, alpha: f32) -> Vec3 {
    current + (target - current) * alpha
}

pub fn transition_weather(mut weather: ResMut<WeatherState>, time: Res<Time>) {
    let targets = weather.current.targets();
    let alpha = (TRANSITION_SPEED * time.delta_secs()).min(1.0);

    weather.wetness = lerp_f32(weather.wetness, targets.wetness, alpha);
    weather.wind_strength = lerp_f32(weather.wind_strength, targets.wind_strength, alpha);
    weather.rain_intensity = lerp_f32(weather.rain_intensity, targets.rain_intensity, alpha);
    weather.fog_density_multiplier = lerp_f32(
        weather.fog_density_multiplier,
        targets.fog_density_multiplier,
        alpha,
    );
    weather.fog_color = lerp_vec3(weather.fog_color, targets.fog_color, alpha);
    weather.light_illuminance =
        lerp_f32(weather.light_illuminance, targets.light_illuminance, alpha);
    weather.light_color = lerp_vec3(weather.light_color, targets.light_color, alpha);
    weather.ambient_brightness = lerp_f32(
        weather.ambient_brightness,
        targets.ambient_brightness,
        alpha,
    );
    weather.clear_color = lerp_vec3(weather.clear_color, targets.clear_color, alpha);
}

// ── Apply weather to grass shader ───────────────────────────────────────────

pub fn apply_weather_to_grass(
    weather: Res<WeatherState>,
    mut materials: ResMut<Assets<GrassMaterial>>,
) {
    for (_handle, material) in materials.iter_mut() {
        material.wetness = weather.wetness;
        material.wind_strength = weather.wind_strength;
        material.wind_direction_x = weather.wind_direction.x;
        material.wind_direction_y = weather.wind_direction.y;
    }
}

// ── Apply weather to atmosphere (fog, lighting, clear color) ────────────────

pub fn apply_weather_to_atmosphere(
    weather: Res<WeatherState>,
    mut fog_query: Query<&mut DistanceFog>,
    mut directional_query: Query<&mut DirectionalLight>,
    mut ambient_query: Query<&mut AmbientLight>,
    mut clear_color: ResMut<ClearColor>,
) {
    // Fog
    for mut fog in &mut fog_query {
        let density = weather.base_fog_density * weather.fog_density_multiplier;
        fog.falloff = FogFalloff::ExponentialSquared { density };
        fog.color = Color::srgb(
            weather.fog_color.x,
            weather.fog_color.y,
            weather.fog_color.z,
        );
    }

    // Directional light
    for mut light in &mut directional_query {
        light.illuminance = weather.light_illuminance;
        light.color = Color::srgb(
            weather.light_color.x,
            weather.light_color.y,
            weather.light_color.z,
        );
    }

    // Ambient light (per-camera component)
    for mut ambient in &mut ambient_query {
        ambient.brightness = weather.ambient_brightness;
    }

    // Clear color
    clear_color.0 = Color::srgb(
        weather.clear_color.x,
        weather.clear_color.y,
        weather.clear_color.z,
    );
}

// ── Rain particles ──────────────────────────────────────────────────────────

#[derive(Component)]
pub struct RainDrop {
    pub velocity: Vec3,
}

const MAX_RAIN_DROPS: usize = 300;
const RAIN_SPAWN_HEIGHT: f32 = 15.0;
const RAIN_GROUND_LEVEL: f32 = -0.5;
const RAIN_SPREAD: f32 = 25.0;

pub fn spawn_rain_drops(
    mut commands: Commands,
    rain_drops: Query<Entity, With<RainDrop>>,
    weather: Res<WeatherState>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut materials: ResMut<Assets<StandardMaterial>>,
) {
    let desired_count = (MAX_RAIN_DROPS as f32 * weather.rain_intensity) as usize;
    let current_count = rain_drops.iter().count();

    if current_count < desired_count {
        let drop_mesh = meshes.add(Cuboid::new(0.02, 0.15, 0.02));
        let drop_material = materials.add(StandardMaterial {
            base_color: Color::srgba(0.6, 0.7, 0.9, 0.4),
            alpha_mode: AlphaMode::Blend,
            unlit: true,
            ..default()
        });

        let mut rng = rand::rng();
        let to_spawn = (desired_count - current_count).min(20); // spawn in batches

        for _ in 0..to_spawn {
            let x = rng.random_range(-RAIN_SPREAD..RAIN_SPREAD);
            let z = rng.random_range(-RAIN_SPREAD..RAIN_SPREAD);
            let y = rng.random_range(0.0..RAIN_SPAWN_HEIGHT);

            let wind_drift = Vec3::new(
                weather.wind_direction.x * weather.wind_strength * 2.0,
                0.0,
                weather.wind_direction.y * weather.wind_strength * 2.0,
            );

            commands.spawn((
                RainDrop {
                    velocity: Vec3::new(0.0, -12.0, 0.0) + wind_drift,
                },
                Mesh3d(drop_mesh.clone()),
                MeshMaterial3d(drop_material.clone()),
                Transform::from_translation(Vec3::new(x, y, z)),
            ));
        }
    }

    // Despawn excess drops
    if current_count > desired_count {
        let to_remove = current_count - desired_count;
        for (index, entity) in rain_drops.iter().enumerate() {
            if index >= to_remove {
                break;
            }
            commands.entity(entity).despawn();
        }
    }
}

pub fn update_rain_drops(
    mut drops: Query<(&RainDrop, &mut Transform)>,
    weather: Res<WeatherState>,
    camera_query: Query<&Transform, (With<IsometricCamera>, Without<RainDrop>)>,
    time: Res<Time>,
) {
    let camera_center = camera_query
        .single()
        .map(|t| t.translation)
        .unwrap_or(Vec3::ZERO);

    let mut rng = rand::rng();

    for (drop, mut transform) in &mut drops {
        transform.translation += drop.velocity * time.delta_secs();

        // Recycle drops that fall below ground
        if transform.translation.y < RAIN_GROUND_LEVEL {
            transform.translation.y = RAIN_SPAWN_HEIGHT;
            transform.translation.x = camera_center.x + rng.random_range(-RAIN_SPREAD..RAIN_SPREAD);
            transform.translation.z = camera_center.z + rng.random_range(-RAIN_SPREAD..RAIN_SPREAD);
        }

        // Keep drops centered around the camera
        let distance_from_camera = (transform.translation.xz() - camera_center.xz()).length();
        if distance_from_camera > RAIN_SPREAD * 1.5 {
            transform.translation.x = camera_center.x + rng.random_range(-RAIN_SPREAD..RAIN_SPREAD);
            transform.translation.z = camera_center.z + rng.random_range(-RAIN_SPREAD..RAIN_SPREAD);
        }
    }

    // Update rain drop visibility based on intensity
    let _ = weather.rain_intensity; // used by spawn system, drops already managed
}

// ── Keyboard weather controls ───────────────────────────────────────────────

pub fn weather_keyboard_controls(
    keyboard: Res<ButtonInput<KeyCode>>,
    mut weather: ResMut<WeatherState>,
) {
    if keyboard.just_pressed(KeyCode::Digit1) {
        weather.set_weather(WeatherKind::Clear);
    }
    if keyboard.just_pressed(KeyCode::Digit2) {
        weather.set_weather(WeatherKind::Rain);
    }
    if keyboard.just_pressed(KeyCode::Digit3) {
        weather.set_weather(WeatherKind::Wind);
    }
    if keyboard.just_pressed(KeyCode::Digit4) {
        weather.set_weather(WeatherKind::Storm);
    }
}
